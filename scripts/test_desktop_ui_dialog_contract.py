"""Native dialog readiness guards; unit tests never count as desktop evidence."""
from copy import deepcopy
from pathlib import Path
import unittest
from unittest.mock import patch

from desktop_ui_tex_journeys import TexJourneys, native_button_ready, native_path_entry_ready


class NativeDialogContract(unittest.TestCase):
    def receipt(self, title='Open', enabled=True):
        return dict(requested_title=title, title=title, role='AXButton', count=1,
                    complete=True, enabled=enabled)

    def test_button_requires_complete_unique_exact_enabled_native_evidence(self):
        receipt = self.receipt()
        self.assertTrue(native_button_ready(receipt, 'Open'))
        for field, value in (('requested_title', 'Save'), ('title', 'Save'),
                             ('role', 'AXTextField'), ('count', 0), ('count', 2),
                             ('count', True), ('complete', False), ('enabled', False),
                             ('enabled', 1)):
            changed = deepcopy(receipt)
            changed[field] = value
            with self.subTest(field=field, value=value):
                self.assertFalse(native_button_ready(changed, 'Open'))
        for absent in (None, {}, []):
            self.assertFalse(native_button_ready(absent, 'Open'))

    def test_open_dialog_presence_can_precede_an_enabled_file_selection(self):
        receipt = self.receipt(enabled=False)
        self.assertTrue(native_button_ready(receipt, 'Open', require_enabled=False))
        self.assertFalse(native_button_ready(receipt, 'Open'))
        receipt['count'] = 2
        self.assertFalse(native_button_ready(receipt, 'Open', require_enabled=False))

    def test_ready_open_uses_the_exact_path_without_fixed_sleep_or_asset_source_focus(self):
        path = Path('/private/tmp/native-fixture/document with spaces.tex')
        for focus_source in (False, True):
            with self.subTest(focus_source=focus_source):
                journey = TexJourneys()
                journey.viewport = '"FFFF"'
                journey.multiple_windows = False
                actions = []
                journey.focus_document_window = lambda: actions.append(('focus',))
                journey.key = lambda *args: actions.append(('key', *args))
                journey.click = lambda *args: actions.append(('click', *args))

                def native(*args):
                    actions.append(args)
                    if args[0] == 'ax-button-title':
                        return self.receipt(args[1])
                    if args[0] == 'ax-dialog-state':
                        return dict(complete=True, focused_ancestry=[], focused_window=dict(
                            role='AXSheet', identifier='GoToWindow'))

                journey.native = native
                opened = {'document': {'path': str(path)}}
                focused = {'document': {'path': str(path), 'editor_keyboard_focused': True}}

                def wait(description, predicate, **kwargs):
                    actions.append(('verified', description))
                    if description == 'native file opened':
                        self.assertEqual(kwargs, {'timeout': 15})
                        self.assertFalse(predicate({'path': None}))
                        self.assertFalse(predicate({'path': str(path.parent)}))
                        self.assertTrue(predicate(opened['document']))
                        return opened
                    self.assertEqual(description, 'opened source owns keyboard')
                    self.assertTrue(focus_source)
                    self.assertFalse(predicate({'path': str(path), 'editor_keyboard_focused': False}))
                    self.assertFalse(predicate({'path': str(path.parent), 'editor_keyboard_focused': True}))
                    self.assertTrue(predicate(focused['document']))
                    return focused

                journey.wait = wait
                with patch('time.sleep', side_effect=AssertionError('ready Open must not sleep')):
                    result = journey.open_native_file(path, focus_source=focus_source)
                expected = [
                    ('focus',), ('key', 31), ('ax-button-title', 'Open'),
                    ('key', 5, 'cmd+shift'), ('ax-dialog-state',), ('key', 0),
                    ('text', str(path)), ('key', 36, 'none'),
                    ('ax-button-title', 'Open'), ('ax-press-title', 'Open'),
                    ('verified', 'native file opened'),
                ]
                if focus_source:
                    expected.extend([('click', 'editor.source'), ('verified', 'opened source owns keyboard')])
                self.assertEqual(actions, expected)
                self.assertIs(result, focused if focus_source else opened)
                self.assertEqual(journey.viewport, '"FFFF"')
                self.assertFalse(journey.multiple_windows)

    def test_save_waits_for_the_panel_and_filename_without_fixed_sleep(self):
        journey = TexJourneys()
        destination = Path('/private/tmp/native-fixture/saved.arbitrary')
        actions = []
        filename_set = False
        journey.key = lambda *args: actions.append(('key', *args))

        def native(*args):
            nonlocal filename_set
            actions.append(args)
            if args[0] == 'ax-button-title':
                return self.receipt(args[1], enabled=filename_set)
            if args[0] == 'ax-dialog-state':
                return dict(complete=True, focused_ancestry=[], focused_window=dict(
                    role='AXSheet', identifier='GoToWindow'))
            if args[0] == 'ax-set-text':
                filename_set = True

        journey.native = native
        with patch('time.sleep', side_effect=AssertionError('ready Save must not sleep')):
            journey.save_to_fixture(destination)
        self.assertEqual(actions, [
            ('key', 1), ('ax-button-title', 'Save'), ('key', 5, 'cmd+shift'),
            ('ax-dialog-state',), ('key', 0), ('text', str(destination.parent)),
            ('key', 36, 'none'), ('ax-button-title', 'Save'),
            ('ax-set-text', destination.name), ('ax-button-title', 'Save'), ('ax-press-title', 'Save'),
        ])

    def test_disabled_save_never_receives_a_confirmation(self):
        journey = TexJourneys()
        actions = []
        journey.key = lambda *args: actions.append(('key', *args))

        def native(*args):
            actions.append(args)
            if args[0] == 'ax-button-title':
                return self.receipt(args[1], enabled=False)
            if args[0] == 'ax-dialog-state':
                return dict(complete=True, focused_ancestry=[], focused_window=dict(
                    role='AXSheet', identifier='GoToWindow'))

        journey.native = native
        with patch('time.monotonic', side_effect=[0, 0, 0, 0, 0, 0, 0, 0, 7]), patch('time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'Native dialog button did not become ready'):
                journey.save_to_fixture(Path('/private/tmp/native-fixture/saved.tex'))
        self.assertIn(('ax-set-text', 'saved.tex'), actions)
        self.assertNotIn(('ax-press-title', 'Save'), actions)

    def test_missing_dialog_stops_before_path_typing_or_button_press(self):
        journey = TexJourneys()
        actions = []
        journey.focus_document_window = lambda: actions.append(('focus',))
        journey.key = lambda *args: actions.append(('key', *args))
        journey.native = lambda *args: actions.append(args) or dict(count=0, complete=True)
        with patch('time.monotonic', side_effect=[0, 0, 7]), patch('time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'Native dialog button did not become ready'):
                journey.open_native_file(Path('/tmp/private/prose.tex'))
        self.assertEqual(actions, [('focus',), ('key', 31), ('ax-button-title', 'Open')])

    def test_disabled_selected_file_never_receives_open_press(self):
        journey = TexJourneys()
        actions = []
        journey.focus_document_window = lambda: actions.append(('focus',))
        journey.key = lambda *args: actions.append(('key', *args))
        def native(*args):
            actions.append(args)
            if args[0] == 'ax-button-title':
                return self.receipt(args[1], enabled=False)
            if args[0] == 'ax-dialog-state':
                return dict(complete=True, focused_ancestry=[],
                            focused_window=dict(role='AXSheet', identifier='GoToWindow'))
        journey.native = native
        # Observe Open, then the real GoToWindow sheet, then disabled Open until deadline.
        with patch('time.monotonic', side_effect=[0, 0, 0, 0, 0, 0, 7]), patch('time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'Native dialog button did not become ready'):
                journey.open_native_file(Path('/tmp/private/prose.tex'))
        self.assertEqual(actions[-1], ('ax-button-title', 'Open'))
        self.assertNotIn(('ax-press-title', 'Open'), actions)
        self.assertIn(('text', '/tmp/private/prose.tex'), actions)


    def test_actual_go_to_folder_sheet_does_not_require_nonexistent_button_or_focus_flag(self):
        receipt = dict(complete=True, focused_ancestry=[], focused_window=dict(
            identifier='GoToWindow', role='AXSheet', title='', enabled=False, focused=False))
        self.assertTrue(native_path_entry_ready(receipt))
        for key, value in (('identifier', 'OpenPanel'), ('role', 'AXWindow')):
            changed = deepcopy(receipt)
            changed['focused_window'][key] = value
            self.assertFalse(native_path_entry_ready(changed))
        for key, value in (('complete', False), ('focused_ancestry', None),
                           ('focused_window', None)):
            changed = deepcopy(receipt)
            changed[key] = value
            self.assertFalse(native_path_entry_ready(changed))
        for missing in (None, {}, []):
            self.assertFalse(native_path_entry_ready(missing))

    def test_available_path_input_focus_must_belong_to_the_go_to_folder_sheet(self):
        sheet = dict(identifier='GoToWindow', role='AXSheet')
        receipt = dict(complete=True, focused_window=sheet,
                       focused_ancestry=[dict(role='AXTextField'), sheet])
        self.assertTrue(native_path_entry_ready(receipt))
        for ancestry in ([dict(role='AXButton'), sheet],
                         [dict(role='AXTextArea'), dict(role='AXWindow', identifier='editor')],
                         [None, sheet]):
            receipt['focused_ancestry'] = ancestry
            self.assertFalse(native_path_entry_ready(receipt))


if __name__ == '__main__':
    unittest.main()
