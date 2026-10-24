"""Fixture transport checks; these are not native interaction evidence."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sys
import unittest
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overleaf_storage_fixture import ASSET, ASSET_BYTES, DOCUMENT, PROJECT, SOURCE, StorageFixture


class StorageFixtureTests(unittest.TestCase):
    def test_loopback_routes_stream_fresh_source_and_hold_assets_until_explicit_release(self):
        with StorageFixture() as fixture:
            self.assertTrue(fixture.url.startswith('http://127.0.0.1:'))
            self.assertIn(b'/fixture.js', urlopen(fixture.url, timeout=2).read())
            origin = fixture.url.rsplit('/project/', 1)[0]
            source = urlopen(f'{origin}/project/{PROJECT}/doc/{DOCUMENT}/download', timeout=2).read()
            self.assertEqual(source, SOURCE.encode())
            self.assertTrue(fixture.document_requested.is_set())
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(lambda: urlopen(f'{origin}/project/{PROJECT}/file/{ASSET}', timeout=3).read())
                self.assertTrue(fixture.asset_requested.wait(2))
                self.assertFalse(pending.done())
                fixture.release_assets.set()
                self.assertEqual(pending.result(timeout=2), ASSET_BYTES)
            urlopen(Request(origin + '/fixture/source', data=b'Updated fixture.', method='POST'), timeout=2).read()
            self.assertEqual(fixture.source, 'Updated fixture.')

    def test_frontend_uses_official_bridge_surfaces_without_native_ipc_shortcuts(self):
        fixture = StorageFixture()
        try:
            script = fixture.javascript()
            for text in ('window.io.connect', 'window.overleaf', 'joinProjectResponse',
                         "'doc:saved'", "'otUpdateApplied'", "'applyOtUpdate'"):
                self.assertIn(text, script)
            for text in ('window.ipc', 'postMessage', 'tiptoptypOverleaf.command'):
                self.assertNotIn(text, script)
        finally:
            fixture.server.server_close()
