"""Native TeX conversion and opt-in autofixes; observation never dispatches edits."""

AUTOFIX_JOURNEYS = ('tex_autofix', 'tex_math_conversion')

AUTOFIX_SOURCE = (
    'α Figure \t\n  \\ref{fig:a}, equation  \\eqref{eq:a}.\n'
    '% diag: off\nProtected \\ref{protected}.\n% diag: on\n'
    '\\begin{verbatim}\nLiteral \\ref{literal}.\n\\end{verbatim}\n'
)
AUTOFIX_EXPECTED = AUTOFIX_SOURCE.replace('Figure \t\n  \\ref', 'Figure~\\ref').replace('equation  \\eqref', 'equation~\\eqref')
AUTOFIX_SAVE_FIELDS = {'manual': 'autofix_on_manual_save', 'auto': 'autofix_on_autosave'}
AUTOFIX_OWNER_FIELDS = ('path', 'document_kind', 'tabs', 'active_tab_id',
                        'preview_tab_id', 'preview_path', 'workspace_root_fingerprint')


def autofix_saved(document, owner, source, trigger, enabled):
    """A saved fix needs its exact source owner and completed save pipeline."""
    from desktop_ui_journeys import fingerprint
    if trigger not in AUTOFIX_SAVE_FIELDS:
        raise AssertionError('Unknown autofix save trigger')
    for field in AUTOFIX_SAVE_FIELDS.values():
        if type(document.get(field)) is not bool:
            raise AssertionError('Effective autofix save-trigger inspection is unavailable: ' + field)
    state = document.get('autofix')
    if not isinstance(state, dict) or state.get('phase') not in ('idle', 'running', 'cancelling', 'failed'):
        raise AssertionError('Autofix lifecycle inspection is unavailable')
    if state['phase'] == 'failed':
        raise AssertionError('TeX autofixing failed before saving')
    for field in (*AUTOFIX_OWNER_FIELDS, 'save_operation', 'formatting_phase', 'dirty'):
        if field not in document or (field in AUTOFIX_OWNER_FIELDS and field not in owner):
            raise AssertionError('Save inspection is missing ' + field)
    return (all(type(document[field]) is type(owner[field]) and document[field] == owner[field]
                for field in AUTOFIX_OWNER_FIELDS)
            and document[AUTOFIX_SAVE_FIELDS[trigger]] is enabled
            and document.get('source_fingerprint') == fingerprint(source)
            and document.get('source_bytes') == len(source.encode('utf-8'))
            and document['dirty'] is False and document.get('modal_open') is False
            and state['phase'] == 'idle' and document['save_operation'] is None
            and document['formatting_phase'] == 'idle')


class AutofixJourneys:
    def autofix_fixture(self, name, source):
        before = self.snapshot()['document']
        path = self.directory / name
        if path.exists():
            raise AssertionError('Autofix fixture must have its own new path')
        path.write_text(source, encoding='utf-8')
        self.open_harper_fixture(path)
        self.source_is(source)
        return path, before

    def tex_autofix(self):
        from desktop_ui_journeys import fingerprint
        before = self.snapshot()['document']
        auto_save = before['auto_save']
        original_rules = before['autofix_rules']
        original_enabled = 'reference-ties' in original_rules['enabled']
        original_save = {trigger: before.get(field) for trigger, field in AUTOFIX_SAVE_FIELDS.items()}
        if any(type(value) is not bool for value in original_save.values()):
            raise AssertionError('Effective autofix save-trigger preferences are unavailable')
        original_formatter = before['tex_formatter']
        if original_formatter not in ('badness', 'tex-fmt', 'disabled'):
            raise AssertionError('Effective TeX formatter inspection is unavailable')
        formatter_changed = False

        def reference_enabled(enabled):
            current = 'reference-ties' in self.snapshot()['document']['autofix_rules']['enabled']
            if current == enabled:
                return
            self.settings()
            self.setting_section('eqref', 'TeX autofixing')
            self.wait_target('settings.autofix.reference-ties')
            self.click('settings.autofix.reference-ties')
            self.wait('Reference autofix checkbox changes the applied preference', lambda d:
                      ('reference-ties' in d['autofix_rules']['enabled']) is enabled)
            self.close_settings()
            self.click('editor.source')

        def on_save_enabled(trigger, enabled):
            field = AUTOFIX_SAVE_FIELDS[trigger]
            if self.snapshot()['document'][field] is enabled:
                return
            other = {key: self.snapshot()['document'][key]
                     for key in AUTOFIX_SAVE_FIELDS.values() if key != field}
            label = 'Autofix on manual save' if trigger == 'manual' else 'Autofix on autosave'
            target = 'settings.autofix.on_manual_save' if trigger == 'manual' else 'settings.autofix.on_autosave'
            self.settings()
            self.setting_section(label.lower(), 'TeX autofixing')
            self.wait_target(target)
            self.click(target)
            self.wait(label + ' checkbox changes only its applied preference', lambda d:
                      d[field] is enabled and all(d[key] is value for key, value in other.items()))
            self.close_settings()
            self.click('editor.source')

        def global_autosave(enabled):
            current = self.snapshot()['document']['auto_save']
            if current is not enabled:
                self.tex_settings([('"auto_save": ' + str(current).lower(),
                                    '"auto_save": ' + str(enabled).lower())])

        if auto_save:
            self.tex_settings([('"auto_save": true', '"auto_save": false')])
        path, owner = self.autofix_fixture('autofix-native.tex', AUTOFIX_SOURCE)
        try:
            on_save_enabled('manual', False)
            on_save_enabled('auto', True)
            reference_enabled(False)
            self.key(3, 'cmd+alt+shift')
            self.wait('Disabled reference rule leaves source and issue unchanged', lambda d:
                      d['source_fingerprint'] == fingerprint(AUTOFIX_SOURCE)
                      and d['autofix']['phase'] == 'idle'
                      and str(d['completion_notice']).startswith('No enabled TeX autofixes found.'), timeout=35)
            reference_enabled(True)
            self.key(3, 'cmd+alt+shift')
            self.wait('One shortcut fixes every enabled reference spacing issue', lambda d:
                      d['source_fingerprint'] == fingerprint(AUTOFIX_EXPECTED)
                      and d['autofix']['phase'] == 'idle', timeout=35)
            revision = self.snapshot()['document']['revision']
            self.key(3, 'cmd+alt+shift')
            self.wait('Repeated autofix reaches an unchanged idle document', lambda d:
                      d['autofix']['phase'] == 'idle' and d['revision'] == revision
                      and str(d['completion_notice']).startswith('No enabled TeX autofixes found.'), timeout=35)
            self.key(6)
            self.source_is(AUTOFIX_SOURCE)
            self.key(6, 'cmd+shift')
            self.source_is(AUTOFIX_EXPECTED)
            self.key(6)
            self.source_is(AUTOFIX_SOURCE)
            if path.read_text(encoding='utf-8') != AUTOFIX_SOURCE:
                raise AssertionError('Autofix bypassed disabled autosave')
            # Manual Save normally also formats. Pin only that independent
            # preference while checking the exact autofix-before-save bytes.
            if original_formatter != 'disabled':
                self.tex_settings([('"formatter": "' + original_formatter + '"',
                                    '"formatter": "disabled"')])
                formatter_changed = True
            save_owner = self.snapshot()['document']
            off_source = AUTOFIX_SOURCE.replace('α Figure', 'β Figure')
            self.click('editor.source')
            self.key(0)
            self.paste_source(off_source)
            self.wait('Native edit is unsaved while manual-save autofix is off', lambda d:
                      d['dirty'] and d['autofix_on_manual_save'] is False
                      and d['autofix_on_autosave'] is True)
            self.key(1)
            self.wait('Saving with autofix off preserves the reference spacing', lambda d:
                      autofix_saved(d, save_owner, off_source, 'manual', False), timeout=35)
            if path.read_text(encoding='utf-8') != off_source:
                raise AssertionError('Disabled manual-save autofix changed or lost the saved source')
            on_save_enabled('manual', True)
            on_save_enabled('auto', False)
            on_source = AUTOFIX_SOURCE.replace('α Figure', 'γ Figure')
            on_expected = AUTOFIX_EXPECTED.replace('α Figure', 'γ Figure')
            self.click('editor.source')
            self.key(0)
            self.paste_source(on_source)
            self.wait('Typing does not trigger the opted-in save fix', lambda d:
                      d['dirty'] and d['autofix_on_manual_save'] is True
                      and d['autofix_on_autosave'] is False
                      and d['source_fingerprint'] == fingerprint(on_source))
            self.key(1)
            self.wait('Save fixes the source before finishing its owned write', lambda d:
                      autofix_saved(d, save_owner, on_expected, 'manual', True), timeout=35)
            if path.read_text(encoding='utf-8') != on_expected:
                raise AssertionError('Autofix on manual save did not write the corrected source')
            # Exercise the actual autosave scheduler, with the manual option
            # opposite each automatic option so one cannot stand in for the other.
            global_autosave(True)
            auto_off_source = AUTOFIX_SOURCE.replace('α Figure', 'δ Figure')
            self.click('editor.source')
            self.key(0)
            self.paste_source(auto_off_source)
            self.wait('Autosave with only manual fixes enabled preserves source spacing', lambda d:
                      d['auto_save'] is True and d['autofix_on_manual_save'] is True
                      and autofix_saved(d, save_owner, auto_off_source, 'auto', False), timeout=35)
            if path.read_text(encoding='utf-8') != auto_off_source:
                raise AssertionError('Manual-save preference leaked into autosave')
            on_save_enabled('manual', False)
            on_save_enabled('auto', True)
            auto_on_source = AUTOFIX_SOURCE.replace('α Figure', 'ε Figure')
            auto_on_expected = AUTOFIX_EXPECTED.replace('α Figure', 'ε Figure')
            self.click('editor.source')
            self.key(0)
            self.paste_source(auto_on_source)
            self.wait('Autosave independently corrects its source before completing', lambda d:
                      d['auto_save'] is True and d['autofix_on_manual_save'] is False
                      and autofix_saved(d, save_owner, auto_on_expected, 'auto', True), timeout=35)
            if path.read_text(encoding='utf-8') != auto_on_expected:
                raise AssertionError('Autofix on autosave did not write corrected source')
            global_autosave(False)
            self.record('tex_autofix', native_shortcut=True, settings_toggle=True,
                        all_enabled_references=True, comment_and_raw_protected=True,
                        one_step_undo_redo=True, idempotent=True, manual_disk_unchanged=True,
                        initial_manual_default_off=not original_save['manual'],
                        initial_autosave_default_off=not original_save['auto'],
                        independent_save_toggles=True, manual_disabled_preserved=True,
                        manual_save_corrected=True, autosave_disabled_preserved=True,
                        autosave_corrected=True, saved_clean_and_idle=True,
                        saved_source_owner_preserved=True)
        finally:
            global_autosave(False)
            for trigger, enabled in original_save.items():
                on_save_enabled(trigger, enabled)
            reference_enabled(original_enabled)
            if formatter_changed:
                self.tex_settings([('"formatter": "disabled"',
                                    '"formatter": "' + original_formatter + '"')])
            self.close_scratch(owner['tabs'])
            if auto_save:
                self.tex_settings([('"auto_save": false', '"auto_save": true')])

    def tex_math_conversion(self):
        from desktop_ui_journeys import fingerprint
        auto_save = self.snapshot()['document']['auto_save']
        if auto_save:
            self.tex_settings([('"auto_save": true', '"auto_save": false')])
        source = 'Before \\(α\\), after.\n'
        path, before = self.autofix_fixture('math-conversion-native.tex', source)
        fixture_owner = self.snapshot()['document']
        owner_fields = ('path', 'tabs', 'active_tab_id', 'preview_tab_id', 'preview_path', 'document_kind')

        def owns_source(document):
            return all(document[field] == fixture_owner[field] for field in owner_fields)

        def source_is_owned(expected):
            self.wait('Exact math source and original document owner', lambda d:
                      owns_source(d) and d['source_fingerprint'] == fingerprint(expected)
                      and d['source_bytes'] == len(expected.encode('utf-8')))

        try:
            with self.exact_math_conversion():
                def menu_choice(label):
                    self.click('editor.source')
                    self.wait('TeX source owns the keyboard after Settings', lambda d:
                              owns_source(d) and d['editor_keyboard_focused'])
                    self.key(126, 'cmd')
                    # Scalar 10 is inside either form, after the Unicode alpha.
                    for _ in range(10):
                        self.key(124, 'none')
                    self.wait('Caret enters the selected formula', lambda d:
                              owns_source(d) and d['editor_keyboard_focused']
                              and d['cursor'] == [10, 10])
                    self.wait_target('editor.caret')
                    caret = self.snapshot()['targets']['editor.caret']
                    self.right_click_at(caret['x'], caret['y'])
                    self.wait_target('menu.Convert to…')
                    row = self.snapshot()['targets']['menu.Convert to…']
                    self.native('move', row['x'], row['y'])
                    self.wait_target('menu.' + label)
                    self.click('menu.' + label)
                menu_choice(r'Display math \[...\]')
                display = 'Before \\[α,\\] after.\n'
                source_is_owned(display)
                menu_choice(r'Inline math \(...\)')
                source_is_owned('Before \\(α,\\) after.\n')
                self.key(6)
                source_is_owned(display)
                self.key(6)
                source_is_owned(source)
                if path.read_text(encoding='utf-8') != source:
                    raise AssertionError('Math conversion bypassed disabled autosave')
                self.record('tex_math_conversion', dynamic_inline_display=True,
                            immediate_punctuation_inside=True, one_step_undo=True,
                            source_owner_preserved=True, disk_unchanged=True)
        finally:
            self.close_scratch(before['tabs'])
            if auto_save:
                self.tex_settings([('"auto_save": false', '"auto_save": true')])
