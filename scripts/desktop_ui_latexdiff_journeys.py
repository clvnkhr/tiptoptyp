"""Single-page LaTeXdiff creation through real native input and read-only inspection."""
import json
import os
import re
import shlex
from pathlib import Path

LATEXDIFF_JOURNEYS = ('latexdiff',)
LATEXDIFF_ARGUMENTS = '--math-markup=1'
LATEXDIFF_OLDER = '\\documentclass{article}\n\\begin{document}\nThe original estimate is uniform.\n\\[a^2+b^2=c^2.\\]\n\\end{document}\n'
LATEXDIFF_NEWER = '\\documentclass{article}\n\\begin{document}\nThe improved estimate is locally uniform.\n\\[a^2+b^2=c^3.\\]\n\\end{document}\n'


def latexdiff_state(document):
    """Missing, old multi-step, or malformed observations cannot pass."""
    state = document.get('latexdiff')
    required = {'visible', 'busy', 'error', 'older_fingerprint',
                'newer_fingerprint', 'output_fingerprint', 'arguments_fingerprint'}
    if (not isinstance(state, dict) or not required.issubset(state) or 'step' in state
            or any(type(state.get(key)) is not bool for key in ('visible', 'busy'))
            or state['busy'] and not state['visible']):
        raise AssertionError('LaTeXdiff form inspection is missing or malformed')
    for key in ('older_fingerprint', 'newer_fingerprint', 'output_fingerprint', 'arguments_fingerprint'):
        value = state[key]
        if value is None and not state['visible']:
            continue
        if not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{16}', value) is None:
            raise AssertionError('LaTeXdiff form path/options fingerprint is invalid')
    error = state['error']
    if error is not None and (not isinstance(error, str) or not error.strip()):
        raise AssertionError('LaTeXdiff form error inspection is invalid')
    return state


def latexdiff_draft(document, older, newer, output=None, arguments=None):
    from desktop_ui_journeys import fingerprint
    state = latexdiff_state(document)
    return (state['visible'] and not state['busy']
            and state['older_fingerprint'] == fingerprint(str(older))
            and state['newer_fingerprint'] == fingerprint(str(newer))
            and (output is None or state['output_fingerprint'] == fingerprint(str(output)))
            and (arguments is None or state['arguments_fingerprint'] == fingerprint(arguments)))


def latexdiff_tab_ids(document):
    from desktop_ui_workspace_switch_journeys import workspace_tab_ids
    ids = workspace_tab_ids(document)
    if (not isinstance(document.get('workspace_root_fingerprint'), str)
            or re.fullmatch(r'[0-9a-f]{16}', document['workspace_root_fingerprint']) is None
            or not isinstance(document.get('source_fingerprint'), str)
            or re.fullmatch(r'[0-9a-f]{16}', document['source_fingerprint']) is None
            or type(document.get('source_bytes')) is not int or document['source_bytes'] < 0
            or type(document.get('active_tab_id')) is not int or document['active_tab_id'] not in ids
            or 'path' not in document or document['path'] is not None
            and (not isinstance(document['path'], str) or not Path(document['path']).is_absolute())):
        raise AssertionError('LaTeXdiff source ownership inspection is missing or malformed')
    return ids


def latexdiff_owner_unchanged(document, before):
    fields = ('path', 'source_fingerprint', 'source_bytes', 'active_tab_id',
              'workspace_root_fingerprint')
    return (latexdiff_tab_ids(document) == latexdiff_tab_ids(before)
            and all(document[key] == before[key] for key in fields))


def latexdiff_created(document, output, before):
    state = latexdiff_state(document)
    if state['visible'] and not state['busy'] and state['error'] is not None:
        raise AssertionError('LaTeXdiff generation failed: ' + state['error'])
    ids, original = latexdiff_tab_ids(document), latexdiff_tab_ids(before)
    notice = document.get('completion_notice')
    return (not state['visible'] and not state['busy'] and not document.get('modal_open', True)
            and document.get('path') == str(output)
            and document.get('document_kind') == 'Tex'
            and document.get('dirty') is False
            and len(ids) == len(original) + 1 and ids[:-1] == original
            and ids[-1] not in original and document.get('active_tab_id') == ids[-1]
            and document.get('preview_tab_id') == ids[-1]
            and document.get('preview_path') == str(output)
            and document.get('workspace_root_fingerprint') == before.get('workspace_root_fingerprint')
            and isinstance(notice, str) and bool(notice.strip())
            and (output.name in notice or output.with_suffix('.pdf').name in notice))


def latexdiff_complete(document, output, source, before):
    from desktop_ui_journeys import fingerprint
    if not latexdiff_created(document, output, before):
        return False
    if document.get('pdf_output_state') == 'Failed':
        raise AssertionError('LaTeXdiff automatic PDF compilation failed: '
                             + str(document.get('completion_notice')))
    return (document.get('source_fingerprint') == fingerprint(source)
            and document.get('source_bytes') == len(source.encode('utf-8'))
            and document.get('pdf_output_state') == 'Saved'
            and document.get('pdf_output_pending') is False
            and str(document.get('pdf_output_revision')) == document.get('revision')
            and document.get('compile_indicator_spinning') is False
            and document.get('preview_requested_backend') == 'Pdfium'
            and document.get('preview_backend') == 'PDFium'
            and document.get('preview_pdfium_ready') is True)


def latexdiff_pdf_artifact(path):
    """A fresh public PDF must be complete and come from the selected pdfLaTeX engine."""
    if not path.is_file() or not 100 < path.stat().st_size <= 10_000_000:
        raise AssertionError('LaTeXdiff did not automatically create a bounded PDF file')
    pdf = path.read_bytes()
    if (not pdf.startswith(b'%PDF-') or b'%%EOF' not in pdf[-1024:]
            or re.search(rb'/Producer\s*\([^)]*pdfTeX', pdf) is None):
        raise AssertionError('LaTeXdiff PDF is incomplete or was not built by selected pdfLaTeX')
    return pdf


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
    def open_latexdiff(self):
        self.focus_document_window()
        self.palette_open()
        palette = self.palette_query('Create LaTeX Diff')
        selected = palette['palette_selected']
        if selected is None or selected not in palette['palette_enabled']:
            raise AssertionError('LaTeXdiff palette command is unavailable')
        self.key(36, 'none')
        self.wait('LaTeXdiff opens one form without page navigation', lambda d:
                  not d['palette_visible'] and latexdiff_state(d)['visible']
                  and not latexdiff_state(d)['busy'])
        for target in ('latexdiff.older', 'latexdiff.newer', 'latexdiff.output',
                       'latexdiff.options.arguments', 'latexdiff.generate'):
            self.wait_target(target)
        if any(name in self.snapshot().get('targets', {})
               for name in ('latexdiff.next', 'latexdiff.back')):
            raise AssertionError('LaTeXdiff still exposes multi-page navigation')

    def latexdiff_type(self, name, value, fingerprint_key):
        from desktop_ui_journeys import fingerprint
        self.click(name)
        self.wait_target(name, focused=True)
        self.key(0)
        self.native('text', str(value))
        self.wait('LaTeXdiff field receives exact native typing', lambda d:
                  latexdiff_state(d)[fingerprint_key] == fingerprint(str(value)))

    def latexdiff(self):
        from desktop_ui_journeys import fingerprint
        self.focus_document_window()
        requested = self.snapshot()['document']['preview_requested_backend']
        if requested not in ('Interactive', 'InteractivePartial', 'Pdfium'):
            raise AssertionError('LaTeXdiff renderer setting is unavailable')
        replacements = [('"build_engine": "tectonic"', '"build_engine": "pdf-latex"')]
        if requested != 'Pdfium':
            replacements.append(('"preview_preference": ' + json.dumps(requested),
                                 '"preview_preference": "Pdfium"'))
        self.tex_settings(replacements)
        self.key(20)  # Keep the selected PDF renderer visible after automatic opening.
        before = self.snapshot()['document']
        folder = self.directory / 'latexdiff fixtures'
        folder.mkdir(exist_ok=False)
        older, newer, output = (folder / name for name in
                                ('older draft.tex', 'newer draft.tex', 'changes.tex'))
        pdf = output.with_suffix('.pdf')
        older.write_text(LATEXDIFF_OLDER, encoding='utf-8')
        newer.write_text(LATEXDIFF_NEWER, encoding='utf-8')
        inputs = [(older, older.read_bytes()), (newer, newer.read_bytes())]
        unchanged = lambda d: latexdiff_owner_unchanged(d, before) and latexdiff_inputs_unchanged(inputs)

        self.open_latexdiff()
        self.click('latexdiff.older')
        self.wait_target('latexdiff.older', focused=True)
        self.key(13)
        self.wait('Cmd+W closes only the LaTeXdiff form', lambda d:
                  not latexdiff_state(d)['visible'] and unchanged(d))
        self.record('latexdiff.close_preserves_owner', source_fingerprint=before['source_fingerprint'])

        self.open_latexdiff()
        self.latexdiff_type('latexdiff.older', older, 'older_fingerprint')
        self.latexdiff_type('latexdiff.newer', newer, 'newer_fingerprint')
        self.latexdiff_type('latexdiff.output', older, 'output_fingerprint')
        self.latexdiff_type('latexdiff.options.arguments', LATEXDIFF_ARGUMENTS, 'arguments_fingerprint')
        self.wait('One form retains both files, output and options together', lambda d:
                  latexdiff_draft(d, older, newer, older, LATEXDIFF_ARGUMENTS) and unchanged(d))
        if getattr(self, 'capture_review', False):
            self.capture_viewport('latexdiff')
        self.click('latexdiff.generate')
        self.wait('An output collision is rejected without altering either input', lambda d:
                  latexdiff_draft(d, older, newer, older, LATEXDIFF_ARGUMENTS)
                  and latexdiff_state(d)['error'] is not None and unchanged(d), timeout=15)
        self.record('latexdiff.input_collision_rejected', source_fingerprint=before['source_fingerprint'])

        self.latexdiff_type('latexdiff.output', output, 'output_fingerprint')
        self.wait('Changing the output on the same form clears the previous error', lambda d:
                  latexdiff_draft(d, older, newer, output, LATEXDIFF_ARGUMENTS)
                  and latexdiff_state(d)['error'] is None and unchanged(d))
        if output.exists() or pdf.exists():
            raise AssertionError('LaTeXdiff generated an artifact before Create was pressed')
        self.click('latexdiff.generate')
        self.wait('Creating the diff opens its new source and visibly responds without another confirmation', lambda d:
                  latexdiff_created(d, output, before) and latexdiff_inputs_unchanged(inputs), timeout=90)
        generated = latexdiff_artifact(output)
        self.wait('The selected compiler automatically saves a PDF and the selected renderer displays it', lambda d:
                  latexdiff_complete(d, output, generated, before)
                  and latexdiff_inputs_unchanged(inputs), timeout=90)
        pdf_bytes = latexdiff_pdf_artifact(pdf)
        self.record('latexdiff.created', source_fingerprint=fingerprint(generated),
                    source_bytes=len(generated.encode('utf-8')), arguments=LATEXDIFF_ARGUMENTS,
                    automatic_open=True, visible_creation_response=True, automatic_pdf=True,
                    compiler='pdfLaTeX', renderer='PDFium', pdf_bytes=len(pdf_bytes))
        self.click('editor.source')
        self.key(13)
        self.wait('Closing the generated tab restores the original source owner', lambda d:
                  unchanged(d) and not latexdiff_state(d)['visible'])
        self.latexdiff_cancel_running(older, newer, folder, before, inputs)
        replacements = [('"build_engine": "pdf-latex"', '"build_engine": "tectonic"')]
        if requested != 'Pdfium':
            replacements.append(('"preview_preference": "Pdfium"',
                                 '"preview_preference": ' + json.dumps(requested)))
        self.tex_settings(replacements)
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
        self.open_latexdiff()
        self.latexdiff_type('latexdiff.older', older, 'older_fingerprint')
        self.latexdiff_type('latexdiff.newer', newer, 'newer_fingerprint')
        self.latexdiff_type('latexdiff.output', output, 'output_fingerprint')
        self.wait('Cancel fixture fills one form', lambda d:
                  latexdiff_draft(d, older, newer, output))
        self.click('latexdiff.generate')
        self.wait('Private LaTeXdiff process actually starts and keeps the form busy', lambda d:
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
                  and not output.exists() and not output.with_suffix('.pdf').exists()
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
        self.wait('Cancelled form closes without changing its owner', lambda d:
                  not latexdiff_state(d)['visible'] and latexdiff_owner_unchanged(d, before))
        self.latexdiff_executable_setting('', previous=executable)
