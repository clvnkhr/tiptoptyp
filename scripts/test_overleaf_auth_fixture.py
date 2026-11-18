"""Local HTTP authentication contracts; these are not native-WebView evidence."""
from html.parser import HTMLParser
from http.cookiejar import CookieJar
from pathlib import Path
import json
import sys
import unittest
from urllib.error import HTTPError
from urllib.parse import urlencode, urljoin, urlsplit
from urllib.request import HTTPCookieProcessor, HTTPRedirectHandler, Request, build_opener

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overleaf_auth_fixture import AuthStorageFixture
from overleaf_storage_fixture import ASSET, ASSET_BYTES, DOCUMENT, PROJECT, SOURCE


class LoginForm(HTMLParser):
    """Read only the same form fields a browser receives, never fixture internals."""
    def __init__(self):
        super().__init__()
        self.forms = []
        self.current = None

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        if tag == 'form':
            self.current = dict(attributes=attributes, fields={})
            self.forms.append(self.current)
        elif tag == 'input' and self.current is not None:
            name = attributes.get('name')
            if attributes.get('type', '').lower() == 'hidden' and name:
                self.current['fields'][name] = attributes.get('value', '')

    def handle_endtag(self, tag):
        if tag == 'form':
            self.current = None


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def browser(*, follow_redirects=True):
    cookies = CookieJar()
    handlers = [HTTPCookieProcessor(cookies)]
    if not follow_redirects:
        handlers.append(NoRedirect())
    return cookies, build_opener(*handlers)


def origin(url):
    address = urlsplit(url)
    return f'{address.scheme}://{address.netloc}'


class AuthStorageFixtureTests(unittest.TestCase):
    def login_form(self, fixture, client):
        with client.open(fixture.login_url, timeout=2) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.geturl(), fixture.login_url)
            parser = LoginForm()
            parser.feed(response.read().decode('utf-8'))
        forms = [form for form in parser.forms
                 if form['attributes'].get('method', '').upper() == 'POST'
                 and urlsplit(urljoin(fixture.login_url, form['attributes'].get('action', ''))).path
                 == '/fixture/auth/sign-in']
        self.assertEqual(len(forms), 1, 'Login must expose one real sign-in form')
        form = forms[0]
        self.assertTrue(form['fields'], 'The rendered form must carry a hidden CSRF field')
        self.assertTrue(all(form['fields'].values()), 'Hidden form tokens must not be empty')
        return urljoin(fixture.login_url, form['attributes']['action']), form['fields']

    def sign_in(self, fixture, client):
        action, fields = self.login_form(fixture, client)
        request = Request(action, data=urlencode(fields).encode('ascii'), method='POST',
                          headers={'Content-Type': 'application/x-www-form-urlencoded',
                                   'Origin': origin(fixture.login_url)})
        with client.open(request, timeout=2) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.geturl(), fixture.dashboard_url,
                             'Sign-in returns to the dashboard; the app must reopen its bound project')
            self.assertNotEqual(response.geturl(), fixture.url)
            response.read()

    def assert_login_redirect(self, fixture, client, url):
        with client.open(url, timeout=2) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.geturl(), fixture.login_url)
            self.assertNotIn(b'/fixture.js', response.read(),
                             'An unauthenticated response must not mount the source editor')

    def assert_forbidden(self, client, request):
        with self.assertRaises(HTTPError) as rejected:
            client.open(request, timeout=2)
        try:
            self.assertEqual(rejected.exception.code, 403)
        finally:
            rejected.exception.close()

    def test_authentication_is_opt_in_and_does_not_modify_the_existing_storage_fixture(self):
        with AuthStorageFixture() as fixture:
            _, client = browser()
            with client.open(fixture.url, timeout=2) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(response.geturl(), fixture.url)
                self.assertIn(b'/fixture.js', response.read())
            self.assertEqual(fixture.auth_observation()['signins'], 0)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_unauthenticated_project_and_hydration_endpoints_redirect_without_data(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser()
            root = origin(fixture.url)
            paths = (f'/project/{PROJECT}', f'/project/{PROJECT}/doc/{DOCUMENT}/download',
                     f'/project/{PROJECT}/file/{ASSET}', '/fixture.js')
            for path in paths:
                with self.subTest(path=path):
                    self.assert_login_redirect(fixture, client, root + path)
            observations = fixture.auth_observation()
            self.assertEqual(observations['signins'], 0)
            self.assertEqual(observations['authenticated_project_requests'], 0)
            self.assertEqual(observations['authenticated_documents'], 0)
            self.assertEqual(observations['authenticated_assets'], 0)
            # Verify the actual HTTP status rather than only urllib's final page.
            _, direct = browser(follow_redirects=False)
            with self.assertRaises(HTTPError) as redirect:
                direct.open(fixture.url, timeout=2)
            try:
                self.assertEqual(redirect.exception.code, 303)
                self.assertEqual(urljoin(fixture.url, redirect.exception.headers['Location']),
                                 fixture.login_url)
            finally:
                redirect.exception.close()
            self.assertFalse(fixture.document_requested.is_set())
            self.assertFalse(fixture.asset_requested.is_set())
            self.assertEqual(fixture.save_count, 0)

    def test_real_login_form_issues_host_only_httponly_cookie_and_hydrates_its_client_only(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            cookies, client = browser()
            self.sign_in(fixture, client)
            session_cookies = list(cookies)
            self.assertTrue(session_cookies, 'The real form response must issue a session cookie')
            self.assertTrue(all(not cookie.domain_specified for cookie in session_cookies),
                            'Session cookies must be host-only')
            self.assertTrue(all(cookie.has_nonstandard_attr('HttpOnly') for cookie in session_cookies),
                            'Session cookies must not be script-readable')
            self.assertTrue(all(cookie.get_nonstandard_attr('SameSite') == 'Lax'
                                for cookie in session_cookies), 'Session cookies require SameSite=Lax')
            self.assertTrue(all(cookie.expires is not None for cookie in session_cookies),
                            'The bounded session must expire')
            with client.open(fixture.url, timeout=2) as response:
                self.assertEqual(response.geturl(), fixture.url)
                self.assertIn(b'/fixture.js', response.read())
            root = origin(fixture.url)
            with client.open(f'{root}/project/{PROJECT}/doc/{DOCUMENT}/download', timeout=2) as response:
                self.assertEqual(response.read(), SOURCE.encode())
            fixture.release_assets.set()
            with client.open(f'{root}/project/{PROJECT}/file/{ASSET}', timeout=2) as response:
                self.assertEqual(response.read(), ASSET_BYTES)
            _, isolated = browser()
            self.assert_login_redirect(fixture, isolated, fixture.url)
            observations = fixture.auth_observation()
            self.assertEqual(observations['signins'], 1)
            self.assertEqual(observations['dashboard_requests'], 1)
            self.assertGreaterEqual(observations['authenticated_project_requests'], 1)
            self.assertEqual(observations['authenticated_documents'], 1)
            self.assertEqual(observations['authenticated_assets'], 1)
            self.assertEqual(fixture.save_count, 0)

    def test_same_cookie_client_can_reconnect_without_another_sign_in(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser()
            self.sign_in(fixture, client)
            for _ in range(3):
                with client.open(fixture.url, timeout=2) as response:
                    self.assertEqual(response.geturl(), fixture.url)
                    response.read()
            observations = fixture.auth_observation()
            self.assertEqual(observations['signins'], 1)
            self.assertEqual(observations['authenticated_project_requests'], 3)
            self.assertEqual(observations['login_requests'], 1)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_expiring_session_revokes_existing_cookie_for_project_and_hydration_until_sign_in(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            cookies, client = browser()
            self.sign_in(fixture, client)
            self.assertTrue(list(cookies), 'The client must really hold its old session cookie')
            fixture.expire_session()
            root = origin(fixture.url)
            for path in (f'/project/{PROJECT}', f'/project/{PROJECT}/doc/{DOCUMENT}/download',
                         f'/project/{PROJECT}/file/{ASSET}', '/fixture.js'):
                with self.subTest(path=path):
                    self.assert_login_redirect(fixture, client, root + path)
            self.assertEqual(fixture.auth_observation()['signins'], 1)
            self.assertEqual(fixture.auth_observation()['authenticated_documents'], 0)
            self.assertEqual(fixture.auth_observation()['authenticated_assets'], 0)
            self.sign_in(fixture, client)
            with client.open(fixture.url, timeout=2) as response:
                self.assertEqual(response.geturl(), fixture.url)
                response.read()
            self.assertEqual(fixture.auth_observation()['signins'], 2)
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_invalid_csrf_is_forbidden_and_cannot_create_a_session(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            cookies, client = browser()
            action, fields = self.login_form(fixture, client)
            invalid_fields = {name: 'invalid-fixture-token' for name in fields}
            request = Request(action, data=urlencode(invalid_fields).encode('ascii'), method='POST',
                              headers={'Content-Type': 'application/x-www-form-urlencoded',
                                       'Origin': origin(fixture.login_url)})
            self.assert_forbidden(client, request)
            self.assertFalse(list(cookies), 'Rejected sign-in must not issue a session cookie')
            self.assertEqual(fixture.auth_observation()['signins'], 0)
            self.assert_login_redirect(fixture, client, fixture.url)
            self.assertEqual(fixture.save_count, 0)

    def test_cross_origin_sign_in_is_forbidden_even_with_a_real_form_token(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            cookies, client = browser()
            action, fields = self.login_form(fixture, client)
            foreign_origin = origin(fixture.login_url).replace('127.0.0.1', 'localhost')
            self.assertNotEqual(foreign_origin, origin(fixture.login_url))
            request = Request(action, data=urlencode(fields).encode('ascii'), method='POST',
                              headers={'Content-Type': 'application/x-www-form-urlencoded',
                                       'Origin': foreign_origin})
            self.assert_forbidden(client, request)
            self.assertFalse(list(cookies), 'A foreign origin cannot create a local session')
            self.assertEqual(fixture.auth_observation()['signins'], 0)
            self.assert_login_redirect(fixture, client, fixture.url)
            self.sign_in(fixture, client)
            self.assertEqual(fixture.auth_observation()['signins'], 1)

    def test_distinct_cookie_clients_keep_distinct_sessions_in_opt_in_profile_fixture(self):
        with AuthStorageFixture(multi_sessions=True) as fixture:
            fixture.enable_auth()
            _, first = browser()
            _, second = browser()
            _, isolated = browser()
            self.sign_in(fixture, first)
            self.sign_in(fixture, second)
            for client in (first, second, first):
                with client.open(fixture.url, timeout=2) as response:
                    self.assertEqual(response.geturl(), fixture.url)
                    response.read()
            self.assertEqual(fixture.auth_observation()['signins'], 2)
            self.assertEqual(fixture.session_project_counts(), {1: 2, 2: 1})
            self.assert_login_redirect(fixture, isolated, fixture.url)
            fixture.expire_session()
            for client in (first, second):
                self.assert_login_redirect(fixture, client, fixture.url)
            self.assertEqual(fixture.session_project_counts(), {})

    def test_multi_session_fixture_rejects_more_than_sixteen_live_sessions(self):
        with AuthStorageFixture(multi_sessions=True) as fixture:
            fixture.enable_auth()
            for _ in range(16):
                _, client = browser()
                self.sign_in(fixture, client)
            _, rejected = browser()
            action, fields = self.login_form(fixture, rejected)
            request = Request(action, data=urlencode(fields).encode('ascii'), method='POST',
                              headers={'Content-Type': 'application/x-www-form-urlencoded',
                                       'Origin': fixture.origin})
            with self.assertRaises(HTTPError) as limited:
                rejected.open(request, timeout=2)
            try:
                self.assertEqual(limited.exception.code, 429)
            finally:
                limited.exception.close()
            self.assertEqual(fixture.auth_observation()['signins'], 16)

    def test_unauthenticated_source_write_cannot_mutate_the_authoritative_document(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser(follow_redirects=False)
            request = Request(origin(fixture.url) + '/fixture/source',
                              data=b'Unauthenticated replacement', method='POST')
            with self.assertRaises(HTTPError) as redirect:
                client.open(request, timeout=2)
            try:
                self.assertEqual(redirect.exception.code, 303)
                self.assertEqual(urljoin(fixture.url, redirect.exception.headers['Location']),
                                 fixture.login_url)
            finally:
                redirect.exception.close()
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_file_csrf_does_not_authorize_delete_without_an_authenticated_session(self):
        from overleaf_files_fixture import CSRF
        with AuthStorageFixture(file_actions=True) as fixture:
            fixture.enable_auth()
            _, client = browser(follow_redirects=False)
            original = fixture.files.entry('main.txt')
            request = Request(f'{fixture.url}/doc/{DOCUMENT}', data=b'', method='DELETE',
                              headers={'X-Csrf-Token': CSRF})
            with self.assertRaises(HTTPError) as redirect:
                client.open(request, timeout=2)
            try:
                self.assertEqual(redirect.exception.code, 303)
                self.assertEqual(urljoin(fixture.url, redirect.exception.headers['Location']),
                                 fixture.login_url)
            finally:
                redirect.exception.close()
            self.assertEqual(fixture.files.entry('main.txt'), original)
            self.assertEqual(fixture.files.operations, [])
            self.assertEqual(fixture.save_count, 0)
            _, authenticated = browser()
            self.sign_in(fixture, authenticated)
            with authenticated.open(request, timeout=2) as response:
                self.assertEqual(response.status, 200)
                response.read()
            self.assertIsNone(fixture.files.entry('main.txt'))
            self.assertEqual(fixture.files.operations,
                             [dict(action='delete', id=DOCUMENT, kind='doc')])

    def test_hostile_child_routes_and_capability_reports_are_inert_until_explicitly_armed(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser()
            self.sign_in(fixture, client)
            for kind in ('iframe', 'popup'):
                routes = ((fixture.url + '?fixture-attack=' + kind, 404),
                          (origin(fixture.url) + '/fixture/auth/attack-attempt?kind='
                           + kind + '&available=1', 400))
                for url, expected in routes:
                    with self.subTest(kind=kind, status=expected):
                        with self.assertRaises(HTTPError) as rejected:
                            client.open(url, timeout=2)
                        try:
                            self.assertEqual(rejected.exception.code, expected)
                        finally:
                            rejected.exception.close()
            observations = fixture.auth_observation()
            self.assertTrue(all(not observations[key]
                                for key in ('iframe_attempted', 'iframe_ipc_available',
                                            'popup_attempted', 'popup_ipc_available')))
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_armed_hostile_child_routes_are_bounded_and_report_only_capability_booleans(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser()
            self.sign_in(fixture, client)
            with client.open(fixture.url, timeout=2) as response:
                unarmed = response.read()
            self.assertNotIn(b'fixture-attack=', unarmed)
            self.assertNotIn(b'window.webkit', unarmed)
            fixture.arm_attacks()
            with client.open(fixture.url, timeout=2) as response:
                armed = response.read()
            self.assertIn(b'?fixture-attack=iframe', armed)
            self.assertIn(b'?fixture-attack=popup', armed)
            self.assertIn(b'<iframe', armed)
            self.assertIn(b'window.open(', armed)
            # urllib does not execute these pages. Native isolation must still
            # be proved separately by the real WebView journey.
            for kind, available in (('iframe', '1'), ('popup', '0')):
                with self.subTest(kind=kind):
                    with client.open(fixture.url + '?fixture-attack=' + kind, timeout=2) as response:
                        page = response.read()
                    self.assertLess(len(page), 4096)
                    self.assertIn(b'window.webkit', page)
                    self.assertIn(b'postMessage', page)
                    self.assertIn(b'auth-fixture-frame-poison', page)
                    report = (origin(fixture.url) + '/fixture/auth/attack-attempt?kind='
                              + kind + '&available=' + available)
                    with client.open(report, timeout=2) as response:
                        self.assertEqual(response.status, 204)
            observations = fixture.auth_observation()
            self.assertTrue(observations['iframe_attempted'])
            self.assertTrue(observations['iframe_ipc_available'])
            self.assertTrue(observations['popup_attempted'])
            self.assertFalse(observations['popup_ipc_available'])
            self.assertEqual(fixture.source, SOURCE)
            self.assertEqual(fixture.save_count, 0)

    def test_auth_observations_expose_only_bounded_counts_and_attack_booleans(self):
        with AuthStorageFixture() as fixture:
            fixture.enable_auth()
            _, client = browser()
            self.sign_in(fixture, client)
            observations = fixture.auth_observation()
            counters = {'login_requests', 'signins', 'dashboard_requests',
                        'authenticated_project_requests', 'authenticated_documents',
                        'authenticated_assets'}
            flags = {'iframe_attempted', 'iframe_ipc_available', 'popup_attempted',
                     'popup_ipc_available'}
            self.assertEqual(set(observations), counters | flags,
                             'Observations must never expose cookie, token, or document values')
            self.assertTrue(all(type(observations[key]) is int and observations[key] >= 0
                                for key in counters))
            self.assertTrue(all(type(observations[key]) is bool for key in flags))
            self.assertLess(len(json.dumps(observations)), 512)


if __name__ == '__main__':
    unittest.main()
