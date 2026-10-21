"""Opened PDF tabs obey the same layout choices as source tabs."""
import hashlib
import time

ASSET_LAYOUT_JOURNEYS = ('pdf_layout',)


def pdf_fixture():
    content = b'BT /F1 20 Tf 30 130 Td (Opened PDF layout fixture) Tj ET\n'
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>',
               b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
               b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 360 180] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
               b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
               b'<< /Length ' + str(len(content)).encode() + b' >>\nstream\n' + content + b'endstream']
    result = bytearray(b'%PDF-1.4\n')
    offsets = []
    for index, obj in enumerate(objects, 1):
        offsets.append(len(result))
        result.extend(str(index).encode() + b' 0 obj\n' + obj + b'\nendobj\n')
    xref = len(result)
    result.extend(b'xref\n0 6\n0000000000 65535 f \n')
    for offset in offsets:
        result.extend(f'{offset:010d} 00000 n \n'.encode())
    result.extend(b'trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n' + str(xref).encode() + b'\n%%EOF\n')
    return bytes(result)


def pdf_layout_settled(document, path, mode, preview_id):
    expected = {'Code': 'Asset', 'Split': 'SplitAsset', 'Preview': 'Preview'}[mode]
    return (document['path'] == str(path) and document['view_mode'] == mode
            and document['workspace_content'] == expected
            and document['preview_surface_visible'] == (mode != 'Code')
            and document['preview_tab_id'] == preview_id
            and document['active_tab_id'] != preview_id
            and (mode == 'Preview' or document['asset_pdfium_ready']))


class AssetLayoutJourneys:
    def pdf_layout(self):
        self.focus_document_window()
        self.ensure_typesetting_tab()
        self.key(35, 'cmd+alt')
        self.wait('The source owns the designated preview', lambda d: d['preview_tab_id'] == d['active_tab_id'])
        self.key(20)
        self.choose_backend('InteractivePartial', save_pdf=False)
        before = self.snapshot()['document']
        preview_id = before['preview_tab_id']
        if preview_id != before['active_tab_id']:
            raise AssertionError('PDF layout requires a designated source tab')
        path = self.directory / 'opened-layout.pdf'
        encoded = pdf_fixture()
        path.write_bytes(encoded)
        self.key(31)
        time.sleep(0.8)
        self.key(5, 'cmd+shift')
        time.sleep(0.3)
        self.key(0)
        self.native('text', str(path))
        self.key(36, 'none')
        time.sleep(0.5)
        self.native('ax-press-title', 'Open')
        self.wait('The real PDF opens beside its designated source preview', lambda d:
                  pdf_layout_settled(d, path, 'Split', preview_id), timeout=30)
        for cycle in range(2):
            for mode in ('Code', 'Preview', 'Split'):
                if cycle == 0 and mode in ('Code', 'Preview'):
                    self.click('preview.open')
                    self.wait('Controls open for the viewer that is about to hide',
                              lambda d: d['preview_controls_visible'] is True)
                self.click('toolbar.layout')
                self.wait_target('layout.' + mode.lower())
                self.click('layout.' + mode.lower())
                settled = lambda d: pdf_layout_settled(d, path, mode, preview_id)
                self.wait('Opened PDF layout becomes ' + mode, settled, timeout=20)
                self.stable_state(settled)
                if cycle == 0 and mode in ('Code', 'Preview'):
                    self.wait('A hidden viewer cannot leave its controls over the visible viewer',
                              lambda d: not d['preview_controls_visible'])
                self.record('pdf.layout', mode=mode, cycle=cycle)
        for key, mode in [(19, 'Code'), (20, 'Split')]:
            self.key(key)
            self.wait('Layout shortcut works with a PDF tab', lambda d:
                      pdf_layout_settled(d, path, mode, preview_id), timeout=20)
        if path.read_bytes() != encoded:
            raise AssertionError('Changing PDF layout rewrote the PDF')
        self.focus_document_window()
        self.key(13)
        self.wait('Closing the PDF restores the original source and preview', lambda d:
                  d['active_tab_id'] == preview_id and d['path'] == before['path']
                  and d['preview_tab_id'] == preview_id
                  and d['source_fingerprint'] == before['source_fingerprint'])
        self.record('pdf.layout.complete', pdf_sha256=hashlib.sha256(encoded).hexdigest())
