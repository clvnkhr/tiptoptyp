"""Native TeX journeys; fixtures are real files and tools are never mocked."""

TEX_JOURNEYS = ("tex_editing", "tex_intelligence", "tex_save", "tex_preview_scroll", "harper_math_prose", "harper_settings")
TEX_SOURCE = "\\documentclass{article}\n\\begin{document}\nA sample with $\\alpha + \\alpha$.\n\\end{document}\n"


TEX_PARITY_SOURCE = (
    "\\documentclass{article}\n"
    "\\usepackage{amsmath}\n"
    "\\newcommand{\\parityname}{Native}\n"
    "\\begin{document}\n"
    "\\section{Native parity section}\n"
    "\\label{sec:parity}\n"
    "\\input{parity-child}\n"
    "See \\ref{sec:parity} and \\ref{sec:child}.\n"
    "% table insertion\n\n"
    "\\end{document}\n"
)
TEX_PARITY_CHILD = (
    "\\subsection{Included parity section}\n"
    "\\label{sec:child}\n"
    "Included body.\n"
)


def tex_source_owner_preserved(document, previous):
    """Tab activation rekeys revisions; compare the visible source and owner instead."""
    import re
    fields = ("path", "active_tab_id", "document_kind", "source_fingerprint",
              "source_bytes", "cursor", "dirty")
    for state in (previous, document):
        if (not isinstance(state, dict) or not all(field in state for field in fields)
                or state["path"] is not None and not isinstance(state["path"], str)
                or type(state["active_tab_id"]) is not int or state["active_tab_id"] < 0
                or not isinstance(state["document_kind"], str)
                or not isinstance(state["source_fingerprint"], str)
                or not re.fullmatch(r"[0-9a-f]{16}", state["source_fingerprint"])
                or type(state["source_bytes"]) is not int or state["source_bytes"] < 0
                or type(state["dirty"]) is not bool
                or not isinstance(state["cursor"], list) or len(state["cursor"]) != 2
                or any(type(position) is not int or position < 0 for position in state["cursor"])):
            raise AssertionError("Complete source ownership observation is required")
    return all(document[field] == previous[field] for field in fields)


def tex_parity_index_ready(document, main, child):
    """Require the actual completed TeX graph, including its source identities."""
    from desktop_ui_journeys import fingerprint
    import re
    index = document.get("project_index")
    if not isinstance(index, dict) or type(index.get("pending")) is not bool:
        raise AssertionError("TeX project-index observation is missing")
    digest = lambda value: isinstance(value, str) and re.fullmatch(r"[0-9a-f]{16}", value)
    for field in ("outline", "symbols", "labels", "references"):
        entries = index.get(field)
        if not isinstance(entries, list) or any(
                not isinstance(row, dict) or not digest(row.get("name"))
                or not digest(row.get("path")) or type(row.get("line")) is not int
                or row["line"] < 1 for row in entries):
            raise AssertionError("Incomplete TeX project-index rows: " + field)
    for field in ("packages", "subfiles"):
        entries = index.get(field)
        if not isinstance(entries, list) or not all(digest(value) for value in entries):
            raise AssertionError("Incomplete TeX project-index entries: " + field)
    if "warning" not in index or index["warning"] is not None and not digest(index["warning"]):
        raise AssertionError("Incomplete TeX project-index warning state")
    if index["pending"] or index["warning"] is not None:
        return False
    main, child = fingerprint(str(main.resolve())), fingerprint(str(child.resolve()))
    expected = {
        "outline": [("Native parity section", main, 5, 1),
                    ("Included parity section", child, 1, 2)],
        "symbols": [("\\parityname", main, 3)],
        "labels": [("sec:parity", main, 6), ("sec:child", child, 2)],
        "references": [("sec:parity", main, 8), ("sec:child", main, 8)],
    }
    for field, rows in expected.items():
        if len(index[field]) != len(rows):
            return False
        for name, path, line, *level in rows:
            if not any(row["name"] == fingerprint(name) and row["path"] == path
                       and row["line"] == line
                       and (not level or type(row.get("level")) is int and row["level"] == level[0])
                       for row in index[field]):
                return False
    return (index["packages"] == [fingerprint("amsmath")]
            and index["subfiles"] == [child])


HARPER_TYPOS = "This sentence contains a misspelld word.\nThis is the the simple result.\n"
HARPER_FORMATTED_REPETITION = "This is the #emph[the] result.\n"


def native_button_ready(receipt, title, require_enabled=True):
    """Require one complete native button observation; never guess from sleeps."""
    return (isinstance(receipt, dict) and receipt.get('requested_title') == title
            and type(receipt.get('count')) is int and receipt['count'] == 1
            and receipt.get('complete') is True and receipt.get('role') == 'AXButton'
            and receipt.get('title') == title and type(receipt.get('enabled')) is bool
            and (receipt['enabled'] or not require_enabled))


def native_path_entry_ready(receipt):
    """macOS 14 exposes Go to Folder as GoToWindow, without a Go button."""
    if not isinstance(receipt, dict) or receipt.get('complete') is not True:
        return False
    window = receipt.get('focused_window')
    if (not isinstance(window, dict) or window.get('role') != 'AXSheet'
            or window.get('identifier') != 'GoToWindow'):
        return False
    ancestry = receipt.get('focused_ancestry')
    if not isinstance(ancestry, list):
        return False
    # Some native Go-to-Folder sheets expose no focused UI element at all.
    # Their exact focused-window identity proves the phase. When focus ancestry
    # is exposed, insist it belongs to that sheet and starts at an input field.
    return not ancestry or (
        isinstance(ancestry[0], dict)
        and ancestry[0].get('role') in ('AXTextField', 'AXComboBox', 'AXTextArea')
        and any(isinstance(parent, dict) and parent.get('identifier') == 'GoToWindow'
                and parent.get('role') == 'AXSheet' for parent in ancestry))


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


def diagnostic_suppression_fixture(suffix):
    """Keep line positions fixed while switching one real exclusion on and off."""
    if suffix not in ("typ", "tex"):
        raise ValueError("Suppression fixture requires a Typst or TeX document")
    marker = "//" if suffix == "typ" else "%"
    header = "" if suffix == "typ" else "\\documentclass{article}\n\\begin{document}\n"
    footer = "" if suffix == "typ" else "\\end{document}\n"
    source = header + (marker + " ttt: off\n").join([HARPER_TYPOS] * 3) + footer
    active = source.replace("ttt: off", "ttt: on", 1)
    offset = header.count("\n")
    locations = [[line + offset, column] for line, column in
                 ((1, 26), (2, 9), (4, 26), (5, 9), (7, 26), (8, 9))]
    return source, active, locations


def harper_suppression_result(document, source, path, locations, visible_locations):
    """Prove actual raw findings survive while exact Problems rows are excluded."""
    from desktop_ui_problems_journeys import problems_state
    if not harper_fresh(document, source, path, locations) or document.get("panel") != "Problems":
        return False
    count, rows = document.get("raw_diagnostic_count"), document.get("diagnostic_rows")
    if type(count) is not int or count < 0 or not isinstance(rows, list) or len(rows) != count:
        raise AssertionError("Suppression diagnostic row inspection is unavailable")
    for index, row in enumerate(rows):
        if (not isinstance(row, dict) or type(row.get("index")) is not int or row["index"] != index
                or "provider" not in row or row["provider"] is not None and not isinstance(row["provider"], str)
                or type(row.get("suppressed")) is not bool
                or any(key not in row or row[key] is not None
                       and (type(row[key]) is not int or row[key] <= 0) for key in ("line", "column"))):
            raise AssertionError("Suppression diagnostic source identity is incomplete")
    layout = problems_state(document)
    if (layout["query"] or any(layout[key] is not None for key in ("source", "type", "category"))
            or layout["grouped"]):
        return False
    harper_rows = [row for row in rows if row["provider"] == "Harper"]
    if [[row["line"], row["column"]] for row in harper_rows] != locations:
        return False
    unsuppressed = [row["index"] for row in rows if not row["suppressed"]]
    visible = layout["visible_indices"]
    actual = [[rows[index]["line"], rows[index]["column"]] for index in visible
              if rows[index]["provider"] == "Harper"]
    return (actual == visible_locations and visible == unsuppressed
            and type(document.get("diagnostic_count")) is int
            and document["diagnostic_count"] == len(unsuppressed) == layout["total_count"]
            and layout["visible_count"] == layout["displayed_count"] == len(unsuppressed)
            and all(row["suppressed"] is ([row["line"], row["column"]] not in visible_locations)
                    for row in harper_rows))


def harper_settings_applied(document, dictionary, ignore_names):
    """Observe applied preferences without exposing personal dictionary contents."""
    from desktop_ui_journeys import fingerprint
    return document.get("harper_dictionary_fingerprint") == fingerprint(dictionary) \
        and document.get("harper_ignore_capitalized_names") is ignore_names



def harper_rule_result(document, source, path, locations, enabled):
    """An applied rule must produce fresh results while preserving real findings."""
    from desktop_ui_journeys import fingerprint
    rules = document.get("writing_rules")
    overrides = document.get("harper_rule_overrides")
    actual = document.get("writing_locations")
    return (document.get("path") == str(path)
            and document.get("source_fingerprint") == fingerprint(source)
            and document.get("writing_current") is True
            and document.get("writing_pending") is False
            and isinstance(overrides, dict)
            and overrides.get("NoFrenchSpaces") is enabled
            and isinstance(rules, list) and rules.count("NoFrenchSpaces") == int(enabled)
            and document.get("writing_count") == len(locations) + int(enabled)
            and isinstance(actual, list)
            and actual == locations + ([[5, 26]] if enabled else [])
            and document.get("writing_paths") == [str(path)] * len(actual))



def quiet_badness_result(document):
    """The visible result must keep correctness findings and omit brace-style advice."""
    current = document.get("tex_diagnostics_current")
    codes = document.get("badness_visible_codes")
    return (isinstance(current, list) and len(current) >= 2 and current[1] is True
            and isinstance(codes, list) and bool(codes)
            and all(isinstance(code, str) and code.strip('"') != "redundant-script-braces"
                    for code in codes))


def assert_harper_stale(document, previous):
    """An edit must retain its last published result until the new one arrives."""
    if document["writing_current"] or not document["writing_pending"]:
        raise AssertionError("did not observe Harper's stale result during the edit debounce")
    for field in ("writing_count", "writing_fingerprint", "writing_lines",
                  "writing_locations", "writing_paths"):
        if document[field] != previous[field]:
            raise AssertionError("Harper discarded or changed stale diagnostics before refreshing")


def settings_json_query_selected(state):
    """A fresh focused field acknowledges Select All before native typing starts."""
    query = state.get("document", {}).get("settings_json", {}).get("query")
    target = state.get("targets", {}).get("settings.json.find")
    if not isinstance(query, str) or not isinstance(target, dict):
        return False
    selection = target.get("selection")
    age = target.get("age_ms")
    return (target.get("enabled") is True and target.get("focused") is True
            and type(age) in (int, float) and 0 <= age <= 500
            and isinstance(selection, list) and len(selection) == 2
            and all(type(position) is int for position in selection)
            and sorted(selection) == [0, len(query)])


class TexJourneys:
    def wait_settings_json_query_selection(self):
        import time
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if settings_json_query_selected(self.snapshot()):
                return
            time.sleep(0.05)
        raise AssertionError("settings JSON query did not acknowledge Select All")

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
            self.click("settings.json.mode", require_hover=True)
        self.wait("JSON settings ready", lambda d: d["settings_json"]["visible"])
        for old, new in replacements:
            self.key(3)
            self.wait_target("settings.json.find", focused=True)
            state = self.snapshot()
            query = state["document"]["settings_json"]["query"]
            # Native menu commands are asynchronous. Do not send text until
            # the existing query's selection confirms Cmd+A was consumed.
            # Empty/already-selected queries need no additional command.
            if query and not settings_json_query_selected(state):
                self.key(0)
                self.wait_settings_json_query_selection()
            self.native("text", old)
            self.wait("settings query entered", lambda d: d["settings_json"]["query"] == old)
            self.key(36, "none")
            self.wait_target("settings.json", focused=True)
            before = self.snapshot()["document"]["settings_json"]["fingerprint"]
            self.paste_source(new, lambda: self.wait("TeX settings edit validates", lambda d: d["settings_json"]["fingerprint"] != before and d["settings_json"]["valid"]))
        self.click("settings.json.save")
        self.wait("TeX settings saved", lambda d: d["settings_json"]["saved"])
        self.close_settings()
        self.click("editor.source")

    def wait_native_button(self, title, require_enabled=True):
        import time
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            receipt = self.native("ax-button-title", title)
            if native_button_ready(receipt, title, require_enabled):
                return
            time.sleep(.05)
        raise AssertionError("Native dialog button did not become ready: " + title)

    def wait_native_path_entry(self):
        import time
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if native_path_entry_ready(self.native("ax-dialog-state")):
                return
            time.sleep(.05)
        raise AssertionError("Native Go-to-Folder path entry did not become ready")

    def open_harper_fixture(self, path):
        """Select the complete real path only after each native dialog is ready."""
        self.focus_document_window()
        self.key(31)
        # No selected file is required yet; the Open button may be disabled.
        self.wait_native_button("Open", require_enabled=False)
        self.key(5, "cmd+shift")
        self.wait_native_path_entry()
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        self.wait_native_button("Open")
        self.native("ax-press-title", "Open")
        self.wait("named Harper fixture opened", lambda d: d["path"] == str(path), timeout=15)
        self.click("editor.source")
        self.wait("Harper fixture owns keyboard", lambda d: d["editor_keyboard_focused"])

    def harper_suppression(self, suffix, path, original):
        """Switch exclusions in real editable source without deleting provider results."""
        source, active, locations = diagnostic_suppression_fixture(suffix)
        panel = self.snapshot()["document"]["panel"]
        if panel is None:
            self.key(23)
        self.click("panel.problems")
        self.key(3)
        self.wait_target("problems.search", focused=True)
        layout = self.snapshot()["document"]["problems"]
        if layout["query"] or any(layout[key] is not None for key in ("source", "type", "category")):
            self.click("problems.reset")
        if layout["grouped"]:
            self.click("problems.grouping")
        for phase, text, visible in (("all", source, locations),
                                     ("excluded", active, locations[:2] + locations[4:]),
                                     ("restored", source, locations)):
            self.click("editor.source")
            self.key(0)
            self.paste_source(text)
            predicate = lambda d: harper_suppression_result(d, text, path, locations, visible)
            state = self.wait("Real " + suffix + " diagnostics " + phase + " around ttt markers",
                              predicate, timeout=30)["document"]
            self.stable_state(predicate)
            self.record("harper.suppression", language=suffix, phase=phase,
                        source_fingerprint=state["source_fingerprint"],
                        raw_locations=state["writing_locations"], visible_locations=visible,
                        raw_diagnostic_count=state["raw_diagnostic_count"],
                        diagnostic_count=state["diagnostic_count"])
            if path.read_text() != original:
                raise AssertionError("Diagnostic exclusions changed the saved source document")
        self.click("editor.source")
        self.key(0)
        self.paste_source(original)
        self.wait("Original " + suffix + " grammar fixture restores after exclusion checks",
                  lambda d: harper_fresh(d, original, path, []), timeout=30)
        if panel is None:
            self.key(23)
            self.wait("Exclusion checks restore the closed bottom panel", lambda d: d["panel"] is None)
        elif panel != "Problems":
            self.click("panel." + panel.lower())
            self.wait("Exclusion checks restore the original bottom tab", lambda d: d["panel"] == panel)
        self.click("editor.source")

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
            typos = HARPER_TYPOS + (HARPER_FORMATTED_REPETITION if suffix == "typ" else "")
            if suffix == "tex":
                # The final newline leaves the caret below \end{document}.
                # Move to that line's beginning before typing ordinary prose.
                self.key(126, "none")
                self.key(123, "cmd")
                prefix = source.removesuffix("\\end{document}\n")
                expected = prefix + typos + "\\end{document}\n"
            else:
                prefix = source
                expected = source + typos
            self.wait("Harper typo insertion caret is exact",
                      lambda d: d["cursor"] == [len(prefix), len(prefix)])
            for line in typos.splitlines():
                self.native("text", line)
                self.key(36, "none")  # Return is a real editor key, not pasted text.
            self.source_is(expected)
            first_line = prefix.count("\n") + 1
            locations = [[first_line, 26], [first_line + 1, 9]]
            if suffix == "typ":
                locations.append([first_line + 2, 9])
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
            self.harper_suppression(suffix, path, source)
            self.close_scratch(count)
        self.tex_settings([('"english_grammar": true', '"english_grammar": false'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true')])

    def harper_settings(self):
        """Form edits must refresh real Harper results without editing the source."""
        from desktop_ui_journeys import fingerprint
        count = self.snapshot()["document"]["tabs"]
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false')])
        source = ("We inspect qzorthal today.\n"
                  "We inspect QzorNim today.\n"
                  "This sentence contains a misspelld word.\n"
                  "This is the the simple result.\n"
                  "This is a short sentence.  This is another short sentence.\n")
        path = self.directory / "harper-settings.typ"
        path.write_text(source)
        self.open_harper_fixture(path)
        locations = [[1, 12], [2, 12], [3, 26], [4, 9]]
        self.wait("Harper reports the unknown word, name, typo and repetition",
                  lambda d: harper_fresh(d, source, path, locations), timeout=30)

        def dictionary_edit(value, ignore_names, expected):
            self.settings()
            self.setting_section("Harper dictionary", "Harper dictionary")
            self.click("settings.harper.dictionary")
            self.wait_target("settings.harper.dictionary", focused=True)
            self.key(0)
            if value:
                self.native("text", value)
            else:
                self.key(51, "none")
            self.wait("personal dictionary form applies without editing source", lambda d:
                      harper_settings_applied(d, value, ignore_names)
                      and d["source_fingerprint"] == fingerprint(source))
            if self.capture_review and value:
                self.capture_viewport("settings")
            self.close_settings()
            self.wait("dictionary edit refreshes Harper at original source locations", lambda d:
                      harper_fresh(d, source, path, expected), timeout=30)
            self.stable_state(lambda d: harper_fresh(d, source, path, expected))

        def toggle_names(value, dictionary, expected):
            self.settings()
            self.setting_section("capitalized names", "Harper: ignore capitalized names")
            self.click("settings.harper.ignore_names")
            self.wait("capitalized name preference applies", lambda d:
                      harper_settings_applied(d, dictionary, value)
                      and d["source_fingerprint"] == fingerprint(source))
            self.close_settings()
            self.wait("name filtering preserves lowercase typos and repetition", lambda d:
                      harper_fresh(d, source, path, expected), timeout=30)
            self.stable_state(lambda d: harper_fresh(d, source, path, expected))

        dictionary_edit("qzorthal", False, locations[1:])
        toggle_names(True, "qzorthal", locations[2:])
        # Reopen Settings: its applied dictionary must remain editable, and
        # removing the entry must immediately restore that word's diagnostic.
        dictionary_edit("", True, [locations[0], *locations[2:]])
        toggle_names(False, "", locations)
        self.tex_settings([('"NoFrenchSpaces": false', '"NoFrenchSpaces": true')])
        self.wait("enabling a style rule refreshes results without dropping real errors", lambda d:
                  harper_rule_result(d, source, path, locations, True), timeout=30)
        self.tex_settings([('"NoFrenchSpaces": true', '"NoFrenchSpaces": false')])
        self.wait("restoring quiet defaults removes only the style finding", lambda d:
                  harper_rule_result(d, source, path, locations, False), timeout=30)
        self.stable_state(lambda d: harper_rule_result(d, source, path, locations, False))
        if path.read_text() != source:
            raise AssertionError("Harper settings altered the fixture on disk")
        self.record("harper.settings", source_fingerprint=fingerprint(source),
                    dictionary_restored=True, names_restored=True,
                    original_paths=self.snapshot()["document"]["writing_paths"])
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
        self.tex_parity(path)
        self.tex_settings([('"auto_save": false', '"auto_save": true')])

    def tex_parity_command(self, query, action):
        self.palette_open()
        result = self.palette_query(query)
        if result["palette_selected"] != action or action not in result["palette_enabled"]:
            raise AssertionError("TeX palette command is missing or disabled: " + action)
        self.key(36, "none")
        self.wait("TeX palette command closes the palette", lambda d: not d["palette_visible"])

    def tex_parity(self, path):
        """Exercise language parity through real menus, grid edits and native Save."""
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        original = self.snapshot()["document"]
        view = original["view_mode"]
        explorer = original["explorer_visible"]
        child = self.directory / "parity-child.tex"
        main = self.directory / "parity-main.tex"
        child.write_text(TEX_PARITY_CHILD)
        main.write_text(TEX_PARITY_SOURCE)
        self.click("editor.source")
        self.key(0)
        self.paste_source(TEX_PARITY_SOURCE)
        self.key(35, "cmd+alt")
        self.key(20)
        self.wait("active native TeX source owns preview before indexing", lambda d:
                  d["path"] == str(path) and d["document_kind"] == "Tex"
                  and d["preview_tab_id"] == d["active_tab_id"]
                  and d["source_fingerprint"] == fingerprint(TEX_PARITY_SOURCE))
        self.wait("TeX index includes real included-file headings and cross references",
                  lambda d: tex_parity_index_ready(d, path, child), timeout=20)
        if not explorer:
            self.key(18)
            self.wait("Explorer is visible for native preview selection", lambda d: d["explorer_visible"])
        before = self.snapshot()["document"]
        self.explorer_menu(main.name, "preview")
        owner = lambda d: tex_source_owner_preserved(d, before)
        self.wait("Explorer selects another TeX preview without switching the edited tab", lambda d:
                  owner(d) and d["preview_path"] == str(main)
                  and d["preview_tab_id"] != d["active_tab_id"]
                  and d["preview_backend"] == "PDFium" and d["preview_pdfium_ready"]
                  and not d["compile_indicator_spinning"], timeout=60)
        self.wait("TeX index follows the new preview owner rather than stale editor paths", lambda d:
                  owner(d) and tex_parity_index_ready(d, main, child), timeout=20)
        if self.capture_review:
            self.capture_viewport("main")
        self.record("tex.explorer_preview", source=path.name, preview=main.name,
                    active_tab_preserved=True, included_file_index=True)
        self.tex_parity_tables(TEX_PARITY_SOURCE)
        self.tex_parity_new_tab()
        self.tex_parity_templates(path)
        self.click("editor.source")
        self.key(0)
        self.paste_source(TEX_SOURCE)
        self.key(35, "cmd+alt")
        self.wait("TeX parity restores source and its preview owner", lambda d:
                  d["path"] == str(path) and d["document_kind"] == "Tex"
                  and d["source_fingerprint"] == fingerprint(TEX_SOURCE)
                  and d["preview_tab_id"] == d["active_tab_id"])
        if not explorer:
            self.key(18)
            self.wait("TeX parity restores Explorer visibility", lambda d: not d["explorer_visible"])
        self.key({"Code": 19, "Split": 20, "Preview": 21}[view])
        self.wait("TeX parity restores the original layout", lambda d: d["view_mode"] == view)
        if path.read_text() != TEX_SOURCE or child.read_text() != TEX_PARITY_CHILD \
                or main.read_text() != TEX_PARITY_SOURCE:
            raise AssertionError("TeX parity changed a fixture on disk despite disabled autosave")

    def tex_parity_tables(self, source):
        from desktop_ui_journeys import fingerprint
        from desktop_ui_math_grid_journeys import math_grid_fingerprint
        from desktop_ui_newest_journeys import assert_table_grid_only
        for disposition in ("cancel", "apply"):
            self.click("editor.source")
            self.key(126, "cmd")
            for _ in range(source.splitlines().index("% table insertion") + 1):
                self.key(125, "none")
            before = self.snapshot()["document"]
            old = {name: target["frame"] for name, target in self.snapshot()["targets"].items()
                   if name.startswith("table.")}
            self.tex_parity_command("New table", "edit.new_table")
            self.wait("New Table opens a native TeX draft", lambda d: d["table_editor_open"])
            self.table_target("table.cell.0.0", previous_frame=old.get("table.cell.0.0"))
            state = self.snapshot()
            assert_table_grid_only(state)
            draft = state["document"]["table_draft"]
            if (draft["math"] is not False or draft["kind"] != "tabular"
                    or [draft["rows"], draft["columns"]] != [2, 2]
                    or draft["fingerprint"] != math_grid_fingerprint([["", ""], ["", ""]])):
                raise AssertionError("New Table chose Typst syntax or a nonempty TeX grid")
            self.click("table.cell.0.0")
            self.wait_target("table.cell.0.0", focused=True)
            self.key(0)
            self.native("text", "Native parity table")
            self.wait("native TeX cell editing changes only the table draft", lambda d:
                      d["table_draft"] is not None and d["table_draft"]["valid"]
                      and d["table_draft"]["fingerprint"] == math_grid_fingerprint(
                          [["Native parity table", ""], ["", ""]])
                      and d["source_fingerprint"] == fingerprint(source))
            if self.capture_review and disposition == "cancel":
                self.capture_viewport("table-editor")
            self.table_target("table." + disposition)
            self.click("table." + disposition)
            self.wait("native TeX table draft closes", lambda d: not d["table_editor_open"])
            if disposition == "cancel":
                self.source_is(source)
                self.stable_state(lambda d: all(d[field] == before[field] for field in
                                  ("active_tab_id", "path", "source_fingerprint", "source_bytes", "revision",
                                   "preview_tab_id", "preview_path")), frames=4)
            else:
                changed = self.wait("New Table applies only to the active TeX source", lambda d:
                                    d["source_fingerprint"] != fingerprint(source)
                                    and d["source_bytes"] > len(source.encode())
                                    and all(d[field] == before[field] for field in
                                            ("active_tab_id", "path", "preview_tab_id", "preview_path")))["document"]
                self.click("editor.source")
                self.key(3)
                self.wait_target("find.query", focused=True)
                self.key(0)
                self.native("text", "Native parity table")
                self.wait("inserted TeX table contains the edited literal cell", lambda d:
                          d["find_selected"] is not None
                          and d["find_query_fingerprint"] == fingerprint("Native parity table")
                          and d["source_fingerprint"] == changed["source_fingerprint"])
                self.click("find.close")
                self.wait("Find closes before TeX table Undo", lambda d: not d["find_visible"])
                self.click("editor.source")
                self.key(6)
                self.source_is(source)
                self.key(6, "cmd+shift")
                self.wait("one Redo restores the complete inserted TeX table", lambda d:
                          d["source_fingerprint"] == changed["source_fingerprint"]
                          and d["source_bytes"] == changed["source_bytes"])
                self.key(6)
                self.source_is(source)
            self.record("tex.new_table", disposition=disposition, kind="tabular",
                        grid=[2, 2], exact_undo=disposition == "apply")

    def tex_parity_new_tab(self):
        from desktop_ui_journeys import fingerprint
        self.click("editor.source")
        before = self.snapshot()["document"]
        self.key(45)  # Cmd+N inherits the active source's language.
        self.wait("New creates a clean blank TeX tab without replacing its pinned preview", lambda d:
                  d["document_kind"] == "Tex" and d["path"] is None and not d["dirty"]
                  and d["tabs"] == before["tabs"] + 1
                  and d["active_tab_id"] != before["active_tab_id"]
                  and d["source_fingerprint"] == fingerprint("") and d["source_bytes"] == 0
                  and not d["modal_open"] and all(d[field] == before[field] for field in
                                               ("preview_tab_id", "preview_path")))
        self.tex_close_created_tab(before, self.snapshot()["document"]["active_tab_id"])
        self.record("tex.new", document_kind="Tex", clean_blank=True,
                    pinned_preview_preserved=True, original_source_restored=True)

    def tex_close_created_tab(self, before, created_id):
        self.key(13)
        self.wait("new TeX tab closes cleanly and preserves the pinned preview", lambda d:
                  not d["modal_open"] and d["tabs"] == before["tabs"]
                  and d["active_tab_id"] != created_id
                  and all(d[field] == before[field] for field in ("preview_tab_id", "preview_path")))
        # Closing selects the left neighbour in tab order, which need not be
        # the source that was active before an appended tab was created.
        self.click("tab." + str(before["active_tab_id"]))
        restored = lambda d: (not d["modal_open"] and tex_source_owner_preserved(d, before)
                              and all(d[field] == before[field] for field in
                                      ("tabs", "preview_tab_id", "preview_path")))
        self.wait("returning to the original TeX tab restores its exact source and caret", restored)
        self.stable_state(restored, frames=4)

    def tex_template_target(self, previous_frame):
        """A just-closed TeX picker must not prove the next picker's language."""
        import time
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            target = state["targets"].get("template.selected.tex")
            if (state["document"]["template_picker_open"] and target
                    and target["enabled"] and target["age_ms"] <= 500
                    and target["frame"] != previous_frame):
                return
            time.sleep(0.05)
        raise AssertionError("template picker did not expose a fresh TeX selection")

    def tex_parity_templates(self, path):
        from pathlib import Path
        from desktop_ui_journeys import fingerprint
        formatter = self.snapshot()["document"].get("tex_formatter")
        if formatter not in ("badness", "tex-fmt", "disabled"):
            raise AssertionError("The active TeX formatter observation is unavailable")
        # This checks typed-template ownership, not the selected formatter's
        # output. Manual-save formatting has its own source/tool coverage.
        if formatter != "disabled":
            self.tex_settings([(f'"formatter": "{formatter}"', '"formatter": "disabled"')])
        for disposition in ("cancel", "create"):
            self.click("editor.source")
            before = self.snapshot()["document"]
            previous = self.snapshot()["targets"].get("template.selected.tex", {}).get("frame")
            self.tex_parity_command("New from template", "file.new_from_template")
            self.wait("template picker opens from native TeX source", lambda d: d["template_picker_open"])
            self.tex_template_target(previous)
            self.wait_target("template." + disposition)
            self.click("template." + disposition)
            if disposition == "cancel":
                self.wait("template Cancel preserves TeX source and tab ownership", lambda d:
                          not d["template_picker_open"] and all(d[field] == before[field] for field in
                              ("tabs", "active_tab_id", "path", "source_fingerprint", "source_bytes",
                               "revision", "preview_path", "preview_tab_id")))
            else:
                template = (Path(__file__).resolve().parents[1]
                            / "manual-tests/templates/article.tex").read_text()
                self.wait("TeX template creates a typed TeX tab before any save", lambda d:
                          not d["template_picker_open"] and d["document_kind"] == "Tex"
                          and d["path"] is None and d["tabs"] == before["tabs"] + 1
                          and d["active_tab_id"] != before["active_tab_id"]
                          and d["source_fingerprint"] == fingerprint(template))
                destination = self.directory / "parity-template.tex"
                self.focus_document_window()
                self.click("editor.source")
                self.save_to_fixture(destination)
                self.wait("native Save preserves the template's TeX document type", lambda d:
                          d["path"] == str(destination) and d["document_kind"] == "Tex"
                          and not d["dirty"] and d["source_fingerprint"] == fingerprint(template), timeout=20)
                if destination.read_text() != template:
                    raise AssertionError("Saving a TeX template changed its source")
                self.tex_close_created_tab(before, self.snapshot()["document"]["active_tab_id"])
            self.record("tex.template", disposition=disposition, default_language="Tex",
                        typed_before_save=disposition == "create")
        if formatter != "disabled":
            self.tex_settings([('"formatter": "disabled"', f'"formatter": "{formatter}"')])

    def tex_intelligence(self):
        from desktop_ui_journeys import fingerprint
        self.tex_document()
        self.tex_settings([('"english_grammar": false', '"english_grammar": true'),
                           ('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false')])
        self.wait("real TeX providers ready", lambda d: d["texlab_ready"] and d["badness_ready"], timeout=30)
        body = "This is is a sentnce.\n\\[ nonwordxyz + nonwordxyz \\]\n\\[ \\text{This is is readable.} \\]\n$x^{2}$\n\\ref{missing-fixture-label}\n"
        text = TEX_SOURCE.replace("A sample with $\\alpha + \\alpha$.\n", body)
        self.key(0)
        self.paste_source(text)
        self.key(23)
        self.wait("TeX Problems and Harper refresh", lambda d: d["panel"] == "Problems" and not d["writing_pending"] and d["writing_count"] >= 2, timeout=30)
        self.wait("Badness keeps a real reference problem and hides brace-style advice", quiet_badness_result, timeout=30)
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
        self.open_harper_fixture(destination)
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
