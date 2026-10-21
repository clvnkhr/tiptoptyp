"""Background preview observation contracts; these do not count as native input."""
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace
import sys
import unittest
from unittest.mock import patch
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_inactive_preview_journeys import (
    InactivePreviewJourneys, inactive_preview_owner, inactive_tinymist_ready,
)


class InactivePreviewContracts(unittest.TestCase):
    def state(self, backend='Interactive'):
        return {'document': dict(path='/fixture.typ', source_fingerprint='same',
                    settings_visible=True, focused=False, preview_requested_backend=backend,
                    preview_backend='Tinymist partial' if backend == 'InteractivePartial' else 'Tinymist',
                    preview_native_ready=True, preview_surface_exists=True, viewport='root'),
                'renderer': dict(age_ms=0, viewport='root', value=dict(loaded=True, ready=True,
                    partial=backend == 'InteractivePartial'))}

    def ready(self, state, backend='Interactive'):
        return inactive_tinymist_ready(state, backend, dict(path='/fixture.typ',
                                       source_fingerprint='same', viewport='root'))

    def test_requires_completed_actual_full_and_partial_renderers_in_inactive_owner(self):
        for backend in ('Interactive', 'InteractivePartial'):
            with self.subTest(backend=backend):
                state = self.state(backend)
                self.assertTrue(self.ready(state, backend))
                for key, value in [('focused', True), ('focused', None),
                                   ('preview_requested_backend', 'Pdfium'), ('preview_backend', 'PDFium'),
                                   ('preview_native_ready', False), ('preview_surface_exists', False)]:
                    broken = deepcopy(state)
                    broken['document'][key] = value
                    self.assertFalse(self.ready(broken, backend), key)
                for key, value in [('ready', False), ('loaded', False),
                                   ('partial', backend != 'InteractivePartial')]:
                    broken = deepcopy(state)
                    broken['renderer']['value'][key] = value
                    self.assertFalse(self.ready(broken, backend), key)
                for renderer in (None, {}, {'age_ms': 0, 'value': None},
                                 {**state['renderer'], 'age_ms': 500},
                                 {**state['renderer'], 'viewport': 'secondary'}):
                    broken = deepcopy(state)
                    broken['renderer'] = renderer
                    self.assertFalse(self.ready(broken, backend))

    def test_secondary_renderer_is_bound_independently_of_global_settings_owner(self):
        state = self.state()
        # Settings belongs to the root process host, not the secondary editor.
        state['document']['settings_visible'] = False
        self.assertTrue(self.ready(state))
        state['renderer']['viewport'] = 'another-document'
        self.assertFalse(self.ready(state))

    def test_source_mutation_and_renderer_failure_are_failures_not_pending(self):
        for key, value, error in [('path', '/other.typ', 'source owner'),
                                   ('source_fingerprint', 'changed', 'source owner'),
                                   ('viewport', 'secondary', 'source owner'),
                                   ('preview_failure', 'server died', 'Inactive Tinymist failed')]:
            state = self.state()
            state['document'][key] = value
            with self.assertRaisesRegex(AssertionError, error):
                self.ready(state)

    def harness(self, state, foreground):
        class Harness(InactivePreviewJourneys):
            def __init__(self): self.events, self.process = [], SimpleNamespace(pid=9)
            def native(self, command):
                self.events.append(command)
                if command != 'foreground': raise AssertionError('unexpected input/recovery')
                return {'pid': foreground}
            def snapshot(self, viewport=None):
                if viewport != 'root': raise AssertionError('unscoped observation')
                self.events.append(('snapshot', viewport))
                return state
            def record(self, name, **values): self.events.append((name, values))
        return Harness()

    def test_targeted_owner_checks_do_not_silently_skip_sibling_snapshots(self):
        state = self.state()
        before = dict(state['document'])
        state['document']['viewport'] = 'secondary'
        harness = self.harness(state, 8)
        with self.assertRaisesRegex(AssertionError, 'source owner'):
            harness.wait_preview_owner(before, 'owner readiness', lambda document: True)
        self.assertEqual(harness.events, [('snapshot', 'root')])

    def test_success_observes_without_input_or_activation(self):
        state = self.state()
        harness = self.harness(state, 8)
        self.assertIs(harness.wait_inactive_tinymist('Interactive', state['document'], 8), state)
        self.assertEqual(harness.events[:2], ['foreground', ('snapshot', 'root')])
        self.assertEqual(harness.events[2][0], 'inactive_preview.ready')

    def test_stolen_activation_fails_before_snapshot_and_is_never_retried(self):
        state = self.state()
        harness = self.harness(state, 9)
        with self.assertRaisesRegex(AssertionError, 'stole application activation'):
            harness.wait_inactive_tinymist('Interactive', state['document'], 8)
        self.assertEqual(harness.events, ['foreground'])

    def test_unrelated_application_focus_is_an_interruption_not_activation_evidence(self):
        state = self.state()
        harness = self.harness(state, 10)
        with self.assertRaisesRegex(AssertionError, 'interrupted by foreground change'):
            harness.wait_inactive_tinymist('Interactive', state['document'], 8)
        self.assertEqual(harness.events, ['foreground'])

    def test_background_wait_is_bounded_and_retains_concise_failure_state(self):
        state = self.state()
        state['document']['preview_native_ready'] = False
        harness = self.harness(state, 8)
        with patch('desktop_ui_inactive_preview_journeys.time.monotonic', side_effect=[0, 0, 1]), \
             patch('desktop_ui_inactive_preview_journeys.time.sleep') as sleep:
            with self.assertRaisesRegex(AssertionError, 'did not render'):
                harness.wait_inactive_tinymist('Interactive', state['document'], 8, timeout=.5)
        sleep.assert_called_once_with(.075)
        self.assertEqual(harness.events[-1][0], 'inactive_preview.timeout')


if __name__ == '__main__':
    unittest.main()
