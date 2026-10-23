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

    def test_missing_dialog_stops_before_path_typing_or_button_press(self):
        journey = TexJourneys()
        actions = []
        journey.focus_document_window = lambda: actions.append(('focus',))
        journey.key = lambda *args: actions.append(('key', *args))
        journey.native = lambda *args: actions.append(args) or dict(count=0, complete=True)
        with patch('time.monotonic', side_effect=[0, 0, 7]), patch('time.sleep'):
            with self.assertRaisesRegex(AssertionError, 'Native dialog button did not become ready'):
                journey.open_harper_fixture(Path('/tmp/private/prose.tex'))
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
                journey.open_harper_fixture(Path('/tmp/private/prose.tex'))
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
