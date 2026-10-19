"""Observer-free native frame pacing evidence for finite editor effects."""
import hashlib
import json
import math
import statistics
import time


def effects_pacing_fixture(typst=False):
    header = ('#set page(width: 240pt, height: 260pt, margin: 20pt)\n'
              '= Effects pacing\nA small live preview with substantial source.\n') if typst else 'Effects pacing: substantial saved plain text.\n'
    prefix = '// ' if typst else ''
    rows = ''.join(
        f'#let pacing_sample_{index:04} = (left: {index % 97}, right: {(index * 17) % 113}, gain: 1.25) // syntax cache fixture\n'
        if typst and index % 5 == 0 else
        f'{prefix}Row {index:04}: amplitude {index % 97:02}, alpha beta gamma, '
        f'coefficient {(index * 17) % 113:03}, stable measurement fixture and deterministic frame pacing content.\n'
        for index in range(2000))
    return header + rows + ('\nPacing tail ' if typst else 'Pacing tail ')


def effects_pacing_ledger(document):
    gpu = document.get('typing_particles', {}).get('gpu', {})
    ledger = gpu.get('timing')
    if not isinstance(ledger, dict) or ledger.get('schema') != 1:
        raise AssertionError('effects pacing requires the opt-in native GPU timing ledger')
    samples, capacity = ledger.get('samples'), ledger.get('capacity')
    if not isinstance(samples, list) or type(capacity) is not int or not 0 < capacity <= 4096 or len(samples) > capacity:
        raise AssertionError('effects pacing ledger is not bounded')
    if not isinstance(ledger.get('now_ms'), (int, float)) or not math.isfinite(ledger['now_ms']):
        raise AssertionError('effects pacing clock is unavailable')
    if type(ledger.get('sample_count')) is not int or type(ledger.get('dropped')) is not int or ledger['dropped'] < 0 or ledger['sample_count'] != len(samples) + ledger['dropped']:
        raise AssertionError('effects pacing sample counter is invalid')
    previous = 0
    for sample in samples:
        if not isinstance(sample, dict) or type(sample.get('sequence')) is not int or sample['sequence'] <= previous:
            raise AssertionError('effects pacing samples are not ordered')
        previous = sample['sequence']
        if sample.get('kind') not in ('draw', 'warm'):
            raise AssertionError('effects pacing sample has an unknown kind')
        for field in ('callback_ms', 'queued_to_callback_ms', 'initialize_ms', 'draw_submission_ms', 'upload_submission_ms'):
            value = sample.get(field)
            if not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                raise AssertionError('invalid effects pacing timing: ' + field)
        for field in ('draw_counter', 'frame', 'viewport', 'instances'):
            if type(sample.get(field)) is not int or sample[field] < 0:
                raise AssertionError('invalid effects pacing identity: ' + field)
        if type(sample.get('uploaded')) is not bool:
            raise AssertionError('effects pacing upload flag is missing')
        if sample['kind'] == 'draw':
            age = sample.get('shader_time_s')
            if not isinstance(age, (int, float)) or not math.isfinite(age) or age < 0:
                raise AssertionError('effects pacing draw has no shader age')
    return ledger


def _pacing_distribution(values):
    ordered = sorted(values)
    if not ordered:
        return {'count': 0, 'p50_ms': None, 'p95_ms': None, 'max_ms': None}
    return {'count': len(ordered), 'p50_ms': statistics.median(ordered),
            'p95_ms': ordered[math.ceil(.95 * len(ordered)) - 1], 'max_ms': ordered[-1]}


def effects_pacing_summary(before, after):
    """Actual GPU callback cadence, never an onscreen-presentation/FPS assertion."""
    prior, current = effects_pacing_ledger(before), effects_pacing_ledger(after)
    cutoff = prior['sample_count']
    new_samples = [sample for sample in current['samples'] if sample['sequence'] > cutoff]
    delta = current['sample_count'] - cutoff
    if delta < 0 or delta > current['capacity'] or len(new_samples) != delta or any(
            sample['sequence'] != cutoff + offset + 1 for offset, sample in enumerate(new_samples)):
        raise AssertionError('effects pacing ledger lost newly recorded samples')
    samples = [sample for sample in new_samples if sample['kind'] == 'draw']
    draw_delta = after['typing_particles']['gpu']['draws'] - before['typing_particles']['gpu']['draws']
    if not samples or len(samples) != draw_delta:
        raise AssertionError('effects pacing lost draws or the effect did not render')
    if len({sample['viewport'] for sample in samples}) != 1:
        raise AssertionError('effects pacing mixed native viewports')
    intervals = {'early': [], 'late': []}
    for previous, sample in zip(samples, samples[1:]):
        delta = sample['callback_ms'] - previous['callback_ms']
        if delta < 0 or sample['frame'] <= previous['frame'] or sample['shader_time_s'] < previous['shader_time_s']:
            raise AssertionError('effects pacing callbacks moved backwards or repeated a frame')
        intervals['early' if sample['shader_time_s'] < .320 else 'late'].append(delta)
    shader_advance_ms = (samples[-1]['shader_time_s'] - samples[0]['shader_time_s']) * 1000
    callback_span_ms = samples[-1]['callback_ms'] - samples[0]['callback_ms']
    clock_offsets = [sample['shader_time_s'] * 1000 - sample['callback_ms'] for sample in samples]
    return {
        'scope': 'native GPU callback intervals; not swap/compositor presentation or GPU execution time',
        'shader_advance_ms': shader_advance_ms, 'callback_span_ms': callback_span_ms,
        'shader_clock_lead_ms': shader_advance_ms - callback_span_ms,
        'shader_callback_offset_range_ms': max(clock_offsets) - min(clock_offsets),
        'split_shader_age_ms': 320, 'draw_count': len(samples),
        'first_shader_age_ms': samples[0]['shader_time_s'] * 1000,
        'last_shader_age_ms': samples[-1]['shader_time_s'] * 1000,
        'early': _pacing_distribution(intervals['early']),
        'late': _pacing_distribution(intervals['late']),
        'queue_to_callback': _pacing_distribution([sample['queued_to_callback_ms'] for sample in samples]),
        'initialize': _pacing_distribution([sample['initialize_ms'] for sample in samples]),
        'draw_submission': _pacing_distribution([sample['draw_submission_ms'] for sample in samples]),
        'upload_submission': _pacing_distribution([sample['upload_submission_ms'] for sample in samples]),
        'max_instances': max(sample['instances'] for sample in samples),
        'uploads': sum(bool(sample.get('uploaded')) for sample in samples),
        'ledger_dropped_before': prior.get('dropped'), 'ledger_dropped_after': current.get('dropped'),
        'samples': samples,
    }


def effects_pacing_assert_clock_consistent(summary):
    """Both clocks sample one Instant; this checks identity, never frame rate.

    Distinct epochs add a constant offset. Its variation should only reflect
    conversion of the rendered age to f32, so 1 ms is a generous allowance even
    when callbacks are delayed. Preserve the report before enforcing this rule.
    """
    variation = summary['shader_callback_offset_range_ms']
    if not math.isfinite(variation) or variation > 1:
        raise AssertionError(
            f'effects animation and callback clocks diverged by {variation:.3f} ms; '
            'this is a clock-consistency regression, not a frame-rate threshold')


def effects_pacing_observe(journey, perform):
    """Keep observation entirely outside the native effect's finite lifetime."""
    before = journey.snapshot()['document']
    started = time.monotonic()
    perform()
    producer_returned = time.monotonic()
    # No socket, screenshot, target lookup, or other repaint-producing request.
    time.sleep(.9)
    after = journey.snapshot()['document']
    return before, after, started, producer_returned, time.monotonic()


class EffectsPacingJourneys:
    def effects_pacing_settings(self):
        """Change only the real JSON editor, through native find/select/paste/save."""
        self.settings()
        if not self.snapshot()['document']['settings_json']['visible']:
            self.click('settings.json.mode')
        self.wait('effects pacing JSON settings opens', lambda d: d['settings_json']['visible'] and d['settings_json']['valid'])

        def find(text):
            self.key(3)
            self.wait_target('settings.json.find')
            self.key(0)
            self.native('text', text)
            self.wait('effects pacing JSON anchor selected', lambda d: d['settings_json']['query'] == text)
            self.key(36, 'none')
            self.wait_target('settings.json', focused=True)

        find('"editor_effects"')
        self.key(125, 'none')
        self.key(123, 'cmd')
        self.key(124, 'cmd+shift')
        before = self.snapshot()['document']['settings_json']['fingerprint']
        self.paste_source('    "enabled": true,', lambda: self.wait(
            'effects master JSON edit validates', lambda d: d['settings_json']['valid'] and d['settings_json']['fingerprint'] != before))
        find('"density": "Medium"')
        before = self.snapshot()['document']['settings_json']['fingerprint']
        self.paste_source('"density": "Low"', lambda: self.wait(
            'effects density JSON edit validates', lambda d: d['settings_json']['valid'] and d['settings_json']['fingerprint'] != before))
        snapshots = getattr(self, 'effects_preview_snapshots', True)
        find('"preview_zoom_snapshot": true')
        if not snapshots:
            before = self.snapshot()['document']['settings_json']['fingerprint']
            self.paste_source('"preview_zoom_snapshot": false', lambda: self.wait(
                'preview snapshot JSON edit validates', lambda d:
                d['settings_json']['valid'] and d['settings_json']['fingerprint'] != before))
            find('"preview_zoom_snapshot": false')
        # Copy the selected real JSON field, preserving the human clipboard.
        # The native preview below separately confirms the applied setting.
        self.copy_with_receipt('"preview_zoom_snapshot": ' + str(snapshots).lower(), lambda: self.key(8))
        self.click('settings.json.save')
        self.wait('effects pacing preferences apply', lambda d:
                  d['settings_json']['saved'] and d['editor_effects']['enabled'] and d['editor_effects']['density'] == 'Low')
        self.close_settings()
        self.effects_preview_snapshots_verified = snapshots

    def effects_pacing(self):
        from desktop_ui_journeys import fingerprint, typing_particle_gpu_state
        self.effects_pacing_settings()
        report = {'schema': 1, 'protocol': 'one native action; no inspection, screenshots or app requests until 0.9 seconds after the producer returns',
                  'first_bursts_separate': True, 'warm_bursts_per_action': 6,
                  'preview_zoom_snapshot': self.effects_preview_snapshots_verified, 'documents': []}
        report_path = self.evidence / 'effects-pacing.json'
        for suffix in ('txt', 'typ'):
            path = self.directory / ('effects-pacing.' + suffix)
            if self.snapshot()['document']['path'] != str(path):
                self.open_harper_fixture(path)
            expected = path.read_text()
            if suffix == 'typ':
                self.key(35, 'cmd+alt')
                self.wait('saved Typst fixture has a live Tinymist preview', lambda d:
                          d['preview_tab_id'] == d['active_tab_id'] and d['preview_path'] == str(path)
                          and d['preview_backend'] in ('Tinymist', 'Tinymist partial') and d['preview_native_ready']
                          and d.get('preview_zoom_overlay', {}).get('enabled') == self.effects_preview_snapshots_verified
                          and not d['compile_indicator_spinning'], timeout=60)
            self.click('editor.source')
            self.key(125)
            self.wait('saved pacing source owns the EOF caret', lambda d:
                      d['path'] == str(path) and d['editor_keyboard_focused'] and d['cursor'] == [len(expected), len(expected)])
            # Pointer-away avoids hover work while preserving keyboard ownership.
            self.native('move', 5, 5)
            time.sleep(1)
            initial = self.snapshot()
            effects_pacing_ledger(initial['document'])
            gpu = typing_particle_gpu_state(initial['document'])
            if not gpu['ready']:
                raise AssertionError('effects pacing renderer did not finish warming before input')
            case = {'suffix': suffix, 'fixture_sha256': hashlib.sha256(expected.encode()).hexdigest(),
                    'fixture_bytes': len(expected.encode()), 'fixture_lines': expected.count('\n') + 1,
                    'build': initial['build'], 'viewport': initial['document']['viewport'],
                    'native_size': initial['document'].get('native_size'), 'dark': initial['document']['dark'],
                    'preview_backend': initial['document']['preview_backend'],
                    'preview_native_ready': initial['document'].get('preview_native_ready'),
                    'preview_zoom_snapshot': self.effects_preview_snapshots_verified,
                    'preview_zoom_snapshot_native_enabled': initial['document'].get('preview_zoom_overlay', {}).get('enabled'),
                    'preferences': initial['document']['editor_effects'], 'bursts': []}
            report['documents'].append(case)

            def measured(action, index, perform):
                before, after, started, producer_returned, observed = effects_pacing_observe(self, perform)
                ledger = effects_pacing_ledger(before)
                previous = before['typing_particles']['burst_count']
                if after['source_fingerprint'] != fingerprint(expected) or not after['editor_keyboard_focused']:
                    raise AssertionError('native pacing action changed the wrong source or lost focus')
                if after['typing_particles']['burst_count'] != previous + 1 or after['typing_particles']['last_action'] != action:
                    raise AssertionError('native pacing action did not produce exactly one expected burst')
                if after['typing_particles']['active_count']:
                    raise AssertionError('finite native effects remained active after the observation-free interval')
                summary = effects_pacing_summary(before, after)
                summary.update(action=action, repetition=index, phase='first' if index == 0 else 'warmed',
                               producer_ms=(producer_returned - started) * 1000,
                               observation_free_ms=(observed - producer_returned) * 1000,
                               ledger_before_ms=ledger['now_ms'], source_bytes=after['source_bytes'])
                case['bursts'].append(summary)
                report_path.write_text(json.dumps(report, indent=2) + '\n')
                self.record('effects_pacing.burst', suffix=suffix, editor_action=action, repetition=index,
                            summary={key: value for key, value in summary.items() if key != 'samples'})
                effects_pacing_assert_clock_consistent(summary)

            for index in range(7):
                expected += 'x'
                measured('Typing', index, lambda: self.native('text', 'x'))
            self.key(123, 'shift')
            self.wait('Copy pacing selects one fixture character', lambda d:
                      d['cursor'] == [len(expected) - 1, len(expected)] or d['cursor'] == [len(expected), len(expected) - 1])
            for index in range(7):
                self.copy_with_receipt('x', lambda: measured('Copy', index, lambda: self.key(8)))
            if self.capture_review:
                self.capture_viewport('main')
        self.record('effects_pacing.complete', report=str(report_path), documents=len(report['documents']))
