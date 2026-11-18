"""Native TeX magic routing/snippets with owned synthetic compiler processes."""
from pathlib import Path
import re

from desktop_ui_snippet_journeys import snippet_completion, snippet_owner_unchanged
from tex_magic_fixture import (MAGIC_MAIN, MAGIC_MAIN_EDITED, MAGIC_ALTERNATE,
                               MAGIC_CHILD, MAGIC_CHILD_ALTERNATE,
                               tex_magic_paths, tex_magic_invocations)

TEX_MAGIC_JOURNEYS = ('tex_magic',)
MAGIC_SNIPPETS = {
    'texroot':'% !TeX root = {{1:main.tex}}{{0}}',
    'texprogram':'% !TeX program = {{1:pdflatex}}{{0}}',
    'texencoding':'% !TeX encoding = {{1:UTF-8}}{{0}}',
    'texspellcheck':'% !TeX spellcheck = {{1:en-US}}{{0}}',
    'texbibprogram':'% !BIB program = {{1:biber}}{{0}}',
}


def _fingerprint(value):
    from desktop_ui_journeys import fingerprint
    return fingerprint(value)


def tex_magic_settings(document):
    digest = document.get('tex_settings_fingerprint')
    if (not isinstance(digest, str) or re.fullmatch(r'[0-9a-f]{16}', digest) is None
            or not isinstance(document.get('tex_build_engine'), str)):
        raise AssertionError('Global TeX settings inspection is missing or malformed')
    return digest


def tex_magic_catalog(document):
    presets = document.get('snippet_presets')
    if not isinstance(presets, list):
        raise AssertionError('The saved snippet catalogue is unavailable')
    for prefix, body in MAGIC_SNIPPETS.items():
        matches = [entry for entry in presets
                   if entry.get('prefix') == prefix and entry.get('language') == 'tex']
        if (len(matches) != 1 or matches[0].get('body') != _fingerprint(body)
                or matches[0].get('enabled') is not True
                or matches[0].get('line_start') is not True
                or matches[0].get('scope') != 'text'):
            return False
    return True


def tex_magic_invoked(records, *, after, engine, entry, source):
    return any(record.get('engine') == engine and record.get('input_name') == Path(entry).name
               and record.get('source_fingerprint') == _fingerprint(source)
               and record.get('source_bytes') == len(source.encode('utf-8'))
               and '-no-shell-escape' in record.get('arguments', [])
               for record in records[after:])


def tex_magic_pdf(path, engine, source):
    if not path.is_file():
        return False
    if not 100 < path.stat().st_size < 16 * 1024:
        raise AssertionError('Synthetic TeX magic PDF exceeded its bounded fixture size')
    data = path.read_bytes()
    return (data.startswith(b'%PDF-') and data.endswith(b'%%EOF\n')
            and ('SyntheticTeXMagic-' + engine).encode() in data
            and _fingerprint(source).encode() in data)


def tex_magic_typst_excluded(document, path):
    labels, origins = document.get('completion_labels'), document.get('completion_origins')
    if (not isinstance(labels, list) or not isinstance(origins, list)
            or len(labels) != len(origins)
            or any(not isinstance(value, str) for value in labels + origins)
            or type(document.get('completion_pending')) is not bool):
        raise AssertionError('Local snippet completion evidence is unavailable')
    return (document.get('document_kind') == 'Typst' and document.get('path') == str(path)
            and document.get('source_fingerprint') == _fingerprint('texroot')
            and document.get('source_bytes') == len('texroot')
            and not document['completion_pending']
            and all(label not in MAGIC_SNIPPETS or origin != 'Snippet'
                    for label, origin in zip(labels, origins)))


def tex_magic_completed(document, records, *, after, engine, entry, source,
                        active, preview, settings):
    current = tex_magic_settings(document)
    if current != settings or document['tex_build_engine'] != 'pdfLaTeX':
        raise AssertionError('A document magic comment changed the global TeX settings')
    magic = document.get('tex_magic')
    label = {'pdflatex':'pdfLaTeX', 'xelatex':'XeLaTeX'}[engine]
    return (isinstance(magic, dict) and magic.get('entry') == str(entry)
            and magic.get('engine') == label and document.get('path') == str(active)
            and document.get('preview_path') == str(preview)
            and document.get('preview_backend') == 'PDFium'
            and document.get('preview_pdfium_ready') is True
            and document.get('compile_indicator_spinning') is False
            and document.get('pdf_output_pending') is False
            and document.get('pdf_output_state') == 'Saved'
            and tex_magic_invoked(records, after=after, engine=engine, entry=entry, source=source)
            and tex_magic_pdf(Path(entry).with_suffix('.pdf'), engine, source))


class TexMagicJourneys:
    def tex_magic_compile(self, *, after, engine, entry, source, active, preview, settings):
        self.key(15)
        state = self.wait('The magic root and compiler produce the fresh PDF for the pinned entry', lambda d:
            tex_magic_completed(d, tex_magic_invocations(self.directory), after=after,
                engine=engine, entry=entry, source=source, active=active, preview=preview,
                settings=settings), timeout=30)
        self.record('tex.magic.compiled', engine=engine, root_fingerprint=_fingerprint(str(entry)),
                    source_fingerprint=_fingerprint(source), source_bytes=len(source.encode('utf-8')),
                    settings_unchanged=True, synthetic_engine=True,
                    effective=state['document']['tex_magic'])

    def tex_magic_library(self):
        before = self.snapshot()['document']
        self.settings()
        self.setting_section('snippets', 'Snippets')
        self.click('settings.snippets.open')
        self.wait('The normal snippet manager opens for TeX magic discovery', lambda d:
                  d['snippet_editor_visible'] and d['snippet_editor_focused'])
        self.native('resize-focused', 1100, 900)
        self.wait_target('settings.snippets.add.tex_magic')
        self.click('settings.snippets.add.tex_magic')
        self.wait('Add TeX magic comments selects the real TeX root preset', lambda d:
                  tex_magic_catalog(d) and snippet_owner_unchanged(d, before)
                  and d.get('snippet_editor_fields', {}).get('prefix') == _fingerprint('texroot')
                  and d.get('snippet_editor_fields', {}).get('source') == _fingerprint(MAGIC_SNIPPETS['texroot']))
        self.close_snippet_manager()
        self.record('tex.magic.snippet_discovery', dedicated_action=True, five_tex_presets=True,
                    main_source_unchanged=True)

    def tex_magic(self):
        paths = tex_magic_paths(self.directory)
        self.tex_settings([
            ('"auto_save": true', '"auto_save": false'),
            ('"build_engine": "tectonic"', '"build_engine": "pdf-latex"'),
            ('"build_driver": "latexmk"', '"build_driver": "direct"'),
            ('"formatter": "badness"', '"formatter": "disabled"'),
        ])
        self.tex_magic_library()
        settings = tex_magic_settings(self.snapshot()['document'])
        first = len(tex_magic_invocations(self.directory))
        self.open_harper_fixture(paths['child'])
        self.key(0)
        self.paste_source('texroot')
        self.accept_snippet('texroot')
        self.source_is('% !TeX root = main.tex')
        self.native('text', '../main.tex')
        self.source_is('% !TeX root = ../main.tex')
        self.key(48, 'none')
        self.paste_source('\n% !TeX program = pdflatex\nChild body.\n',
                          lambda: self.source_is(MAGIC_CHILD))
        self.key(35, 'cmd+alt')
        self.key(20)
        self.wait('The child remains the explicitly pinned TeX preview entry', lambda d:
                  d['preview_path'] == str(paths['child'])
                  and d['preview_tab_id'] == d['active_tab_id'] and d['view_mode'] == 'Split')
        self.tex_magic_compile(after=first, engine='xelatex', entry=paths['main'], source=MAGIC_MAIN,
                               active=paths['child'], preview=paths['child'], settings=settings)

        changed = len(tex_magic_invocations(self.directory))
        self.open_harper_fixture(paths['main'])
        self.key(0)
        self.paste_source(MAGIC_MAIN_EDITED)
        self.wait('A root compiler edit stays unsaved while the child keeps its preview pin', lambda d:
                  d['dirty'] and d['preview_path'] == str(paths['child'])
                  and d['active_tab_id'] != d['preview_tab_id'])
        self.tex_magic_compile(after=changed, engine='pdflatex', entry=paths['main'], source=MAGIC_MAIN_EDITED,
                               active=paths['main'], preview=paths['child'], settings=settings)
        if paths['main'].read_text(encoding='utf-8') != MAGIC_MAIN:
            raise AssertionError('Building an unsaved magic root changed its original disk source')

        redirected = len(tex_magic_invocations(self.directory))
        self.open_harper_fixture(paths['child'])
        self.key(0)
        self.paste_source(MAGIC_CHILD_ALTERNATE)
        self.tex_magic_compile(after=redirected, engine='xelatex', entry=paths['alternate'],
                               source=MAGIC_ALTERNATE, active=paths['child'],
                               preview=paths['child'], settings=settings)
        if paths['child'].read_text(encoding='utf-8') != MAGIC_CHILD:
            raise AssertionError('A magic root edit was saved despite disabled autosave')
        self.record('tex.magic.root_and_compiler', final_root_overrides_child_program=True,
                    unsaved_root_overlay=True, dynamic_root_redirect=True,
                    pinned_preview_entry=True, global_settings_unchanged=True,
                    real_tex_engine=False)

        self.open_harper_fixture(paths['snippets'])
        for prefix in ('% texroot', '% !TeX ro'):
            self.key(0)
            self.paste_source(prefix)
            self.accept_snippet('texroot')
            self.source_is('% !TeX root = main.tex')
            self.key(6)
            self.source_is(prefix)
            self.key(6, 'cmd+shift')
            self.source_is('% !TeX root = main.tex')
        self.tex_magic_compile(after=redirected, engine='xelatex', entry=paths['alternate'],
                               source=MAGIC_ALTERNATE, active=paths['snippets'],
                               preview=paths['child'], settings=settings)
        self.open_harper_fixture(paths['typst'])
        self.key(125, 'cmd')
        self.key(49, 'ctrl')
        excluded = lambda d: tex_magic_typst_excluded(d, paths['typst'])
        self.wait('TeX magic snippets are excluded from an actual Typst source', excluded, timeout=20)
        self.stable_state(excluded)
        self.key(53, 'none')
        self.source_is('texroot')
        if tex_magic_settings(self.snapshot()['document']) != settings:
            raise AssertionError('Snippet insertion changed global TeX build settings')
        self.record('tex.magic.snippets', bare_prefix=True, comment_prefix=True,
                    header_namespace=True, editable_root_placeholder=True,
                    undo_redo=True, typst_excluded=True,
                    unrelated_active_file_did_not_hijack_pinned_preview=True)
