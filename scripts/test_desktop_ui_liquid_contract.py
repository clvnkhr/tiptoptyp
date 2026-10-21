"""Failure detectors for the optional native Liquid journey, not interaction evidence."""
from copy import deepcopy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_liquid_journeys import (
    LIQUID_JOURNEYS, LIQUID_DEFAULT_INPUT, LIQUID_NONDEFAULT_INPUT, LIQUID_DIFF_STYLES, LIQUID_MODEL_ID, LIQUID_REVISION, LIQUID_NATIVE_FILES, liquid_asset_path, liquid_native_target, LiquidJourneys,
    liquid_comparison, liquid_fixture, liquid_input_edits, liquid_input_options, liquid_ready,
    liquid_model_directory, liquid_model_metadata, liquid_preparation_evidence,
    liquid_selection_point, liquid_sent, liquid_source, liquid_state, liquid_progress, liquid_partial,
)


class LiquidJourneyContracts(unittest.TestCase):
    def document(self):
        return {'liquid': {'visible': True, 'busy': False, 
                           'current': True, 'changes': 1, 'error': None}}

    def test_ready_requires_completed_current_nonempty_suggestions(self):
        self.assertTrue(liquid_ready(self.document()))
        for change in ({'visible': False}, {'busy': True}, 
                       {'current': False}, {'changes': None}, {'changes': 0}):
            document=self.document();document['liquid'].update(change)
            with self.subTest(change=change):
                self.assertFalse(liquid_ready(document))
        document=self.document();document['liquid']['error']='Model is missing'
        with self.assertRaisesRegex(AssertionError,'selection check failed'):
            liquid_ready(document)

    def test_missing_or_malformed_state_never_passes(self):
        for document in ({}, {'liquid':None}, {'liquid':{}},
                         {'liquid':{**self.document()['liquid'],'changes':True}},
                         {'liquid':{**self.document()['liquid'],'busy':1}}):
            with self.subTest(document=document),self.assertRaises(AssertionError):
                liquid_state(document)

    def geometry(self):
        return {'document':{'cursor':[40,10],'editor_keyboard_focused':True,'frame':12},
                'targets':{'editor.ime_cursor':dict(x=200,y=100,enabled=True,age_ms=0,frame=12,viewport='root'),
                           'editor.source':dict(x=250,y=200,width=480,height=300,enabled=True,age_ms=0,frame=12,viewport='root')}}

    def test_secondary_click_requires_fresh_selected_geometry_inside_editor(self):
        self.assertEqual(liquid_selection_point(self.geometry(),10,40),(195,100))
        for mutate in (lambda s:s['document'].update(cursor=[40,40]),
                       lambda s:s['document'].update(editor_keyboard_focused=False),
                       lambda s:s['targets']['editor.ime_cursor'].update(age_ms=501),
                       lambda s:s['targets']['editor.ime_cursor'].update(frame=11),
                       lambda s:s['targets']['editor.ime_cursor'].update(viewport='other'),
                       lambda s:s['targets']['editor.ime_cursor'].update(x=0),
                       lambda s:s['targets']['editor.source'].update(width=float('nan'))):
            state=deepcopy(self.geometry());mutate(state)
            with self.subTest(state=state),self.assertRaises(AssertionError):
                liquid_selection_point(state,10,40)

    def partial_snapshot(self, fixture):
        document = self.comparison_document(fixture, 'SideBySide')
        document.update(frame=12, source_fingerprint=fingerprint(fixture['working']),
                        source_bytes=len(fixture['working'].encode('utf-8')))
        document['liquid'].update(busy=True, progress={
            'passage': 1, 'passages': 1, 'chunk': 1, 'chunks': 8, 'model_tokens': 125})
        target = dict(age_ms=0, frame=331, viewport='liquid-child',
                      x=300, y=500, width=45, height=21)
        return {'document': document, 'targets': {
            'liquid.apply': {**target, 'enabled': False},
            'liquid.cancel': {**target, 'enabled': True},
            'liquid.diff': {**target, 'enabled': True, 'width': 600, 'height': 300}}}

    def test_streaming_fixture_is_one_long_wrapped_paragraph_with_protected_math(self):
        for kind, math, wrapper in [('typst', '$u$', '#strong['), ('tex', r'\(u\)', r'\textbf{')]:
            fixture = liquid_fixture(kind, streaming=True)
            self.assertNotIn('\n', fixture['selected_before'])
            self.assertEqual(fixture['selected_before'].count('This is fine.'), 180)
            self.assertTrue(fixture['selected_before'].startswith(wrapper + 'She go to school'))
            self.assertIn(math, fixture['selected_before'])
            self.assertIn(math, fixture['selected_after'])
            self.assertEqual(fixture['selected_after'], fixture['selected_before'].replace('She go ', 'She goes '))
            self.assertEqual(len(fixture['sent_fragments']), 1)
            self.assertTrue(fixture['sent_fragments'][0].endswith('We estimate x.'))
            self.assertNotIn(math, fixture['sent_fragments'][0])
            self.assertIn('He go to school.', fixture['corrected'])
            self.assertEqual(fixture['corrected'].replace('She goes ', 'She go '), fixture['working'])

    def test_partial_diff_requires_native_progress_and_a_fresh_disabled_apply_target(self):
        fixture = liquid_fixture('typst', streaming=True)
        valid = self.partial_snapshot(fixture)
        self.assertTrue(liquid_partial(valid, fixture, 'SideBySide'))
        self.assertNotEqual(valid['document']['frame'], valid['targets']['liquid.apply']['frame'],
                            'root and child frames must be allowed to differ')
        for mutate in (lambda s: s['document']['liquid'].update(busy=False),
                       lambda s: s['document']['liquid'].update(visible=False),
                       lambda s: s['document']['liquid'].update(current=False),
                       lambda s: s['document']['liquid'].update(changes=0),
                       lambda s: s['document']['liquid'].update(progress=None),
                       lambda s: s['document']['liquid']['progress'].update(chunk=8),
                       lambda s: s['document']['liquid']['progress'].update(chunks=1),
                       lambda s: s['document']['liquid']['progress'].update(passages=2),
                       lambda s: s['targets']['liquid.apply'].update(enabled=True),
                       lambda s: s['targets']['liquid.apply'].update(age_ms=501),
                       lambda s: s['targets']['liquid.apply'].update(frame=11),
                       lambda s: s['targets']['liquid.cancel'].update(frame=330),
                       lambda s: s['targets']['liquid.diff'].update(frame=330),
                       lambda s: s['targets']['liquid.cancel'].update(viewport='other-child'),
                       lambda s: s['targets']['liquid.cancel'].update(enabled=False),
                       lambda s: s['targets']['liquid.apply'].update(height=-151.5),
                       lambda s: s['targets']['liquid.diff'].update(width=0),
                       lambda s: s['targets']['liquid.apply'].update(x=float('nan')),
                       lambda s: s['targets']['liquid.cancel'].update(age_ms=501),
                       lambda s: s['targets']['liquid.diff'].update(age_ms=True),
                       lambda s: s['targets']['liquid.apply'].update(frame=True),
                       lambda s: s['document'].update(source_fingerprint='changed'),
                       lambda s: s['document']['liquid']['comparison'].update(after=fingerprint('goes'))):
            changed = deepcopy(valid)
            mutate(changed)
            with self.subTest(changed=changed):
                self.assertFalse(liquid_partial(changed, fixture, 'SideBySide'))
        valid['targets'].clear()
        self.assertFalse(liquid_partial(valid, fixture, 'SideBySide'))

    def test_progress_is_optional_but_malformed_or_oversized_model_counts_never_pass(self):
        document = self.document()
        self.assertIsNone(liquid_progress(document))
        valid = self.partial_snapshot(liquid_fixture('tex', streaming=True))['document']['liquid']['progress']
        document['liquid']['progress'] = valid
        self.assertEqual(liquid_progress(document), valid)
        for change in ({'chunk': 0}, {'chunks': 0}, {'passage': 2}, {'chunk': 9},
                       {'model_tokens': 129}, {'model_tokens': True}, {'passages': 1.0}):
            document['liquid']['progress'] = {**valid, **change}
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, 'chunk progress inspection'):
                liquid_progress(document)
        for malformed in ([], {}, {**valid, 'extra': 1}):
            document['liquid']['progress'] = malformed
            with self.subTest(malformed=malformed), self.assertRaisesRegex(AssertionError, 'chunk progress inspection'):
                liquid_progress(document)

    def test_source_check_requires_exact_bytes_tab_and_document_identity(self):
        document=dict(path='/fixture.typ',tabs=2,active_tab_id=4,
                      source_fingerprint=fingerprint('Valid.'),source_bytes=6)
        self.assertTrue(liquid_source(document,'Valid.',Path('/fixture.typ'),2,4))
        for change in ({'tabs':1},{'active_tab_id':5},{'source_fingerprint':fingerprint('Other.')},
                       {'source_bytes':7},{'path':'/other.typ'}):
            self.assertFalse(liquid_source({**document,**change},'Valid.',Path('/fixture.typ'),2,4))

    def test_both_real_fixtures_keep_an_outside_error_and_preserve_markup(self):
        for kind,wrapper in [('typst','#strong['),('tex','\\textbf{')]:
            fixture=liquid_fixture(kind)
            selected=fixture['working'][fixture['start']:fixture['end']]
            self.assertTrue(selected.startswith(wrapper))
            self.assertIn('She go to school every day.',selected)
            self.assertIn('He go to school.',fixture['corrected'])
            self.assertIn(wrapper+'This is fine. She goes to school every day.',fixture['corrected'])
            self.assertIn('This is fine.', fixture['selected_before'])
            self.assertIn('This is fine.', fixture['selected_after'])
            self.assertEqual(selected, fixture['selected_before'])
            self.assertEqual(fixture['selected_after'], selected.replace('She go ', 'She goes '))
            self.assertNotIn('He go to school.', selected)
            self.assertEqual(fixture['working'][:fixture['start']],fixture['corrected'][:fixture['start']])
            self.assertTrue(fixture['working'].endswith(fixture['marker']))

    def comparison_document(self, fixture, style):
        document = self.document()
        document['liquid']['comparison'] = {
            'before': fingerprint(fixture['selected_before']),
            'after': fingerprint(fixture['selected_after']),
            'before_chars': len(fixture['selected_before']),
            'after_chars': len(fixture['selected_after']),
            'style': style,
        }
        return document

    def test_full_comparison_requires_unchanged_selection_context_and_effective_style(self):
        for kind in ('typst', 'tex'):
            fixture = liquid_fixture(kind)
            for style in LIQUID_DIFF_STYLES:
                document = self.comparison_document(fixture, style)
                self.assertTrue(liquid_comparison(document, fixture, style))
                for change in ({'before': fingerprint('go')}, {'after': fingerprint('goes')},
                               {'before': fingerprint(fixture['working'])},
                               {'before_chars': len(fixture['selected_before']) - 1},
                               {'after_chars': len(fixture['selected_after']) + 1},
                               {'style': 'SideBySide' if style == 'Unified' else 'Unified'}):
                    modified = deepcopy(document)
                    modified['liquid']['comparison'].update(change)
                    with self.subTest(kind=kind, style=style, change=change):
                        self.assertFalse(liquid_comparison(modified, fixture, style))
                document['liquid']['busy'] = True
                document['liquid']['comparison'] = None
                self.assertFalse(liquid_comparison(document, fixture, style))

    def test_comparison_rejects_malformed_inspection_and_uses_scalar_counts(self):
        fixture = dict(selected_before='α🙂 go', selected_after='α🙂 goes')
        document = self.comparison_document(fixture, 'Unified')
        self.assertTrue(liquid_comparison(document, fixture, 'Unified'))
        document['liquid']['comparison']['before_chars'] = len(fixture['selected_before'].encode('utf-8'))
        self.assertFalse(liquid_comparison(document, fixture, 'Unified'))
        for comparison in (None, {}, {'before': 1, 'after': 'hash', 'before_chars': 1,
                                     'after_chars': 2, 'style': 'Unified'},
                           {**self.comparison_document(fixture, 'Unified')['liquid']['comparison'],
                            'before_chars': True},
                           {**self.comparison_document(fixture, 'Unified')['liquid']['comparison'],
                            'style': 'Impossible'}):
            document['liquid']['comparison'] = comparison
            with self.subTest(comparison=comparison), self.assertRaisesRegex(AssertionError, 'comparison inspection'):
                liquid_comparison(document, fixture, 'Unified')
        with self.assertRaises(ValueError):
            liquid_comparison(self.document(), fixture, 'Impossible')

    def test_journey_exercises_both_languages_in_each_real_settings_style(self):
        class Recorder(LiquidJourneys):
            liquid_model_dir = '/preinstalled'
            def __init__(self, directory):
                self.directory = directory
                self.settings_edits, self.fixtures, self.records, self.streaming = [], [], [], []
                self.input_options = dict(LIQUID_DEFAULT_INPUT)
            def tex_settings(self, edits):
                self.settings_edits.append(edits)
                for _, new in edits:
                    for key, value in json.loads('{' + new + '}').items():
                        if key in self.input_options:
                            self.input_options[key] = value
            def snapshot(self):
                return {'document': {'tabs': 2, 'liquid': {
                    'visible': False, 'busy': False, 
                    'current': False, 'changes': None, 'error': None,
                    'input_options': dict(self.input_options)}}}
            def wait(self, description, predicate):
                if not predicate(self.snapshot()['document']):
                    raise AssertionError(description)
            def record(self, name, **data):
                self.records.append((name, data))
            def liquid_selection_fixture(self, style, kind, suffix, tabs, *, first_use=False):
                self.fixtures.append((style, kind, suffix, tabs, first_use))
            def liquid_streaming_fixture(self, style, kind, suffix, tabs):
                self.streaming.append((style, kind, suffix, tabs))
        with tempfile.TemporaryDirectory() as directory:
            journey = Recorder(Path(directory))
            with patch('desktop_ui_liquid_journeys.liquid_model_directory', return_value=Path('/preinstalled')):
                journey.liquid_selection()
            fresh = journey.directory / 'liquid-first-use-cache'
            self.assertFalse(fresh.exists(), 'the driver must let the app create its own model cache')
            self.assertIn(('"liquid_cache_directory": ""', '"liquid_cache_directory": ' + json.dumps(str(fresh))),
                          journey.settings_edits[0])
            self.assertTrue(all('/preinstalled' not in replacement for edits in journey.settings_edits
                                for _, replacement in edits))
        self.assertEqual(journey.fixtures,
                         [('Unified', 'typst', 'typ', 2, True), ('Unified', 'tex', 'tex', 2, False),
                          ('SideBySide', 'typst', 'typ', 2, False), ('SideBySide', 'tex', 'tex', 2, False)])
        self.assertEqual(journey.streaming, [('SideBySide', 'typst', 'typ', 2),
                                             ('SideBySide', 'tex', 'tex', 2)])
        self.assertEqual(journey.settings_edits[2],
                         [('"git_diff_style": "Unified"', '"git_diff_style": "SideBySide"')])
        self.assertIn(('"git_diff_style": "SideBySide"', '"git_diff_style": "Unified"'),
                      journey.settings_edits[-1])
        self.assertEqual(journey.settings_edits[1],
                         liquid_input_edits(LIQUID_NONDEFAULT_INPUT, LIQUID_DEFAULT_INPUT))
        self.assertEqual(journey.records[0], ('liquid.input.settings.roundtrip',
                         {'saved': LIQUID_NONDEFAULT_INPUT, 'restored': LIQUID_DEFAULT_INPUT}))

    def model_fixture(self, root):
        installed = root / 'onnxruntime' / LIQUID_MODEL_ID
        installed.mkdir(parents=True)
        files = []
        identities = [(None, name) for name in ('liquid.onnx', 'tokenizer.json')]
        identities.extend((target, name) for target, names in LIQUID_NATIVE_FILES.items() for name in names)
        for target, name in identities:
            content = b'preinstalled native model marker'
            entry = {'name': name, 'bytes': len(content), 'sha256': 'b' * 64,
                     'compressed_bytes': 8, 'compressed_sha256': 'c' * 64}
            if target is not None:
                entry['target'] = target
            path = installed / liquid_asset_path(entry)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            files.append(entry)
        manifest = {'format': 3, 'model_id': LIQUID_MODEL_ID,
                    'upstream_revision': LIQUID_REVISION, 'files': files}
        (installed / 'manifest.json').write_text(json.dumps(manifest))
        return manifest

    def test_first_use_automatically_prepares_assets_without_an_install_click(self):
        owner = self
        fixture = liquid_fixture('typst')
        class Interactive(LiquidJourneys):
            def __init__(self, root):
                self.liquid_preparation_cache = root / 'new-cache'
                self.liquid_reference_model = root / 'reference'
                owner.model_fixture(self.liquid_reference_model)
                self.events = []
                self.state = {'document': owner.document(), 'targets': {}}
            def click(self, name):
                self.events.append(('click', name))
                if name == 'menu.Check selection with Liquid':
                    owner.assertFalse(self.liquid_preparation_cache.exists())
                    owner.model_fixture(self.liquid_preparation_cache)
                    self.state = owner.geometry()
                    self.state['document'] = owner.comparison_document(fixture, 'Unified')
                    self.state['document']['liquid']['sent'] = [
                        {'fingerprint': fingerprint(text), 'chars': len(text)} for text in fixture['sent_fragments']]
                    self.state['document']['liquid']['sent_open'] = False
                elif name != 'editor.source':
                    raise AssertionError('Unexpected control: ' + name)
            def key(self, *args): pass
            def native(self, *args): pass
            def wait(self, description, predicate, **kwargs):
                if not description.startswith('keyboard '):
                    owner.assertTrue(predicate(self.state['document']), description)
            def wait_target(self, name): pass
            def snapshot(self):
                state = owner.geometry()
                state['document']['cursor'] = [fixture['end'], fixture['start']]
                return state
            def right_click_at(self, *args): pass
            def wait_liquid_menu(self, enabled): owner.assertTrue(enabled)
            def record(self, name, **data): self.events.append(('record', name, data))
        with tempfile.TemporaryDirectory() as directory:
            journey = Interactive(Path(directory))
            journey.select_liquid_fixture_line(fixture, 'Unified', first_use=True)
            clicks = [event[1] for event in journey.events if event[0] == 'click']
            self.assertEqual(clicks, ['editor.source', 'menu.Check selection with Liquid'])
            record = next(event for event in journey.events if event[0] == 'record')
            self.assertEqual(record[1], 'liquid.first-use.prepared')
            self.assertTrue(record[2]['automatic'])
            self.assertTrue(record[2]['cache_initially_absent'])
            self.assertFalse(record[2]['python_runtime_created'])
            with self.assertRaisesRegex(AssertionError, 'already exists'):
                journey.select_liquid_fixture_line(fixture, 'Unified', first_use=True)

    def test_first_use_evidence_rejects_different_assets_and_preserves_external_reference(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reference, installed = root / 'reference', root / 'installed'
            self.model_fixture(reference); self.model_fixture(installed)
            original = {path.relative_to(reference): path.read_bytes()
                        for path in reference.rglob('*') if path.is_file()}
            evidence = liquid_preparation_evidence(installed, reference)
            self.assertEqual(len(evidence['files']), 4)
            self.assertEqual(len(evidence['manifest_sha256']), 64)
            path = installed / 'onnxruntime' / LIQUID_MODEL_ID / 'manifest.json'
            altered = json.loads(path.read_text()); altered['files'][0]['sha256'] = 'd' * 64
            path.write_text(json.dumps(altered))
            with self.assertRaisesRegex(AssertionError, 'reference model assets'):
                liquid_preparation_evidence(installed, reference)
            self.assertEqual(original, {path.relative_to(reference): path.read_bytes()
                                       for path in reference.rglob('*') if path.is_file()})

    def test_sent_input_detector_requires_exact_fragments_not_markup_or_a_new_parse(self):
        fixture = liquid_fixture('typst')
        document = self.document()
        sent = [{'fingerprint': fingerprint(text), 'chars': len(text)}
                for text in fixture['sent_fragments']]
        document['liquid'].update(sent=sent, sent_open=False)
        self.assertTrue(liquid_sent(document, fixture))
        self.assertTrue(liquid_sent(document, fixture, opened=False))
        self.assertFalse(liquid_sent(document, fixture, opened=True))
        document['liquid']['sent_open'] = True
        self.assertTrue(liquid_sent(document, fixture, opened=True))
        for changed in ([], sent + sent,
                        [{'fingerprint': fingerprint(fixture['selected_before']),
                          'chars': len(fixture['selected_before'])}],
                        [{'fingerprint': sent[0]['fingerprint'], 'chars': sent[0]['chars'] + 1}]):
            document['liquid']['sent'] = changed
            with self.subTest(changed=changed):
                self.assertFalse(liquid_sent(document, fixture))
        document['liquid']['sent'] = None
        self.assertFalse(liquid_sent(document, fixture))
        # Prepared fragments remain inspectable even after inference errors.
        document['liquid'].update(sent=sent, error='Runtime is unavailable', changes=None)
        self.assertTrue(liquid_sent(document, fixture, opened=True))

    def test_sent_input_detector_rejects_malformed_state_and_counts_unicode_scalars(self):
        fixture = {'sent_fragments': ['A α🙂 sentence.']}
        document = self.document()
        valid = [{'fingerprint': fingerprint(fixture['sent_fragments'][0]),
                  'chars': len(fixture['sent_fragments'][0])}]
        document['liquid'].update(sent=valid, sent_open=True)
        self.assertTrue(liquid_sent(document, fixture))
        document['liquid']['sent'][0]['chars'] = len(fixture['sent_fragments'][0].encode('utf-8'))
        self.assertFalse(liquid_sent(document, fixture))
        for sent in ({}, [None], [{'fingerprint': 'hash', 'chars': True}],
                     [{'fingerprint': 1, 'chars': 2}], valid * 16_385):
            document['liquid']['sent'] = sent
            with self.subTest(sent=sent), self.assertRaisesRegex(AssertionError, 'sent-input inspection'):
                liquid_sent(document, fixture)
        document['liquid'].update(sent=valid, sent_open=None)
        with self.assertRaisesRegex(AssertionError, 'sent-input inspection'):
            liquid_sent(document, fixture)

    def test_sent_input_detector_accepts_many_passages_within_the_runtime_limit(self):
        fixture = {'sent_fragments': ['A sentence.'] * 65}
        document = self.document()
        document['liquid'].update(sent=[{'fingerprint': fingerprint(text), 'chars': len(text)}
                                       for text in fixture['sent_fragments']], sent_open=False)
        self.assertTrue(liquid_sent(document, fixture))

    def test_settings_detector_requires_effective_valid_options(self):
        document = self.document()
        document['liquid']['input_options'] = dict(LIQUID_DEFAULT_INPUT)
        self.assertTrue(liquid_input_options(document, LIQUID_DEFAULT_INPUT))
        self.assertFalse(liquid_input_options(document, LIQUID_NONDEFAULT_INPUT))
        document['liquid']['input_options'] = dict(LIQUID_NONDEFAULT_INPUT)
        self.assertTrue(liquid_input_options(document, LIQUID_NONDEFAULT_INPUT))
        for value in (None, {}, {**LIQUID_DEFAULT_INPUT, 'placeholder': 1},
                      {**LIQUID_DEFAULT_INPUT, 'protected_content': 'raw'},
                      {**LIQUID_DEFAULT_INPUT, 'formatted_text': 'raw'},
                      {**LIQUID_DEFAULT_INPUT, 'other': True}):
            document['liquid']['input_options'] = value
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, 'input settings inspection'):
                liquid_input_options(document, LIQUID_DEFAULT_INPUT)
        edits = liquid_input_edits(LIQUID_DEFAULT_INPUT, LIQUID_NONDEFAULT_INPUT)
        self.assertEqual(len(edits), 3)
        self.assertIn(('"placeholder": "x"', '"placeholder": "MATH"'), edits)
        self.assertTrue(all('\n' not in before + after for before, after in edits))

    def test_model_preflight_requires_native_assets_and_never_installs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            with self.assertRaises(ValueError): liquid_model_directory(None)
            with self.assertRaises(ValueError): liquid_model_directory('relative')
            with self.assertRaises(ValueError): liquid_model_directory(root)
            installed = root / 'onnxruntime' / LIQUID_MODEL_ID
            manifest = self.model_fixture(root)
            manifest_file = installed / 'manifest.json'
            before = {str(path) for path in root.rglob('*')}
            self.assertEqual(liquid_model_directory(root), root)
            self.assertEqual({str(path) for path in root.rglob('*')}, before)
            self.assertFalse((root / 'runtime').exists())
            for mutation in (lambda m: m.update(model_id='old-model'),
                             lambda m: m['files'][0].update(name='../liquid.onnx'),
                             lambda m: m['files'][0].update(bytes=1),
                             lambda m: m['files'][0].update(bytes=True)):
                altered = deepcopy(manifest); mutation(altered)
                manifest_file.write_text(json.dumps(altered))
                with self.subTest(manifest=altered), self.assertRaises(ValueError):
                    liquid_model_directory(root)
            manifest_file.write_text(json.dumps(manifest))
            (installed / 'liquid.onnx').unlink()
            with self.assertRaises(ValueError): liquid_model_directory(root)

    def test_wrapped_line_selection_uses_document_boundaries_and_exact_suffix(self):
        owner = self
        for kind in ('typst', 'tex'):
            fixture = liquid_fixture(kind, streaming=True)
            class Navigation(LiquidJourneys):
                def __init__(self):
                    self.cursor = [0, 0]
                    self.events = []
                    self.review = False
                def click(self, name):
                    self.events.append(('click', name))
                    self.review |= name == 'menu.Check selection with Liquid'
                def key(self, code, flags='cmd'):
                    self.events.append(('key', code, flags))
                    if (code, flags) == (126, 'cmd'):
                        self.cursor = [0, 0]
                    elif (code, flags) == (125, 'none'):
                        at = fixture['working'].index('\n', self.cursor[0]) + 1
                        self.cursor = [at, at]
                    elif (code, flags) == (125, 'cmd+shift'):
                        self.cursor[0] = len(fixture['working'])
                    elif (code, flags) == (124, 'cmd+shift'):
                        # Model the actual failed native behavior: only one
                        # visual row would be selected, never the paragraph.
                        self.cursor[0] += 58
                    else:
                        raise AssertionError((code, flags))
                def native(self, command, code, flags, count, interval):
                    owner.assertEqual((command, code, flags), ('key-burst', 123, 'shift'))
                    owner.assertTrue(1 <= count <= 60)
                    self.events.append((command, count))
                    self.cursor[0] -= count
                def snapshot(self):
                    state = owner.geometry()
                    state['document']['cursor'] = self.cursor[:]
                    if self.review:
                        state['document'].update(owner.comparison_document(fixture, 'SideBySide'))
                        state['document']['liquid'].update(sent_open=False, sent=[
                            {'fingerprint': fingerprint(text), 'chars': len(text)}
                            for text in fixture['sent_fragments']])
                    return state
                def wait(self, description, predicate, **kwargs):
                    owner.assertTrue(predicate(self.snapshot()['document']), description)
                def wait_target(self, name): pass
                def right_click_at(self, *point):
                    owner.assertEqual(self.cursor, [fixture['end'], fixture['start']])
                def wait_liquid_menu(self, enabled): owner.assertTrue(enabled)
            journey = Navigation()
            journey.select_liquid_fixture_line(fixture, 'SideBySide')
            self.assertNotIn(('key', 124, 'cmd+shift'), journey.events)
            self.assertIn(('key', 125, 'cmd+shift'), journey.events)
            self.assertEqual(sum(event[1] for event in journey.events if event[0] == 'key-burst'),
                             len(fixture['working']) - fixture['end'])

    def test_focused_streaming_lane_runs_only_long_language_cases_and_own_cache(self):
        owner = self
        class Recorder(LiquidJourneys):
            def __init__(self, directory):
                self.directory = directory
                self.settings_edits, self.fixtures = [], []
            def tex_settings(self, edits): self.settings_edits.append(edits)
            def snapshot(self):
                return {'document': {'tabs': 2, 'liquid': {
                    'visible': False, 'busy': False, 'current': False, 'changes': None,
                    'error': None, 'input_options': dict(LIQUID_DEFAULT_INPUT)}}}
            def wait(self, description, predicate): owner.assertTrue(predicate(self.snapshot()['document']))
            def record(self, *args, **kwargs): pass
            def liquid_selection_fixture(self, *args, **kwargs):
                raise AssertionError('focused streaming lane reran short selections')
            def liquid_streaming_fixture(self, style, kind, suffix, tabs):
                self.fixtures.append((style, kind, suffix, tabs))
                if kind == 'typst': owner.model_fixture(self.liquid_preparation_cache)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = root / 'reference'
            self.model_fixture(reference)
            journey = Recorder(root / 'journey')
            with patch('desktop_ui_liquid_journeys.liquid_model_directory', return_value=reference):
                journey.liquid_streaming()
            self.assertEqual(journey.fixtures, [('SideBySide', 'typst', 'typ', 2),
                                                ('SideBySide', 'tex', 'tex', 2)])
            self.assertEqual(len(journey.settings_edits), 2)
            cache = journey.directory / 'liquid-streaming-cache'
            self.assertIn(('"liquid_cache_directory": ""', '"liquid_cache_directory": ' + json.dumps(str(cache))),
                          journey.settings_edits[0])

    def test_optional_journey_is_registered_and_excluded_from_all(self):
        spec=importlib.util.spec_from_file_location('liquid_runner',Path(__file__).with_name('test-desktop-ui.py'))
        runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
        for name in LIQUID_JOURNEYS:
            self.assertIn(name,runner.JOURNEY_NAMES)
            self.assertTrue(callable(getattr(runner.Journey,name,None)))
            self.assertNotIn(name,runner.selected_journeys('all'))
            self.assertEqual(runner.selected_journeys(name),[name])


if __name__=='__main__':unittest.main()
