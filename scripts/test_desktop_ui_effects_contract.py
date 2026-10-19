"""Pure checks of the observer-free native effects measurement contract."""
from copy import deepcopy
from unittest.mock import patch
import unittest

from desktop_ui_effects_journeys import (
    effects_pacing_assert_clock_consistent, effects_pacing_fixture, effects_pacing_ledger,
    effects_pacing_observe, effects_pacing_summary,
)


class EffectsPacingContract(unittest.TestCase):
    def document(self, count=0):
        samples = [{
            'sequence': index + 1, 'kind': 'draw', 'draw_counter': index + 1,
            'frame': index + 10, 'viewport': 1, 'callback_ms': index * 20,
            'queued_to_callback_ms': 1, 'shader_time_s': age, 'initialize_ms': 0,
            'draw_submission_ms': .02, 'upload_submission_ms': .01 if index == 0 else 0,
            'instances': 80, 'uploaded': index == 0,
        } for index, age in enumerate([0, .1, .32, .6][:count])]
        return {'typing_particles': {'gpu': {'draws': count, 'timing': {
            'schema': 1, 'capacity': 256, 'sample_count': count, 'dropped': 0,
            'now_ms': 1000, 'samples': samples}}}}

    def test_early_late_callbacks_and_shader_clock_are_reported_without_timing_thresholds(self):
        report = effects_pacing_summary(self.document(), self.document(4))
        self.assertEqual(report['draw_count'], 4)
        self.assertEqual(report['early']['count'], 1)
        self.assertEqual(report['late']['count'], 2)
        self.assertEqual(report['early']['p50_ms'], 20)
        self.assertEqual(report['shader_clock_lead_ms'], 540)
        self.assertEqual(report['shader_callback_offset_range_ms'], 540)
        self.assertEqual(report['uploads'], 1)
        self.assertIn('not swap', report['scope'])

    def test_clock_consistency_rejects_the_recurring_edit_clock_jump(self):
        document = self.document(4)
        samples = document['typing_particles']['gpu']['timing']['samples']
        # The native baseline repeatedly advanced the shader about 19 ms while
        # the first two callbacks were only 6 ms apart, even after warming.
        for sample, callback, age in zip(samples, [100, 106, 123, 140], [0, .019, .036, .053]):
            sample.update(callback_ms=callback, shader_time_s=age)
        report = effects_pacing_summary(self.document(), document)
        self.assertAlmostEqual(report['shader_callback_offset_range_ms'], 13)
        with self.assertRaisesRegex(AssertionError, 'clock-consistency regression'):
            effects_pacing_assert_clock_consistent(report)

    def test_clock_consistency_accepts_delayed_callbacks_without_an_fps_threshold(self):
        document = self.document(4)
        samples = document['typing_particles']['gpu']['timing']['samples']
        for sample, callback, age in zip(samples, [100, 133, 166, 199], [.007, .040, .073, .106]):
            sample.update(callback_ms=callback, shader_time_s=age)
        report = effects_pacing_summary(self.document(), document)
        self.assertAlmostEqual(report['shader_callback_offset_range_ms'], 0)
        self.assertEqual(report['early']['p50_ms'], 33)
        effects_pacing_assert_clock_consistent(report)

    def test_new_measurement_excludes_earlier_ledger_history(self):
        report = effects_pacing_summary(self.document(1), self.document(4))
        self.assertEqual(report['draw_count'], 3)
        self.assertEqual(report['samples'][0]['sequence'], 2)
        self.assertEqual(report['uploads'], 0)

    def test_missing_unbounded_or_corrupt_timing_is_rejected(self):
        mutations = [
            lambda d: d['typing_particles']['gpu'].pop('timing'),
            lambda d: d['typing_particles']['gpu']['timing'].update(capacity=0),
            lambda d: d['typing_particles']['gpu']['timing'].update(sample_count=2),
            lambda d: d['typing_particles']['gpu']['timing']['samples'][0].update(callback_ms=float('nan')),
            lambda d: d['typing_particles']['gpu']['timing']['samples'][0].pop('uploaded'),
        ]
        for mutate in mutations:
            document = self.document(1)
            mutate(document)
            with self.subTest(document=document), self.assertRaises(AssertionError):
                effects_pacing_ledger(document)

    def test_truncated_burst_mixed_viewports_and_repeated_frames_are_rejected(self):
        for field, value in [('viewport', 2), ('frame', 10), ('callback_ms', -1)]:
            document = self.document(4)
            document['typing_particles']['gpu']['timing']['samples'][1][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                effects_pacing_summary(self.document(), document)
        document = self.document(4)
        timing = document['typing_particles']['gpu']['timing']
        timing['samples'] = timing['samples'][1:]
        timing['dropped'] = 1
        with self.assertRaises(AssertionError):
            effects_pacing_summary(self.document(), document)

    def test_single_frame_stall_reports_missing_late_samples_instead_of_hiding_failure(self):
        report = effects_pacing_summary(self.document(), self.document(1))
        self.assertEqual(report['draw_count'], 1)
        self.assertEqual(report['late']['count'], 0)
        self.assertIsNone(report['late']['p95_ms'])

    def test_observation_is_outside_the_full_native_animation_interval(self):
        events = []
        class Journey:
            def snapshot(self):
                events.append('snapshot')
                return {'document': {}}
        def perform():
            events.append('native-input')
        def sleep(seconds):
            self.assertGreaterEqual(seconds, .85)
            events.append('idle-without-observation')
        with patch('desktop_ui_effects_journeys.time.sleep', side_effect=sleep):
            effects_pacing_observe(Journey(), perform)
        self.assertEqual(events, ['snapshot', 'native-input', 'idle-without-observation', 'snapshot'])

    def test_large_saved_fixtures_are_deterministic_and_typst_definitions_are_not_output(self):
        plain, typst = effects_pacing_fixture(), effects_pacing_fixture(typst=True)
        self.assertGreater(len(plain.encode()), 200_000)
        self.assertGreater(len(typst.encode()), 200_000)
        self.assertEqual(typst.count('#let pacing_sample_'), 400)
        self.assertIn('#set page(', typst)
        self.assertEqual(effects_pacing_fixture(typst=True), typst)


if __name__ == '__main__':
    unittest.main()
