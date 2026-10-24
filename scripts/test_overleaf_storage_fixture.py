"""Fixture transport checks; these are not native interaction evidence."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sys
import json
import unittest
from urllib.request import Request, urlopen
from urllib.error import HTTPError

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
            self.assertEqual(fixture.save_count, 0)
            urlopen(Request(origin + '/fixture/source', data=b'Updated fixture.', method='POST'), timeout=2).read()
            self.assertEqual(fixture.source, 'Updated fixture.')
            self.assertEqual(fixture.save_count, 1)
            collaborator = 'Remote collaborator changed this sentence. α\n'
            fixture.replace_remote(collaborator)
            self.assertEqual(fixture.source, collaborator)
            self.assertEqual(fixture.save_count, 1)
            self.assertEqual(urlopen(f'{origin}/project/{PROJECT}/doc/{DOCUMENT}/download',
                                     timeout=2).read(), collaborator.encode())
            self.assertIn(json.dumps(collaborator), fixture.javascript())

    def test_collaborator_edits_reject_unbounded_or_invalid_inputs_without_changing_server(self):
        fixture = StorageFixture()
        try:
            for invalid in (None, b'not text', 'α' * 32769):
                with self.subTest(invalid_type=type(invalid)), self.assertRaises(ValueError):
                    fixture.replace_remote(invalid)
                self.assertEqual(fixture.source, SOURCE)
                self.assertEqual(fixture.save_count, 0)
        finally:
            fixture.server.server_close()

    def test_server_connection_loss_reaches_only_the_current_frontend_transport(self):
        with StorageFixture(connection_events=True) as fixture:
            origin = fixture.url.rsplit('/project/', 1)[0]
            def connection(generation):
                try:
                    with urlopen(origin + '/fixture/connection?generation=' + str(generation),
                                 timeout=3) as response:
                        return response.status
                except HTTPError as error:
                    status = error.code
                    error.close()
                    return status
            self.assertIn('/fixture/connection?generation=0', fixture.javascript())
            with ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(connection, 0)
                self.assertTrue(fixture.connection_requested.wait(2))
                self.assertFalse(pending.done())
                self.assertEqual(fixture.lose_connection(), 1)
                self.assertEqual(pending.result(timeout=2), 503)
                # The old browser cannot become live again, while a newly
                # loaded official frontend watches only the new generation.
                self.assertEqual(connection(0), 503)
                self.assertIn('/fixture/connection?generation=1', fixture.javascript())
                fixture.connection_requested.clear()
                pending = pool.submit(connection, 1)
                self.assertTrue(fixture.connection_requested.wait(2))
                self.assertFalse(pending.done())
                fixture.lose_connection()
                self.assertEqual(pending.result(timeout=2), 503)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

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
