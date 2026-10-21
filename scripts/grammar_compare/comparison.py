"""Dependency-free corpus validation, conservative scoring and offline reports."""
from __future__ import annotations

from collections import Counter
from difflib import SequenceMatcher
import hashlib
import html
import json
import math
from pathlib import Path
import re
import statistics
import unicodedata
from urllib.parse import urlsplit

MAX_CORPUS_BYTES = 2 * 1024 * 1024
MAX_CASES = 1000


def canonical(text: str) -> str:
    """Ignore transport whitespace and canonical Unicode, not words/punctuation."""
    return " ".join(unicodedata.normalize("NFC", text).split())


def load_corpus(path: Path) -> tuple[dict, str]:
    data = path.read_bytes()
    if len(data) > MAX_CORPUS_BYTES:
        raise ValueError("Corpus must be 2 MiB or smaller.")
    corpus = json.loads(data)
    if not isinstance(corpus, dict) or corpus.get("version") != 1:
        raise ValueError("Corpus must be an object with version: 1.")
    cases = corpus.get("cases")
    if not isinstance(cases, list) or not 1 <= len(cases) <= MAX_CASES:
        raise ValueError("Corpus must contain between 1 and 1000 cases.")
    seen = set()
    for index, case in enumerate(cases, 1):
        where = f"Case {index}"
        if not isinstance(case, dict):
            raise ValueError(f"{where} must be an object.")
        for key in ("id", "category", "text", "format"):
            if not isinstance(case.get(key), str) or not case[key].strip():
                raise ValueError(f"{where}: {key} must be a nonempty string.")
        if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,99}", case["id"]):
            raise ValueError(f"{where}: id must use lowercase letters, numbers, - or _.")
        if case["id"] in seen:
            raise ValueError(f"Duplicate case id: {case['id']}")
        seen.add(case["id"])
        if case["format"] not in ("plain", "tex", "typst"):
            raise ValueError(f"{where}: format must be plain, tex or typst.")
        if "expected" in case and (not isinstance(case["expected"], str) or not case["expected"].strip()):
            raise ValueError(f"{where}: expected must be a nonempty string when present.")
        if len(case["text"]) > 10000 or len(case.get("expected", "")) > 10000:
            raise ValueError(f"{where}: text/expected must be at most 10000 characters.")
        for key in ("source", "notes"):
            if key in case and not isinstance(case[key], str):
                raise ValueError(f"{where}: {key} must be a string.")
        for key in ("protected", "accepted"):
            values = case.get(key, [])
            if not isinstance(values, list) or any(not isinstance(v, str) or not v.strip() for v in values):
                raise ValueError(f"{where}: {key} must be a list of nonempty strings.")
        if "expected" not in case:
            if not case.get("notes", "").strip():
                raise ValueError(f"{where}: unscored research passages require notes explaining their status.")
            if "accepted" in case:
                raise ValueError(f"{where}: unscored research passages cannot have accepted references.")
        if "cohort" in case and (not isinstance(case["cohort"], str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,99}", case["cohort"])):
            raise ValueError(f"{where}: cohort must use lowercase letters, numbers, - or _.")
        if "kind" in case and case["kind"] not in ("sentence", "excerpt", "paragraph"):
            raise ValueError(f"{where}: kind must be sentence, excerpt or paragraph.")
        if "citation" in case:
            citation = case["citation"]
            if not isinstance(citation, dict) or any(not isinstance(citation.get(key), str) or not citation[key].strip() for key in ("title", "url")):
                raise ValueError(f"{where}: citation requires nonempty title and url strings.")
            if not _safe_source_url(citation["url"]):
                raise ValueError(f"{where}: citation url must be an absolute HTTP(S) URL.")
            if "location" in citation and not isinstance(citation["location"], str):
                raise ValueError(f"{where}: citation location must be a string.")
            if "authors" in citation and (not isinstance(citation["authors"], list) or any(not isinstance(author, str) or not author.strip() for author in citation["authors"])):
                raise ValueError(f"{where}: citation authors must be nonempty strings.")
        protected = case.get("protected", [])
        if len(protected) != len(set(protected)):
            raise ValueError(f"{where}: protected entries must be unique.")
        if any(not _protected_matches(case["text"], value) for value in protected):
            raise ValueError(f"{where}: protected text must occur as a complete protected value in the input.")
        if "expected" not in case:
            continue
        for target in [case["expected"], *case.get("accepted", [])]:
            if protected_issues(case, target):
                raise ValueError(f"{where}: reference changes protected text.")
        unchanged = canonical(case["text"]) == canonical(case["expected"])
        if unchanged and case.get("accepted"):
            raise ValueError(f"{where}: unchanged controls cannot accept rewritten alternatives.")
        if not unchanged and any(canonical(value) == canonical(case["text"]) for value in case.get("accepted", [])):
            raise ValueError(f"{where}: an error case cannot accept unchanged input as a correction.")
    return corpus, hashlib.sha256(data).hexdigest()


def _safe_source_url(value: str) -> bool:
    try:
        parsed = urlsplit(value)
        return parsed.scheme in ("http", "https") and bool(parsed.netloc) and not any(char.isspace() for char in value)
    except ValueError:
        return False


def _protected_matches(text: str, value: str):
    text, value = unicodedata.normalize("NFC", text), unicodedata.normalize("NFC", value)
    left = r"(?<!\w)" if re.match(r"\w", value[0]) else ""
    right = r"(?!\w)" if re.match(r"\w", value[-1]) else ""
    return list(re.finditer(left + re.escape(value) + right, text))


def protected_issues(case: dict, output: str) -> list[str]:
    """Protect exact spelling, occurrence counts and ordering, not just presence."""
    original = unicodedata.normalize("NFC", case["text"])
    output = unicodedata.normalize("NFC", output)
    protected = [unicodedata.normalize("NFC", value) for value in case.get("protected", [])]
    issues = [value for value in protected if len(_protected_matches(original, value)) != len(_protected_matches(output, value))]

    def order(text):
        positions = []
        for value in protected:
            positions.extend((match.start(), -len(value), value) for match in _protected_matches(text, value))
        return [value for _, _, value in sorted(positions)]

    if not issues and order(original) != order(output):
        issues.append("protected text order")
    return issues


def assess(case: dict, result: dict) -> dict:
    if result.get("error") or not isinstance(result.get("output"), str) or not result["output"].strip():
        return {"status": "error", "error": result.get("error", "No output returned.")}
    source, output = canonical(case["text"]), canonical(result["output"])
    issues = protected_issues(case, result["output"])
    if issues:
        return {"status": "unsafe", "protected_issues": issues}
    if "expected" not in case:
        return {"status": "unscored_unchanged" if output == source else "unscored_edit", "protected_issues": []}
    targets = {canonical(target) for target in [case["expected"], *case.get("accepted", [])]}
    needs_change = source != canonical(case["expected"])
    if not needs_change:
        status = "false_alarm" if output != source else "unchanged"
    elif output in targets:
        status = "corrected"
    else:
        status = "missed" if output == source else "other_edit"
    return {"status": status, "protected_issues": issues}


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)]


def summarize(cases: list[dict], rows: list[dict]) -> dict:
    indexed = {row["id"]: row for row in rows}
    totals = Counter()
    timings = []
    correct_controls = 0
    error_cases = 0
    for case in cases:
        row = indexed.get(case["id"], {"error": "Case not evaluated."})
        assessment = assess(case, row)
        status = assessment["status"]
        totals[status] += 1
        if "expected" not in case:
            totals["unscored_cases"] += 1
            if status == "error":
                totals["unscored_errors"] += 1
            elif status == "unsafe":
                totals["unscored_unsafe"] += 1
        if status == "error":
            continue
        if "expected" not in case:
            totals["unscored_evaluated"] += 1
        elif canonical(case["text"]) == canonical(case["expected"]):
            correct_controls += 1
            if status in ("false_alarm", "unsafe"):
                totals["changed_controls"] += 1
        else:
            error_cases += 1
        latency = row.get("latency_ms")
        if type(latency) in (int, float) and math.isfinite(latency) and latency >= 0:
            timings.append(latency)
    return {
        "cases": len(cases), "evaluated": len(cases) - totals["error"],
        "errors": totals["error"], "correct_controls": correct_controls,
        "changed_controls": totals["changed_controls"], "error_cases": error_cases,
        "exact_corrections": totals["corrected"], "missed": totals["missed"],
        "other_edits": totals["other_edit"], "unsafe": totals["unsafe"],
        "unscored_cases": totals["unscored_cases"], "unscored_evaluated": totals["unscored_evaluated"],
        "unscored_edits": totals["unscored_edit"], "unscored_unchanged": totals["unscored_unchanged"],
        "unscored_unsafe": totals["unscored_unsafe"], "unscored_errors": totals["unscored_errors"],
        "false_alarm_rate": totals["changed_controls"] / correct_controls if correct_controls else None,
        "exact_correction_rate": totals["corrected"] / error_cases if error_cases else None,
        "median_ms": statistics.median(timings) if timings else None,
        "p95_ms": percentile(timings, .95),
    }


def attach_scores(document: dict) -> dict:
    cases = document["corpus"]["cases"]
    for model in document["models"]:
        rows = model.get("results", [])
        for row in rows:
            case = next((case for case in cases if case["id"] == row["id"]), None)
            if case is not None:
                row["assessment"] = assess(case, row)
        model["summary"] = summarize(cases, rows)
        model["cohorts"] = {
            name: summarize([case for case in cases if case.get("cohort", "baseline") == name], rows)
            for name in sorted({case.get("cohort", "baseline") for case in cases})
        }
        model["groups"] = {
            name: summarize(selected, rows)
            for name, selected in {
                "plain": [case for case in cases if case["format"] == "plain"],
                "projected_math": [case for case in cases if case["format"] != "plain"],
            }.items()
        }
    return document


def edited_html(source: str, target: str) -> str:
    """Render the actual edit, escaping every text fragment before HTML use."""
    parts = []
    for op, a, b, c, d in SequenceMatcher(None, source, target, autojunk=False).get_opcodes():
        if op == "equal":
            parts.append(html.escape(source[a:b]))
        else:
            if a != b:
                parts.append(f"<del>{html.escape(source[a:b])}</del>")
            if c != d:
                parts.append(f"<ins>{html.escape(target[c:d])}</ins>")
    return "".join(parts)


def _ms(value) -> str:
    return "—" if value is None else (f"{value:.3f} ms" if 0 < value < 1 else f"{value:,.1f} ms")


def _ratio(num, denom) -> str:
    return f"{num}/{denom}" if denom else "—"


def render_report(document: dict) -> str:
    attach_scores(document)
    esc = lambda value: html.escape(str(value), quote=True)
    models = document["models"]
    cases = document["corpus"]["cases"]
    cards = []
    harper_adapter = bool(document.get("harper_adapter"))
    for model in models:
        summary = model["summary"]
        metadata = model.get("metadata", {})
        details = json.dumps(metadata, indent=2, ensure_ascii=False)
        errors = f"<p class='bad'>{esc(model['error'])}</p>" if model.get("error") else ""
        groups = "".join(
            f"<tr><td>{esc(name.replace('_', ' '))}</td><td>{_ratio(s['exact_corrections'], s['error_cases'])}</td>"
            f"<td>{_ratio(s['changed_controls'], s['correct_controls'])}</td><td>{s['unsafe']}</td>"
            f"<td>{s['unscored_edits']}/{s['unscored_evaluated']}</td></tr>"
            for name, s in [*model["cohorts"].items(), *([] if harper_adapter else model["groups"].items())]
        )
        latency_label = "median timed check" if harper_adapter else "median inference"
        guard = metadata.get("source_mapping") if harper_adapter else None
        guard_note = (f"<p class='muted'>Source guard rejected {guard['rejected_proposals']}/{guard['changed_outputs']} changed proposals. Mappable coordinates do not imply a correct edit.</p>" if guard else "")
        cards.append(f"""<section class='card'><h2>{esc(model.get('label', model['name']))}</h2>
{errors}<div class='stats'><div><strong>{_ratio(summary['exact_corrections'], summary['error_cases'])}</strong>exact fixes ↑</div>
<div><strong>{_ratio(summary['changed_controls'], summary['correct_controls'])}</strong>correct text changed ↓</div>
<div><strong>{summary['unsafe']}</strong>protected text damaged ↓</div>
<div><strong>{_ms(summary['median_ms'])}</strong>{latency_label} ↓</div></div>
<p class='muted'>p95 {_ms(summary['p95_ms'])} · load {_ms(model.get('load_ms'))} · peak process RSS {esc(model.get('peak_rss_mib', '—'))} MiB · {summary['errors']} unavailable cases</p>{guard_note}
<table class='groups'><thead><tr><th>Input</th><th>Exact fixes</th><th>Correct changed</th><th>Unsafe</th><th>Unscored edits</th></tr></thead><tbody>{groups}</tbody></table>
<details><summary>Model and runtime details</summary><pre>{esc(details)}</pre></details></section>""")
    indexed = [{row['id']: row for row in model.get('results', [])} for model in models]
    table_rows = []
    status_labels = {"unchanged": "Kept correct", "corrected": "Exact fix", "false_alarm": "Unnecessary edit", "unsafe": "Protected text changed", "missed": "Missed", "other_edit": "Other edit · review", "error": "Unavailable", "unscored_edit": "Unscored edit · review", "unscored_unchanged": "Unscored · unchanged"}
    for case in cases:
        outcomes = [lookup.get(case['id'], {'error': model.get('error', 'Case not evaluated.')}) for lookup, model in zip(indexed, models)]
        assessments = [assess(case, row) for row in outcomes]
        statuses = [a['status'] for a in assessments]
        outputs = {canonical(row['output']) for row in outcomes if isinstance(row.get('output'), str)}
        flags = set(statuses)
        if len(outputs) > 1:
            flags.add('disagreement')
        scored = "expected" in case
        if not scored:
            flags.add('unscored')
        label = ('Correct control' if canonical(case['text']) == canonical(case['expected']) else 'Grammar error') if scored else 'Unscored research passage'
        source = f"<details><summary>Original {esc(case['format'])} source</summary><pre>{esc(case['source'])}</pre></details>" if case.get('source') else ''
        notes = f"<p class='muted'>{esc(case['notes'])}</p>" if case.get('notes') else ''
        alternatives = ''.join(f"<p>Also accepted: {esc(value)}</p>" for value in case.get('accepted', []))
        expected = (('Keep unchanged.' if label == 'Correct control' else edited_html(case['text'], case['expected'])) if scored else '')
        reference = f"<div class='expected'>{expected}</div>" if scored else ''
        citation = case.get('citation', {})
        provenance = ''
        if citation:
            title = esc(citation.get('title', 'Source'))
            url = citation.get('url', '')
            linked = f"<a href='{esc(url)}' rel='noreferrer'>{title}</a>" if isinstance(url, str) and _safe_source_url(url) else title
            provenance = f"<p class='muted'>{linked}<small>{esc(citation.get('location', ''))}</small></p>"
        kind = f" · {esc(case['kind'])}" if case.get('kind') else ''
        cells = [f"<td class='case'><b>{esc(case['id'])}</b><small>{esc(case['category'])} · {esc(label)} · {esc(case['format'])}{kind}</small><p>{esc(case['text'])}</p>{reference}{alternatives}{notes}{provenance}{source}</td>"]
        for row, assessment in zip(outcomes, assessments):
            status = assessment['status']
            body = f"<pre class='bad'>{esc(assessment['error'])}</pre>" if status == 'error' else f"<p class='diff'>{edited_html(case['text'], row['output'])}</p>"
            instability = " · varying outputs" if len(row.get('distinct_outputs', [])) > 1 else ''
            timing = _ms(row.get('latency_ms')) + instability
            unsafe = f"<small>Changed: {esc(', '.join(assessment.get('protected_issues', [])))}</small>" if assessment.get('protected_issues') else ''
            mapping = row.get("source_mapping")
            source_guard = ""
            if isinstance(mapping, dict):
                guard_label = "No source edits" if not mapping.get("edits") and not mapping.get("rejected") else ("Source coordinates mappable" if mapping.get("all_safe") else "Source edit rejected")
                guard_details = {key: mapping.get(key, []) for key in ("edits", "rejected")}
                source_guard = f"<details><summary>{esc(guard_label)}</summary><pre>{esc(json.dumps(guard_details, ensure_ascii=False, indent=2))}</pre></details>"
            lint_details = ""
            if "shown_lints" in row:
                lints = row["shown_lints"]
                lint_details = f"<details><summary>{len(lints)} Harper diagnostics</summary><pre>{esc(json.dumps(lints, ensure_ascii=False, indent=2))}</pre></details>"
            cells.append(f"<td><span class='badge {status}'>{status_labels[status]}</span>{body}{unsafe}<small>{esc(timing)}</small>{source_guard}{lint_details}</td>")
        searchable = esc(' '.join([case['id'], case['category'], case['text'], case.get('source', '')]).lower())
        table_rows.append(f"<tr data-flags='{esc(' '.join(sorted(flags)))}' data-format='{esc(case['format'])}' data-search='{searchable}'>{''.join(cells)}</tr>")
    headings = ''.join(f"<th>{esc(model.get('label', model['name']))}</th>" for model in models)
    corpus_note = f"{len(cases)} cases · {len({case['text'] for case in cases})} unique inputs · {sum('expected' in case and canonical(case['text']) == canonical(case['expected']) for case in cases)} unchanged controls · {sum('expected' not in case for case in cases)} unscored research passages"
    run_meta = {key: value for key, value in document.items() if key not in ('corpus', 'models')}
    timing_note = ("ML timing includes uncached inference with a loaded worker. Harper curated times only single-paragraph documents using a fresh Checker and resident process dictionary; unchanged cache hits are measured separately in the token export. Harper spelling times only the warm linter on prebuilt Documents, excluding parsing, tagging and diagnostic filtering. These scopes are not equivalent. Peak RSS is the whole isolated ML worker; no isolated Harper RSS is claimed."
                   if harper_adapter else "Latency covers uncached model inference on all evaluated passages, separate from model loading and warmup. It does not measure editor typing latency. Peak RSS is for an isolated worker including the runtime and model, not weights alone.")
    input_note = ("Harper receives actual production paragraph source plus masked tokens. ML receives an explicit text serialization of those tokens, with protected x standing for opaque math/code. This exercises the parser but is not a lossless plain-text equivalent of Harper input. Harper rows illustrate first compatible suggestions; they are not automatic app edits. Source mapping checks geometry, not meaning or syntax correctness. Spelling-only timing excludes parsing/Document construction; see runtime details before comparing speed."
                  if document.get("harper_adapter") else
                  "All models receive identical prose. Unscored research passages have no approved reference and do not contribute to quality rates. TeX/Typst source is shown for context; this comparison does not test the editor’s parser. Protected math placeholders carry no mathematical meaning.")
    return """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Mathematical grammar comparison</title><style>
:root{color-scheme:light dark;--bg:#f4f6fa;--panel:#fff;--text:#202b3b;--muted:#596b81;--line:#d8e0eb;--green:#096b50;--red:#a72a36;--accent:#315fcb}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 system-ui,sans-serif}main{max-width:1700px;margin:auto;padding:24px}h1{font-size:24px;margin:0 0 6px}h2{font-size:17px;margin:0 0 12px}p{margin:8px 0}small{display:block;color:var(--muted);font-size:12px}.muted{color:var(--muted)}.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:12px;margin:20px 0}.card{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:16px}.stats{display:grid;grid-template-columns:1fr 1fr;gap:12px}.stats strong{display:block;font-size:23px;font-weight:650}.stats div{color:var(--muted);font-size:12px}.stats strong{color:var(--text)}table{border-collapse:collapse;width:100%}.groups{font-size:12px;margin:10px 0}.groups th,.groups td{text-align:left;padding:4px;border-bottom:1px solid var(--line)}details{margin-top:8px}summary{cursor:pointer;color:var(--accent)}pre{white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.5 ui-monospace,monospace}.toolbar{display:flex;flex-wrap:wrap;gap:10px;align-items:center;padding:12px 0}input,select{font:inherit;color:inherit;background:var(--panel);border:1px solid var(--line);border-radius:5px;padding:7px}input{min-width:240px;flex:1}label{display:flex;align-items:center;gap:6px}.table-wrap{overflow:auto;border:1px solid var(--line);border-radius:8px;background:var(--panel)}.results{table-layout:fixed;min-width:1000px}.results th{position:sticky;top:0;background:var(--panel);text-align:left;padding:12px;z-index:1;border-bottom:2px solid var(--line)}.results td{padding:12px;vertical-align:top;overflow-wrap:anywhere;border-bottom:1px solid var(--line);border-right:1px solid var(--line)}.results th:first-child{width:32%}.case b{font-size:12px}.expected{color:var(--muted);font-size:13px}.badge{display:inline-block;padding:2px 7px;border-radius:4px;font-size:12px;background:#e9eef6;color:var(--muted)}.corrected{background:#d9f4e8;color:var(--green)}.false_alarm,.unsafe,.error{background:#ffe2e5;color:var(--red)}.missed,.other_edit{background:#fff0cb;color:#795500}del{background:#ffe2e5;color:var(--red);text-decoration:line-through}ins{background:#d9f4e8;color:var(--green);text-decoration:none}.bad{color:var(--red)}[hidden]{display:none!important}footer{margin:24px 0;color:var(--muted)}
@media(prefers-color-scheme:dark){:root{--bg:#151b24;--panel:#1d2633;--text:#e4ebf5;--muted:#a5b4c8;--line:#344258;--accent:#9abaff}.badge{background:#344258;color:#d1ddec}.corrected,ins{background:#194c3c;color:#a0ebc5}.false_alarm,.unsafe,.error,del{background:#552b36;color:#ffc0c7}.missed,.other_edit{background:#514124;color:#ffe1a2}}
</style><main><h1>Mathematical grammar comparison</h1><p class="muted">A curated developer test set, not a general grammar benchmark. Lower unnecessary edits matters as much as finding errors.</p>
""" + f"<p>{esc(input_note)}</p>" + f"<p class='muted'>{esc(corpus_note)}</p><div class='cards'>{''.join(cards)}</div>" + """<div class="toolbar"><input id="search" aria-label="Filter cases" placeholder="Search sentences, names or categories"><label>Show <select id="outcome"><option value="">All outcomes</option><option value="disagreement">Models disagree</option><option value="false_alarm">Unnecessary edits</option><option value="unsafe">Protected text changed</option><option value="missed">Missed errors</option><option value="other_edit">Other edits to review</option><option value="corrected">Exact fixes</option><option value="unscored">Unscored research passages</option><option value="error">Unavailable cases</option></select></label><label>Input <select id="format"><option value="">All</option><option value="plain">Plain prose</option><option value="tex">TeX projection</option><option value="typst">Typst projection</option></select></label><span id="count" class="muted"></span></div>
""" + f"<div class='table-wrap'><table class='results'><thead><tr><th>Input / reference where scored</th>{headings}</tr></thead><tbody>{''.join(table_rows)}</tbody></table></div>" + f"""<footer><p>Exact fixes accept only the listed references; “other edit” requires human review and may be valid. Unavailable cases and unscored research passages are excluded from quality-rate denominators and reported separately. Editing an unscored passage is not proof of a correction; leaving it unchanged is not scored as a miss. Whitespace and Unicode NFC are normalised for scoring; words and punctuation are not.</p><p>{esc(timing_note)}</p><details><summary>Run metadata</summary><pre>{esc(json.dumps(run_meta, indent=2, ensure_ascii=False))}</pre></details></footer></main>""" + """<script>
const search=document.querySelector('#search'),outcome=document.querySelector('#outcome'),format=document.querySelector('#format'),rows=[...document.querySelectorAll('tbody tr[data-flags]')];
function filter(){let n=0;for(const row of rows){const ok=row.dataset.search.includes(search.value.toLowerCase())&&(!outcome.value||row.dataset.flags.split(' ').includes(outcome.value))&&(!format.value||row.dataset.format===format.value);row.hidden=!ok;if(ok)n++}document.querySelector('#count').textContent=`${n} / ${rows.length} cases`}
for(const control of [search,outcome,format])control.addEventListener('input',filter);filter();
</script></html>"""


def save_report(document: dict, output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    attach_scores(document)
    (output / "results.json").write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    (output / "report.html").write_text(render_report(document), encoding="utf-8")
