#!/usr/bin/env python3
"""Compare JVMs through the editor's real LTeX+ manual-check protocol.

No installation or app defaults change. Each fixture/candidate run owns a fresh
server. One repetition is a balanced forward/reverse candidate block (ABBA for
2 JVMs). Initialization, first grammar check, unchanged cached checks, distinct
prose edits and code-action enrichment have separate timings. Warmup edits are
never recycled into measured edits. Receipts contain hashes/counts, not prose or
raw diagnostic messages; the bounded stderr tail is also represented by a hash.

Example:
  python3 scripts/benchmark-ltex-jvm.py --server /path/to/ltex-ls-plus \
    --candidate temurin=/path/to/temurin/Home --candidate graal=/path/to/graal/Home \
    --fixtures /path/to/fixtures.json --warmups 4 --rounds 8 --repetitions 1 \
    --output /path/to/receipt.json
"""
import argparse
from collections import Counter, deque
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
MAX_BODY = 16 * 1024 * 1024
MAX_SOURCE = 2 * 1024 * 1024
MAX_HEADER = 16 * 1024
MAX_DIAGNOSTICS = 4096
CONFIGURATION = {
    'language': 'en-GB', 'checkFrequency': 'manual', 'completionEnabled': False,
    'additionalRules': {'enablePickyRules': False}, 'ltex-ls': {'logLevel': 'warning'},
}
JAVA_OVERRIDES = ('JAVA_OPTS', 'JAVA_TOOL_OPTIONS', 'JDK_JAVA_OPTIONS', '_JAVA_OPTIONS',
                  'CLASSPATH_PREFIX', 'CLASSPATH')


class BenchmarkError(Exception):
    """Only source-free runner descriptions belong in the public receipt."""


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def digest(value):
    return hashlib.sha256(value).hexdigest()


def file_digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def encode_message(value):
    body = canonical(value).encode('utf-8')
    if len(body) > MAX_BODY:
        raise BenchmarkError('Outgoing LSP message exceeds the bounded size')
    return f'Content-Length: {len(body)}\r\n\r\n'.encode('ascii') + body


def read_message(stream):
    length, header_bytes = None, 0
    while True:
        line = stream.readline(MAX_HEADER + 1)
        if not line:
            if header_bytes:
                raise BenchmarkError('LSP header ended unexpectedly')
            return None
        header_bytes += len(line)
        if header_bytes > MAX_HEADER or not line.endswith(b'\n'):
            raise BenchmarkError('LSP header exceeds the bounded size')
        if line in (b'\r\n', b'\n'):
            break
        try:
            name, value = line.decode('ascii').strip().split(':', 1)
        except (UnicodeError, ValueError) as error:
            raise BenchmarkError('Malformed LSP header') from error
        if name.lower() == 'content-length':
            if length is not None or not value.strip().isascii() or not value.strip().isdigit():
                raise BenchmarkError('Invalid or duplicate LSP Content-Length')
            length = int(value.strip())
    if length is None or not 0 < length <= MAX_BODY:
        raise BenchmarkError('Missing or oversized LSP Content-Length')
    chunks, remaining = [], length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise BenchmarkError('LSP body ended unexpectedly')
        chunks.append(chunk)
        remaining -= len(chunk)
    try:
        value = json.loads(b''.join(chunks))
    except (ValueError, UnicodeError) as error:
        raise BenchmarkError('Malformed LSP JSON body') from error
    if not isinstance(value, dict) or value.get('jsonrpc') != '2.0':
        raise BenchmarkError('LSP message must be a JSON-RPC 2.0 object')
    return value


def configuration_response(params, configuration=CONFIGURATION):
    result = []
    for item in params.get('items', []):
        section = item.get('section', '')
        value = {'ltex': configuration} if not section else configuration if section == 'ltex' else None
        if isinstance(section, str) and section.startswith('ltex.'):
            value = configuration
            for part in section[5:].split('.'):
                value = value.get(part) if isinstance(value, dict) else None
        result.append(value)
    return result


def server_request_response(message, root_uri):
    if 'method' not in message or 'id' not in message:
        return None
    method = message['method']
    if method in ('workspace/configuration', 'ltex/workspaceSpecificConfiguration'):
        result = configuration_response(message.get('params', {}))
    elif method == 'workspace/workspaceFolders':
        result = [{'uri': root_uri, 'name': 'Writing workspace'}]
    elif method in ('client/registerCapability', 'client/unregisterCapability', 'window/workDoneProgress/create'):
        result = None
    else:
        return {'jsonrpc': '2.0', 'id': message['id'],
                'error': {'code': -32601, 'message': 'Unsupported client method'}}
    return {'jsonrpc': '2.0', 'id': message['id'], 'result': result}


@dataclass
class PendingCheck:
    request_id: int
    uri: str
    diagnostics: object = None
    completed: bool = False

    def observe(self, message):
        if message.get('method') == 'textDocument/publishDiagnostics':
            params = message.get('params', {})
            if params.get('uri') == self.uri:
                diagnostics = params.get('diagnostics')
                if not isinstance(diagnostics, list) or len(diagnostics) > MAX_DIAGNOSTICS:
                    raise BenchmarkError('Invalid or excessive diagnostic publication')
                self.diagnostics = diagnostics
        elif 'method' not in message and type(message.get('id')) is int and message['id'] == self.request_id:
            if 'error' in message or not isinstance(message.get('result'), dict) or message['result'].get('success') is not True:
                raise BenchmarkError('LTeX did not complete its explicit grammar check')
            self.completed = True
        return self.completed and self.diagnostics is not None


def normalized_range(value):
    try:
        start, end = value['start'], value['end']
        points = [start['line'], start['character'], end['line'], end['character']]
    except (KeyError, TypeError) as error:
        raise BenchmarkError('Diagnostic or edit has an invalid range') from error
    if any(type(point) is not int or point < 0 for point in points) or points[:2] > points[2:]:
        raise BenchmarkError('Diagnostic or edit has an invalid UTF-16 range')
    return points


def multiset_hash(values):
    # Counts matter: a set would hide duplicate or missing diagnostic regressions.
    return digest(canonical(sorted(Counter(canonical(value) for value in values).items())).encode())


def diagnostic_identity(diagnostics, minimum, required_rules, required_lines=()):
    values, rules = [], []
    for diagnostic in diagnostics:
        if not isinstance(diagnostic, dict) or not isinstance(diagnostic.get('message'), str):
            raise BenchmarkError('Invalid diagnostic payload')
        code = diagnostic.get('code')
        if code is not None and type(code) not in (str, int):
            raise BenchmarkError('Invalid diagnostic rule code')
        if code is not None:
            rules.append(str(code))
        values.append({'range': normalized_range(diagnostic.get('range')),
                       'code': code, 'severity': diagnostic.get('severity'),
                       'source': diagnostic.get('source'), 'message': diagnostic['message']})
    if len(values) < minimum or not set(required_rules).issubset(rules):
        missing = sorted(set(required_rules) - set(rules))
        raise BenchmarkError(f'Expected grammar sentinel diagnostics are missing: count={len(values)}, missing_rules={missing}')
    covered_lines = {value['range'][0] for value in values}
    missing_lines = sorted(set(required_lines) - covered_lines)
    if missing_lines:
        raise BenchmarkError(f'Expected grammar sentinel diagnostic lines are missing: lines={missing_lines}')
    identity = {'count': len(values), 'rules': sorted(rules), 'fingerprint': multiset_hash(values)}
    if required_lines:
        identity['covered_required_lines'] = sorted(required_lines)
    return identity


def same_file_edits(action, uri):
    """Match the editor's action_replacements workspace-edit admission rules."""
    edit = action.get('edit')
    if not isinstance(edit, dict):
        return []
    changes = edit.get('documentChanges')
    if isinstance(changes, list):
        if any(not isinstance(change, dict) or change.get('textDocument', {}).get('uri') != uri for change in changes):
            return []
        return [item for change in changes for item in
                (change.get('edits') if isinstance(change.get('edits'), list) else [])]
    changes = edit.get('changes')
    if isinstance(changes, dict) and len(changes) == 1 and isinstance(changes.get(uri), list):
        return changes[uri]
    return []


def action_identity(actions, uri, minimum_replacements=0):
    if actions is None:
        actions = []
    if not isinstance(actions, list):
        raise BenchmarkError('Invalid code-action response')
    values, preferred = [], {}
    for action in actions:
        if not isinstance(action, dict):
            raise BenchmarkError('Invalid code-action entry')
        normalized = []
        for item in same_file_edits(action, uri):
            location = normalized_range(item.get('range'))
            replacement = item.get('newText')
            if not isinstance(replacement, str):
                raise BenchmarkError('Invalid code-action replacement')
            normalized.append({'range': location, 'newText': replacement})
            if len(replacement.encode('utf-8')) > MAX_SOURCE:
                continue
            choices = preferred.setdefault(tuple(location), [])
            if replacement not in choices and len(choices) < 32:
                choices.append(replacement)
        command = action.get('command')
        values.append({'title': action.get('title'), 'kind': action.get('kind'),
                       'edits': normalized,
                       'command': command.get('command') if isinstance(command, dict) else command})
    replacement_count = sum(len(choices) for choices in preferred.values())
    if replacement_count < minimum_replacements:
        raise BenchmarkError('Expected same-file grammar correction edits are missing')
    return {'count': len(values), 'same_file_replacements': replacement_count,
            'fingerprint': multiset_hash(values),
            'preferred_fingerprint': digest(canonical(sorted(preferred.items())).encode())}


def balanced_order(names, repetitions):
    if not names or len(set(names)) != len(names):
        raise BenchmarkError('Candidate names must be distinct')
    return [name for _ in range(repetitions) for name in [*names, *reversed(names)]]


def summary(samples):
    if not samples or any(not math.isfinite(value) or value < 0 for value in samples):
        raise BenchmarkError('Timing samples must be finite, nonnegative and nonempty')
    ordered = sorted(samples)
    return {'samples': len(samples), 'median_ms': statistics.median(samples),
            'mean_ms': statistics.mean(samples), 'min_ms': ordered[0], 'max_ms': ordered[-1],
            'p95_ms': ordered[max(0, math.ceil(len(ordered) * .95) - 1)]}


def benchmark_environment(java_home):
    environment = {key: value for key, value in os.environ.items() if key not in JAVA_OVERRIDES}
    environment.update(JAVA_HOME=str(java_home), JAVACMD=str(java_home / 'bin/java'),
                       LANG='C', LC_ALL='C', TZ='UTC')
    return environment


class Session:
    def __init__(self, launcher, java_home, workspace):
        self.started = time.perf_counter_ns()
        self.process = subprocess.Popen([str(launcher)], cwd=workspace,
            env=benchmark_environment(java_home), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, start_new_session=os.name == 'posix')
        self.incoming, self.outgoing = queue.Queue(64), queue.Queue(8)
        self.stop = threading.Event()
        self.stderr = deque(maxlen=8192)
        self.failure = None
        self.threads = [threading.Thread(target=target, daemon=True) for target in
                        (self._reader, self._writer, self._stderr)]
        for thread in self.threads:
            thread.start()
        self.root_uri = workspace.as_uri()
        self.next_id = 2
        self.opened, self.version = False, 0

    def _put(self, value):
        while not self.stop.is_set():
            try:
                self.incoming.put(value, timeout=.1)
                return
            except queue.Full:
                pass

    def _reader(self):
        try:
            while not self.stop.is_set():
                message = read_message(self.process.stdout)
                if message is None:
                    self._put(BenchmarkError('LTeX closed its output'))
                    return
                self._put(message)
        except (OSError, ValueError, BenchmarkError):
            self._put(BenchmarkError('LTeX output framing failed'))

    def _writer(self):
        try:
            while not self.stop.is_set():
                try:
                    message = self.outgoing.get(timeout=.1)
                except queue.Empty:
                    continue
                self.process.stdin.write(encode_message(message))
                self.process.stdin.flush()
        except (OSError, ValueError, BenchmarkError):
            self.failure = BenchmarkError('LTeX input stopped accepting requests')

    def _stderr(self):
        try:
            while True:
                chunk = self.process.stderr.read(1024)
                if not chunk:
                    return
                self.stderr.extend(chunk)
        except (OSError, ValueError):
            pass

    def send(self, message):
        try:
            self.outgoing.put(message, timeout=1)
        except queue.Full as error:
            raise BenchmarkError('Bounded LTeX request queue is full') from error

    def notify(self, method, params):
        self.send({'jsonrpc': '2.0', 'method': method, 'params': params})

    def request(self, method, params):
        request_id = self.next_id
        self.next_id += 1
        self.send({'jsonrpc': '2.0', 'id': request_id, 'method': method, 'params': params})
        return request_id

    def receive(self, deadline):
        while True:
            if self.failure:
                raise self.failure
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise BenchmarkError('LTeX protocol operation timed out')
            try:
                message = self.incoming.get(timeout=min(remaining, .1))
            except queue.Empty:
                if self.process.poll() is not None:
                    raise BenchmarkError('LTeX exited before protocol completion')
                continue
            if isinstance(message, Exception):
                raise message
            response = server_request_response(message, self.root_uri)
            if response is not None:
                self.send(response)
                continue
            return message

    def initialize(self, timeout):
        self.send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'processId': os.getpid(), 'rootUri': self.root_uri,
            'workspaceFolders': [{'uri': self.root_uri, 'name': 'Writing workspace'}],
            'clientInfo': {'name': 'tiptoptyp'}, 'locale': 'en',
            'capabilities': {'general': {'positionEncodings': ['utf-16']},
                'workspace': {'configuration': True}, 'textDocument': {
                    'publishDiagnostics': {'versionSupport': True},
                    'codeAction': {'codeActionLiteralSupport': {'codeActionKind': {'valueSet': ['quickfix']}}}}},
            'initializationOptions': {'customCapabilities': {'workspaceSpecificConfiguration': True}},
        }})
        deadline = time.monotonic() + timeout
        while True:
            message = self.receive(deadline)
            if 'method' not in message and message.get('id') == 1:
                if 'error' in message:
                    raise BenchmarkError('LTeX initialization failed')
                if message.get('result', {}).get('capabilities', {}).get('positionEncoding', 'utf-16') != 'utf-16':
                    raise BenchmarkError('LTeX selected unsupported position encoding')
                self.notify('initialized', {})
                return (time.perf_counter_ns() - self.started) / 1_000_000

    def check(self, uri, language, source, timeout):
        started = time.perf_counter_ns()
        request_id = self.request('workspace/executeCommand', {'command': '_ltex.checkDocument',
            'arguments': [{'uri': uri, 'codeLanguageId': language, 'text': source}]})
        pending = PendingCheck(request_id, uri)
        deadline = time.monotonic() + timeout
        while not pending.observe(self.receive(deadline)):
            pass
        grammar_ms = (time.perf_counter_ns() - started) / 1_000_000
        self.last_diagnostics_from_process_ms = (time.perf_counter_ns() - self.started) / 1_000_000
        actions, action_ms = [], 0.0
        if pending.diagnostics:
            # Match the app: manual configuration is pulled before any didOpen.
            self.version += 1
            if not self.opened:
                self.notify('textDocument/didOpen', {'textDocument': {'uri': uri,
                    'languageId': language, 'version': self.version, 'text': source}})
                self.opened = True
            else:
                self.notify('textDocument/didChange', {'textDocument': {'uri': uri,
                    'version': self.version}, 'contentChanges': [{'text': source}]})
            lines = source.split('\n')
            action_started = time.perf_counter_ns()
            action_id = self.request('textDocument/codeAction', {'textDocument': {'uri': uri},
                'range': {'start': {'line': 0, 'character': 0}, 'end': {
                    'line': len(lines) - 1, 'character': len(lines[-1].encode('utf-16-le')) // 2}},
                'context': {'diagnostics': pending.diagnostics, 'only': ['quickfix']}})
            deadline = time.monotonic() + timeout
            while True:
                message = self.receive(deadline)
                if 'method' not in message and message.get('id') == action_id:
                    if 'error' in message:
                        raise BenchmarkError('LTeX code-action enrichment failed')
                    actions = message.get('result')
                    break
            action_ms = (time.perf_counter_ns() - action_started) / 1_000_000
        self.last_suggestions_from_process_ms = (time.perf_counter_ns() - self.started) / 1_000_000
        return pending.diagnostics, actions, grammar_ms, action_ms

    def close(self):
        if self.process.poll() is None:
            try:
                self.send({'jsonrpc': '2.0', 'id': 0, 'method': 'shutdown', 'params': None})
                deadline = time.monotonic() + .25
                while time.monotonic() < deadline:
                    message = self.receive(deadline)
                    if message.get('id') == 0:
                        break
                self.notify('exit', None)
                self.process.wait(timeout=.15)
            except (BenchmarkError, subprocess.TimeoutExpired):
                pass
        self.stop.set()
        # A launcher may exit while its Java descendant still owns the pipes.
        # The dedicated process group must be retired even in that case.
        if os.name == 'posix':
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        elif self.process.poll() is None:
            self.process.terminate()
        if self.process.poll() is None:
            try:
                self.process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
        # Escalate the group even when its original launcher has exited. A
        # surviving descendant can otherwise hold a buffered pipe lock forever.
        if os.name == 'posix':
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        elif self.process.poll() is None:
            self.process.kill()
        if self.process.poll() is None:
            self.process.wait(timeout=2)
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            try:
                stream.close()
            except (OSError, ValueError):
                pass
        for thread in self.threads:
            thread.join(timeout=.2)
        tail = bytes(self.stderr)
        return {'tail_bytes': len(tail), 'tail_sha256': digest(tail)}


def read_source(path):
    if not path.is_file() or not 0 < path.stat().st_size <= MAX_SOURCE:
        raise BenchmarkError('A benchmark source is missing, empty or exceeds the app 2 MiB limit')
    data = path.read_bytes()
    return data.decode('utf-8'), {'sha256': digest(data), 'bytes': len(data)}


def load_fixtures(path, warmups, rounds):
    data = path.read_bytes()
    if len(data) > 256 * 1024:
        raise BenchmarkError('Fixture manifest exceeds the bounded size')
    manifest = json.loads(data)
    if manifest.get('version') != 1 or not isinstance(manifest.get('fixtures'), list):
        raise BenchmarkError('Unsupported fixture manifest')
    fixtures, names, total = [], set(), 0
    for item in manifest['fixtures']:
        name, language = item.get('name'), item.get('language')
        if not isinstance(name, str) or not name or name in names or language not in ('latex', 'typst'):
            raise BenchmarkError('Fixtures need unique names and latex/typst language IDs')
        names.add(name)
        source_path, edit_paths = item.get('source'), item.get('edits', [])
        if not isinstance(source_path, str) or not isinstance(edit_paths, list) or any(not isinstance(edit, str) for edit in edit_paths):
            raise BenchmarkError('Fixture source and edits must be file paths')
        source, metadata = read_source(path.parent / source_path)
        edits = [read_source(path.parent / edit) for edit in edit_paths]
        if len(edits) < warmups + rounds:
            raise BenchmarkError('Not enough distinct edit variants for warmups plus measured rounds')
        hashes = [metadata['sha256'], *(entry[1]['sha256'] for entry in edits)]
        if len(hashes) != len(set(hashes)):
            raise BenchmarkError('Edit variants must be distinct and must not recycle the base document')
        minimum, required = item.get('min_diagnostics', 1), item.get('required_rules', [])
        if type(minimum) is not int or minimum < 1 or not isinstance(required, list) or any(not isinstance(rule, str) for rule in required):
            raise BenchmarkError('Every workload needs a positive diagnostic sentinel and valid required rules')
        required_lines = item.get('required_diagnostic_lines', [])
        if (not isinstance(required_lines, list) or any(type(line) is not int or line < 0 for line in required_lines)
                or len(required_lines) != len(set(required_lines)) or len(required_lines) > MAX_DIAGNOSTICS):
            raise BenchmarkError('Required diagnostic lines must be distinct nonnegative zero-based line numbers')
        shortest_source_lines = min(text.count('\n') + 1 for text in [source, *(entry[0] for entry in edits)])
        if required_lines and max(required_lines) >= shortest_source_lines:
            raise BenchmarkError('A required diagnostic line is outside a source or edit variant')
        total += metadata['bytes'] + sum(entry[1]['bytes'] for entry in edits)
        if total > 64 * 1024 * 1024 or len(fixtures) >= 32:
            raise BenchmarkError('Fixture collection exceeds its bounded size')
        fixtures.append({'name': name, 'language': language, 'source': source, 'metadata': metadata,
                         'edits': edits, 'minimum': minimum, 'required': required,
                         'required_lines': required_lines})
    if not fixtures:
        raise BenchmarkError('No benchmark fixtures were supplied')
    return fixtures, digest(data)


def workload(fixture, warmups, rounds):
    yield 'first_document', 0, fixture['source'], fixture['metadata']
    for index in range(warmups + rounds):
        yield ('unchanged_warmup' if index < warmups else 'unchanged'), index, fixture['source'], fixture['metadata']
    for index, (source, metadata) in enumerate(fixture['edits'][:warmups + rounds]):
        yield ('edit_warmup' if index < warmups else 'edit'), index, source, metadata


def candidate_metadata(name, home):
    java = home / 'bin/java'
    version = subprocess.run([str(java), '-version'], env=benchmark_environment(home),
                             capture_output=True, text=True, timeout=15)
    if version.returncode:
        raise BenchmarkError('A candidate JVM failed its version check')
    release = home / 'release'
    properties = {}
    if release.is_file():
        for line in release.read_text().splitlines():
            if '=' in line:
                key, value = line.split('=', 1)
                properties[key] = value.strip('"')
    cds = home / 'lib/server/classes.jsa'
    return {'name': name, 'java_home': str(home), 'java_sha256': file_digest(java),
            'version': (version.stdout + version.stderr).strip(), 'release': properties,
            'cds_archive_bytes': cds.stat().st_size if cds.is_file() else 0,
            'explicit_jvm_flags': []}


def host_metadata():
    host = {'platform': platform.platform(), 'machine': platform.machine(),
            'logical_cpus': os.cpu_count(), 'python': sys.version.split()[0]}
    if sys.platform == 'darwin':
        for key, query in [('cpu', 'machdep.cpu.brand_string'), ('ram_bytes', 'hw.memsize')]:
            try:
                value = subprocess.check_output(['/usr/sbin/sysctl', '-n', query], text=True, timeout=3).strip()
                host[key] = int(value) if key == 'ram_bytes' else value
            except (OSError, subprocess.SubprocessError, ValueError):
                host[key] = None
    return host


def run_benchmark(args):
    launcher = args.server.resolve()
    if launcher.is_dir():
        launcher /= 'bin/ltex-ls-plus'
    if not launcher.is_file() or not os.access(launcher, os.X_OK):
        raise BenchmarkError('Choose an executable LTeX launcher or its distribution directory')
    homes = {}
    for value in args.candidate:
        name, separator, directory = value.partition('=')
        if not separator or not name or name in homes:
            raise BenchmarkError('Candidates use distinct NAME=JAVA_HOME values')
        home = Path(directory).resolve()
        if not (home / 'bin/java').is_file() or not os.access(home / 'bin/java', os.X_OK):
            raise BenchmarkError('A candidate JAVA_HOME does not contain executable bin/java')
        homes[name] = home
    fixtures, fixture_manifest_hash = load_fixtures(args.fixtures.resolve(), args.warmups, args.rounds)
    receipt = {'version': 1, 'passed': False, 'host': host_metadata(),
        'script_sha256': file_digest(__file__), 'app_protocol_sha256': file_digest(ROOT / 'src/ltex.rs'),
        'server': {'launcher': str(launcher), 'sha256': file_digest(launcher),
                   'jars': [{'name': str(path.relative_to(launcher.parent.parent)),
                             'bytes': path.stat().st_size, 'sha256': file_digest(path)}
                            for path in sorted((launcher.parent.parent / 'lib').rglob('*.jar'))]},
        'candidates': [candidate_metadata(name, home) for name, home in homes.items()],
        'configuration': CONFIGURATION, 'fixture_manifest_sha256': fixture_manifest_hash,
        'fixtures': [{'name': fixture['name'], 'language': fixture['language'],
                      'source': fixture['metadata'], 'edits': [entry[1] for entry in fixture['edits']],
                      'min_diagnostics': fixture['minimum'], 'required_rules': fixture['required'],
                      'required_diagnostic_lines': fixture['required_lines']}
                     for fixture in fixtures],
        'policy': {'warmups': args.warmups, 'rounds': args.rounds, 'balanced_blocks': args.repetitions,
                   'fresh_process_per_fixture': True, 'app_default_caches': True,
                   'filesystem_cache': 'warm; metadata reads precede runs; no OS cache flush',
                   'environment_removed': JAVA_OVERRIDES, 'locale': 'C', 'timezone': 'UTC',
                   'home': 'same inherited HOME for every candidate',
                   'debounce_included': False, 'app_poll_interval_included': False,
                   'parity': 'raw diagnostic bag plus admitted same-file action proposals and ordered replacements; no UI source mapping',
                   'stderr': 'bounded private tail; receipt stores only size and SHA256'},
        'order': [], 'runs': [], 'summaries': []}
    expected = {}
    try:
        with tempfile.TemporaryDirectory(prefix='ltex-jvm-') as temporary:
            workspace = Path(temporary)
            for fixture in fixtures:
                document = workspace / ('benchmark.tex' if fixture['language'] == 'latex' else 'benchmark.typ')
                document.write_text(fixture['source'], encoding='utf-8')
                for name in balanced_order(list(homes), args.repetitions):
                    receipt['order'].append({'candidate': name, 'fixture': fixture['name']})
                    run = {'candidate': name, 'fixture': fixture['name'], 'checks': []}
                    receipt['runs'].append(run)
                    session = Session(launcher, homes[name], workspace)
                    try:
                        run['initialize_ms'] = session.initialize(args.startup_timeout)
                        for phase, index, source, metadata in workload(fixture, args.warmups, args.rounds):
                            diagnostics, actions, grammar_ms, action_ms = session.check(
                                document.as_uri(), fixture['language'], source, args.check_timeout)
                            if phase == 'first_document':
                                run['first_diagnostics_from_process_ms'] = session.last_diagnostics_from_process_ms
                                run['first_suggestions_from_process_ms'] = session.last_suggestions_from_process_ms
                            identity = diagnostic_identity(diagnostics, fixture['minimum'], fixture['required'],
                                                           fixture['required_lines'])
                            suggestions = action_identity(actions, document.as_uri(), minimum_replacements=1)
                            key = (fixture['name'], metadata['sha256'])
                            quality = (identity, suggestions)
                            if key in expected and expected[key] != quality:
                                raise BenchmarkError('Diagnostic or suggestion parity differs across JVM runs')
                            expected[key] = quality
                            run['checks'].append({'phase': phase, 'index': index, 'source': metadata,
                                'grammar_ms': grammar_ms, 'code_actions_ms': action_ms,
                                'diagnostics': identity, 'suggestions': suggestions})
                    finally:
                        run['stderr_tail'] = session.close()
                    print(f'Completed process {len(receipt["runs"])}: {name} / {fixture["name"]}', flush=True)
        for name in homes:
            for fixture in fixtures:
                runs = [run for run in receipt['runs'] if run['candidate'] == name and run['fixture'] == fixture['name']]
                values = {field: [run[field + '_ms'] for run in runs] for field in
                          ('initialize', 'first_diagnostics_from_process', 'first_suggestions_from_process')}
                for phase in ('first_document', 'unchanged', 'edit'):
                    checks = [check for run in runs for check in run['checks'] if check['phase'] == phase]
                    values[phase] = [check['grammar_ms'] for check in checks]
                    values[phase + '_code_actions'] = [check['code_actions_ms'] for check in checks]
                receipt['summaries'].append({'candidate': name, 'fixture': fixture['name'],
                                            'metrics': {phase: summary(samples) for phase, samples in values.items()}})
        receipt['passed'] = True
    except (BenchmarkError, OSError, subprocess.SubprocessError) as error:
        # Protocol errors deliberately do not embed server messages or source text.
        receipt['failure'] = {'type': type(error).__name__, 'reason': str(error) if isinstance(error, BenchmarkError) else 'Process or filesystem operation failed'}
    return receipt


def positive(value):
    number = int(value)
    if not 1 <= number <= 50:
        raise argparse.ArgumentTypeError('choose an integer from 1 to 50')
    return number


def nonnegative(value):
    number = int(value)
    if not 0 <= number <= 50:
        raise argparse.ArgumentTypeError('choose an integer from 0 to 50')
    return number


def seconds(value):
    number = float(value)
    if not 0 < number <= 300 or not math.isfinite(number):
        raise argparse.ArgumentTypeError('choose seconds above zero and no more than 300')
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', required=True, type=Path)
    parser.add_argument('--candidate', required=True, action='append', metavar='NAME=JAVA_HOME')
    parser.add_argument('--fixtures', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--rounds', type=positive, default=6)
    parser.add_argument('--warmups', type=nonnegative, default=2)
    parser.add_argument('--repetitions', type=positive, default=1)
    parser.add_argument('--startup-timeout', type=seconds, default=60)
    parser.add_argument('--check-timeout', type=seconds, default=120)
    args = parser.parse_args()
    try:
        receipt = run_benchmark(args)
    except (BenchmarkError, OSError, ValueError, subprocess.SubprocessError) as error:
        parser.error(str(error) if isinstance(error, BenchmarkError) else 'Invalid input or candidate metadata could not be read')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=args.output.parent, delete=False) as stream:
        json.dump(receipt, stream, indent=2)
        stream.write('\n')
        temporary = Path(stream.name)
    temporary.replace(args.output)
    print('LTeX JVM benchmark ' + ('passed' if receipt['passed'] else 'failed') + ': ' + str(args.output))
    raise SystemExit(0 if receipt['passed'] else 1)


if __name__ == '__main__':
    main()
