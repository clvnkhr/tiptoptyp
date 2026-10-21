#!/usr/bin/env python3
"""Fast comparison-harness checks; no model packages or downloads required."""
from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
from grammar_compare import comparison, models, worker

SPEC = importlib.util.spec_from_file_location("compare_grammar_cli", SCRIPTS / "compare-grammar.py")
CLI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CLI)


def example(identifier="agreement", **overrides):
    return {
        "id": identifier, "category": "agreement", "format": "plain",
        "text": "These estimates holds.", "expected": "These estimates hold.",
        **overrides,
    }


def document(cases=None, models=None):
    return {
        "version": 1, "corpus": {"version": 1, "cases": cases or [example()]},
        "models": models or [{"name": "fake", "results": []}],
    }


class CorpusTests(unittest.TestCase):
    def load(self, cases):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "cases.json"
            path.write_text(json.dumps({"version": 1, "cases": cases}), encoding="utf-8")
            return comparison.load_corpus(path)

    def test_repository_corpus_has_control_error_and_markup_boundaries(self):
        corpus, digest = comparison.load_corpus(SCRIPTS / "grammar_compare/corpus.json")
        cases = corpus["cases"]
        self.assertEqual(len(digest), 64)
        self.assertEqual({case["format"] for case in cases}, {"plain", "tex", "typst"})
        self.assertTrue(any(case["text"] == case["expected"] for case in cases))
        self.assertTrue(any(case["text"] != case["expected"] for case in cases))
        self.assertTrue(any("MATH" in case.get("protected", []) for case in cases))
        self.assertTrue(any("Tao" in case["text"] for case in cases))

    def test_duplicate_ids_and_bad_formats_fail_before_inference(self):
        for cases, message in [
            ([example(), example()], "Duplicate"),
            ([example(format="latex")], "format"),
            ([example(id="../escape")], "id"),
            ([example(text="")], "text"),
        ]:
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.load(cases)

    def test_references_cannot_change_or_reorder_protected_values(self):
        source = "Barbato and Romito holds MATH."
        for expected in ["Barbados and Romito hold MATH.", "Romito and Barbato hold MATH."]:
            with self.subTest(expected=expected), self.assertRaisesRegex(ValueError, "protected"):
                self.load([example(text=source, expected=expected,
                                   protected=["Barbato", "Romito", "MATH"])])
        with self.assertRaisesRegex(ValueError, "protected"):
            self.load([example(text=source, expected="Barbato and Romito hold MATH.",
                               accepted=["Barbato and Romito hold mathematics."],
                               protected=["MATH"])])

    def test_controls_cannot_accept_optional_rewrites(self):
        with self.assertRaisesRegex(ValueError, "unchanged"):
            self.load([example(text="This holds.", expected="This holds.",
                               accepted=["This remains valid."])])

    def test_required_correction_cannot_accept_the_unchanged_input(self):
        with self.assertRaisesRegex(ValueError, "unchanged|input"):
            self.load([example(accepted=["These estimates holds."])])

    def test_protected_entries_need_a_whole_lexical_occurrence(self):
        with self.assertRaisesRegex(ValueError, "protected"):
            self.load([example(protected=["mates"])])

    def test_missing_and_duplicate_protected_values_are_rejected(self):
        for protected in [["not present"], ["estimates", "estimates"]]:
            with self.subTest(protected=protected), self.assertRaisesRegex(ValueError, "protected"):
                self.load([example(protected=protected)])

    def test_corpus_digest_tracks_even_notes_and_selection_provenance(self):
        _, first = self.load([example(notes="First review.")])
        _, second = self.load([example(notes="Second review.")])
        self.assertNotEqual(first, second)


class ScoringTests(unittest.TestCase):
    def status(self, case, output):
        return comparison.assess(case, {"output": output})["status"]

    def test_all_outcomes_are_distinguished(self):
        case = example()
        control = example(text="These estimates hold.")
        self.assertEqual(self.status(case, case["expected"]), "corrected")
        self.assertEqual(self.status(case, case["text"]), "missed")
        self.assertEqual(self.status(case, "The estimates hold."), "other_edit")
        self.assertEqual(self.status(control, control["text"]), "unchanged")
        self.assertEqual(self.status(control, "The estimates hold."), "false_alarm")
        self.assertEqual(comparison.assess(case, {"error": "Failed."})["status"], "error")
        self.assertEqual(comparison.assess(case, {})["status"], "error")
        for output in ["", " \n ", None, 123]:
            with self.subTest(output=output):
                self.assertEqual(comparison.assess(case, {"output": output})["status"], "error")

    def test_accepted_variants_are_exact_but_whitespace_and_nfc_are_ignored(self):
        case = example(text="We have prove cafe\u0301 bounds.",
                       expected="We have proved café bounds.",
                       accepted=["We have proven café bounds."])
        self.assertEqual(self.status(case, "  We have\n proven café bounds. "), "corrected")
        self.assertEqual(self.status(case, "We have proven café bounds!"), "other_edit")

    def test_unsafe_wins_over_success_and_counts_and_order_are_protected(self):
        case = example(text="Barbato and Romito holds MATH and MATH.",
                       expected="Barbato and Romito hold MATH and MATH.",
                       protected=["Barbato", "Romito", "MATH"])
        for output in [
            "Barbados and Romito hold MATH and MATH.",
            "Barbato and Romito hold MATH.",
            "Romito and Barbato hold MATH and MATH.",
            "Barbato and Romito hold MATH and MATH and MATH.",
        ]:
            with self.subTest(output=output):
                self.assertEqual(self.status(case, output), "unsafe")

    def test_protected_words_cannot_be_extended_into_other_words(self):
        case = example(text="Barbato holds MATH.", expected="Barbato holds MATH.",
                       protected=["Barbato", "MATH"])
        for output in ["Barbatos holds MATH.", "Barbato holds MATHS.", "XBarbato holds MATH."]:
            with self.subTest(output=output):
                self.assertEqual(self.status(case, output), "unsafe")

    def test_protection_normalizes_unicode_before_counting(self):
        case = example(text="Andre\u0301 holds MATH.", expected="André holds MATH.",
                       protected=["Andre\u0301", "MATH"])
        self.assertEqual(self.status(case, "André holds MATH."), "unchanged")
        self.assertEqual(self.status(case, "Andre holds MATH."), "unsafe")

    def test_missing_cases_are_unavailable_not_successful_unchanged_predictions(self):
        cases = [example("bad"), example("clean", text="These estimates hold.")]
        summary = comparison.summarize(cases, [])
        self.assertEqual(summary["errors"], 2)
        self.assertEqual(summary["evaluated"], 0)
        self.assertIsNone(summary["exact_correction_rate"])
        self.assertIsNone(summary["false_alarm_rate"])
        self.assertIsNone(summary["median_ms"])

    def test_rates_use_evaluated_error_and_control_groups_separately(self):
        cases = [example("fixed"), example("missed"), example("failed"),
                 example("kept", text="These estimates hold."),
                 example("changed", text="These estimates hold.")]
        rows = [{"id": "fixed", "output": "These estimates hold.", "latency_ms": 4},
                {"id": "missed", "output": "These estimates holds.", "latency_ms": 10},
                {"id": "failed", "error": "Unavailable.", "latency_ms": 1},
                {"id": "kept", "output": "These estimates hold.", "latency_ms": 2},
                {"id": "changed", "output": "The estimates hold.", "latency_ms": 20}]
        summary = comparison.summarize(cases, rows)
        self.assertEqual((summary["evaluated"], summary["errors"]), (4, 1))
        self.assertEqual(summary["exact_correction_rate"], .5)
        self.assertEqual(summary["false_alarm_rate"], .5)
        self.assertEqual((summary["median_ms"], summary["p95_ms"]), (7, 20))

    def test_nonfinite_negative_and_boolean_latencies_never_pollute_statistics(self):
        invalid = [float("nan"), float("inf"), -1, True, "12"]
        cases = [example(f"case-{index}") for index in range(len(invalid) + 1)]
        rows = [{"id": case["id"], "output": case["expected"], "latency_ms": value}
                for case, value in zip(cases, [*invalid, 5])]
        summary = comparison.summarize(cases, rows)
        self.assertEqual((summary["median_ms"], summary["p95_ms"]), (5, 5))


class ReportTests(unittest.TestCase):
    def test_all_untrusted_text_is_escaped_in_content_attributes_and_metadata(self):
        attack = "<script>alert('case')</script>\" onmouseover=\"bad"
        case = example(text=attack, expected=attack, source=attack, notes=attack,
                       category=attack, id="safe-id", format="tex")
        data = document(cases=[case], models=[{
            "name": attack, "label": attack, "metadata": {"attack": attack},
            "results": [{"id": "safe-id", "error": attack}],
        }])
        data["runtime"] = {"attack": attack}
        rendered = comparison.render_report(data)
        self.assertNotIn("<script>alert", rendered)
        self.assertNotIn(' onmouseover="bad', rendered)
        self.assertIn("&lt;script&gt;", rendered)
        self.assertIn("&quot;", rendered)
        self.assertEqual(rendered.count("<script>"), 1)  # Only the fixed filter script.

    def test_report_keeps_model_failures_and_valid_alternative_visible(self):
        data = document(cases=[example(accepted=["Those estimates hold."])], models=[
            {"name": "failed", "error": "Download refused.", "results": []},
            {"name": "ok", "results": [{"id": "agreement", "output": "Those estimates hold."}]},
        ])
        rendered = comparison.render_report(data)
        self.assertIn("Download refused.", rendered)
        self.assertIn("Also accepted: Those estimates hold.", rendered)
        self.assertIn("Unavailable", rendered)
        self.assertIn("Exact fix", rendered)
        self.assertIn("disagreement", rendered)
        self.assertEqual(data["models"][0]["summary"]["errors"], 1)
        self.assertEqual(data["models"][1]["summary"]["exact_corrections"], 1)

    def test_saved_json_and_html_retain_the_same_scores_and_grouping(self):
        data = document(cases=[example("plain"), example("math", format="typst")], models=[
            {"name": "fake", "results": [{"id": "plain", "output": "These estimates hold."}]},
        ])
        with tempfile.TemporaryDirectory() as temporary:
            comparison.save_report(data, Path(temporary))
            saved = json.loads((Path(temporary) / "results.json").read_text())
            self.assertTrue((Path(temporary) / "report.html").is_file())
        model = saved["models"][0]
        self.assertEqual(model["summary"]["errors"], 1)
        self.assertEqual(model["groups"]["plain"]["exact_corrections"], 1)
        self.assertEqual(model["groups"]["projected_math"]["errors"], 1)


class AdapterUtilityTests(unittest.TestCase):
    def test_verified_download_is_atomic_and_cache_hits_do_not_redownload(self):
        payload = b"small fake weight artifact"
        checksum = hashlib.sha256(payload).hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "weights.bin"
            with mock.patch.object(models, "urlopen", return_value=io.BytesIO(payload)) as opening:
                self.assertEqual(models._download("https://fixture.invalid/model", target, checksum), target)
                self.assertEqual(target.read_bytes(), payload)
                models._download("https://fixture.invalid/model", target, checksum)
                opening.assert_called_once()
            self.assertEqual(list(Path(temporary).iterdir()), [target])

    def test_corrupt_or_interrupted_download_never_publishes_weights(self):
        checksum = hashlib.sha256(b"expected").hexdigest()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            target = directory / "weights.bin"
            with mock.patch.object(models, "urlopen", return_value=io.BytesIO(b"wrong")), \
                    self.assertRaisesRegex(ValueError, "checksum mismatch"):
                models._download("https://fixture.invalid/model", target, checksum)
            self.assertEqual(list(directory.iterdir()), [])
            with mock.patch.object(models, "urlopen", side_effect=OSError("Interrupted")), \
                    self.assertRaisesRegex(OSError, "Interrupted"):
                models._download("https://fixture.invalid/model", target, checksum)
            self.assertEqual(list(directory.iterdir()), [])
            target.write_bytes(b"corrupt cache")
            with mock.patch.object(models, "urlopen") as opening:
                with self.assertRaisesRegex(ValueError, "Cached artifact checksum mismatch"):
                    models._download("https://fixture.invalid/model", target, checksum)
                opening.assert_not_called()
            self.assertEqual(target.read_bytes(), b"corrupt cache")

    def test_curly_apostrophes_are_tokenized_as_possessives_and_source_glyph_survives(self):
        adapter = object.__new__(models._Adapter)
        adapter.moses = types.SimpleNamespace(tokenize=mock.Mock(return_value=["Tao's", "estimates", "holds", "."]))
        adapter.detokenizer = types.SimpleNamespace(detokenize=mock.Mock(return_value="Tao's estimates hold."))
        original = "Tao’s estimates holds."
        self.assertEqual(adapter._tokens(original), ["Tao's", "estimates", "holds", "."])
        adapter.moses.tokenize.assert_called_with("Tao's estimates holds.", escape=False)
        self.assertEqual(adapter._untokenize("Tao's estimates holds .", original), original)
        self.assertEqual(adapter._untokenize("Tao's estimates hold .", original), "Tao’s estimates hold.")


class WorkerTests(unittest.TestCase):
    def test_adapter_failure_keeps_other_cases_and_full_repeat_timings(self):
        calls = []
        def correct(text):
            calls.append(text)
            if text == "Broken.":
                raise ValueError("Deliberate fixture failure.")
            return "These estimates hold."
        adapter = types.SimpleNamespace(metadata={"model_revision": "fixed"}, correct=correct)
        models = types.ModuleType("grammar_compare.models")
        models.MODEL_SPECS = {"fake": types.SimpleNamespace(label="Fixture adapter")}
        models.load_model = mock.Mock(return_value=adapter)
        torch = types.ModuleType("torch")
        torch.__version__ = "fixture"
        torch.manual_seed = mock.Mock()
        torch.set_num_threads = mock.Mock()
        torch.set_num_interop_threads = mock.Mock()
        request = {"model": "fake", "threads": 2, "warmup": 1, "repeats": 2,
                   "cache_dir": "/unused", "cases": [example("ok"), example("bad", text="Broken.")]}
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.dict(sys.modules, {"grammar_compare.models": models, "torch": torch}), \
                mock.patch.object(worker.importlib.metadata, "version", return_value="fixture"), \
                redirect_stdout(io.StringIO()):
            destination = Path(temporary) / "worker.json"
            result = worker.run(request, destination)
            saved = json.loads(destination.read_text())
        self.assertEqual(saved, result)
        self.assertEqual(len(result["results"][0]["samples_ms"]), 2)
        self.assertEqual(result["results"][0]["distinct_outputs"], ["These estimates hold."])
        self.assertIn("Deliberate fixture failure", result["results"][1]["error"])
        self.assertEqual(len(calls), 4)  # One warmup, two measured, one failed.
        self.assertEqual(result["metadata"]["seed"], 0)
        self.assertEqual(result["metadata"]["warmup_requests"], 1)
        torch.set_num_threads.assert_called_once_with(2)
        models.load_model.assert_called_once()


class CliTests(unittest.TestCase):
    def run_cli(self, argv):
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            return CLI.main(argv)

    def test_validation_and_report_only_never_import_model_dependencies(self):
        program = r'''
import builtins, runpy, sys
original = builtins.__import__
def guarded(name, *args, **kwargs):
    if name.split('.')[0] in {'torch', 'transformers', 'huggingface_hub', 'sacremoses'} or name == 'grammar_compare.models':
        raise AssertionError('Heavyweight import during dependency-free path: ' + name)
    return original(name, *args, **kwargs)
builtins.__import__ = guarded
sys.path.insert(0, sys.argv[1])
sys.argv = [sys.argv[1] + '/compare-grammar.py', *sys.argv[2:]]
runpy.run_path(sys.argv[0], run_name='__main__')
'''
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "results.json"
            source.write_text(json.dumps(document()), encoding="utf-8")
            for args in [["--validate"], ["--report-only", str(source), "--output", str(Path(temporary) / "report")]]:
                completed = subprocess.run([sys.executable, "-c", program, str(SCRIPTS), *args],
                                           capture_output=True, text=True, check=False)
                self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_unknown_selection_fails_instead_of_silently_running_other_cases(self):
        for args in [["--case", "not-a-case"], ["--category", "not-a-category"]]:
            with self.subTest(args=args), self.assertRaisesRegex(ValueError, "Unknown"):
                self.run_cli(["--validate", *args])

    def test_worker_launch_failure_still_writes_an_inspectable_report(self):
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.object(CLI.subprocess, "check_output", side_effect=OSError("no git")), \
                mock.patch.object(CLI.subprocess, "run", side_effect=OSError("No worker available.")):
            result = self.run_cli(["--models", "coedit", "--limit", "1", "--output", temporary])
            saved = json.loads((Path(temporary) / "results.json").read_text())
            self.assertTrue((Path(temporary) / "report.html").is_file())
        self.assertEqual(result, 1)
        self.assertIn("No worker available.", saved["models"][0]["error"])
        self.assertEqual(saved["models"][0]["summary"]["errors"], 1)

    def test_zero_exit_worker_with_missing_rows_is_still_a_failed_run(self):
        with tempfile.TemporaryDirectory() as temporary, \
                mock.patch.object(CLI.subprocess, "check_output", side_effect=OSError("no git")), \
                mock.patch.object(CLI.subprocess, "run", return_value=types.SimpleNamespace(returncode=0)):
            result = self.run_cli(["--models", "coedit", "--limit", "1", "--output", temporary])
            saved = json.loads((Path(temporary) / "results.json").read_text())
        self.assertEqual(result, 1)
        self.assertEqual(saved["models"][0]["summary"]["errors"], 1)

    def test_invalid_or_incomplete_worker_result_fails_without_hiding_other_evidence(self):
        payloads = ["not json", "[]", json.dumps({"results": [{"id": "clean-agreement"}]}),
                    json.dumps({"results": [{"id": "other", "output": "Unexpected."}]}),
                    json.dumps({"results": [{"id": "clean-agreement", "output": "Duplicate."}] * 2})]
        for payload in payloads:
            with self.subTest(payload=payload), tempfile.TemporaryDirectory() as temporary:
                def finish(command, **kwargs):
                    Path(command[-1]).write_text(payload)
                    return types.SimpleNamespace(returncode=0)
                with mock.patch.object(CLI.subprocess, "check_output", side_effect=OSError("no git")), \
                        mock.patch.object(CLI.subprocess, "run", side_effect=finish):
                    result = self.run_cli(["--models", "coedit", "--limit", "1", "--output", temporary])
                self.assertEqual(result, 1)
                saved = json.loads((Path(temporary) / "results.json").read_text())
                self.assertGreater(saved["models"][0]["summary"]["errors"], 0)
                self.assertTrue((Path(temporary) / "report.html").is_file())

    def test_timeout_retains_partial_results_without_reusing_a_previous_worker_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            workers = output / "per-worker"
            workers.mkdir()
            stale = workers / "coedit.json"
            stale.write_text(json.dumps({"name": "coedit", "results": [{"id": "clean-agreement", "output": "stale"}]}))
            def timeout(command, **kwargs):
                self.assertFalse(stale.exists())
                Path(command[-1]).write_text(json.dumps({"name": "coedit", "results": [
                    {"id": "clean-agreement", "output": "These estimates hold for every bounded operator."}
                ]}))
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            with mock.patch.object(CLI.subprocess, "check_output", side_effect=OSError("no git")), \
                    mock.patch.object(CLI.subprocess, "run", side_effect=timeout):
                result = self.run_cli(["--models", "coedit", "--limit", "2", "--output", temporary])
            saved = json.loads((output / "results.json").read_text())
        self.assertEqual(result, 1)
        model = saved["models"][0]
        self.assertEqual(model["summary"]["evaluated"], 1)
        self.assertEqual(model["summary"]["errors"], 1)
        self.assertIn("partial results retained", model["error"])


class ResearchPassageTests(unittest.TestCase):
    def research(self, identifier="research", **overrides):
        case = example(identifier, notes="Unreviewed excerpt; context and references need review.",
                       cohort="arxiv", kind="excerpt", **overrides)
        del case["expected"]
        return case

    def load(self, case):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "cases.json"
            path.write_text(json.dumps({"version": 1, "cases": [case]}), encoding="utf-8")
            return comparison.load_corpus(path)

    def test_unscored_passages_preserve_source_metadata_and_protection(self):
        case = self.research(text="Barbato holds MATH.", protected=["Barbato", "MATH"],
                             citation={"title": "Paper title", "url": "https://arxiv.org/pdf/1234.56789v1#page=2",
                                       "location": "Page 2", "authors": ["Example Author"]})
        loaded, _ = self.load(case)
        self.assertEqual(loaded["cases"], [case])
        self.assertEqual(comparison.assess(case, {"output": case["text"]})["status"], "unscored_unchanged")
        self.assertEqual(comparison.assess(case, {"output": "Barbato proves MATH."})["status"], "unscored_edit")
        self.assertEqual(comparison.assess(case, {"output": "Barbados holds MATH."})["status"], "unsafe")

    def test_unscored_passages_require_notes_and_cannot_smuggle_in_references(self):
        for overrides, message in [({"notes": ""}, "notes"), ({"accepted": []}, "accepted"),
                                   ({"expected": None}, "expected"), ({"expected": ""}, "expected")]:
            case = self.research()
            case.update(overrides)
            with self.subTest(overrides=overrides), self.assertRaisesRegex(ValueError, message):
                self.load(case)

    def test_unscored_metadata_is_validated(self):
        for overrides, message in [({"protected": ["missing"]}, "protected"),
                                   ({"kind": "fragment"}, "kind"), ({"cohort": "Arxiv"}, "cohort"),
                                   ({"citation": {"title": "Paper", "url": "javascript:alert(1)"}}, "HTTP"),
                                   ({"citation": {"title": "Paper"}}, "citation"),
                                   ({"citation": {"title": "Paper", "url": "https://arxiv.org", "authors": "Name"}}, "authors")]:
            case = self.research()
            case.update(overrides)
            with self.subTest(overrides=overrides), self.assertRaisesRegex(ValueError, message):
                self.load(case)

    def test_unscored_results_never_change_quality_denominators(self):
        cases = [example("fixed"), example("control", text="These estimates hold."),
                 self.research("edited"), self.research("kept"), self.research("missing"),
                 self.research("unsafe", text="Barbato holds MATH.", protected=["MATH"])]
        rows = [{"id": "fixed", "output": cases[0]["expected"], "latency_ms": 2},
                {"id": "control", "output": cases[1]["text"], "latency_ms": 4},
                {"id": "edited", "output": "These estimates hold.", "latency_ms": 6},
                {"id": "kept", "output": cases[3]["text"], "latency_ms": 8},
                {"id": "unsafe", "output": "Barbato holds mathematics.", "latency_ms": 10}]
        summary = comparison.summarize(cases, rows)
        self.assertEqual((summary["error_cases"], summary["correct_controls"]), (1, 1))
        self.assertEqual((summary["exact_correction_rate"], summary["false_alarm_rate"]), (1, 0))
        self.assertEqual((summary["unscored_cases"], summary["unscored_evaluated"]), (4, 3))
        self.assertEqual((summary["unscored_edits"], summary["unscored_unchanged"],
                          summary["unscored_unsafe"], summary["unscored_errors"]), (1, 1, 1, 1))
        self.assertEqual((summary["missed"], summary["other_edits"], summary["changed_controls"]), (0, 0, 0))
        self.assertEqual(summary["median_ms"], 6)
        research_only = comparison.summarize(cases[2:], rows)
        self.assertIsNone(research_only["exact_correction_rate"])
        self.assertIsNone(research_only["false_alarm_rate"])

    def test_report_separates_cohorts_and_shows_unscored_sources_without_a_reference(self):
        case = self.research(citation={"title": "Paper <title>", "url": "https://arxiv.org/pdf/1234.56789v1#page=2",
                                       "location": "Page 2"})
        data = document(cases=[example("baseline"), case], models=[{
            "name": "fake", "results": [{"id": "baseline", "output": "These estimates hold."},
                                          {"id": "research", "output": "These estimates hold."}]}])
        rendered = comparison.render_report(data)
        self.assertIn("Unscored research passage", rendered)
        self.assertIn("Unscored edit · review", rendered)
        self.assertIn("research passages are excluded", rendered)
        self.assertIn("href='https://arxiv.org/pdf/1234.56789v1#page=2'", rendered)
        self.assertIn("Paper &lt;title&gt;", rendered)
        self.assertEqual(rendered.count("<div class='expected'>"), 1)
        cohorts = data["models"][0]["cohorts"]
        self.assertEqual(cohorts["baseline"]["exact_corrections"], 1)
        self.assertEqual(cohorts["arxiv"]["unscored_edits"], 1)
        self.assertIsNone(cohorts["arxiv"]["exact_correction_rate"])

    def test_report_only_never_turns_untrusted_citation_urls_into_executable_links(self):
        case = self.research(citation={"title": "Unsafe source", "url": "javascript:alert(1)"})
        rendered = comparison.render_report(document(cases=[case]))
        self.assertNotIn("href='javascript:", rendered)
        self.assertIn("Unsafe source", rendered)


class RealPaperCorpusTests(unittest.TestCase):
    def test_expanded_corpus_preserves_all_baseline_fixtures(self):
        baseline, _ = comparison.load_corpus(SCRIPTS / "grammar_compare/corpus.json")
        expanded, _ = comparison.load_corpus(SCRIPTS / "grammar_compare/arxiv-corpus.json")
        controls = [case for case in expanded["cases"] if case["cohort"] == "baseline"]
        without_labels = [{key: value for key, value in case.items() if key not in ("cohort", "kind")}
                          for case in controls]
        self.assertEqual(len(expanded["cases"]), 158)
        self.assertEqual(len(controls), 64)
        self.assertEqual(without_labels, baseline["cases"])

    def test_real_papers_are_distinct_and_context_dependent_excerpts_remain_unscored(self):
        expanded, _ = comparison.load_corpus(SCRIPTS / "grammar_compare/arxiv-corpus.json")
        papers = [case for case in expanded["cases"] if case["cohort"] == "arxiv"]
        self.assertEqual(len(papers), 94)
        self.assertEqual(sum("expected" in case for case in papers), 56)
        self.assertEqual(sum("expected" not in case for case in papers), 38)
        excerpts = [case for case in papers if case["kind"] == "excerpt"]
        self.assertEqual(len(excerpts), 35)
        self.assertTrue(all("expected" not in case for case in excerpts))
        paper_ids = []
        for case in papers:
            with self.subTest(case=case["id"]):
                citation = case["citation"]
                match = re.fullmatch(r"https://arxiv\.org/(?:pdf|html)/((?:\d{4}\.\d{4,5}|math/\d{7}))v\d+(?:#[A-Za-z0-9.=]+)?", citation["url"])
                self.assertIsNotNone(match, "Real-paper citations must pin an arXiv version.")
                paper_ids.append(match.group(1))
                self.assertTrue(citation["location"].strip())
                self.assertTrue(case["notes"].strip())
                if "expected" not in case:
                    self.assertIn("unscored", case["notes"].lower())
        self.assertEqual(len(set(paper_ids)), 94)


if __name__ == "__main__":
    unittest.main()
