"""Native TeX journeys; fixtures are real files and tools are never mocked."""

TEX_JOURNEYS = ("tex_editing", "tex_intelligence", "tex_save", "harper_math_prose")
TEX_SOURCE = "\\documentclass{article}\n\\begin{document}\nA sample with $\\alpha + \\alpha$.\n\\end{document}\n"


HARPER_TYPOS = "This sentence contains a misspelld word.\nThis is the the simple result.\n"


def harper_fresh(document, source, path, locations):
    """Require the completed writing result, not merely an idle old result."""
    from desktop_ui_journeys import fingerprint
    return document["path"] == str(path) \
        and document["source_fingerprint"] == fingerprint(source) \
        and document["source_bytes"] == len(source.encode("utf-8")) \
        and document["writing_current"] and not document["writing_pending"] \
        and document["writing_count"] == len(locations) \
        and document["writing_locations"] == locations \
        and document["writing_lines"] == [location[0] for location in locations] \
        and document["writing_paths"] == [str(path)] * len(locations)


def assert_harper_stale(document, previous):
    """An edit must retain its last published result until the new one arrives."""
    if document["writing_current"] or not document["writing_pending"]:
        raise AssertionError("did not observe Harper's stale result during the edit debounce")
    for field in ("writing_count", "writing_fingerprint", "writing_lines",
                  "writing_locations", "writing_paths"):
        if document[field] != previous[field]:
            raise AssertionError("Harper discarded or changed stale diagnostics before refreshing")


class TexJourneys:
    def tex_document(self):
        from pathlib import Path
        import time
        path = self.directory / "journey.tex"
        if self.snapshot()["document"]["path"] != str(path):
            path.write_text(TEX_SOURCE)
            self.focus_document_window()
            self.key(31)  # Open a named TeX document, not an untyped scratch tab.
            time.sleep(0.8)
            self.key(5, "cmd+shift")
            time.sleep(0.3)
            self.key(0)
            self.native("text", str(path))
            self.key(36, "none")
            time.sleep(0.5)
            self.native("ax-press-title", "Open")
            self.wait("TeX document opened", lambda d: d["path"] == str(path), timeout=15)
        self.click("editor.source")
        return Path(path)

    def tex_settings(self, replacements):
        """Edit the complete settings schema through its real JSON editor."""
        self.settings()
        if not self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
        self.wait("JSON settings ready", lambda d: d["settings_json"]["visible"])
        for old, new in replacements:
            self.key(3)
            self.wait_target("settings.json.find")
            self.key(0)
            self.native("text", old)
            self.wait("settings query entered", lambda d: d["settings_json"]["query"] == old)
            self.key(36, "none")
            before = self.snapshot()["document"]["settings_json"]["fingerprint"]
            self.paste_source(new, lambda: self.wait("TeX settings edit validates", lambda d: d["settings_json"]["fingerprint"] != before and d["settings_json"]["valid"]))
        self.click("settings.json.save")
        self.wait("TeX settings saved", lambda d: d["settings_json"]["saved"])
        self.close_settings()
        self.click("editor.source")

    def open_harper_fixture(self, path):
        """Select the complete real file path in macOS's native Open panel."""
        import time
        self.focus_document_window()
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("named Harper fixture opened", lambda d: d["path"] == str(path), timeout=15)
        self.click("editor.source")
        self.wait("Harper fixture owns keyboard", lambda d: d["editor_keyboard_focused"])

    def harper_math_prose(self):
        from pathlib import Path
        count = self.snapshot()["document"]["tabs"]
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false')])
        for suffix in ("typ", "tex"):
            source = (Path(__file__).resolve().parent.parent / "manual-tests" /
                      ("harper-math-prose." + suffix)).read_text()
            path = self.directory / ("harper-math-prose." + suffix)
            path.write_text(source)
            self.open_harper_fixture(path)
            self.wait("correct " + suffix + " sentences survive display math and compounds",
                      lambda d: harper_fresh(d, source, path, []), timeout=30)
            self.stable_state(lambda d: harper_fresh(d, source, path, []))
            self.key(125, "cmd")
            if suffix == "tex":
                # The final newline leaves the caret below \end{document}.
                # Move to that line's beginning before typing ordinary prose.
                self.key(126, "none")
                self.key(123, "cmd")
                prefix = source.removesuffix("\\end{document}\n")
                expected = prefix + HARPER_TYPOS + "\\end{document}\n"
            else:
                prefix = source
                expected = source + HARPER_TYPOS
            self.wait("Harper typo insertion caret is exact",
                      lambda d: d["cursor"] == [len(prefix), len(prefix)])
            for line in HARPER_TYPOS.splitlines():
                self.native("text", line)
                self.key(36, "none")  # Return is a real editor key, not pasted text.
            self.source_is(expected)
            first_line = prefix.count("\n") + 1
            locations = [[first_line, 26], [first_line + 1, 9]]
            errors = self.wait("Harper reports only the intentional " + suffix + " typo and repetition",
                               lambda d: harper_fresh(d, expected, path, locations), timeout=30)["document"]
            self.stable_state(lambda d: harper_fresh(d, expected, path, locations))
            self.record("harper.math-prose", language=suffix, source_fingerprint=errors["source_fingerprint"],
                        writing_fingerprint=errors["writing_fingerprint"], locations=locations,
                        original_paths=errors["writing_paths"])
            self.key(0)

            def corrected_with_stale_diagnostics():
                stale = self.source_is(source)["document"]
                assert_harper_stale(stale, errors)
                self.record("harper.stale-preserved", language=suffix,
                            source_fingerprint=stale["source_fingerprint"],
                            writing_fingerprint=stale["writing_fingerprint"])

            self.paste_source(source, corrected_with_stale_diagnostics)
            self.wait("fresh corrected " + suffix + " prose replaces stale diagnostics",
                      lambda d: harper_fresh(d, source, path, []), timeout=30)
            self.stable_state(lambda d: harper_fresh(d, source, path, []))
            if path.read_text() != source:
                raise AssertionError("Harper journey wrote edits to disk despite disabled autosave")
            self.close_scratch(count)
        self.tex_settings([('"english_grammar": true', '"english_grammar": false'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true')])

    def tex_editing(self):
        from desktop_ui_journeys import fingerprint
        path = self.tex_document()
        self.tex_settings([('"auto_save": true', '"auto_save": false')])
        self.key(0)
        self.paste_source(TEX_SOURCE)
        self.key(125, "cmd")
        suffix = "% café 🙂"
        self.native("text", suffix)
        typed = TEX_SOURCE + suffix
        self.wait("TeX typing preserves Unicode caret", lambda d: d["source_fingerprint"] == fingerprint(typed) and d["cursor"] == [len(typed), len(typed)] and d["dirty"])
        self.key(6)
        self.source_is(TEX_SOURCE)
        self.key(6, "cmd+shift")
        self.wait("TeX redo restores post-edit caret", lambda d: d["source_fingerprint"] == fingerprint(typed) and d["cursor"] == [len(typed), len(typed)])
        self.native("text", "!")
        self.source_is(typed + "!")
        # Exercise TeX's incremental highlighter and services past the viewport.
        long_text = TEX_SOURCE.replace("\\end{document}", "".join(f"% α long source line {index:03}\n" for index in range(320)) + "\\end{document}")
        self.key(0)
        self.paste_source(long_text)
        self.key(125, "cmd")
        self.native("text", "% long edit")
        self.wait("long TeX edit keeps the caret at the input", lambda d: d["source_fingerprint"] == fingerprint(long_text + "% long edit") and d["cursor"] == [len(long_text) + 11, len(long_text) + 11])
        self.key(6)
        self.source_is(long_text)
        self.key(6, "cmd+shift")
        self.source_is(long_text + "% long edit")
        self.key(0)
        self.paste_source("  α sample\n\tbeta\nlast")
        self.key(0)
        self.key(44)
        self.source_is("  % α sample\n\t% beta\n% last")
        self.key(44)
        self.source_is("  α sample\n\tbeta\nlast")
        self.key(126, "cmd")
        self.key(125, "shift")
        self.key(44)
        self.source_is("  % α sample\n\t% beta\nlast")
        self.key(44)
        self.source_is("  α sample\n\tbeta\nlast")
        self.key(0)
        self.paste_source("α sample")
        for opener, closer in (("$", "$"), ("{", "}"), ("(", ")"), ("[", "]")):
            self.key(0)
            self.native("text", opener)
            wrapped = opener + "α sample" + closer
            self.source_is(wrapped)
            self.key(6)
            self.source_is("α sample")
            self.key(6, "cmd+shift")
            self.source_is(wrapped)
            self.key(6)
            self.source_is("α sample")
        self.key(0)
        self.paste_source(TEX_SOURCE)
        self.key(3)
        self.wait("TeX Find focused", lambda d: d["find_focused"])
        self.click("find.query")
        self.key(0)
        self.native("text", "\\alpha")
        self.wait("TeX command found", lambda d: d["find_selected"] is not None and d["find_query_fingerprint"] == fingerprint("\\alpha"))
        self.click("find.replace")
        self.wait("TeX Replace open", lambda d: d["replace_visible"])
        self.click("find.replacement")
        self.key(0)
        self.native("text", "\\beta")
        self.click("find.replace_all")
        self.source_is(TEX_SOURCE.replace("\\alpha", "\\beta"))
        self.key(53, "none")
        self.wait("TeX Find closes", lambda d: not d["find_visible"])
        self.key(6)
        self.source_is(TEX_SOURCE)
        self.key(6, "cmd+shift")
        self.source_is(TEX_SOURCE.replace("\\alpha", "\\beta"))
        self.key(6)
        self.source_is(TEX_SOURCE)
        self.stable_state(lambda d: d["path"] == str(path) and d["source_fingerprint"] == fingerprint(TEX_SOURCE))
        if path.read_text() != TEX_SOURCE:
            raise AssertionError("TeX editing changed disk despite disabled autosave")
        self.tex_settings([('"auto_save": false', '"auto_save": true')])

    def tex_intelligence(self):
        from desktop_ui_journeys import fingerprint
        self.tex_document()
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false')])
        self.wait("real TeX providers ready", lambda d: d["texlab_ready"] and d["badness_ready"], timeout=30)
        body = "This is is a sentnce.\n\\[ nonwordxyz + nonwordxyz \\]\n\\[ \\text{This is is readable.} \\]\n$x^{2}$\n"
        text = TEX_SOURCE.replace("A sample with $\\alpha + \\alpha$.\n", body)
        self.key(0)
        self.paste_source(text)
        self.key(23)
        self.wait("TeX Problems and Harper refresh", lambda d: d["panel"] == "Problems" and not d["writing_pending"] and d["writing_count"] >= 2, timeout=30)
        self.wait("Badness publishes a real TeX lint", lambda d: d["tex_diagnostics_current"][1] and d["tex_diagnostic_counts"][1] > 0, timeout=30)
        before = self.snapshot()["document"]
        if 4 in before["writing_lines"]:
            raise AssertionError("Harper incorrectly checks TeX display-math identifiers")
        if 5 not in before["writing_lines"]:
            raise AssertionError("Harper misses prose inside TeX math text")
        self.wait_target("problem.0")
        self.double_click("problem.0")
        self.wait("TeX problem navigates to source", lambda d: d["editor_keyboard_focused"] and d["cursor"] is not None)
        self.key(0)
        corrected = TEX_SOURCE
        self.paste_source(corrected)
        stale = self.snapshot()["document"]
        if stale["writing_pending"] and stale["writing_fingerprint"] != before["writing_fingerprint"]:
            raise AssertionError("TeX editing dropped stale Harper diagnostics before replacement")
        if not stale["tex_diagnostics_current"][1] and stale["tex_diagnostic_counts"][1] == 0:
            raise AssertionError("TeX editing dropped stale Badness diagnostics before replacement")
        self.wait("fresh TeX prose replaces stale warnings", lambda d: not d["writing_pending"] and d["writing_count"] == 0, timeout=30)
        self.stable_state(lambda d: d["writing_count"] == 0)
        self.key(23)
        self.wait("TeX Problems closes", lambda d: d["panel"] is None)
        self.click("editor.source")
        self.key(0)
        completion_source = "\\documentclass{article}\n\\newcommand{\\journeycommand}{Test}\n\\begin{document}\n\\jour\n\\end{document}"
        self.paste_source(completion_source)
        self.key(126, "none")
        self.key(124, "cmd")
        command_end = completion_source.index("\n\\jour\n") + 6
        self.wait("TeX completion caret in document body", lambda d: d["cursor"] == [command_end, command_end])
        self.key(49, "ctrl")
        self.wait("TexLab command completions arrive", lambda d: d["completion_visible"] and d["completion_count"] > 0, timeout=30)
        self.key(48, "none")
        completed = completion_source.replace("\\jour\n", "\\journeycommand\n")
        self.source_is(completed)
        self.wait("TeX completion restores source keyboard ownership", lambda d: not d["completion_visible"] and d["editor_keyboard_focused"])
        self.key(6)
        self.source_is(completion_source)
        self.key(6, "cmd+shift")
        self.source_is(completed)
        self.key(0)
        self.paste_source(TEX_SOURCE)
        self.tex_settings([('"english_grammar": true', '"english_grammar": false'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true')])

    def save_to_fixture(self, destination):
        """Native Save panels choose their own directory; never assume the cwd."""
        import time
        self.key(1)
        time.sleep(0.8)
        self.key(5, "cmd+shift")  # Native Go to Folder.
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(destination.parent))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-set-text", destination.name)
        self.native("ax-press-title", "Save")

    def tex_save(self):
        import time
        self.tex_document()
        self.tex_settings([('"auto_save": true', '"auto_save": false'),
                           ('"formatter": "badness"', '"formatter": "disabled"')])
        self.key(45)
        self.wait("new TeX save starts with an empty untyped document", lambda d: d["path"] is None and d["source_bytes"] == 0)
        self.click("editor.source")
        self.paste_source(TEX_SOURCE)
        destination = self.directory / "saved.tex"
        self.save_to_fixture(destination)
        self.wait("Save accepts TeX extension and clears dirty state", lambda d: d["path"] == str(destination) and not d["dirty"], timeout=15)
        self.source_is(TEX_SOURCE)
        if destination.read_text() != TEX_SOURCE:
            raise AssertionError("TeX Save wrote different contents")
        self.key(125, "cmd")
        self.native("text", "% saved edit")
        edited = TEX_SOURCE + "% saved edit"
        self.source_is(edited)
        self.key(1)
        self.wait("TeX manual Save finishes", lambda d: not d["dirty"], timeout=15)
        if destination.read_text() != edited:
            raise AssertionError("TeX manual Save lost the last edit")
        self.key(13)
        self.wait("saved TeX tab closes without a discard prompt", lambda d: d["path"] != str(destination) and not d["modal_open"])
        self.key(31)
        time.sleep(0.8)
        self.native("ax-press-description", "search")
        self.native("text", destination.name)
        time.sleep(0.8)
        self.native("ax-press-title", "Open")
        self.wait("saved TeX file reopens", lambda d: d["path"] == str(destination), timeout=15)
        self.source_is(edited)
        # TeX uses its single engine gear for both building and saving a PDF.
        # Pin this tab so an aggregate run cannot build a previous Typst entry.
        self.key(35, "cmd+alt")
        self.key(20)
        self.wait("saved TeX owns the PDFium preview", lambda d: d["preview_path"] == str(destination) and d["preview_tab_id"] == d["active_tab_id"] and d["preview_backend"] == "PDFium" and d["preview_pdfium_ready"] and not d["compile_indicator_spinning"], timeout=60)
        pdf_path = destination.with_suffix(".pdf")
        if pdf_path.exists():
            raise AssertionError("background TeX preview unexpectedly saved a public PDF")
        self.wait_target("build.preview")
        pdf_button = self.snapshot()["targets"].get("build.pdf")
        if pdf_button and pdf_button["age_ms"] <= 500:
            raise AssertionError("TeX exposed a separate PDF button")
        self.click("build.preview")
        self.wait("TeX gear commits an adjacent PDF", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        first_pdf = pdf_path.read_bytes()
        if not first_pdf.startswith(b"%PDF-"):
            raise AssertionError("TeX gear produced no public PDF")
        first_mtime = pdf_path.stat().st_mtime_ns
        self.key(15, "cmd+shift")
        self.wait("TeX live compilation pauses", lambda d: d["compilation_paused"] and not d["compile_indicator_spinning"])
        self.click("editor.source")
        self.key(0)
        paused_source = edited.replace("A sample with", "A fresh manually compiled sample with")
        self.paste_source(paused_source)
        self.wait("paused TeX edit marks PDF stale", lambda d: d["compilation_paused"] and d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"])
        self.stable_state(lambda d: d["compilation_paused"] and not d["compile_indicator_spinning"] and pdf_path.read_bytes() == first_pdf and pdf_path.stat().st_mtime_ns == first_mtime)
        self.click("build.preview")
        self.wait("TeX manual gear rebuilds while remaining paused", lambda d: d["compilation_paused"] and d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and not d["compile_indicator_spinning"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        fresh_pdf = pdf_path.read_bytes()
        if not fresh_pdf.startswith(b"%PDF-") or fresh_pdf == first_pdf:
            raise AssertionError("paused TeX gear did not save the changed document")
        if destination.read_text() != edited:
            raise AssertionError("TeX build unexpectedly saved unsaved source changes")
        self.source_is(paused_source)
        self.key(15, "cmd+shift")
        self.wait("TeX live compilation resumes only on request", lambda d: not d["compilation_paused"])
        self.click("editor.source")
        self.key(0)
        self.paste_source(edited)
        self.tex_settings([('"auto_save": false', '"auto_save": true'),
                           ('"formatter": "disabled"', '"formatter": "badness"')])
