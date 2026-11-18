"""Real localhost cookie authentication for native Overleaf session journeys.

Happy paths use ordinary HTTP forms, redirects and the production project bridge.
Only the explicitly armed hostile child pages attempt raw native IPC; they must
never enter the owning project's decoder. No browser cookie value is observed by
or reported to the application or journey driver.
"""
from http.cookies import CookieError, SimpleCookie
import html
import json
import secrets
import threading
from urllib.parse import parse_qs, urlsplit

from overleaf_storage_fixture import ASSET, DOCUMENT, PROJECT, StorageFixture


class AuthStorageFixture(StorageFixture):
    def __init__(self, *, multi_sessions=False, **kwargs):
        if type(multi_sessions) is not bool:
            raise ValueError('Fixture multi-session mode must be a boolean')
        super().__init__(**kwargs)
        self._auth_lock = threading.Lock()
        self._require_auth = False
        self._session_name = 'ttt_fixture_' + secrets.token_hex(8)
        self._session_value = None
        self._multi_sessions = multi_sessions
        self._sessions = {}
        self._session_project_counts = {}
        self._csrf = secrets.token_hex(16)
        self._attacks = False
        self._auth_observation = dict(login_requests=0, signins=0, dashboard_requests=0,
                                      authenticated_project_requests=0, authenticated_documents=0,
                                      authenticated_assets=0, iframe_attempted=False,
                                      iframe_ipc_available=False, popup_attempted=False,
                                      popup_ipc_available=False)
        fixture = self
        parent = self.server.RequestHandlerClass

        class Handler(parent):
            def redirect(self, path):
                self.send_response(303)
                self.send_header('Location', path)
                self.send_header('Content-Length', '0')
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()

            def authorized(self):
                with fixture._auth_lock:
                    if not fixture._require_auth:
                        return True
                return self.session_slot() is not None

            def session_slot(self):
                """Server-only synthetic identity; never expose a cookie to the app."""
                try:
                    with fixture._auth_lock:
                        name = fixture._session_name
                        sessions = tuple(fixture._sessions.items())
                    actual = SimpleCookie(self.headers.get('Cookie', '')).get(name)
                    if actual is not None:
                        for expected, slot in sessions:
                            if secrets.compare_digest(actual.value, expected):
                                return slot
                except CookieError:
                    pass
                return None

            def do_GET(self):
                route = urlsplit(self.path)
                if route.path == '/login':
                    fixture.page_requested.set()
                    with fixture._auth_lock:
                        fixture._auth_observation['login_requests'] += 1
                    if self.authorized():
                        self.redirect('/project')
                    else:
                        page = ('<!doctype html><meta charset="utf-8"><title>Fixture sign-in</title>'
                                '<form method="post" action="/fixture/auth/sign-in">'
                                '<input type="hidden" name="csrf" value="' + html.escape(fixture._csrf) + '">'
                                '<button type="submit" style="position:fixed;left:16px;top:16px;'
                                'width:200px;height:36px">Sign in to fixture</button></form>')
                        self.reply(200, page.encode(), 'text/html; charset=utf-8')
                    return
                if route.path == '/fixture/auth/attack-attempt':
                    values = parse_qs(route.query)
                    kind, available = values.get('kind'), values.get('available')
                    with fixture._auth_lock:
                        armed = fixture._attacks
                    if (not armed or not self.authorized() or kind not in (['iframe'], ['popup'])
                            or available not in (['0'], ['1'])):
                        self.reply(400, b'Invalid attack report', 'text/plain')
                        return
                    with fixture._auth_lock:
                        fixture._auth_observation[kind[0] + '_attempted'] = True
                        fixture._auth_observation[kind[0] + '_ipc_available'] = available == ['1']
                    self.reply(204, b'', 'text/plain')
                    return
                if not self.authorized():
                    self.redirect('/login')
                    return
                if route.path == '/project':
                    with fixture._auth_lock:
                        fixture._auth_observation['dashboard_requests'] += 1
                    self.reply(200, b'<!doctype html><title>Fixture projects</title><p>Signed in.</p>',
                               'text/html; charset=utf-8')
                    return
                if route.path == '/project/' + PROJECT:
                    attack = parse_qs(route.query).get('fixture-attack')
                    with fixture._auth_lock:
                        attacks = fixture._attacks
                    if attack in (['iframe'], ['popup']) and not attacks:
                        self.reply(404, b'Fixture attacks are not armed', 'text/plain')
                        return
                    if attack in (['iframe'], ['popup']):
                        self.reply(200, fixture.attack_page(attack[0]).encode(),
                                   'text/html; charset=utf-8')
                        return
                    slot = self.session_slot()
                    with fixture._auth_lock:
                        fixture._auth_observation['authenticated_project_requests'] += 1
                        if fixture._multi_sessions and slot is not None:
                            fixture._session_project_counts[slot] = (
                                fixture._session_project_counts.get(slot, 0) + 1)
                        attacks = fixture._attacks
                    if attacks:
                        page = ('<!doctype html><meta charset="utf-8"><title>Storage fixture</title>'
                                '<p>Offline authenticated storage fixture</p><script src="/fixture.js"></script>'
                                '<template id="attacks"><iframe style="position:fixed;width:1px;height:1px;opacity:0" '
                                'src="/project/' + PROJECT + '?fixture-attack=iframe"></iframe></template>'
                                '<button style="position:fixed;left:16px;top:64px;width:200px;height:36px" '
                                'onclick="this.disabled=true;document.body.appendChild(document.getElementById(\'attacks\')'
                                '.content.cloneNode(true));window.open(\'/project/' + PROJECT + '?fixture-attack=popup\')">'
                                'Test child isolation</button>')
                        self.reply(200, page.encode(), 'text/html; charset=utf-8')
                        return
                elif route.path == f'/project/{PROJECT}/doc/{DOCUMENT}/download':
                    with fixture._auth_lock:
                        fixture._auth_observation['authenticated_documents'] += 1
                elif route.path == f'/project/{PROJECT}/file/{ASSET}':
                    with fixture._auth_lock:
                        fixture._auth_observation['authenticated_assets'] += 1
                super().do_GET()

            def do_POST(self):
                if self.path == '/fixture/auth/sign-in':
                    length = self.headers.get('Content-Length', '')
                    if (self.headers.get('Origin') != fixture.origin
                            or not length.isascii() or not length.isdigit()
                            or not 0 < int(length) <= 2048):
                        self.reply(403, b'Invalid fixture sign-in', 'text/plain')
                        return
                    try:
                        fields = parse_qs(self.rfile.read(int(length)).decode('utf-8'))
                    except UnicodeDecodeError:
                        fields = {}
                    if fields.get('csrf') != [fixture._csrf]:
                        self.reply(403, b'Invalid fixture sign-in', 'text/plain')
                        return
                    with fixture._auth_lock:
                        if fixture._multi_sessions and len(fixture._sessions) >= 16:
                            self.reply(429, b'Fixture session limit reached', 'text/plain')
                            return
                        fixture._session_value = secrets.token_hex(24)
                        if not fixture._multi_sessions:
                            fixture._sessions.clear()
                        slot = fixture._auth_observation['signins'] + 1
                        fixture._sessions[fixture._session_value] = slot
                        cookie = (fixture._session_name + '=' + fixture._session_value
                                  + '; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600')
                        fixture._auth_observation['signins'] += 1
                    self.send_response(303)
                    self.send_header('Location', '/project')
                    self.send_header('Set-Cookie', cookie)
                    self.send_header('Cache-Control', 'no-store')
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                elif not self.authorized():
                    self.redirect('/login')
                else:
                    super().do_POST()

            def do_DELETE(self):
                if not self.authorized():
                    self.redirect('/login')
                else:
                    super().do_DELETE()

        self.server.RequestHandlerClass = Handler

    @property
    def origin(self):
        return self.url.rsplit('/project/', 1)[0]

    @property
    def login_url(self):
        return self.origin + '/login'

    @property
    def dashboard_url(self):
        return self.origin + '/project'

    def enable_auth(self):
        with self._auth_lock:
            self._require_auth = True
            self._session_value = None
            self._sessions.clear()
            self._session_project_counts.clear()

    def expire_session(self):
        self.enable_auth()

    def arm_attacks(self):
        with self._auth_lock:
            self._attacks = True

    def auth_observation(self):
        with self._auth_lock:
            return dict(self._auth_observation)

    def session_project_counts(self):
        """Only bounded synthetic ordinal/count pairs, never names or tokens."""
        with self._auth_lock:
            return dict(self._session_project_counts)

    @staticmethod
    def attack_page(kind):
        # A malformed reliable event is terminal if admitted, so a later valid
        # parent event cannot conceal a missing native frame/WebView check.
        payload = json.dumps(json.dumps(dict(event='auth-fixture-frame-poison')))
        return f'''<!doctype html><meta charset="utf-8"><title>Hostile fixture child</title>
<script>
let available = false;
try {{
  const ipc = window.webkit?.messageHandlers?.ipc;
  available = Boolean(ipc);
  if (ipc) ipc.postMessage({payload});
}} catch (_) {{}}
fetch('/fixture/auth/attack-attempt?kind={kind}&available=' + Number(available))
  .finally(() => {{ if ({json.dumps(kind)} === 'popup') window.close(); }});
</script>'''
