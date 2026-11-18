"""Failure detectors for native comment controls; no UI interaction is simulated."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_journeys import fingerprint
from desktop_ui_comment_controls_journeys import (
    COMMENT_CONTROLS_JOURNEYS, COMMENT_MARKER_TARGETS, COMMENT_SETTINGS_QUERIES,
    CUSTOM_COMMENT_MARKERS, DEFAULT_COMMENT_MARKERS, CommentControlsJourneys, comment_diagnostic_fixture,
    comment_format_fixture, comment_formatted, comment_marker_body, comment_marker_lines,
    comment_markers_applied, comment_diagnostic_result, comment_settings_fields_ready,
    assert_comment_settings_preserved,
)


def marker_observation(markers):
    return {'comment_controls': {
        group: {switch + '_fingerprint': fingerprint(value)
                for switch, value in zip(('off', 'on'), pair)}
        for group, pair in markers.items()}}


class CommentControlsContracts(unittest.TestCase):
    def fresh_marker_fields(self):
        return {'targets': {name: dict(enabled=True, age_ms=0, width=400, height=20)
                            for name in COMMENT_MARKER_TARGETS}}

    def test_settings_discovery_is_a_focused_registered_journey_with_literal_comment_queries(self):
        self.assertIn('comment_controls_settings', COMMENT_CONTROLS_JOURNEYS)
        self.assertEqual(COMMENT_SETTINGS_QUERIES,
                         ('ttt:off', 'diag: off/on', '%fmt:on', '//diag:off'))
        self.assertEqual(len(set(COMMENT_MARKER_TARGETS)), 6)

    def test_settings_discovery_requires_all_six_current_visible_editable_fields(self):
        good = self.fresh_marker_fields()
        self.assertTrue(comment_settings_fields_ready(good))
        for name in COMMENT_MARKER_TARGETS:
            missing = deepcopy(good)
            missing['targets'].pop(name)
            self.assertFalse(comment_settings_fields_ready(missing))
            for field, value in (('enabled', False), ('enabled', 1), ('age_ms', 501),
                                 ('age_ms', -1), ('age_ms', True), ('age_ms', float('nan')),
                                 ('width', 0), ('height', -1), ('width', True),
                                 ('height', float('inf')), ('width', float('nan'))):
                changed = deepcopy(good)
                changed['targets'][name][field] = value
                with self.subTest(name=name, field=field, value=value):
                    self.assertFalse(comment_settings_fields_ready(changed))
        for bad in ({}, {'targets': None}, {'targets': []}):
            self.assertFalse(comment_settings_fields_ready(bad))

    def settings_document(self):
        document = self.owner('This is unchanged.\n')
        document.update(marker_observation(DEFAULT_COMMENT_MARKERS), revision=3,
                        dirty=False, cursor=[2, 2])
        return document

    def test_settings_discovery_preserves_source_selection_preview_and_marker_values(self):
        before = self.settings_document()
        assert_comment_settings_preserved(deepcopy(before), before)
        for field, value in (('path', '/private/tmp/another.typ'), ('source_fingerprint', fingerprint('changed')),
                             ('source_bytes', 0), ('revision', 4), ('dirty', True), ('cursor', [0, 0]),
                             ('active_tab_id', 1), ('preview_tab_id', 1), ('tab_order', [7, 1]),
                             ('workspace_root_fingerprint', fingerprint('/elsewhere'))):
            changed = deepcopy(before)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                assert_comment_settings_preserved(changed, before)
        changed = deepcopy(before)
        changed['comment_controls']['diagnostics']['off_fingerprint'] = fingerprint('different')
        with self.assertRaises(AssertionError):
            assert_comment_settings_preserved(changed, before)
        for field in ('source_fingerprint', 'source_bytes', 'revision', 'dirty', 'cursor'):
            changed = deepcopy(before)
            del changed[field]
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                assert_comment_settings_preserved(changed, before)

    def diagnostic_transition(self, *, protected=True):
        from test_desktop_ui_suppression_contract import observation
        pair = DEFAULT_COMMENT_MARKERS['both' if protected else 'formatting']
        _, source, locations = comment_diagnostic_fixture('tex', pair)
        _, path, _, _, document = observation('tex', metadata=True)
        visible = locations[:2] + locations[4:] if protected else locations
        # Retain the exact twelve raw Harper locations from run-4o63wfyi:
        # six body findings and six configured-marker metadata findings.
        raw = [[3, 26], [4, 9], [5, 2], [5, 2], [6, 26], [7, 9],
               [8, 2], [8, 12], [8, 12], [8, 15], [9, 26], [10, 9]]
        rows = [{'index': index, 'provider': 'Harper', 'line': line, 'column': column,
                 'suppressed': line in (5, 8) or [line, column] not in visible}
                for index, (line, column) in enumerate(raw)]
        document.update(source_fingerprint=fingerprint(source), source_bytes=len(source.encode()),
                        writing_count=len(raw), writing_locations=raw,
                        writing_lines=[line for line, _ in raw], writing_paths=[str(path)] * len(raw),
                        diagnostic_rows=rows, raw_diagnostic_count=len(rows),
                        ltex={'enabled': True, 'current': False, 'state': 'Working',
                              'count': 0, 'locations': [], 'rules': [], 'paths': []})
        self.settle_layout(document)
        pending = deepcopy(document)
        document['ltex'] = {'enabled': True, 'current': True, 'state': 'Up to date / idle',
                            'count': len(locations), 'locations': deepcopy(locations),
                            'rules': ['MORFOLOGIK_RULE_EN_GB', 'ENGLISH_WORD_REPEAT_RULE'] * 3,
                            'paths': [str(path)] * len(locations)}
        document['diagnostic_rows'] += [
            {'index': len(raw) + index, 'provider': 'LTeX+', 'line': line, 'column': column,
             'suppressed': [line, column] not in visible}
            for index, (line, column) in enumerate(locations)]
        document['raw_diagnostic_count'] = len(document['diagnostic_rows'])
        document['diagnostic_count'] = sum(not row['suppressed'] for row in document['diagnostic_rows'])
        published = deepcopy(document)
        self.settle_layout(document)
        return source, path, locations, visible, pending, published, document

    def settle_layout(self, document):
        rows = document['diagnostic_rows']
        indices = [row['index'] for row in rows if not row['suppressed']]
        document['diagnostic_count'] = len(indices)
        layout = document['problems']
        layout.update(visible_indices=indices, visible_count=len(indices),
                      displayed_count=len(indices), total_count=len(indices))
        layout['groups'] = [
            {'provider': provider, 'type': 'Help', 'count': count, 'open': True,
             'target': 'problems.group.' + provider + '.Help'}
            for provider in ('Harper', 'LTeX+')
            if (count := sum(row['provider'] == provider and not row['suppressed'] for row in rows))]

    def test_diagnostics_wait_for_both_providers_and_their_complete_painted_layout(self):
        source, path, locations, visible, pending, published, settled = self.diagnostic_transition()
        check = lambda document: comment_diagnostic_result(
            document, source, path, locations, visible, markers=DEFAULT_COMMENT_MARKERS)
        self.assertEqual(pending['writing_count'], 12)
        self.assertEqual(pending['ltex']['count'], 0)
        self.assertFalse(check(pending), 'current Harper alone must not authorize stability')
        self.assertEqual(published['raw_diagnostic_count'], 18)
        self.assertEqual(published['problems']['visible_count'], 4)
        self.assertEqual(published['diagnostic_count'], 8)
        self.assertFalse(check(published), 'a preceding Problems layout is not the current full collection')
        self.assertTrue(check(settled))

    def test_ltex_requires_actual_enabled_current_source_paths_and_exact_raw_body_locations(self):
        source, path, locations, visible, _, _, document = self.diagnostic_transition()
        check = lambda value: comment_diagnostic_result(
            value, source, path, locations, visible, markers=DEFAULT_COMMENT_MARKERS)
        for field, value in (('enabled', False), ('current', False),
                             ('paths', ['/tmp/other.tex'] * 6),
                             ('locations', locations[:4])):
            changed = deepcopy(document); changed['ltex'][field] = value
            with self.subTest(field=field):
                try:
                    accepted = check(changed)
                except AssertionError:
                    accepted = False
                self.assertFalse(accepted)
        changed = deepcopy(document)
        changed['ltex']['locations'][0] = [2, 26]
        row = next(row for row in changed['diagnostic_rows'] if row['provider'] == 'LTeX+')
        row['line'] = 2
        self.settle_layout(changed)
        self.assertFalse(check(changed), 'unexpected raw body lines cannot become expected findings')
        for field, value in (('count', True), ('count', 5), ('rules', []), ('current', 1)):
            changed = deepcopy(document); changed['ltex'][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                check(changed)

    def test_ltex_body_suppression_is_independent_for_formatting_and_diagnostic_channels(self):
        for protected in (True, False):
            source, path, locations, visible, _, _, document = self.diagnostic_transition(protected=protected)
            check = lambda value: comment_diagnostic_result(
                value, source, path, locations, visible, markers=DEFAULT_COMMENT_MARKERS)
            self.assertTrue(check(document))
            middle = next(row for row in document['diagnostic_rows']
                          if row['provider'] == 'LTeX+' and row['line'] == 6)
            middle['suppressed'] = not protected
            self.settle_layout(document)
            self.assertFalse(check(document), 'LTeX body suppression must match the selected channel')

    def test_ltex_marker_metadata_must_remain_raw_and_hidden_with_consistent_layout(self):
        source, path, locations, visible, _, _, document = self.diagnostic_transition()
        check = lambda value: comment_diagnostic_result(
            value, source, path, locations, visible, markers=DEFAULT_COMMENT_MARKERS)
        row = {'index': len(document['diagnostic_rows']), 'provider': 'LTeX+',
               'line': 5, 'column': 2, 'suppressed': True}
        document['diagnostic_rows'].append(row)
        document['raw_diagnostic_count'] += 1
        ltex = document['ltex']
        ltex['count'] += 1
        ltex['locations'].append([5, 2])
        ltex['rules'].append('METADATA_FINDING')
        ltex['paths'].append(str(path))
        self.settle_layout(document)
        self.assertTrue(check(document))
        row['suppressed'] = False
        self.settle_layout(document)
        self.assertFalse(check(document), 'metadata from LTeX must stay hidden even when all counts agree')

    def test_async_settings_search_does_not_authorize_an_unpainted_first_click(self):
        owner = self
        class FirstClickObserved(Exception):
            pass
        class Readiness(CommentControlsJourneys):
            def __init__(self):
                self.ready = set()
            def settings(self):
                pass
            def native(self, command, *arguments):
                owner.assertEqual((command, arguments), ('resize-focused', (700, 900)))
            def setting_section(self, query, title):
                owner.assertEqual((query, title), ('comment controls', 'Comment controls'))
                # The search command returned, but its deferred category and
                # scroll frames have not painted an actionable control yet.
                self.ready.clear()
            def wait_target(self, target, focused=False):
                self.ready.add(target)
            def click(self, target):
                owner.assertIn(target, self.ready, 'native click used an unpainted Settings target')
                raise FirstClickObserved(target)
        for operation, first in (
                (lambda journey: journey.comment_marker_settings(DEFAULT_COMMENT_MARKERS, reset=True),
                 'settings.comments.reset'),
                (lambda journey: journey.comment_marker_invalid_draft(),
                 'settings.comments.formatting.off')):
            with self.subTest(first=first), self.assertRaisesRegex(FirstClickObserved, first):
                operation(Readiness())

    def test_all_three_effective_pairs_are_required_and_independently_customizable(self):
        for markers in (DEFAULT_COMMENT_MARKERS, CUSTOM_COMMENT_MARKERS):
            document = marker_observation(markers)
            self.assertTrue(comment_markers_applied(document, markers))
            for group in markers:
                for switch in ('off', 'on'):
                    changed = deepcopy(document)
                    changed['comment_controls'][group][switch + '_fingerprint'] = fingerprint('different')
                    self.assertFalse(comment_markers_applied(changed, markers))
        self.assertFalse(comment_markers_applied(marker_observation(DEFAULT_COMMENT_MARKERS),
                                                CUSTOM_COMMENT_MARKERS))
        self.assertEqual(len({value for pair in CUSTOM_COMMENT_MARKERS.values() for value in pair}), 6)

    def test_settings_receipts_require_exact_entered_spacing_despite_equivalent_source_markers(self):
        compact = {group: tuple(comment_marker_body(value) for value in pair)
                   for group, pair in CUSTOM_COMMENT_MARKERS.items()}
        self.assertNotEqual(CUSTOM_COMMENT_MARKERS, compact)
        self.assertTrue(comment_markers_applied(marker_observation(CUSTOM_COMMENT_MARKERS),
                                               CUSTOM_COMMENT_MARKERS))
        self.assertFalse(comment_markers_applied(marker_observation(compact), CUSTOM_COMMENT_MARKERS))
        for suffix, prefix in (('typ', '//'), ('tex', '%')):
            for settings in (DEFAULT_COMMENT_MARKERS, CUSTOM_COMMENT_MARKERS):
                for pair in settings.values():
                    off, on = comment_marker_lines(suffix, pair)
                    self.assertTrue(off.startswith(prefix + comment_marker_body(pair[0])))
                    self.assertNotEqual(on[len(prefix):].strip(), pair[1])
                    self.assertIn('\t', on)
                    self.assertEqual(tuple(comment_marker_body(line[len(prefix):]) for line in (off, on)),
                                     tuple(comment_marker_body(value) for value in pair))

    def test_missing_partial_or_malformed_effective_settings_cannot_look_applied(self):
        good = marker_observation(DEFAULT_COMMENT_MARKERS)
        for document in ({}, {'comment_controls': None}, {'comment_controls': {}},
                         {'comment_controls': {**good['comment_controls'], 'legacy': {}}}):
            with self.subTest(document=document), self.assertRaises(AssertionError):
                comment_markers_applied(document, DEFAULT_COMMENT_MARKERS)
        for group in DEFAULT_COMMENT_MARKERS:
            for value in (None, {}, {'off_fingerprint': 'not-a-fingerprint'},
                          {'off_fingerprint': True, 'on_fingerprint': fingerprint('on')}):
                document = deepcopy(good)
                document['comment_controls'][group] = value
                with self.subTest(group=group, value=value), self.assertRaises(AssertionError):
                    comment_markers_applied(document, DEFAULT_COMMENT_MARKERS)

    def test_protected_formatter_fixtures_change_both_outer_lines_and_keep_exact_control_bytes(self):
        for suffix in ('typ', 'tex'):
            for pair in (DEFAULT_COMMENT_MARKERS['both'], CUSTOM_COMMENT_MARKERS['formatting']):
                original, expected = comment_format_fixture(suffix, pair)
                self.assertNotEqual(original, expected)
                start, stop = comment_marker_lines(suffix, pair)
                block = lambda source: source[source.index(start):source.index(stop) + len(stop)]
                self.assertEqual(block(original), block(expected))
                self.assertNotEqual(original.split(start)[0], expected.split(start)[0])
                self.assertNotEqual(original.split(stop)[1], expected.split(stop)[1])
                self.assertEqual(original.count(start), 1)
                self.assertEqual(original.count(stop), 1)
            original, expected = comment_format_fixture(suffix, CUSTOM_COMMENT_MARKERS['diagnostics'],
                                                        protected=False)
            start, stop = comment_marker_lines(suffix, CUSTOM_COMMENT_MARKERS['diagnostics'])
            self.assertNotEqual(block(original), block(expected), 'diagnostic markers must not stop formatting')
        with self.assertRaises(ValueError):
            comment_format_fixture('txt', DEFAULT_COMMENT_MARKERS['both'])

    def test_diagnostic_fixtures_preserve_exact_positions_for_every_default_and_custom_channel(self):
        for suffix, offset in (('typ', 0), ('tex', 2)):
            for markers in (DEFAULT_COMMENT_MARKERS, CUSTOM_COMMENT_MARKERS):
                for pair in markers.values():
                    original, active, locations = comment_diagnostic_fixture(suffix, pair)
                    off, on = comment_marker_lines(suffix, pair)
                    self.assertEqual(original.replace(on, off, 1), active)
                    self.assertEqual(original.count(on), 2)
                    self.assertEqual(active.count(off), 1)
                    self.assertEqual(locations, [[line + offset, column] for line, column in
                                                 ((1, 26), (2, 9), (4, 26), (5, 9), (7, 26), (8, 9))])
                    for line, column in locations:
                        self.assertIn('misspelld' if column == 26 else 'the the',
                                      active.splitlines()[line - 1])

    def owner(self, source):
        return {'path': '/private/tmp/comments.typ', 'tab_order': [1, 7], 'tabs': 2,
                'active_tab_id': 7, 'preview_tab_id': 7,
                'workspace_root_fingerprint': fingerprint('/private/tmp'),
                'source_fingerprint': fingerprint(source), 'source_bytes': len(source.encode()),
                'modal_open': False}

    def test_formatter_detector_requires_changed_outer_source_and_exact_protected_bytes(self):
        original, expected = comment_format_fixture('typ', DEFAULT_COMMENT_MARKERS['both'])
        before, result = self.owner(original), self.owner(expected)
        self.assertTrue(comment_formatted(result, before, expected))
        self.assertFalse(comment_formatted(before, before, expected), 'an unchanged buffer is not formatting evidence')
        for changes in (
                {'source_fingerprint': fingerprint(expected.replace('4,  5,6', '4, 5, 6'))},
                {'source_bytes': 0}, {'active_tab_id': 1}, {'preview_tab_id': 1},
                {'tab_order': [7, 1]}, {'tabs': 3}, {'path': '/private/tmp/another.typ'},
                {'workspace_root_fingerprint': fingerprint('/elsewhere')}, {'modal_open': True}):
            with self.subTest(changes=changes):
                try:
                    outcome = comment_formatted({**result, **changes}, before, expected)
                except AssertionError:
                    outcome = False
                self.assertFalse(outcome)

    def test_formatter_detector_rejects_incomplete_source_and_tab_identity(self):
        original, expected = comment_format_fixture('typ', DEFAULT_COMMENT_MARKERS['both'])
        before, result = self.owner(original), self.owner(expected)
        for field in result:
            broken = dict(result)
            broken.pop(field)
            with self.subTest(missing=field), self.assertRaises(AssertionError):
                comment_formatted(broken, before, expected)
        for changes in ({'source_bytes': True}, {'source_fingerprint': 'stale'},
                        {'active_tab_id': True}, {'tab_order': [7, 7]},
                        {'modal_open': 0}, {'workspace_root_fingerprint': None}):
            with self.subTest(changes=changes), self.assertRaises(AssertionError):
                comment_formatted({**result, **changes}, before, expected)


if __name__ == '__main__':
    unittest.main()
