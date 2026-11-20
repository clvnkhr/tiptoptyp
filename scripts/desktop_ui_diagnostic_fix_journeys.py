"""Opt-in native diagnostic repairs using the real local Liquid foundation model.

The prepared reference cache is read only. Its current-architecture files are
copied into the disposable journey directory; the app still verifies, loads and
runs the actual model. No fixed suggestion or diagnostic is injected into it.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil

DIAGNOSTIC_FIX_JOURNEYS = ('diagnostic_fix',)
DIAGNOSTIC_FIX_SETUP_JOURNEYS = ('diagnostic_fix_setup',)
DIAGNOSTIC_FIX_MODEL_ID = 'lfm3-1.7b-q4-k-m'
DIAGNOSTIC_FIX_MENU = 'menu.Suggest fix'
DIAGNOSTIC_FIX_DIFF_STYLES = ('Unified', 'SideBySide')
MAX_CACHE_FILES = 128
MAX_CACHE_BYTES = 3 * 1024**3


def diagnostic_fix_setup_defaults(document):
    """A fresh profile must use exactly the requested provider defaults."""
    ltex = document.get('ltex')
    if (type(document.get('harper_enabled')) is not bool or not isinstance(ltex, dict)
            or type(ltex.get('enabled')) is not bool):
        raise AssertionError('Grammar provider defaults inspection is unavailable')
    return document['harper_enabled'] is False and ltex['enabled'] is True


def assert_diagnostic_fix_setup_busy(snapshot):
    state = snapshot['document'].get('resource_repair')
    if (not isinstance(state, dict) or state.get('busy') is not True
            or state.get('status') != 'Downloading Liquid fix model…'):
        raise AssertionError('Liquid fix download never entered its working state')
    for name, enabled in [('settings.diagnostic_fix.download', False),
                          ('settings.diagnostic_fix.cancel_download', True)]:
        target = snapshot.get('targets', {}).get(name)
        if (not isinstance(target, dict) or target.get('enabled') is not enabled
                or type(target.get('age_ms')) not in (int, float)
                or not 0 <= target['age_ms'] <= 500):
            raise AssertionError('Liquid fix controls are missing, stale or wrongly enabled: ' + name)
    if snapshot.get('targets', {}).get('settings.liquid.cancel_download', {}).get('age_ms', 501) <= 500:
        raise AssertionError('Foundation download leaked into selection grammar controls')


def diagnostic_fix_setup_cancelled(document):
    state = document.get('resource_repair')
    if not isinstance(state, dict) or type(state.get('busy')) is not bool:
        raise AssertionError('Liquid fix resource inspection is unavailable')
    if state['busy']:
        return False
    status = state.get('status')
    if not isinstance(status, str) or 'cancel' not in status.lower():
        raise AssertionError('Liquid fix download did not stop through cancellation: ' + str(status))
    return True


def diagnostic_fix_native_target():
    return ('aarch64-apple-darwin' if platform.machine() in ('arm64', 'aarch64')
            else 'x86_64-apple-darwin')


def _regular(path, limit):
    if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= limit:
        raise ValueError('Diagnostic-fix cache has a missing, linked or oversized file')
    return path


def diagnostic_fix_catalog():
    catalog_path = Path(__file__).resolve().parent.parent / 'resources/diagnostic-fix/manifest.json'
    data = _regular(catalog_path, 65536).read_bytes()
    catalog = json.loads(data)
    if catalog.get('format') != 1 or catalog.get('model_id') != DIAGNOSTIC_FIX_MODEL_ID:
        raise ValueError('Diagnostic-fix compiled catalog has an unsupported model')
    catalog['catalog_sha256'] = hashlib.sha256(data).hexdigest()
    return catalog


def diagnostic_fix_model_files(root):
    """Cheap availability check; only the application certifies pinned hashes."""
    root = Path(root)
    if not root.is_absolute() or root.is_symlink() or not root.is_dir():
        raise ValueError('--diagnostic-fix-model-dir must be an existing absolute cache folder')
    base = root / 'foundation' / DIAGNOSTIC_FIX_MODEL_ID
    runtime = base / 'runtime' / diagnostic_fix_native_target()
    for directory in (root / 'foundation', base, base / 'runtime', runtime):
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError('Diagnostic-fix model cache is not prepared for this Mac')
    files = [Path('model.gguf'), Path('LICENSE-model.txt'), Path('LICENSE-runtime.txt')]
    _regular(base / files[0], 2 * 1024**3)
    catalog = diagnostic_fix_catalog()
    if (base / files[0]).stat().st_size != catalog['model']['bytes']:
        raise ValueError('Diagnostic-fix model size differs from its compiled catalog')
    for relative in files[1:]:
        _regular(base / relative, 16 * 1024**2)
    candidates = [entry for entry in catalog['runtimes'] if entry['target'] == diagnostic_fix_native_target()]
    if len(candidates) != 1:
        raise ValueError('Diagnostic-fix compiled catalog has no unique native runtime')
    expected = {entry['name']: entry['bytes'] for entry in candidates[0]['files']}
    if (not expected or 'llama-server' not in expected
            or any(Path(name).name != name or name in ('.', '..') for name in expected)):
        raise ValueError('Diagnostic-fix compiled catalog has invalid runtime files')
    for index, path in enumerate(runtime.iterdir()):
        if index + 3 >= MAX_CACHE_FILES:
            raise ValueError('Diagnostic-fix runtime has too many entries')
        _regular(path, 256 * 1024**2)
        if expected.pop(path.name, None) != path.stat().st_size:
            raise ValueError('Diagnostic-fix runtime files differ from the compiled catalog')
        files.append(path.relative_to(base))
    if expected:
        raise ValueError('Diagnostic-fix native runtime is incomplete')
    server = runtime / 'llama-server'
    if not server.is_file() or not os.access(server, os.X_OK):
        raise ValueError('Diagnostic-fix native model server is unavailable')
    if sum((base / relative).stat().st_size for relative in files) > MAX_CACHE_BYTES:
        raise ValueError('Diagnostic-fix cache is too large')
    return base, sorted(files)


def diagnostic_fix_model_directory(value):
    if value is None:
        raise ValueError('Diagnostic-fix journey requires --diagnostic-fix-model-dir with a prepared model cache')
    root = Path(value)
    diagnostic_fix_model_files(root)
    return root.resolve()


def copy_diagnostic_fix_model(reference, destination):
    """Copy regular files rather than writable aliases into the private cache."""
    base, files = diagnostic_fix_model_files(reference)
    destination = Path(destination)
    if destination.exists():
        raise AssertionError('Diagnostic-fix private cache must initially be absent')
    copied = destination / 'foundation' / DIAGNOSTIC_FIX_MODEL_ID
    metadata = []
    for relative in files:
        source = base / relative
        before = source.stat()
        target = copied / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        after = source.stat()
        if ((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
                or target.stat().st_size != before.st_size
                or (target.stat().st_dev, target.stat().st_ino)
                == (source.stat().st_dev, source.stat().st_ino)):
            raise AssertionError('Diagnostic-fix reference changed or was aliased while copying')
        metadata.append({'name': str(relative), 'bytes': before.st_size})
    catalog = diagnostic_fix_catalog()
    return {'model_id': DIAGNOSTIC_FIX_MODEL_ID, 'target': diagnostic_fix_native_target(),
            'model_revision': catalog['model_revision'], 'runtime_version': catalog['runtime_version'],
            'declared_model_sha256': catalog['model']['sha256'],
            'catalog_sha256': catalog['catalog_sha256'],
            'files': metadata, 'reference_read_only': True, 'private_regular_copies': True}


def diagnostic_fix_state(document):
    state = document.get('diagnostic_fix')
    if (not isinstance(state, dict) or any(type(state.get(key)) is not bool
            for key in ('visible', 'busy', 'current'))
            or 'error' not in state or state['error'] is not None
            and not isinstance(state['error'], str)):
        raise AssertionError('Diagnostic-fix review inspection is missing or malformed')
    return state


def diagnostic_fix_ready(document):
    state = diagnostic_fix_state(document)
    if state['visible'] and not state['busy'] and state['error'] is not None:
        raise AssertionError('Diagnostic suggestion failed: ' + state['error'])
    comparison = state.get('comparison')
    if comparison is not None and (not isinstance(comparison, dict)
            or any(not isinstance(comparison.get(key), str) for key in ('before', 'after'))
            or any(type(comparison.get(key)) is not int or comparison[key] < 0
                   for key in ('before_chars', 'after_chars'))
            or comparison.get('style') not in DIAGNOSTIC_FIX_DIFF_STYLES):
        raise AssertionError('Diagnostic-fix diff inspection is missing or malformed')
    return (state['visible'] and not state['busy'] and state['current']
            and state['error'] is None and comparison is not None
            and comparison['before'] != comparison['after'])


def diagnostic_fix_source(document, source, owner):
    from desktop_ui_journeys import fingerprint
    return (document['path'] == owner['path'] and document['tabs'] == owner['tabs']
            and document['active_tab_id'] == owner['active_tab_id']
            and document['source_fingerprint'] == fingerprint(source)
            and document['source_bytes'] == len(source.encode('utf-8')))


def diagnostic_fix_preview_owner(document, path):
    return (document.get('path') == str(path)
            and document.get('preview_path') == str(path)
            and document.get('active_tab_id') is not None
            and document.get('preview_tab_id') == document['active_tab_id'])


def diagnostic_fix_comparison(document, fixture, style):
    from desktop_ui_journeys import fingerprint
    if not diagnostic_fix_ready(document):
        return False
    state = diagnostic_fix_state(document)
    comparison = state['comparison']
    return (state.get('line') == fixture['line']
            and state.get('diagnostic_fingerprint') == fingerprint(fixture['diagnostic'])
            and comparison['before'] == fingerprint(fixture['original_line'])
            and comparison['after'] == fingerprint(fixture['replacement'])
            and comparison['before_chars'] == len(fixture['original_line'])
            and comparison['after_chars'] == len(fixture['replacement'])
            and comparison['style'] == style)


def diagnostic_fix_fixture(name='typst-local-name'):
    catalog = Path(__file__).with_name('fixtures') / 'diagnostic-fixes.json'
    data = json.loads(catalog.read_text(encoding='utf-8'))
    if data.get('format') != 1:
        raise ValueError('Diagnostic-fix evaluation fixtures have an unsupported format')
    found = [entry for entry in data['cases'] if entry['id'] == name]
    if len(found) != 1:
        raise ValueError('Diagnostic-fix fixture is missing or duplicated')
    fixture = dict(found[0])
    lines = fixture['source'].splitlines(keepends=True)
    current = lines[fixture['line'] - 1]
    fixture['original_line'] = current.rstrip('\r\n')
    ending = current[len(fixture['original_line']):]
    lines[fixture['line'] - 1] = fixture['replacement'] + ending
    marker = '// native undo sentinel' if fixture['language'] == 'typst' else '% native undo sentinel'
    fixture['working'] = fixture['source'] + marker
    fixture['corrected'] = ''.join(lines) + marker
    return fixture


def diagnostic_fix_row(document, fixture):
    from desktop_ui_journeys import fingerprint
    rows = document.get('diagnostic_rows')
    if not isinstance(rows, list):
        raise AssertionError('Diagnostic rows inspection is unavailable')
    if any(not isinstance(row, dict) or type(row.get('index')) is not int or row['index'] < 0
           or 'line' not in row or row['line'] is not None
           and (type(row['line']) is not int or row['line'] < 1)
           or not isinstance(row.get('message_fingerprint'), str)
           or type(row.get('suppressed')) is not bool for row in rows):
        raise AssertionError('Diagnostic row identity inspection is unavailable')
    matches = [row for row in rows if row.get('line') == fixture['line']
               and row.get('message_fingerprint') == fingerprint(fixture['diagnostic'])
               and row.get('suppressed') is False]
    # Compiler and LSP providers can publish equivalent findings. Selecting
    # the first exact line/message is safe even when both remain visible.
    return min(row['index'] for row in matches) if matches else None


class DiagnosticFixJourneys:
    def diagnostic_fix_setup_settings_fingerprint(self):
        self.settings()
        if not self.snapshot()['document']['settings_json']['visible']:
            self.click('settings.json.mode')
        document = self.wait('Complete Settings JSON validates for setup comparison', lambda d:
                             d['settings_json']['visible'] and d['settings_json']['valid'])['document']
        value = document['settings_json']['fingerprint']
        if not isinstance(value, str) or len(value) != 16:
            raise AssertionError('Complete preferences identity is unavailable')
        self.close_settings()
        return value

    def diagnostic_fix_setup(self):
        from desktop_ui_comment_controls_journeys import DEFAULT_COMMENT_MARKERS
        from desktop_ui_journeys import demo_owner_unchanged

        original = self.snapshot()['document']
        if not diagnostic_fix_setup_defaults(original):
            raise AssertionError('Fresh settings must default to Harper off and LTeX+ on')
        original_preferences = self.diagnostic_fix_setup_settings_fingerprint()
        self.comment_marker_settings(DEFAULT_COMMENT_MARKERS, verify_only=True)
        self.record('diagnostic_fix_setup.discoverability', comment_controls_search=True,
                    six_default_markers_read=True, harper_default=False, ltex_default=True)

        cache = self.directory / 'diagnostic-fix-setup-cache'
        if cache.exists():
            raise AssertionError('Liquid setup cache must initially be absent')
        cache_setting = '"liquid_cache_directory": ' + json.dumps(str(cache), ensure_ascii=False)
        self.tex_settings([('"liquid_cache_directory": ""', cache_setting)])
        preferences = self.diagnostic_fix_setup_settings_fingerprint()
        before = self.snapshot()['document']
        self.settings()
        self.setting_section('fix model', 'Liquid fix suggestions')
        self.wait_target('settings.diagnostic_fix.download')
        self.click('settings.diagnostic_fix.download')
        self.wait('The real Liquid fix download starts on the background worker', lambda d:
                  d['resource_repair']['busy'])
        self.wait_target('settings.diagnostic_fix.cancel_download')
        assert_diagnostic_fix_setup_busy(self.snapshot())
        self.click('settings.diagnostic_fix.cancel_download')
        self.wait('Cancel stops the real Liquid fix download', diagnostic_fix_setup_cancelled, timeout=30)
        self.wait_target('settings.diagnostic_fix.download')
        after = self.snapshot()['document']
        if not demo_owner_unchanged(after, before) or not diagnostic_fix_setup_defaults(after):
            raise AssertionError('Liquid setup changed source ownership or grammar activation')
        model = cache / 'foundation' / DIAGNOSTIC_FIX_MODEL_ID / 'model.gguf'
        if model.exists():
            raise AssertionError('Cancelled setup unexpectedly published the complete model')
        self.close_settings()
        if self.diagnostic_fix_setup_settings_fingerprint() != preferences:
            raise AssertionError('Liquid download/cancel changed complete applied preferences')
        self.record('diagnostic_fix_setup.cancelled', actual_worker_started=True,
                    cancel_stops_download=True, incomplete_model_not_published=True,
                    settings_unchanged=True, source_owner_unchanged=True)
        self.tex_settings([(cache_setting, '"liquid_cache_directory": ""')])
        if self.diagnostic_fix_setup_settings_fingerprint() != original_preferences:
            raise AssertionError('Liquid setup journey did not restore complete original preferences')
        self.wait('Liquid setup restores the fresh provider defaults', diagnostic_fix_setup_defaults)
        final = self.snapshot()['document']
        fields = ('tabs', 'active_tab_id', 'preview_tab_id', 'path', 'source_bytes',
                  'source_fingerprint', 'dirty', 'workspace_root_fingerprint')
        if any(final.get(field) != original.get(field) for field in fields):
            raise AssertionError('Liquid setup journey changed the original source/workspace')
        self.record('diagnostic_fix_setup.complete', original_preferences_restored=True,
                    original_source_preserved=True, no_model_download_completed=True)

    def use_diagnostic_fix_preview(self, path):
        if not diagnostic_fix_preview_owner(self.snapshot()['document'], path):
            self.key(35, 'cmd+alt')  # Use the actual error fixture for preview/compilation.
        return self.wait('diagnostic fixture owns the actual compiler preview', lambda d:
                         diagnostic_fix_preview_owner(d, path), timeout=15)['document']

    def request_diagnostic_fix(self, fixture, owner, style, entry):
        row = self.wait('live compiler publishes the exact diagnostic line and message', lambda d:
                        diagnostic_fix_source(d, fixture['working'], owner)
                        and diagnostic_fix_row(d, fixture) is not None, timeout=45)['document']
        if entry == 'Problems':
            target = 'problem.' + str(diagnostic_fix_row(row, fixture))
            self.wait_target(target)
            self.right_click(target)
        elif entry == 'editor':
            self.click('editor.source')
            self.key(126, 'cmd')
            for _ in range(fixture['line'] - 1):
                self.key(125, 'none')
            start = sum(map(len, fixture['working'].splitlines(keepends=True)[:fixture['line'] - 1]))
            self.wait('source caret reaches the actual diagnostic line', lambda d:
                      d['editor_keyboard_focused'] and d['cursor'] == [start, start])
            self.wait_target('editor.caret')
            self.right_click('editor.caret')
        else:
            raise ValueError('Diagnostic-fix menu entry must be Problems or editor')
        self.wait_target(DIAGNOSTIC_FIX_MENU)
        self.click(DIAGNOSTIC_FIX_MENU)
        self.wait('real Liquid inference produces a review without changing the source', lambda d:
                  diagnostic_fix_source(d, fixture['working'], owner)
                  and diagnostic_fix_comparison(d, fixture, style), timeout=180)
        self.wait_target('diagnostic_fix.diff')
        self.wait_target('diagnostic_fix.accept')
        self.wait_target('diagnostic_fix.cancel')

    def diagnostic_fix(self):
        reference = diagnostic_fix_model_directory(getattr(self, 'diagnostic_fix_model_dir', None))
        cache = self.directory / 'diagnostic-fix-cache'
        metadata = copy_diagnostic_fix_model(reference, cache)
        cache_setting = '"liquid_cache_directory": ' + json.dumps(str(cache), ensure_ascii=False)
        self.tex_settings([('"auto_save": true', '"auto_save": false'),
                           ('"liquid_cache_directory": ""', cache_setting)])
        original_tabs = self.snapshot()['document']['tabs']
        fixture = diagnostic_fix_fixture()
        path = self.directory / 'diagnostic-fix.typ'
        path.write_text(fixture['source'], encoding='utf-8')
        self.open_native_file(path)
        self.key(125, 'cmd')
        self.native('text', '// native undo sentinel')
        self.source_is(fixture['working'])
        owner = self.use_diagnostic_fix_preview(path)
        same = lambda d: diagnostic_fix_source(d, fixture['working'], owner)
        if owner['panel'] != 'Problems':
            self.key(23)
        self.wait('Problems shows the real compiler diagnostic', lambda d:
                  d['panel'] == 'Problems' and diagnostic_fix_row(d, fixture) is not None, timeout=45)
        current_style = 'Unified'
        for style in DIAGNOSTIC_FIX_DIFF_STYLES:
            entry = 'Problems' if style == 'Unified' else 'editor'
            if style != current_style:
                self.tex_settings([(json.dumps('git_diff_style') + ': ' + json.dumps(current_style),
                                    json.dumps('git_diff_style') + ': ' + json.dumps(style))])
                current_style = style
            self.request_diagnostic_fix(fixture, owner, style, entry)
            self.stable_state(lambda d: same(d) and diagnostic_fix_comparison(d, fixture, style))
            self.key(6)  # Cmd+Z belongs to the open review.
            self.stable_state(lambda d: same(d) and diagnostic_fix_comparison(d, fixture, style))
            self.click('diagnostic_fix.cancel')
            self.wait('Cancel closes only the suggestion and preserves the complete source', lambda d:
                      same(d) and not diagnostic_fix_state(d)['visible'])

            self.request_diagnostic_fix(fixture, owner, style, entry)
            self.stable_state(lambda d: same(d) and diagnostic_fix_comparison(d, fixture, style))
            self.click('diagnostic_fix.accept')
            self.wait('Accept applies only the reviewed line', lambda d:
                      diagnostic_fix_source(d, fixture['corrected'], owner)
                      and not diagnostic_fix_state(d)['visible'])
            self.key(6)
            self.wait('one Undo restores the diagnostic source including the earlier edit', same)
            self.stable_state(same)
            if path.read_text(encoding='utf-8') != fixture['source']:
                raise AssertionError('Diagnostic-fix review unexpectedly saved its disposable source')
            self.record('diagnostic.fix', style=style, entry=entry, actual_model_inference=True,
                        line=fixture['line'], cancelled_source_unchanged=True,
                        outside_line_unchanged=True, source_unchanged_before_accept=True,
                        single_undo=True, modal_shortcuts_owned=True, **metadata)
        self.close_scratch(original_tabs)
        self.tex_settings([(cache_setting, '"liquid_cache_directory": ""'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"git_diff_style": "SideBySide"', '"git_diff_style": "Unified"')])
