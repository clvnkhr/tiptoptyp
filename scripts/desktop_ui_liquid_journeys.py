"""Opt-in native Liquid review, including installation into a fresh private cache.

Only keyboard, pointer, and the real Settings JSON editor perform app actions.
The first request downloads and prepares the optional native model without Python
or an installation button. A reference cache supplies artifact identity.
"""
import hashlib
import json
import math
import platform
from pathlib import Path
import time

LIQUID_JOURNEYS = ('liquid_selection', 'liquid_streaming')
LIQUID_MENU = 'menu.Check selection with Liquid'
LIQUID_DIFF_STYLES = ('Unified', 'SideBySide')
LIQUID_DEFAULT_INPUT = {'protected_content': 'placeholder', 'placeholder': 'x',
                        'formatted_text': 'include'}
LIQUID_NONDEFAULT_INPUT = {'protected_content': 'split', 'placeholder': 'MATH',
                           'formatted_text': 'skip'}

LIQUID_MODEL_ID = 'lfm25-encoder-350m-onnxruntime-v1'
LIQUID_REVISION = '4e2f724c10985e9ec90f34189c22be5d5f2e41e0'

LIQUID_NATIVE_FILES = {
    'aarch64-apple-darwin': ('liquid-onnx', 'libonnxruntime.dylib'),
    'x86_64-apple-darwin': ('liquid-onnx', 'libonnxruntime.dylib'),
}

def liquid_native_target():
    return 'aarch64-apple-darwin' if platform.machine() in ('arm64', 'aarch64') else 'x86_64-apple-darwin'

def liquid_asset_path(entry):
    target = entry.get('target')
    return Path('runtime') / target / entry['name'] if target is not None else Path(entry['name'])

def liquid_selected_assets(manifest):
    return [entry for entry in manifest['files'] if entry.get('target') in (None, liquid_native_target())]

def liquid_model_directory(value):
    """Check native model availability without preparing or hashing large files."""
    if value is None:
        raise ValueError('Liquid journeys require --liquid-model-dir with a prepared native Liquid model folder')
    root = Path(value)
    if not root.is_absolute() or not root.is_dir():
        raise ValueError('--liquid-model-dir must be an existing absolute model folder')
    installed = root / 'onnxruntime' / LIQUID_MODEL_ID
    manifest_file = installed / 'manifest.json'
    try:
        if manifest_file.is_symlink() or not 0 < manifest_file.stat().st_size <= 65536:
            raise ValueError('invalid native model manifest')
        with manifest_file.open('rb') as handle:
            data = handle.read(65537)
        if len(data) > 65536:
            raise ValueError('native model manifest is too large')
        manifest = json.loads(data)
        if (not isinstance(manifest, dict) or type(manifest.get('format')) is not int
                or manifest.get('format') != 3
                or manifest.get('model_id') != LIQUID_MODEL_ID
                or manifest.get('upstream_revision') != LIQUID_REVISION):
            raise ValueError('native model version does not match')
        files = manifest.get('files')
        expected = {(None, 'liquid.onnx'), (None, 'tokenizer.json')}
        expected.update((target, name) for target, names in LIQUID_NATIVE_FILES.items() for name in names)
        actual = [(entry.get('target'), entry.get('name')) for entry in files if isinstance(entry, dict)] if isinstance(files, list) else []
        if len(actual) != len(expected) or set(actual) != expected:
            raise ValueError('native model files do not match')
        for entry in liquid_selected_assets(manifest):
            relative = liquid_asset_path(entry)
            path = installed / relative
            size = entry.get('bytes')
            if (type(size) is not int or not 0 < size <= 2 * 1024**3
                    or path.is_symlink() or not path.is_file() or path.stat().st_size != size
                    or any((installed / parent).is_symlink() for parent in relative.parents)):
                raise ValueError('native model file is unavailable')
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ValueError('--liquid-model-dir is not prepared for the current native model; run a Liquid selection check once before the journey') from error
    # The app verifies all pinned asset hashes before loading. This cheap check
    # never claims to certify model integrity and does not require Python packages.
    return root.resolve()

def liquid_model_metadata(root):
    """Small manifest evidence; the application verifies the large asset hashes."""
    root = liquid_model_directory(root)
    installed = root / 'onnxruntime' / LIQUID_MODEL_ID
    data = (installed / 'manifest.json').read_bytes()
    manifest = json.loads(data)
    return {
        'model_id': manifest['model_id'],
        'upstream_revision': manifest['upstream_revision'],
        'manifest_sha256': hashlib.sha256(data).hexdigest(),
        'files': sorted(({'name': str(liquid_asset_path(entry)), 'bytes': entry['bytes'],
                          'declared_sha256': entry['sha256']} for entry in liquid_selected_assets(manifest)),
                        key=lambda entry: entry['name']),
    }


def liquid_preparation_evidence(installed_root, reference_root):
    installed = liquid_model_metadata(installed_root)
    reference = liquid_model_metadata(reference_root)
    for key in ('model_id', 'upstream_revision', 'files'):
        if installed[key] != reference[key]:
            raise AssertionError('Liquid first-use installation does not match the reference model assets')
    if (Path(installed_root) / 'runtime').exists():
        raise AssertionError('Liquid first-use setup unexpectedly created a Python runtime')
    return installed


def liquid_state(document):
    state = document.get('liquid')
    if not isinstance(state, dict) or any(type(state.get(key)) is not bool
            for key in ('visible', 'busy', 'current')):
        raise AssertionError('Liquid modal inspection is missing or malformed')
    changes, error = state.get('changes'), state.get('error')
    if ('changes' not in state or 'error' not in state
            or changes is not None and (type(changes) is not int or changes < 0)
            or error is not None and not isinstance(error, str)):
        raise AssertionError('Liquid result inspection is missing or malformed')
    return state

def liquid_progress(document):
    """Validate model-measured progress; text length is not a token count."""
    progress = liquid_state(document).get('progress')
    if progress is None:
        return None
    keys = {'passage', 'passages', 'chunk', 'chunks', 'model_tokens'}
    if (not isinstance(progress, dict) or set(progress) != keys
            or any(type(progress.get(key)) is not int or progress[key] < 1 for key in keys)
            or progress['passage'] > progress['passages']
            or progress['chunk'] > progress['chunks'] or progress['model_tokens'] > 128):
        raise AssertionError('Liquid chunk progress inspection is missing or malformed')
    return progress


def liquid_ready(document):
    state = liquid_state(document)
    if state['visible'] and not state['busy'] and state['error'] is not None:
        raise AssertionError('Liquid selection check failed: ' + state['error'])
    return (state['visible'] and not state['busy']
            and state['current'] and state['error'] is None
            and type(state['changes']) is int and state['changes'] > 0)


def liquid_input_options(document, expected):
    """Read effective app settings rather than trusting the JSON editor buffer."""
    options = liquid_state(document).get('input_options')
    if (not isinstance(options, dict)
            or set(options) != {'protected_content', 'placeholder', 'formatted_text'}
            or options.get('protected_content') not in ('placeholder', 'split')
            or not isinstance(options.get('placeholder'), str)
            or options.get('formatted_text') not in ('include', 'skip')):
        raise AssertionError('Liquid input settings inspection is missing or malformed')
    return options == expected

def liquid_input_edits(before, after):
    """Unique scalar settings entries work with the real single-line JSON find."""
    return [(json.dumps(key) + ': ' + json.dumps(before[key]),
             json.dumps(key) + ': ' + json.dumps(after[key])) for key in LIQUID_DEFAULT_INPUT]


def liquid_sent(document, fixture, *, opened=None):
    """Verify the actual submitted fragments without writing private text to logs."""
    from desktop_ui_journeys import fingerprint
    state = liquid_state(document)
    sent = state.get('sent')
    if sent is None:
        return False
    if (not isinstance(sent, list) or len(sent) > 16_384
            or type(state.get('sent_open')) is not bool
            or any(not isinstance(item, dict) or set(item) != {'fingerprint', 'chars'}
                   or not isinstance(item.get('fingerprint'), str)
                   or type(item.get('chars')) is not int or item['chars'] < 0 for item in sent)):
        raise AssertionError('Liquid sent-input inspection is missing or malformed')
    expected = [{'fingerprint': fingerprint(text), 'chars': len(text)}
                for text in fixture['sent_fragments']]
    return sent == expected and (opened is None or state['sent_open'] is opened)

def liquid_comparison(document, fixture, style):
    """Prove the shared diff receives the full selected source, not edit snippets."""
    from desktop_ui_journeys import fingerprint
    if style not in LIQUID_DIFF_STYLES:
        raise ValueError('Liquid comparison style must be Unified or SideBySide')
    if not liquid_ready(document):
        return False
    return liquid_comparison_contents(document, fixture, style)

def liquid_comparison_contents(document, fixture, style):
    from desktop_ui_journeys import fingerprint
    comparison = liquid_state(document).get('comparison')
    if (not isinstance(comparison, dict)
            or any(not isinstance(comparison.get(key), str) for key in ('before', 'after'))
            or any(type(comparison.get(key)) is not int or comparison[key] < 0
                   for key in ('before_chars', 'after_chars'))
            or comparison.get('style') not in LIQUID_DIFF_STYLES):
        raise AssertionError('Liquid full-selection comparison inspection is missing or malformed')
    return (comparison['before'] == fingerprint(fixture['selected_before'])
            and comparison['after'] == fingerprint(fixture['selected_after'])
            and comparison['before_chars'] == len(fixture['selected_before'])
            and comparison['after_chars'] == len(fixture['selected_after'])
            and comparison['style'] == style)

def liquid_partial(snapshot, fixture, style):
    """A real unfinished review must render edits while Apply stays disabled."""
    from desktop_ui_journeys import fingerprint
    document = snapshot['document']
    state = liquid_state(document)
    progress = liquid_progress(document)
    if not (state['visible'] and state['busy'] and state['current']
            and state['error'] is None and type(state['changes']) is int and state['changes'] > 0
            and progress is not None and progress['passages'] == 1
            and progress['chunks'] > 1 and progress['chunk'] < progress['chunks']):
        return False
    if not liquid_comparison_contents(document, fixture, style):
        return False
    targets = snapshot.get('targets', {})
    controls = [targets.get(name) for name in ('liquid.apply', 'liquid.cancel', 'liquid.diff')]
    if any(not isinstance(target, dict) for target in controls):
        return False
    apply, cancel, diff = controls
    if apply.get('enabled') is not False or cancel.get('enabled') is not True or diff.get('enabled') is not True:
        return False
    # Child and document viewports have independent frame counters. Prove the
    # rendered diff and footer belong to one fresh child pass, not ROOT's frame.
    for target in controls:
        if (type(target.get('age_ms')) not in (int, float) or not 0 <= target['age_ms'] <= 500
                or type(target.get('frame')) is not int or target['frame'] < 0
                or not isinstance(target.get('viewport'), str) or not target['viewport']
                or any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
                       for key in ('x', 'y', 'width', 'height'))
                or target['width'] <= 0 or target['height'] <= 0):
            return False
    return (all(target['frame'] == apply['frame'] and target['viewport'] == apply['viewport']
                for target in controls)
            and document.get('source_fingerprint') == fingerprint(fixture['working'])
            and document.get('source_bytes') == len(fixture['working'].encode('utf-8')))


def liquid_source(document, source, path, tabs, tab):
    from desktop_ui_journeys import fingerprint
    return (document['path'] == str(path) and document['tabs'] == tabs
            and document['active_tab_id'] == tab
            and document['source_fingerprint'] == fingerprint(source)
            and document['source_bytes'] == len(source.encode('utf-8')))

def liquid_selection_point(state, start, end):
    """A right-click must land inside the fresh, keyboard-created selection."""
    document = state['document']
    if sorted(document['cursor']) != [start, end] or not document['editor_keyboard_focused']:
        raise AssertionError('Liquid menu needs the exact focused selection')
    targets = [state['targets'].get(name) for name in ('editor.ime_cursor', 'editor.source')]
    if any(not target or not target.get('enabled') or target.get('age_ms', 501) > 500
           or target.get('frame') != document['frame'] for target in targets):
        raise AssertionError('Liquid selection hit geometry is missing or stale')
    caret, editor = targets
    if caret.get('viewport') != editor.get('viewport'):
        raise AssertionError('Liquid selection belongs to another viewport')
    if any(type(target.get(key)) not in (int, float) or not math.isfinite(target[key])
           for target, keys in ((caret, ('x', 'y')), (editor, ('x', 'y', 'width', 'height')))
            for key in keys) or editor['width'] <= 0 or editor['height'] <= 0:
        raise AssertionError('Liquid selection hit geometry is invalid')
    left, right = editor['x'] - editor['width'] / 2, editor['x'] + editor['width'] / 2
    top, bottom = editor['y'] - editor['height'] / 2, editor['y'] + editor['height'] / 2
    # The primary cursor is at line end. Move a few logical pixels into the
    # selected closing delimiter so a secondary click cannot collapse it.
    x, y = caret['x'] - 5, caret['y']
    if not left + 1 <= x <= right - 1 or not top <= y <= bottom:
        raise AssertionError('Liquid selection point is outside the visible editor')
    return x, y

def liquid_fixture(kind, *, streaming=False):
    outside = 'He go to school.\n'
    math = '$u$' if kind == 'typst' else r'\(u\)'
    prose = ('She go to school every day. ' + 'This is fine. ' * 180 + 'We estimate ' + math + '.'
             if streaming else 'This is fine. She go to school every day.')
    if kind == 'typst':
        base = outside + '#strong[' + prose + ']\nUnchanged afterward.\n'
        marker = '// native undo sentinel'
        line = 1
    elif kind == 'tex':
        base = ('\\documentclass{article}\n\\begin{document}\n' + outside
                + '\\textbf{' + prose + '}\nUnchanged afterward.\n\\end{document}\n')
        marker = '% native undo sentinel'
        line = 3
    else:
        raise ValueError('Liquid journey fixture must be typst or tex')
    working = base + marker
    lines = working.splitlines(keepends=True)
    start = sum(map(len, lines[:line]))
    end = start + len(lines[line].rstrip('\r\n'))
    selected_before = working[start:end]
    selected_after = selected_before.replace('She go ', 'She goes ')
    corrected = working[:start] + selected_after + working[end:]
    return dict(base=base, marker=marker, working=working, corrected=corrected,
                selected_before=selected_before, selected_after=selected_after,
                sent_fragments=[prose.replace(math, 'x')],
                line=line, start=start, end=end)


class LiquidJourneys:
    def wait_liquid_menu(self, enabled):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            state = self.snapshot()
            target = state['targets'].get(LIQUID_MENU)
            paste = state['targets'].get('menu.Paste')
            if (target and paste and target['age_ms'] <= 500 and paste['age_ms'] <= 500
                    and target['frame'] == paste['frame'] and paste['enabled']):
                if target['enabled'] is not enabled:
                    raise AssertionError('Liquid menu enabled state does not match the actual selection')
                return state
            time.sleep(.05)
        raise AssertionError('Liquid context-menu row did not become current')

    def select_liquid_fixture_line(self, fixture, style, *, first_use=False, await_partial=False):
        if first_use and self.liquid_preparation_cache.exists():
            raise AssertionError('First-use cache already exists before requesting Liquid')
        self.click('editor.source')
        self.key(126, 'cmd')
        for _ in range(fixture['line']):
            self.key(125, 'none')
        self.wait('keyboard reaches the logical prose-line start', lambda d:
                  d['cursor'] == [fixture['start'], fixture['start']]
                  and d['editor_keyboard_focused'])
        # Cmd+Right is a visual-row boundary under word wrap. Select through
        # document EOF instead, then trim only the known outside suffix.
        end = len(fixture['working'])
        self.key(125, 'cmd+shift')
        self.wait('keyboard selects from the prose line through document EOF', lambda d:
                  d['cursor'] == [end, fixture['start']]
                  and d['editor_keyboard_focused'])
        remaining = end - fixture['end']
        while remaining:
            count = min(remaining, 60)
            self.native('key-burst', 123, 'shift', count, 12)
            remaining -= count
        self.wait('keyboard selects the complete wrapped prose line', lambda d:
                          sorted(d['cursor']) == [fixture['start'], fixture['end']]
                          and d['editor_keyboard_focused'])
        self.wait_target('editor.ime_cursor')
        point = liquid_selection_point(self.snapshot(), fixture['start'], fixture['end'])
        self.right_click_at(*point)
        self.wait_liquid_menu(True)
        self.click(LIQUID_MENU)
        if await_partial:
            deadline = time.monotonic() + 180
            while time.monotonic() < deadline:
                snapshot = self.snapshot()
                if liquid_partial(snapshot, fixture, style):
                    self.record('liquid.streaming.partial', apply_enabled=False,
                                source_unchanged=True, **liquid_progress(snapshot['document']))
                    break
                if liquid_ready(snapshot['document']):
                    raise AssertionError('Liquid finished without an observed unfinished review with disabled Apply')
                time.sleep(.025)
            else:
                raise AssertionError('Liquid did not stream a partial diff before the remaining model chunks')
        self.wait('Liquid reviews the complete selection in the configured diff style',
                  lambda d: liquid_comparison(d, fixture, style) and liquid_sent(d, fixture),
                  timeout=900 if first_use else 180)
        if first_use:
            metadata = liquid_preparation_evidence(self.liquid_preparation_cache, self.liquid_reference_model)
            self.record('liquid.first-use.prepared', cache_initially_absent=True, automatic=True,
                        cache_scope='journey temporary directory', python_runtime_created=False,
                        **metadata)

    def liquid_selection(self):
        self.liquid_reference_model = liquid_model_directory(getattr(self, 'liquid_model_dir', None))
        self.liquid_preparation_cache = self.directory / 'liquid-first-use-cache'
        if self.liquid_preparation_cache.exists():
            raise AssertionError('Liquid first-use cache must be absent; no existing cache is deleted')
        cache_setting = '"liquid_cache_directory": ' + json.dumps(str(self.liquid_preparation_cache), ensure_ascii=False)
        self.tex_settings([('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false'),
                           ('"liquid_cache_directory": ""', cache_setting)]
                          + liquid_input_edits(LIQUID_DEFAULT_INPUT, LIQUID_NONDEFAULT_INPUT))
        self.wait('nondefault Liquid filtering options are effective after saving Settings JSON',
                  lambda d: liquid_input_options(d, LIQUID_NONDEFAULT_INPUT))
        self.tex_settings(liquid_input_edits(LIQUID_NONDEFAULT_INPUT, LIQUID_DEFAULT_INPUT))
        self.wait('restored Liquid defaults are effective before actual inference',
                  lambda d: liquid_input_options(d, LIQUID_DEFAULT_INPUT))
        self.record('liquid.input.settings.roundtrip', saved=LIQUID_NONDEFAULT_INPUT,
                    restored=LIQUID_DEFAULT_INPUT)
        original = self.snapshot()['document']
        original_tabs = original['tabs']
        current_style = 'Unified'  # The isolated journey preset uses app defaults.
        for style in LIQUID_DIFF_STYLES:
            if style != current_style:
                self.tex_settings([(json.dumps('git_diff_style') + ': ' + json.dumps(current_style),
                                    json.dumps('git_diff_style') + ': ' + json.dumps(style))])
                current_style = style
            for kind, suffix in (('typst', 'typ'), ('tex', 'tex')):
                self.liquid_selection_fixture(style, kind, suffix, original_tabs,
                                              first_use=style == LIQUID_DIFF_STYLES[0] and kind == 'typst')
        for kind, suffix in (('typst', 'typ'), ('tex', 'tex')):
            self.liquid_streaming_fixture(current_style, kind, suffix, original_tabs)
        self.tex_settings([(cache_setting, '"liquid_cache_directory": ""'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true'),
                           ('"git_diff_style": "SideBySide"', '"git_diff_style": "Unified"')])

    def liquid_streaming(self):
        """Focused long-input checks; the complete Liquid lane still runs these too."""
        self.liquid_reference_model = liquid_model_directory(getattr(self, 'liquid_model_dir', None))
        self.liquid_preparation_cache = self.directory / 'liquid-streaming-cache'
        if self.liquid_preparation_cache.exists():
            raise AssertionError('Streaming cache must be initially absent; no existing cache is deleted')
        cache_setting = '"liquid_cache_directory": ' + json.dumps(str(self.liquid_preparation_cache), ensure_ascii=False)
        self.tex_settings([('"auto_save": true', '"auto_save": false'),
                           ('"build_enabled": true', '"build_enabled": false'),
                           ('"liquid_cache_directory": ""', cache_setting),
                           ('"git_diff_style": "Unified"', '"git_diff_style": "SideBySide"')])
        self.wait('streaming checks use default source protection', lambda d:
                  liquid_input_options(d, LIQUID_DEFAULT_INPUT))
        tabs = self.snapshot()['document']['tabs']
        for kind, suffix in (('typst', 'typ'), ('tex', 'tex')):
            self.liquid_streaming_fixture('SideBySide', kind, suffix, tabs)
            if kind == 'typst':
                metadata = liquid_preparation_evidence(self.liquid_preparation_cache, self.liquid_reference_model)
                self.record('liquid.first-use.prepared', cache_initially_absent=True, automatic=True,
                            cache_scope='journey temporary directory', python_runtime_created=False, **metadata)
        self.tex_settings([(cache_setting, '"liquid_cache_directory": ""'),
                           ('"auto_save": false', '"auto_save": true'),
                           ('"build_enabled": false', '"build_enabled": true'),
                           ('"git_diff_style": "SideBySide"', '"git_diff_style": "Unified"')])

    def liquid_selection_fixture(self, style, kind, suffix, original_tabs, *, first_use=False):
        from desktop_ui_journeys import fingerprint
        fixture = liquid_fixture(kind)
        path = self.directory / ('liquid-selection-' + style.lower() + '.' + suffix)
        path.write_text(fixture['base'])
        self.open_harper_fixture(path)
        self.key(125, 'cmd')
        self.native('text', fixture['marker'])
        self.source_is(fixture['working'])
        opened = self.snapshot()['document']
        tabs, tab = opened['tabs'], opened['active_tab_id']
        same = lambda d, source=fixture['working']: liquid_source(d, source, path, tabs, tab)

        # No selection cannot launch a grammar request. The row must be
        # present but disabled, rather than a stale target from another menu.
        self.key(126, 'cmd')
        self.wait('Liquid negative check has a collapsed cursor', lambda d: d['cursor'] == [0, 0])
        self.wait_target('editor.caret')
        self.right_click('editor.caret')
        self.wait_liquid_menu(False)
        self.key(53, 'none')
        self.stable_state(lambda d: same(d) and not liquid_state(d)['visible'])

        self.select_liquid_fixture_line(fixture, style, first_use=first_use)
        self.stable_state(lambda d: same(d) and liquid_comparison(d, fixture, style))
        # A preparation edit exists in the undo stack. This catches a modal
        # Cmd+Z escaping into the source, rather than a harmless empty undo.
        self.key(6)
        self.stable_state(lambda d: same(d) and liquid_comparison(d, fixture, style))
        if self.capture_review and kind == 'typst' and style == LIQUID_DIFF_STYLES[0]:
            self.capture_viewport('liquid')
        # Open the real disclosure and verify it contains the exact submitted
        # prose, including unchanged context but excluding formatting wrappers.
        if liquid_state(self.snapshot()['document'])['sent_open']:
            self.click('liquid.sent')
            self.wait('sent-input disclosure closes', lambda d: liquid_sent(d, fixture, opened=False))
        self.click('liquid.sent')
        self.wait('sent-input disclosure shows the actual prepared fragments',
                  lambda d: same(d) and liquid_sent(d, fixture, opened=True))
        if self.capture_review:
            self.capture_viewport('liquid')
            self.record('liquid.comparison.capture', kind=kind, style=style,
                        before_fingerprint=fingerprint(fixture['selected_before']),
                        after_fingerprint=fingerprint(fixture['selected_after']),
                        sent_fingerprints=[fingerprint(text) for text in fixture['sent_fragments']],
                        sent_open=True)
        self.click('liquid.cancel')
        self.wait('Cancel leaves source and active tab unchanged', lambda d:
                  same(d) and not liquid_state(d)['visible'])

        self.select_liquid_fixture_line(fixture, style)
        self.key(13)  # Cmd+W belongs to the review, never the document tab.
        self.wait('Cmd+W closes only the Liquid review', lambda d:
                  same(d) and not liquid_state(d)['visible'])

        self.select_liquid_fixture_line(fixture, style)
        self.stable_state(lambda d: same(d) and liquid_comparison(d, fixture, style))
        self.click('liquid.apply')
        self.wait('Apply changes only the selected prose and preserves its wrapper', lambda d:
                  liquid_source(d, fixture['corrected'], path, tabs, tab)
                  and not liquid_state(d)['visible'])
        self.key(6)
        self.wait('one Undo restores the whole Liquid application, preserving the earlier edit', same)
        self.stable_state(same)
        if path.read_text() != fixture['base']:
            raise AssertionError('Liquid review or Apply autosaved the disposable source unexpectedly')
        self.record('liquid.selection', kind=kind, style=style, source_fingerprint=fingerprint(fixture['working']),
                    corrected_fingerprint=fingerprint(fixture['corrected']),
                    outside_unchanged=True, wrapper_preserved=True, cancel_unchanged=True,
                    modal_shortcuts_owned=True, single_undo=True, active_tab_id=tab)
        self.close_scratch(original_tabs)

    def liquid_streaming_fixture(self, style, kind, suffix, original_tabs):
        from desktop_ui_journeys import fingerprint
        fixture = liquid_fixture(kind, streaming=True)
        path = self.directory / ('liquid-streaming-' + style.lower() + '.' + suffix)
        path.write_text(fixture['base'])
        self.open_harper_fixture(path)
        self.key(125, 'cmd')
        self.native('text', fixture['marker'])
        self.source_is(fixture['working'])
        opened = self.snapshot()['document']
        tabs, tab = opened['tabs'], opened['active_tab_id']
        same = lambda d: liquid_source(d, fixture['working'], path, tabs, tab)
        self.select_liquid_fixture_line(fixture, style, await_partial=True)
        document = self.snapshot()['document']
        progress = liquid_progress(document)
        if (progress is None or progress['passages'] != 1 or progress['chunks'] <= 1
                or progress['chunk'] != progress['chunks']):
            raise AssertionError('The long wrapped paragraph did not complete multiple model-measured chunks')
        self.stable_state(lambda d: same(d) and liquid_comparison(d, fixture, style))
        self.click('liquid.apply')
        self.wait('streamed Apply preserves math, formatting, and outside source', lambda d:
                  liquid_source(d, fixture['corrected'], path, tabs, tab)
                  and not liquid_state(d)['visible'])
        self.key(6)
        self.wait('one Undo restores all streamed model chunks', same)
        self.stable_state(same)
        if path.read_text() != fixture['base']:
            raise AssertionError('Streaming Liquid review autosaved its disposable source')
        self.record('liquid.streaming', kind=kind, style=style, single_undo=True,
                    math_preserved=True, wrapper_preserved=True, outside_unchanged=True,
                    source_fingerprint=fingerprint(fixture['working']),
                    corrected_fingerprint=fingerprint(fixture['corrected']), **progress)
        self.close_scratch(original_tabs)
