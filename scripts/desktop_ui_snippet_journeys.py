"""Real snippet library editing and insertion; observations never mutate state."""


def saved_snippet(document, origin, language, prefix):
    """A unique saved entry, including which collection owns it."""
    collection = {"preset": "snippet_presets", "custom": "snippets"}[origin]
    matches = [entry for entry in document[collection]
               if entry["language"] == language and entry["prefix"] == prefix]
    if len(matches) != 1:
        return None
    return matches[0]


def snippet_absent(document, origin, language, prefix):
    collection = {"preset": "snippet_presets", "custom": "snippets"}[origin]
    return not any(entry["language"] == language and entry["prefix"] == prefix
                   for entry in document[collection])


def snippet_matches(document, origin, language, prefix, body, enabled, opening="{{", closing="}}"):
    from desktop_ui_journeys import fingerprint
    entry = saved_snippet(document, origin, language, prefix)
    return entry is not None and entry["body"] == fingerprint(body) \
        and entry["enabled"] is enabled and entry["placeholder_open"] == opening \
        and entry["placeholder_close"] == closing


def snippet_completion(document, prefix):
    """Do not confuse a language-server item with an enabled local snippet."""
    if not document["completion_visible"] or not document["completion_current"]:
        return None
    return next((index for index, (label, origin) in enumerate(zip(
        document["completion_labels"], document["completion_origins"]))
        if label == prefix and origin == "Snippet"), None)


def snippet_field_matches(document, field, text):
    from desktop_ui_journeys import fingerprint
    return document["snippet_editor_visible"] and document.get(
        "snippet_editor_fields", {}).get(field) == fingerprint(text)


def snippet_owner_unchanged(document, before):
    return all(document[field] == before[field] for field in (
        "tabs", "active_tab_id", "path", "source_bytes", "source_fingerprint"))


class SnippetJourneys:
    def accept_snippet(self, prefix, keyboard=False):
        self.key(49, "ctrl")
        state = self.wait("snippet prefix is available with its provider", lambda d:
                          snippet_completion(d, prefix) is not None)
        index = snippet_completion(state["document"], prefix)
        # Local snippets precede server replies, preserving this index.
        self.wait_target("completion.item." + str(index))
        if keyboard:
            for _ in range(index):
                self.key(125, "none")
            self.key(36, "none")
        else:
            self.click("completion.item." + str(index))

    def snippet_is_absent(self, source, prefix):
        count = self.scratch(source)
        self.key(49, "ctrl")

        def absent(document):
            return not document["completion_pending"] and not any(
                label == prefix and origin == "Snippet"
                for label, origin in zip(document["completion_labels"],
                                         document["completion_origins"]))

        self.wait("disabled or removed snippet is absent from live completions", absent)
        self.stable_state(absent)
        self.key(53, "none")
        self.source_is(source)
        self.close_scratch(count)

    def open_snippet_manager(self):
        self.settings()
        self.setting_section("snippets", "Snippets")
        self.click("settings.snippets.open")
        self.wait("dedicated snippet editor opens", lambda d:
                  d["snippet_editor_visible"] and d["snippet_editor_focused"])
        self.native_wait("snippet editor native window focused", lambda windows:
                         any(w["AXTitle"] == "tiptoptyp Snippets" and w.get("AXFocused")
                             for w in windows))
        self.native("resize-focused", 1100, 900)
        self.wait_target("settings.snippets.search")
        if not getattr(self, "snippet_window_checked", False):
            self.check_snippet_window_shortcuts()
            self.snippet_window_checked = True

    def check_snippet_window_shortcuts(self):
        from desktop_ui_journeys import fingerprint
        before = self.snapshot()["document"]
        self.edit_snippet_text("source", "Draft source stays in this window.")
        self.wait("snippet source typing cannot edit its document owner", lambda d:
                  snippet_owner_unchanged(d, before))
        self.click("settings.snippets.revert")
        self.wait_snippet_save_disabled()
        self.click("settings.snippets.source")
        self.wait_target("settings.snippets.source", focused=True)
        self.key(3)
        self.wait_target("settings.snippets.search", focused=True)
        self.key(0)
        self.native("text", "frac")
        self.wait("Cmd+F routes typing only to snippet search", lambda d:
                  d["snippet_editor_focused"] and d["snippet_editor_text_focused"]
                  and d["snippet_editor_query_fingerprint"] == fingerprint("frac")
                  and d["find_visible"] == before["find_visible"]
                  and snippet_owner_unchanged(d, before))
        self.key(0)
        self.key(51, "none")
        self.wait("snippet search clears without changing the main source", lambda d:
                  d["snippet_editor_query_fingerprint"] == fingerprint("")
                  and snippet_owner_unchanged(d, before))

    def close_snippet_manager(self):
        before = self.snapshot()["document"]
        self.key(13)
        self.wait("Cmd+W closes only the snippet editor", lambda d:
                  not d["snippet_editor_visible"] and d["settings_visible"]
                  and snippet_owner_unchanged(d, before))
        self.native_wait("Settings regains focus after closing snippets", lambda windows:
                         any("Settings" in w["AXTitle"] and w.get("AXFocused") for w in windows)
                         and not any(w["AXTitle"] == "tiptoptyp Snippets" and w.get("AXFocused")
                                     for w in windows))
        self.close_settings()

    def edit_snippet_text(self, field, text, dirty=True):
        target = "settings.snippets." + field
        self.click(target)
        self.key(0)
        def consumed():
            return self.wait("exact snippet draft field received native input", lambda d:
                             snippet_field_matches(d, field, text))
        # Keep the clipboard adapter alive until the deferred native field has
        # consumed its exact value. Save may already be enabled for older edits.
        if text:
            self.paste_source(text, consumed)
        else:
            # egui ignores empty Paste events; Delete clears the selection.
            self.key(51, "none")
            consumed()
        if dirty:
            self.wait_target("settings.snippets.save")

    def change_snippet_delimiters(self, opening, closing, keep_source=False):
        self.edit_snippet_text("opening", opening, dirty=False)
        self.edit_snippet_text("closing", closing, dirty=False)
        action = "keep-source" if keep_source else "rewrite"
        target = "settings.snippets.delimiters." + action
        self.wait_target(target)
        self.click(target)
        self.wait_target("settings.snippets.save")

    def filter_snippets(self, field, value):
        target = "settings.snippets.filter." + field
        self.click(target)
        self.wait_target(target + "." + value)
        self.click(target + "." + value)

    def select_snippet(self, origin, language, prefix):
        # Selection is independent of filtering. Always set each filter so an
        # earlier disabled-only or language search cannot conceal the target.
        self.filter_snippets("status", "all")
        self.filter_snippets("origin", origin)
        self.filter_snippets("language", language)
        self.edit_snippet_text("search", prefix, dirty=False)
        name = f"settings.snippets.select.{origin}.{language}.{prefix}"
        self.wait_target(name)
        self.click(name)

    def save_snippet(self, origin, language, prefix, body, enabled=True, opening="{{", closing="}}"):
        self.click("settings.snippets.save")
        self.wait("snippet source and options saved together", lambda d:
                  snippet_matches(d, origin, language, prefix, body, enabled, opening, closing))

    def show_all_snippets(self):
        for option in ("status", "origin", "language"):
            self.filter_snippets(option, "all")
        self.edit_snippet_text("search", "", dirty=False)

    def wait_snippet_save_disabled(self):
        import time
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            target = state["targets"].get("settings.snippets.save")
            if state["document"]["snippet_editor_visible"] and target \
                    and target["age_ms"] <= 500 and target["enabled"] is False:
                return
            time.sleep(0.05)
        raise AssertionError("reverted snippet draft did not disable Save")

    def saved_snippet_is_absent(self, origin, language, prefix):
        self.click("settings.snippets.save")
        self.wait("removed snippet is absent from the saved library", lambda d:
                  snippet_absent(d, origin, language, prefix))

    def settings_snippet_library(self):
        self.settings()
        self.setting_section("snippets", "Snippets")
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.snippet_library.search")
        self.key(0)
        self.native("text", "fncode")
        row = "preset.typst.fncode"
        self.wait_target("settings.snippet_library.edit." + row)
        original = saved_snippet(self.snapshot()["document"], "preset", "typst", "fncode")
        for enabled in (False, True):
            self.click("settings.snippet_library.enabled." + row)
            self.wait("Settings library saves the row toggle", lambda d:
                      (entry := saved_snippet(d, "preset", "typst", "fncode")) is not None
                      and entry["enabled"] is enabled and entry["body"] == original["body"])
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.snippet_library.edit." + row)
        self.wait("Settings Edit opens the exact saved snippet", lambda d:
                  d["snippet_editor_visible"] and d["snippet_editor_focused"]
                  and d["snippet_editor_fields"].get("source") == original["body"])
        self.close_snippet_manager()
        self.settings()
        self.setting_section("snippets", "Snippets")
        self.wait_target("settings.snippet_library.delete." + row)
        self.click("settings.snippet_library.delete." + row)
        self.wait("Settings library deletes the exact saved snippet", lambda d:
                  snippet_absent(d, "preset", "typst", "fncode")
                  and saved_snippet(d, "preset", "typst", "fn") is not None)
        self.click("settings.snippets.open")
        self.wait("snippet editor opens for restoring the library", lambda d:
                  d["snippet_editor_visible"] and d["snippet_editor_focused"])
        self.click("settings.snippets.restore")
        self.click("settings.snippets.save")
        self.wait("restoring suggestions includes the deleted row", lambda d:
                  saved_snippet(d, "preset", "typst", "fncode") is not None
                  and len(d["snippet_presets"]) == 54)
        self.close_snippet_manager()

    def snippets(self):
        presets = self.snapshot()["document"]["snippet_presets"]
        assert len(presets) == 54 and sum(s["language"] == "tex" for s in presets) == 25
        assert all(s["enabled"] and s["placeholder_open"] == "{{" and s["placeholder_close"] == "}}" for s in presets)
        self.settings_snippet_library()

        # Math-only defaults do not take over prose, comments or other providers.
        for prose in ("frac", "// frac"):
            count = self.scratch(prose)
            self.key(49, "ctrl")
            self.wait("math snippets excluded outside equations", lambda d:
                      not d["completion_pending"] and "Snippet" not in d["completion_origins"])
            self.key(53, "none")
            self.source_is(prose)
            self.close_scratch(count)
        count = self.scratch("$frac")
        self.accept_snippet("frac")
        self.source_is("$frac(a, b)")
        self.native("text", "alpha")
        self.source_is("$frac(alpha, b)")
        self.key(48, "none")
        self.native("text", "beta")
        self.source_is("$frac(alpha, beta)")
        self.key(48, "shift")
        self.native("text", "gamma")
        self.source_is("$frac(gamma, beta)")
        self.key(48, "none")
        self.key(48, "none")
        self.native("text", "$")
        self.source_is("$frac(gamma, beta)$")
        self.close_scratch(count)
        count = self.scratch()
        self.key(46, "cmd+alt")
        self.wait("auto-miTeX owns displayed dollar math", lambda d: d["mitex_enabled"])
        self.paste_source("$ frac $")
        self.key(123, "none")
        self.key(123, "none")
        self.accept_snippet("frac", keyboard=True)
        self.source_is("$ \\frac{a}{b} $")
        self.native("text", "alpha")
        self.key(48, "none")
        self.native("text", "beta")
        self.source_is("$ \\frac{alpha}{beta} $")
        self.close_scratch(count)
        count = self.snapshot()["document"]["tabs"]
        self.tex_document()
        original = "\\documentclass{article}\n\\begin{document}\n$\\frac"
        self.key(0)
        self.paste_source(original)
        self.accept_snippet("frac")
        self.source_is(original[:-5] + "\\frac{a}{b}")
        self.native("text", "alpha")
        self.key(48, "none")
        self.native("text", "beta")
        self.source_is(original[:-5] + "\\frac{alpha}{beta}")
        self.close_scratch(count)

        # Every bundled entry is editable, temporarily disableable and removable.
        # Filtering exposes the exact language/provider rather than a combo box.
        self.open_snippet_manager()
        self.select_snippet("preset", "typst", "frac")
        frac = saved_snippet(self.snapshot()["document"], "preset", "typst", "frac")["body"]
        self.click("settings.snippets.enabled")
        self.click("settings.snippets.save")
        self.wait("preset disabled without deleting its source", lambda d:
                  (entry := saved_snippet(d, "preset", "typst", "frac")) is not None
                  and entry["enabled"] is False and entry["body"] == frac)
        self.close_snippet_manager()
        self.snippet_is_absent("$frac", "frac")
        self.open_snippet_manager()
        self.select_snippet("preset", "typst", "frac")
        self.click("settings.snippets.enabled")
        changed_frac = "frac({{1:left}}, {{2:right}}){{0}}"
        self.edit_snippet_text("source", changed_frac)
        self.save_snippet("preset", "typst", "frac", changed_frac)
        self.close_snippet_manager()
        count = self.scratch("$frac")
        self.accept_snippet("frac")
        self.source_is("$frac(left, right)")
        self.close_scratch(count)

        self.open_snippet_manager()
        self.select_snippet("preset", "typst", "frac")
        self.click("settings.snippets.duplicate")
        self.edit_snippet_text("prefix", "nativefrac")
        self.save_snippet("custom", "typst", "nativefrac", changed_frac)
        self.click("settings.snippets.remove")
        self.saved_snippet_is_absent("custom", "typst", "nativefrac")
        self.select_snippet("preset", "typst", "frac")
        self.click("settings.snippets.remove")
        self.saved_snippet_is_absent("preset", "typst", "frac")
        self.close_snippet_manager()
        self.snippet_is_absent("$frac", "frac")
        self.open_snippet_manager()
        self.click("settings.snippets.restore")
        self.click("settings.snippets.save")
        self.wait("restore recreates the complete enabled default library", lambda d:
                  d["snippet_presets"] == presets)

        # Custom delimiters reserve only the markers chosen for that snippet.
        # Literal dollar math and even old ${...} text must survive unchanged.
        self.click("settings.snippets.add.tex")
        self.edit_snippet_text("prefix", "nativeenv")
        self.change_snippet_delimiters("<<", ">>")
        tex_body = ('\\begin{<<1:enumerate>>}\n'
                    '  $x$ \\${2} \\\\\n'
                    '\\end{<<1>>}<<0>>')
        self.edit_snippet_text("source", tex_body)
        self.save_snippet("custom", "tex", "nativeenv", tex_body, opening="<<", closing=">>")
        if self.capture_review:
            self.show_all_snippets()
            self.capture_viewport("snippets")
        self.click("settings.snippets.add.typst")
        self.edit_snippet_text("prefix", "nativeeq")
        typst_body = "{{1:literal}}\n$ [[1:x]] = [[2:y]] $\n[[0]]"
        self.edit_snippet_text("source", typst_body, dirty=False)
        self.change_snippet_delimiters("[[", "]]", keep_source=True)
        self.save_snippet("custom", "typst", "nativeeq", typst_body, opening="[[", closing="]]")
        self.click("settings.snippets.enabled")
        self.save_snippet("custom", "typst", "nativeeq", typst_body, enabled=False, opening="[[", closing="]]")
        self.close_snippet_manager()
        self.snippet_is_absent("nativeeq", "nativeeq")
        self.open_snippet_manager()
        self.select_snippet("custom", "typst", "nativeeq")
        self.click("settings.snippets.enabled")
        self.save_snippet("custom", "typst", "nativeeq", typst_body, opening="[[", closing="]]")
        self.edit_snippet_text("source", "Unsaved source must never expand.")
        self.click("settings.snippets.revert")
        self.wait_snippet_save_disabled()
        self.wait("revert preserves the saved source", lambda d:
                  snippet_matches(d, "custom", "typst", "nativeeq", typst_body, True, "[[", "]]"))
        if self.capture_review:
            self.select_snippet("custom", "typst", "nativeeq")
            self.show_all_snippets()
            self.capture_viewport("snippets")
        self.close_snippet_manager()

        count = self.scratch("nativeeq")
        self.accept_snippet("nativeeq")
        if self.capture_review:
            self.capture_viewport("main")
        self.source_is("{{1:literal}}\n$ x = y $\n")
        self.native("text", "alpha")
        self.source_is("{{1:literal}}\n$ alpha = y $\n")
        self.key(48, "none")
        self.native("text", "beta")
        self.source_is("{{1:literal}}\n$ alpha = beta $\n")
        self.close_scratch(count)
        count = self.snapshot()["document"]["tabs"]
        self.tex_document()
        original = "\\documentclass{article}\n\\begin{document}\nnativeenv"
        self.key(0)
        self.paste_source(original)
        self.accept_snippet("nativeenv")
        expanded = original[:-9] + '\\begin{enumerate}\n  $x$ \\${2} \\\\\n\\end{enumerate}'
        self.source_is(expanded)
        self.native("text", "itemize")
        changed = expanded.replace("enumerate", "itemize")
        self.source_is(changed)
        self.key(6)
        self.source_is(expanded)
        self.key(6, "cmd+shift")
        self.source_is(changed)
        self.close_scratch(count)
        self.open_snippet_manager()
        for language, prefix in (("typst", "nativeeq"), ("tex", "nativeenv")):
            self.select_snippet("custom", language, prefix)
            self.click("settings.snippets.remove")
            self.saved_snippet_is_absent("custom", language, prefix)
        self.close_snippet_manager()
