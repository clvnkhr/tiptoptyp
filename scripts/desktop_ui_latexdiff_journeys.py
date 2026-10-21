"""LaTeXdiff wizard journeys using real native input and read-only inspection."""
import json
import os
import re
import shlex
from pathlib import Path

LATEXDIFF_JOURNEYS = ('latexdiff_wizard',)
LATEXDIFF_ARGUMENTS = '--math-markup=1'
LATEXDIFF_OLDER = '\\documentclass{article}\n\\begin{document}\nThe original estimate is uniform.\n\\[a^2+b^2=c^2.\\]\n\\end{document}\n'
LATEXDIFF_NEWER = '\\documentclass{article}\n\\begin{document}\nThe improved estimate is locally uniform.\n\\[a^2+b^2=c^3.\\]\n\\end{document}\n'


def latexdiff_state(document):
    """Missing or malformed observations must never be mistaken for a closed UI."""
    state = document.get('latexdiff')
    required = {'visible', 'busy', 'step', 'error', 'output_path', 'older_fingerprint',
                'newer_fingerprint', 'output_fingerprint', 'arguments_fingerprint'}
    if (not isinstance(state, dict) or not required.issubset(state)
            or any(type(state.get(key)) is not bool for key in ('visible', 'busy'))
            or state['step'] not in (None, 'files', 'output', 'review')
            or state['visible'] and state['step'] is None
            or state['busy'] and not state['visible']):
        raise AssertionError('LaTeXdiff wizard inspection is missing or malformed')
    for key in ('older_fingerprint', 'newer_fingerprint', 'output_fingerprint', 'arguments_fingerprint'):
        value = state[key]
        if value is None and not state['visible']:
            continue
        if not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{16}', value) is None:
            raise AssertionError('LaTeXdiff wizard path/options fingerprint is invalid')
    error = state['error']
    if error is not None and (not isinstance(error, str) or not error.strip()):
        raise AssertionError('LaTeXdiff wizard error inspection is invalid')
    output = state['output_path']
    if output is not None and (not isinstance(output, str) or not Path(output).is_absolute()):
        raise AssertionError('LaTeXdiff completed output path is invalid')
    return state


def latexdiff_draft(document, step, older, newer, output=None, arguments=None):
    from desktop_ui_journeys import fingerprint
    state = latexdiff_state(document)
    return (state['visible'] and not state['busy'] and state['step'] == step
            and state['older_fingerprint'] == fingerprint(str(older))
            and state['newer_fingerprint'] == fingerprint(str(newer))
            and (output is None or state['output_fingerprint'] == fingerprint(str(output)))
            and (arguments is None or state['arguments_fingerprint'] == fingerprint(arguments)))


def latexdiff_complete(document, output):
    state = latexdiff_state(document)
    if state['visible'] and not state['busy'] and state['error'] is not None:
        raise AssertionError('LaTeXdiff generation failed: ' + state['error'])
    return (state['visible'] and not state['busy'] and state['step'] == 'review'
            and state['output_path'] == str(output) and state['error'] is None)


def latexdiff_owner_unchanged(document, before):
    return all(document[key] == before[key] for key in
               ('path', 'source_fingerprint', 'source_bytes', 'tabs', 'active_tab_id'))


def latexdiff_inputs_unchanged(inputs):
    for path, original in inputs:
        if not path.is_file() or path.read_bytes() != original:
            raise AssertionError('LaTeXdiff modified an original input file')
    return True


def latexdiff_artifact(path):
    """Check real changed body text, not only preamble macro definitions."""
    if not path.is_file() or not 0 < path.stat().st_size <= 1_000_000:
        raise AssertionError('LaTeXdiff did not create a bounded nonempty source file')
    source = path.read_text(encoding='utf-8')
    marker = re.search(r'\\begin\s*\{\s*document\s*\}', source)
    body = source[marker.end():] if marker else ''
    if (not re.search(r'\\DIFadd(?:\{|begin)', body)
            or not re.search(r'\\DIFdel(?:\{|begin)', body)
            or 'original' not in body or 'improved' not in body):
        raise AssertionError('LaTeXdiff output has no real additions and deletions in its body')
    return source


class LatexdiffJourneys:
    def open_latexdiff_wizard(self):
        self.focus_document_window()
        self.palette_open()
        palette = self.palette_query('Create LaTeX Diff')
        selected = palette['palette_selected']
        if selected is None or selected not in palette['palette_enabled']:
            raise AssertionError('LaTeXdiff palette command is unavailable')
        self.key(36, 'none')
        self.wait('LaTeXdiff opens on the Files step', lambda d:
                  not d['palette_visible'] and latexdiff_state(d)['visible']
                  and latexdiff_state(d)['step'] == 'files' and not latexdiff_state(d)['busy'])

    def latexdiff_type(self, name, value, fingerprint_key):
        from desktop_ui_journeys import fingerprint
        self.click(name)
        self.wait_target(name, focused=True)
        self.key(0)
        self.native('text', str(value))
        self.wait('LaTeXdiff field receives exact native typing', lambda d:
                  latexdiff_state(d)[fingerprint_key] == fingerprint(str(value)))

    def latexdiff_wizard(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        before = self.snapshot()['document']
        folder = self.directory / 'latexdiff fixtures'
        folder.mkdir(exist_ok=False)
        older, newer, output = (folder / name for name in
                                ('older draft.tex', 'newer draft.tex', 'changes.tex'))
        older.write_text(LATEXDIFF_OLDER, encoding='utf-8')
        newer.write_text(LATEXDIFF_NEWER, encoding='utf-8')
        inputs = [(older, older.read_bytes()), (newer, newer.read_bytes())]
        unchanged = lambda d: latexdiff_owner_unchanged(d, before) and latexdiff_inputs_unchanged(inputs)

        # Closing the focused child must leave its source owner/tab untouched.
        self.open_latexdiff_wizard()
        self.click('latexdiff.older')
        self.wait_target('latexdiff.older', focused=True)
        self.key(13)
        self.wait('Cmd+W closes only the LaTeXdiff wizard', lambda d:
                  not latexdiff_state(d)['visible'] and unchanged(d))
        self.record('latexdiff.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_latexdiff_wizard()
        self.latexdiff_type('latexdiff.older', older, 'older_fingerprint')
        self.latexdiff_type('latexdiff.newer', newer, 'newer_fingerprint')
        self.wait('Files step holds both exact paths', lambda d:
                  latexdiff_draft(d, 'files', older, newer) and unchanged(d))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('latexdiff')
        self.click('latexdiff.next')
        self.wait('Files advances to Output and options', lambda d:
                  latexdiff_draft(d, 'output', older, newer) and unchanged(d))
        self.latexdiff_type('latexdiff.output', older, 'output_fingerprint')
        self.latexdiff_type('latexdiff.options.arguments', LATEXDIFF_ARGUMENTS, 'arguments_fingerprint')
        self.click('latexdiff.back')
        self.wait('Back retains both files and output/options drafts', lambda d:
                  latexdiff_draft(d, 'files', older, newer, older, LATEXDIFF_ARGUMENTS) and unchanged(d))
        self.click('latexdiff.next')
        self.wait('Output draft survives returning from Files', lambda d:
                  latexdiff_draft(d, 'output', older, newer, older, LATEXDIFF_ARGUMENTS) and unchanged(d))
        self.click('latexdiff.next')
        self.wait('Output advances to Review', lambda d:
                  latexdiff_draft(d, 'review', older, newer, older, LATEXDIFF_ARGUMENTS) and unchanged(d))
        self.click('latexdiff.generate')
        self.wait('An output collision is rejected without altering either input', lambda d:
                  latexdiff_draft(d, 'review', older, newer, older, LATEXDIFF_ARGUMENTS)
                  and latexdiff_state(d)['error'] is not None
                  and latexdiff_state(d)['output_path'] is None and unchanged(d), timeout=15)
        self.record('latexdiff.input_collision_rejected', source_fingerprint=before['source_fingerprint'])

        self.click('latexdiff.back')
        self.wait('Review returns to Output after validation error', lambda d:
                  latexdiff_draft(d, 'output', older, newer, older, LATEXDIFF_ARGUMENTS) and unchanged(d))
        self.latexdiff_type('latexdiff.output', output, 'output_fingerprint')
        self.wait('A changed output clears the previous error', lambda d:
                  latexdiff_draft(d, 'output', older, newer, output, LATEXDIFF_ARGUMENTS)
                  and latexdiff_state(d)['error'] is None and unchanged(d))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('latexdiff')
        self.click('latexdiff.next')
        self.wait('Review contains the final files, destination and option', lambda d:
                  latexdiff_draft(d, 'review', older, newer, output, LATEXDIFF_ARGUMENTS) and unchanged(d))
        if output.exists():
            raise AssertionError('LaTeXdiff generated output before Create was pressed')
        if getattr(self, 'capture_review', False):
            self.capture_viewport('latexdiff')
        self.click('latexdiff.generate')
        self.wait('Real LaTeXdiff finishes creating the requested file', lambda d:
                  latexdiff_complete(d, output) and unchanged(d), timeout=90)
        generated = latexdiff_artifact(output)
        self.record('latexdiff.created', source_fingerprint=fingerprint(generated),
                    source_bytes=len(generated.encode('utf-8')), arguments=LATEXDIFF_ARGUMENTS)
        self.click('latexdiff.open')
        self.wait('Open selects the complete generated TeX source in its own tab', lambda d:
                  not latexdiff_state(d)['visible'] and d['path'] == str(output)
                  and d['source_fingerprint'] == fingerprint(generated)
                  and d['source_bytes'] == len(generated.encode('utf-8'))
                  and d['tabs'] == before['tabs'] + 1 and d['active_tab_id'] != before['active_tab_id']
                  and not d['dirty'] and latexdiff_inputs_unchanged(inputs), timeout=20)
        self.click('editor.source')
        self.key(13)
        self.wait('Closing the generated tab restores the original source owner', lambda d:
                  unchanged(d) and not latexdiff_state(d)['visible'])
        self.latexdiff_cancel_running(older, newer, folder, before, inputs)
        self.record('latexdiff.complete', source_fingerprint=before['source_fingerprint'])

    def latexdiff_executable_setting(self, value, previous=''):
        """Edit only LaTeXdiff's field; several system tools share its name."""
        self.settings()
        if not self.snapshot()['document']['settings_json']['visible']:
            self.click('settings.json.mode')
        self.wait('LaTeXdiff JSON settings ready', lambda d:
                  d['settings_json']['visible'] and d['settings_json']['valid'])

        def find(query):
            self.key(3)
            self.wait_target('settings.json.find', focused=True)
            self.key(0)
            self.native('text', query)
            self.wait('LaTeXdiff settings query found', lambda d:
                      d['settings_json']['query'] == query)
            self.key(36, 'none')
            self.wait_target('settings.json', focused=True)

        def line(executable):
            return '    "executable": ' + json.dumps(str(executable)) + ','

        def select_executable(executable):
            if executable:
                # This newly created private path is unique in the test profile.
                # Find selects the complete logical range even when it wraps.
                find(line(executable))
            else:
                # Blank executable values are shared by several tools; scope the
                # short, unwrapped line to its unique owning object first.
                find('"latexdiff": {')
                self.key(125, 'none')
                self.key(123, 'cmd')
                self.key(124, 'cmd+shift')

        select_executable(previous)
        # A changed schema/selection must fail before editing another binary.
        self.copy_with_receipt(line(previous), lambda: self.key(8))
        before = self.snapshot()['document']['settings_json']['fingerprint']
        self.paste_source(line(value), lambda: self.wait(
            'Scoped LaTeXdiff executable edit validates', lambda d:
            d['settings_json']['valid'] and d['settings_json']['fingerprint'] != before))
        select_executable(value)
        self.copy_with_receipt(line(value), lambda: self.key(8))
        self.click('settings.json.save')
        self.wait('LaTeXdiff executable setting saved', lambda d: d['settings_json']['saved'])
        self.close_settings()
        self.click('editor.source')

    def latexdiff_cancel_running(self, older, newer, folder, before, inputs):
        """A private executable makes the actual busy/Cancel transition observable."""
        receipt = folder / 'worker.pid'
        executable = folder / 'controlled latexdiff'
        output = folder / 'cancelled.tex'
        executable.write_text("#!/bin/sh\nprintf '%s' \"$$\" > " + shlex.quote(str(receipt))
                              + '\n/bin/sleep 30\n', encoding='utf-8')
        executable.chmod(0o700)
        self.latexdiff_executable_setting(executable)
        self.open_latexdiff_wizard()
        self.latexdiff_type('latexdiff.older', older, 'older_fingerprint')
        self.latexdiff_type('latexdiff.newer', newer, 'newer_fingerprint')
        self.click('latexdiff.next')
        self.wait('Cancel fixture reaches Output', lambda d:
                  latexdiff_draft(d, 'output', older, newer))
        self.latexdiff_type('latexdiff.output', output, 'output_fingerprint')
        self.click('latexdiff.next')
        self.wait('Cancel fixture reaches Review', lambda d:
                  latexdiff_draft(d, 'review', older, newer, output))
        self.click('latexdiff.generate')
        self.wait('Private LaTeXdiff process actually starts and keeps the wizard busy', lambda d:
                  latexdiff_state(d)['visible'] and latexdiff_state(d)['busy']
                  and receipt.is_file() and receipt.stat().st_size > 0, timeout=15)
        if not 0 < receipt.stat().st_size <= 20:
            raise AssertionError('Controlled LaTeXdiff returned an invalid PID receipt')
        pid_text = receipt.read_text()
        if not pid_text.isdecimal() or int(pid_text) <= 1:
            raise AssertionError('Controlled LaTeXdiff returned an invalid PID')
        pid = int(pid_text)
        self.click('latexdiff.cancel')
        self.wait('Cancel stops generation without creating a diff or altering source', lambda d:
                  latexdiff_state(d)['visible'] and not latexdiff_state(d)['busy']
                  and latexdiff_state(d)['output_path'] is None and not output.exists()
                  and latexdiff_owner_unchanged(d, before)
                  and latexdiff_inputs_unchanged(inputs), timeout=15)
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError('LaTeXdiff Cancel left its controlled process alive')
        self.record('latexdiff.running_cancel', process_stopped=True, output_exists=False)
        self.click('latexdiff.close')
        self.wait('Cancelled wizard closes without changing its owner', lambda d:
                  not latexdiff_state(d)['visible'] and latexdiff_owner_unchanged(d, before))
        self.latexdiff_executable_setting('', previous=executable)
