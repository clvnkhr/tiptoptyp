"""Failure detectors for PDF pane routing; not native interaction evidence."""
import importlib.util
from pathlib import Path
import re
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from desktop_ui_asset_layout_journeys import pdf_fixture, pdf_layout_settled


class PdfLayoutContracts(unittest.TestCase):
    def test_fixture_has_valid_pdf_offsets(self):
        data = pdf_fixture()
        self.assertTrue(data.startswith(b'%PDF-1.4'))
        xref = int(re.search(rb'startxref\n(\d+)', data)[1])
        self.assertEqual(data[xref:xref + 4], b'xref')
        offsets = re.findall(rb'(\d{10}) 00000 n', data)
        self.assertEqual(len(offsets), 5)
        for number, offset in enumerate(offsets, 1):
            self.assertTrue(data[int(offset):].startswith(f'{number} 0 obj'.encode()))

    def test_open_pdf_layout_requires_rendering_and_preserves_pinned_source(self):
        for mode, content in [('Code', 'Asset'), ('Preview', 'Preview'), ('Split', 'SplitAsset')]:
            d = dict(path='/opened.pdf', view_mode=mode, workspace_content=content,
                     preview_surface_visible=mode != 'Code', preview_tab_id=1, active_tab_id=2,
                     asset_pdfium_ready=True)
            self.assertTrue(pdf_layout_settled(d, Path('/opened.pdf'), mode, 1))
            for field, wrong in [('path', '/paper.typ'), ('view_mode', 'invalid'),
                                 ('workspace_content', 'Source'),
                                 ('preview_surface_visible', mode == 'Code'), ('preview_tab_id', 2), ('active_tab_id', 1)]:
                self.assertFalse(pdf_layout_settled({**d, field: wrong}, Path('/opened.pdf'), mode, 1))
            if mode != 'Preview':
                self.assertFalse(pdf_layout_settled({**d, 'asset_pdfium_ready': False}, Path('/opened.pdf'), mode, 1))

    def test_journey_is_registered_once_and_in_aggregate(self):
        spec = importlib.util.spec_from_file_location('desktop_runner_pdf_contract', Path(__file__).with_name('test-desktop-ui.py'))
        runner = importlib.util.module_from_spec(spec); spec.loader.exec_module(runner)
        self.assertEqual(runner.JOURNEY_NAMES.count('pdf_layout'), 1)
        self.assertEqual(runner.selected_journeys('all').count('pdf_layout'), 1)


if __name__ == '__main__': unittest.main()
