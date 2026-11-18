"""Offline file API contracts; real-app journeys separately prove UI actions."""
import json
import random
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from urllib.error import HTTPError
from urllib.request import Request, urlopen

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overleaf_files_fixture import CSRF, DOCUMENT, PROJECT, ROOT, SOURCE, ASSET
from overleaf_storage_fixture import StorageFixture


class FileFixtureTests(unittest.TestCase):
    def request(self, fixture, suffix, value=None, method='POST', csrf=True):
        origin = fixture.url.rsplit('/project/', 1)[0]
        data = json.dumps(value).encode() if value is not None else b''
        request = Request(origin + '/project/' + PROJECT + suffix, data=data, method=method,
                          headers={'Content-Type': 'application/json', **({'X-Csrf-Token': CSRF} if csrf else {})})
        with urlopen(request, timeout=2) as response:
            return json.loads(response.read())

    def test_server_authority_creates_moves_renames_and_deletes_exact_id(self):
        with StorageFixture(file_actions=True) as fixture:
            folder = self.request(fixture, '/folder', dict(parent_folder_id=ROOT, name='notes'))
            doc = self.request(fixture, '/doc', dict(parent_folder_id=folder['_id'], name='new.tex'))
            self.assertEqual(fixture.files.entry('notes/new.tex')['id'], doc['_id'])
            self.assertEqual(fixture.files.source(doc['_id']), '')
            origin = fixture.url.rsplit('/project/', 1)[0]
            urlopen(Request(origin + '/fixture/source/' + doc['_id'], data=b'Native edited source.', method='POST'), timeout=2).read()
            self.assertEqual(fixture.files.source(doc['_id']), 'Native edited source.')
            self.request(fixture, '/doc/' + doc['_id'] + '/rename', dict(name='renamed.tex'))
            self.assertIsNone(fixture.files.entry('notes/new.tex'))
            self.assertEqual(fixture.files.entry('notes/renamed.tex')['source'], 'Native edited source.')
            self.request(fixture, '/doc/' + doc['_id'] + '/move', dict(folder_id=ROOT))
            self.assertIsNone(fixture.files.entry('notes/renamed.tex'))
            self.assertEqual(fixture.files.entry('renamed.tex')['id'], doc['_id'])
            self.request(fixture, '/folder/' + folder['_id'], method='DELETE')
            self.assertIsNone(fixture.files.entry('notes'))
            self.assertEqual(fixture.files.entry('renamed.tex')['id'], doc['_id'])
            self.request(fixture, '/doc/' + doc['_id'], method='DELETE')
            self.assertIsNone(fixture.files.entry('renamed.tex'))
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 1)

    def test_csrf_collisions_and_parent_cycles_do_not_mutate_authoritative_files(self):
        with StorageFixture(file_actions=True) as fixture:
            for value, csrf in [(dict(parent_folder_id=ROOT, name='unauthorized.tex'), False),
                                (dict(parent_folder_id=ROOT, name='main.txt'), True),
                                (dict(parent_folder_id=ROOT, name='../bad.tex'), True),
                                (dict(parent_folder_id='f' * 24, name='bad.tex'), True)]:
                with self.subTest(value=value), self.assertRaises(HTTPError) as caught:
                    self.request(fixture, '/doc', value, csrf=csrf)
                caught.exception.close()
            folder = self.request(fixture, '/folder', dict(parent_folder_id=ROOT, name='notes'))
            with self.assertRaises(HTTPError) as caught:
                self.request(fixture, '/folder/' + folder['_id'] + '/move', dict(folder_id=folder['_id']))
            caught.exception.close()
            self.assertEqual(fixture.files.entry('notes')['parent'], ROOT)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(len(fixture.files.operations), 1)
            self.assertEqual(fixture.save_count, 0)

    def test_generated_file_operation_sequences_preserve_tree_identity_and_event_order(self):
        # The native workflow depends on this server as its authoritative oracle.
        # Generate mixed operations against actual HTTP routes and independently
        # model identity/parent/source; source-token checks cannot establish it.
        for seed in (0, 17, 451):
            with self.subTest(seed=seed), StorageFixture(file_actions=True) as fixture:
                rng = random.Random(seed)
                model = {ROOT: dict(parent=None, name='', kind='folder'),
                         DOCUMENT: dict(parent=ROOT, name='main.txt', kind='doc', source=SOURCE),
                         ASSET: dict(parent=ROOT, name='fixture.bin', kind='file')}
                generation = operations = writes = 0
                origin = fixture.url.rsplit('/project/', 1)[0]

                def flattened_tree():
                    tree = fixture.files._tree()
                    self.assertEqual(tree['_id'], PROJECT)
                    actual = {}
                    pending = [(None, root) for root in tree['rootFolder']]
                    while pending:
                        parent, folder = pending.pop()
                        self.assertNotIn(folder['_id'], actual, 'cycles or duplicate identities')
                        actual[folder['_id']] = dict(parent=parent, name=folder['name'], kind='folder')
                        for key, kind in (('docs', 'doc'), ('fileRefs', 'file')):
                            for child in folder[key]:
                                self.assertNotIn(child['_id'], actual)
                                actual[child['_id']] = dict(parent=folder['_id'], name=child['name'], kind=kind)
                        pending.extend((folder['_id'], child) for child in folder['folders'])
                    return actual

                def assert_model():
                    self.assertEqual(flattened_tree(), {
                        identity: {key: entry[key] for key in ('parent', 'name', 'kind')}
                        for identity, entry in model.items()})
                    self.assertEqual(len(fixture.files.operations), operations)
                    self.assertEqual(fixture.save_count, writes)
                    for identity, entry in model.items():
                        if entry['kind'] == 'doc':
                            with urlopen(origin + '/project/' + PROJECT + '/doc/' + identity + '/download', timeout=2) as response:
                                self.assertEqual(response.read().decode(), entry['source'])

                for step in range(72):
                    folders = [identity for identity, entry in model.items() if entry['kind'] == 'folder']
                    mutable = [identity for identity in model if identity not in (ROOT, DOCUMENT, ASSET)]
                    docs = [identity for identity, entry in model.items() if entry['kind'] == 'doc']
                    action = rng.choice(('create', 'create', 'rename', 'move', 'delete', 'write'))
                    event = None
                    if action == 'create' or not mutable:
                        kind = rng.choice(('doc', 'folder'))
                        parent = rng.choice(folders)
                        name = f'item-{seed}-{step}-α' + ('.tex' if kind == 'doc' else '')
                        result = self.request(fixture, '/' + kind, dict(parent_folder_id=parent, name=name))
                        identity = result['_id']
                        self.assertNotIn(identity, model, 'server reused an existing entity identity')
                        model[identity] = dict(parent=parent, name=name, kind=kind)
                        if kind == 'doc':
                            model[identity]['source'] = ''
                        created = result if kind == 'doc' else {**result, 'docs': [], 'fileRefs': [], 'folders': []}
                        event = ('reciveNewDoc' if kind == 'doc' else 'reciveNewFolder', [parent, created])
                    elif action == 'write':
                        identity = rng.choice(docs)
                        source = f"line {seed}/{step}: Tao's α🙂\n$ x = y $.\n"
                        with urlopen(Request(origin + '/fixture/source/' + identity, data=source.encode(), method='POST'), timeout=2) as response:
                            self.assertEqual(json.loads(response.read()), {})
                        model[identity]['source'] = source
                        writes += 1
                    else:
                        identity = rng.choice(mutable)
                        entry = model[identity]
                        route = '/' + entry['kind'] + '/' + identity
                        if action == 'rename':
                            name = f'renamed-{seed}-{step}'
                            self.request(fixture, route + '/rename', dict(name=name))
                            entry['name'] = name
                            event = ('reciveEntityRename', [identity, name])
                        elif action == 'move':
                            def allowed(parent):
                                while parent is not None:
                                    if parent == identity:
                                        return False
                                    parent = model[parent]['parent']
                                return True
                            parent = rng.choice([folder for folder in folders if allowed(folder)])
                            self.request(fixture, route + '/move', dict(folder_id=parent))
                            entry['parent'] = parent
                            event = ('reciveEntityMove', [identity, parent])
                        else:
                            self.request(fixture, route, method='DELETE')
                            removed = {identity}
                            while True:
                                descendants = {item for item, value in model.items() if value['parent'] in removed}
                                if descendants <= removed:
                                    break
                                removed |= descendants
                            for item in removed:
                                del model[item]
                            event = ('removeEntity', [identity])
                    if event:
                        with urlopen(origin + '/fixture/files/events?generation=' + str(generation), timeout=2) as response:
                            events = json.loads(response.read())
                        generation += 1
                        operations += 1
                        self.assertEqual(events['generation'], generation)
                        self.assertEqual(len(events['events']), 1)
                        observed = events['events'][0]
                        self.assertEqual((observed['event'], observed['arguments']), event)
                    if step % 9 == 0:
                        before = flattened_tree()
                        for value, csrf in ((dict(parent_folder_id=ROOT, name='../escape'), True),
                                            (dict(parent_folder_id=ROOT, name='forbidden'), False),
                                            (dict(parent_folder_id=ROOT, name='main.txt'), True)):
                            with self.assertRaises(HTTPError) as caught:
                                self.request(fixture, '/doc', value, csrf=csrf)
                            caught.exception.close()
                        self.assertEqual(flattened_tree(), before, 'rejected requests mutated the server')
                    assert_model()

    @unittest.skipUnless(shutil.which('node'), 'Node is required to parse the browser fixture')
    def test_generated_frontend_is_syntactically_valid(self):
        fixture = StorageFixture(file_actions=True)
        try:
            result = subprocess.run(['node', '--check'], input=fixture.javascript(),
                                    text=True, capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0, result.stderr)
        finally:
            fixture.server.server_close()


if __name__ == '__main__':
    unittest.main()
