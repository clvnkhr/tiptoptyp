"""Private loopback GitHub service and CLI double; never real GitHub authentication.

The application invokes an owned fake `gh` executable using its production
command schema. Native UI starts authentication and repository creation. Provider
approval is deliberately simulated by the journey, not claimed as browser OAuth.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import secrets
import subprocess
import sys
import threading

from desktop_ui_git_journeys import git_setup_environment, git_fixture_command, git_fixture_paths

DEVICE_CODE = 'ABCD-EFGH'
FIRST_ACCOUNT = dict(id=123, login='ttt-fixture', type='User')
SECOND_ACCOUNT = dict(id=124, login='ttt-fixture-second', type='User')

HELPER = r'''#!PYTHON
import json
import os
from pathlib import Path
import sys
import time
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

root = Path(__file__).resolve().parent.parent
args = sys.argv[1:]
if args in [['--version'], ['version']]:
    print('gh version 2.103.3 (synthetic local fixture)')
    raise SystemExit(0)
config = json.loads((root / 'github-fixture.json').read_text())
address = urlsplit(config['origin'])
if (address.scheme != 'http' or address.hostname != '127.0.0.1'
        or address.username is not None or address.password is not None
        or not address.port or address.path or address.query or address.fragment):
    raise SystemExit('Unsafe synthetic provider endpoint')

def request(path, data=None):
    headers = {'X-TTT-Fixture': config['nonce']}
    if data is not None:
        headers['Content-Type'] = 'application/json'
    query = Request(config['origin'] + path,
                    data=None if data is None else json.dumps(data).encode(), headers=headers)
    try:
        with urlopen(query, timeout=25) as response:
            return json.load(response)
    except HTTPError as error:
        print('Synthetic GitHub request rejected (HTTP ' + str(error.code) + ')', file=sys.stderr)
        raise SystemExit(1)

if args == ['auth', 'status', '--active', '--hostname', 'github.com', '--json', 'hosts']:
    print(json.dumps(request('/auth/status')))
elif args == ['api', '--hostname', 'github.com', '--method', 'GET', 'user']:
    print(json.dumps(request('/api/user')))
elif (len(args) == 6 and args[:5] == ['api', '--hostname', 'github.com', '--method', 'GET']
      and args[5].startswith('repos/')):
    print(json.dumps(request('/api/' + args[5])))
elif args == ['auth', 'login', '--web', '--hostname', 'github.com', '--git-protocol', 'https',
              '--skip-ssh-key', '--clipboard=false']:
    device = request('/device/start', {})
    print('! First copy your one-time code: ' + device['code'], file=sys.stderr, flush=True)
    print('Open this URL to continue in your web browser: ' + device['verification_uri'],
          file=sys.stderr, flush=True)
    for _ in range(1200):
        status = request('/device/poll')
        if status['approved']:
            profile = Path(os.environ['GH_CONFIG_DIR']).resolve()
            if profile != root / 'github-auth':
                raise SystemExit('Unsafe synthetic authentication profile')
            profile.mkdir(mode=0o700, exist_ok=True)
            (profile / 'hosts.yml').write_text('github.com:\n    user: ' + status['account']['login']
                                             + '\n    git_protocol: https\n')
            (profile / 'hosts.yml').chmod(0o600)
            print('Synthetic authentication complete', flush=True)
            raise SystemExit(0)
        time.sleep(0.05)
    raise SystemExit('Synthetic approval timed out')
elif (len(args) == 12 and args[:6] == ['api', '--hostname', 'github.com', '--method', 'POST', 'user/repos']
      and args[6] == '--raw-field' and args[7].startswith('name=')
      and args[8:] == ['--field', 'private=true', '--field', 'auto_init=false']):
    print(json.dumps(request('/api/user/repos', {'name':args[7][5:], 'private':True, 'auto_init':False})))
else:
    print('Unsupported synthetic gh command', file=sys.stderr)
    raise SystemExit(2)
'''


def prepare_github_helper(directory):
    """The production build never consults this desktop-test-only helper path."""
    directory = Path(directory).resolve()
    helper = directory / 'github-helper/gh'
    helper.parent.mkdir(mode=0o700)
    helper.write_text(HELPER.replace('#!PYTHON', '#!' + sys.executable, 1), encoding='utf-8')
    helper.chmod(0o700)
    for account in (FIRST_ACCOUNT, SECOND_ACCOUNT):
        remote = directory / 'github-remotes' / account['login']
        remote.mkdir(mode=0o700, parents=True)
        git_fixture_command(directory, 'config', '--file', git_fixture_paths(directory)['global_config'],
                            '--add', 'url.' + str(remote) + '/.insteadOf',
                            'https://github.com/' + account['login'] + '/')
    return helper


def github_fixture_environment(directory, environment=None):
    environment = git_setup_environment(directory, environment)
    environment = {key: value for key, value in environment.items()
                   if not key.startswith(('GH_', 'GITHUB_'))}
    environment['GH_CONFIG_DIR'] = str(Path(directory).resolve() / 'github-auth')
    environment['GH_PROMPT_DISABLED'] = '1'
    # Canonical GitHub origins are rewritten to owned bare repositories. A
    # missing/broken rewrite must fail before any external Git transport starts.
    environment['GIT_ALLOW_PROTOCOL'] = 'file'
    return environment


class GitHubFixture:
    def __init__(self, directory):
        self.directory = Path(directory).resolve()
        self._lock = threading.Lock()
        self._nonce = secrets.token_hex(24)
        self._account = None
        self._approved = False
        self._device_started = False
        self._create_count = 0
        self._create_requests = 0
        self._auth_starts = 0
        self._approvals = 0
        self._polls = 0
        self._user_reads = 0
        self._recovery_reads = 0
        self._repositories = []
        self._remotes = {}
        self._pause_creation = False
        self.creation_started = threading.Event()
        self.release_creation = threading.Event()
        self.release_creation.set()
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def reply(self, status, value):
                content = json.dumps(value).encode('utf-8')
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(content)))
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                try:
                    self.wfile.write(content)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def allowed(self):
                return (self.headers.get('Host') == '127.0.0.1:' + str(fixture.server.server_port)
                        and secrets.compare_digest(self.headers.get('X-TTT-Fixture', ''), fixture._nonce))

            def do_GET(self):
                if self.path == '/login/device':
                    content = ('<!doctype html><meta charset="utf-8"><title>Synthetic GitHub approval</title>'
                               '<p>This fixture does not sign in to GitHub.</p>')
                    body = content.encode('utf-8')
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html; charset=utf-8')
                    self.send_header('Content-Length', str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                if not self.allowed():
                    self.reply(403, {'message':'Fixture access denied'})
                    return
                with fixture._lock:
                    account = fixture._account
                    if self.path == '/auth/status':
                        value = {'hosts':{}}
                        if account is not None:
                            value['hosts']['github.com'] = [dict(
                                state='success', active=True, host='github.com',
                                login=account['login'], tokenSource='keyring', gitProtocol='https')]
                    elif self.path == '/api/user':
                        fixture._user_reads += 1
                        value = account
                    elif self.path == '/device/poll':
                        fixture._polls += 1
                        value = {'approved':fixture._approved, 'account':account}
                    elif self.path.startswith('/api/repos/'):
                        fixture._recovery_reads += 1
                        if account is None:
                            self.reply(401, {'message':'Synthetic sign-in required'})
                            return
                        identity = self.path.removeprefix('/api/repos/').split('/')
                        value = next((dict(repository) for repository in fixture._repositories
                                      if identity == [repository['owner']['login'], repository['name']]), None)
                        if value is None:
                            self.reply(404, {'message':'Synthetic repository not found'})
                            return
                    else:
                        self.reply(404, {'message':'Unknown fixture route'})
                        return
                if value is None:
                    self.reply(401, {'message':'Synthetic sign-in required'})
                else:
                    self.reply(200, value)

            def do_POST(self):
                if not self.allowed():
                    self.reply(403, {'message':'Fixture access denied'})
                    return
                size = self.headers.get('Content-Length', '')
                if not size.isdigit() or not 0 < int(size) <= 4096:
                    self.reply(400, {'message':'Invalid bounded fixture request'})
                    return
                try:
                    body = json.loads(self.rfile.read(int(size)))
                except (ValueError, UnicodeDecodeError):
                    self.reply(400, {'message':'Invalid fixture JSON'})
                    return
                if self.path == '/device/start':
                    with fixture._lock:
                        fixture._device_started = True
                        fixture._approved = False
                        fixture._auth_starts += 1
                    self.reply(200, {'code':DEVICE_CODE, 'verification_uri':fixture.verification_uri})
                    return
                if self.path != '/api/user/repos':
                    self.reply(404, {'message':'Unknown fixture route'})
                    return
                with fixture._lock:
                    fixture._create_requests += 1
                if (not isinstance(body, dict) or body.get('private') is not True
                        or body.get('auto_init') is not False
                        or not isinstance(body.get('name'), str)
                        or re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,99}', body['name']) is None):
                    self.reply(400, {'message':'Only a valid private empty fixture repository is permitted'})
                    return
                with fixture._lock:
                    account = fixture._account
                    if account is None:
                        self.reply(401, {'message':'Synthetic sign-in required'})
                        return
                    root = fixture.directory / 'github-remotes' / account['login']
                    remote = root / (body['name'] + '.git')
                    if remote.exists():
                        self.reply(422, {'message':'Fixture repository already exists'})
                        return
                    root.mkdir(parents=True, exist_ok=True)
                    environment = github_fixture_environment(fixture.directory)
                    subprocess.run(['git', 'init', '--bare', '--quiet', str(remote)],
                                   check=True, env=environment, capture_output=True, timeout=10)
                    fixture._create_count += 1
                    value = dict(id=456 + fixture._create_count, name=body['name'], private=True,
                                 owner=dict(id=account['id'], login=account['login']),
                                 html_url='https://github.com/' + account['login'] + '/' + body['name'],
                                 clone_url='https://github.com/' + account['login'] + '/' + body['name'] + '.git')
                    fixture._repositories.append(value)
                    fixture._remotes[value['html_url']] = str(remote.resolve())
                    paused = fixture._pause_creation
                    fixture._pause_creation = False
                    fixture.creation_started.set()
                if paused and not fixture.release_creation.wait(20):
                    self.reply(504, {'message':'Fixture repository response release timed out'})
                    return
                self.reply(201, value)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever,
                                       name='github-service-fixture', daemon=True)

    @property
    def origin(self):
        return 'http://127.0.0.1:' + str(self.server.server_port)

    @property
    def verification_uri(self):
        return self.origin + '/login/device'

    def approve(self, account=FIRST_ACCOUNT):
        """Explicit service simulation; this is not browser consent or real OAuth."""
        with self._lock:
            if not self._device_started:
                raise AssertionError('Cannot simulate approval before native Sign in starts a flow')
            self._account = dict(account)
            self._approved = True
            self._approvals += 1

    def switch_account(self, account):
        with self._lock:
            self._account = dict(account)

    def pause_next_creation(self):
        with self._lock:
            self._pause_creation = True
            self.creation_started.clear()
            self.release_creation.clear()

    def observation(self):
        with self._lock:
            return dict(auth_starts=self._auth_starts, simulated_approvals=self._approvals,
                        polls=self._polls, user_reads=self._user_reads,
                        recovery_reads=self._recovery_reads,
                        create_count=self._create_count,
                        create_requests=self._create_requests,
                        repositories=[dict(value, fixture_remote=self._remotes[value['html_url']])
                                      for value in self._repositories])

    def __enter__(self):
        config = self.directory / 'github-fixture.json'
        config.write_text(json.dumps({'origin':self.origin, 'nonce':self._nonce}), encoding='utf-8')
        config.chmod(0o600)
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.release_creation.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
