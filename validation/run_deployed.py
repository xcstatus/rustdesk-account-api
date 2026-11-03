#!/usr/bin/env python3
"""Controlled smoke checks for a deployed, single-instance account API.

No network requests run without --execute. Supply --origin (HTTPS), --username
(default administrator), --password-file (regular, non-symlink, mode 0600), and
--report. Example: --execute --origin https://desk.166198.xyz --password-file
/private/path/password --report /private/path/smoke-report.json

Only the selected account's initially empty personal book and this run's sessions
are changed. Use a quiet maintenance window: the API has no compare-and-swap for
books. No accounts or passwords are created, removed, or changed. Audit records
of login/logout remain. HTTP redirect checks use the same hostname on port 80.
TLS uses the system trust configuration; redirects are never followed. Reports
contain only fixed case IDs, statuses, and fixed diagnostic messages.
"""

import argparse
import copy
import hashlib
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
import re
import signal
import ssl
import stat
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid


DEFAULT_REPORT = Path(__file__).resolve().parent / '.runtime/deployed-smoke-report.json'
MAX_RESPONSE = 2 * 1024 * 1024
TOKEN = re.compile(r'[0-9a-fA-F]{64}\Z')
MESSAGES = {
    'execute': 'Execution refused; --execute is required to authorize HTTP tests and temporary writes.',
    'arguments': 'Invalid or incomplete command-line arguments.',
    'origin': 'A valid HTTPS origin without credentials, path, query or fragment is required.',
    'password': 'Password file must be a readable regular non-symlink file with mode 0600 and a nonempty UTF-8 password.',
    'report': 'The private report could not be written safely.',
    'response': 'The expected HTTP or response contract was not satisfied.',
    'request': 'An HTTP request failed; response and exception details were withheld.',
    'empty': 'Address-book writes refused; both peers and tags must initially be empty and the complete snapshot must support restoration.',
    'concurrent': 'Address-book contents changed unexpectedly; overwriting unrelated data was refused.',
    'restore': 'The complete original address book could not be restored and verified.',
    'revoke': 'A session created by this run could not be confirmed revoked.',
    'uncertain': 'A login response was lost or unusable; an untracked session may remain.',
    'interrupted': 'Execution was interrupted; cleanup was attempted.',
    'internal': 'The check failed unexpectedly; sensitive details were withheld.',
}


class Failure(Exception):
    def __init__(self, reason='response'):
        self.reason = reason if reason in MESSAGES else 'internal'
        super().__init__(MESSAGES[self.reason])


class Halt(Exception):
    pass


def require(condition, reason='response'):
    if not condition:
        raise Failure(reason)


class SafeParser(argparse.ArgumentParser):
    def error(self, _message):
        raise Failure('arguments')


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, _request, _response, _code, _message, _headers, _url):
        return None


class HTTP:
    def __init__(self, origin):
        self.origin = origin
        parsed = urllib.parse.urlsplit(origin)
        host = parsed.hostname
        self.http_origin = 'http://' + ('[' + host + ']' if ':' in host else host)
        # No cookie jar, environment proxies, custom CA, or cross-origin redirect.
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect(),
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
        )

    def request(self, method, path, *, data=None, headers=None, plain_http=False):
        require(path.startswith('/') and not path.startswith('//'))
        payload = None if data is None else json.dumps(data, ensure_ascii=False).encode('utf-8')
        request_headers = {'Accept': 'application/json', 'User-Agent': 'RustDesk-Controlled-Smoke/1'}
        if payload is not None:
            request_headers['Content-Type'] = 'application/json'
        request_headers.update(headers or {})
        # The only plaintext request is an unauthenticated redirect probe.
        require(not plain_http or (method == 'GET' and data is None and not headers))
        request = urllib.request.Request(
            (self.http_origin if plain_http else self.origin) + path,
            data=payload, headers=request_headers, method=method,
        )
        try:
            try:
                response = self.opener.open(request, timeout=15)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                body = response.read(MAX_RESPONSE + 1)
                require(len(body) <= MAX_RESPONSE)
                return response.status, response.headers, body
        except Failure:
            raise
        except Exception:
            raise Failure('request') from None


def value(response):
    try:
        return json.loads(response[2])
    except (ValueError, UnicodeError, TypeError):
        raise Failure('response') from None


def no_store(response):
    directives = [part.strip().lower() for part in response[1].get('Cache-Control', '').split(',')]
    require('no-store' in directives)


def normalized_origin(raw):
    try:
        require(isinstance(raw, str) and not any(character.isspace() for character in raw), 'origin')
        parsed = urllib.parse.urlsplit(raw)
        require(parsed.scheme == 'https' and parsed.hostname and not parsed.username
                and parsed.password is None and parsed.path in ('', '/')
                and not parsed.query and not parsed.fragment, 'origin')
        host = parsed.hostname.encode('idna').decode('ascii').lower()
        host = '[' + host + ']' if ':' in host else host
        port = parsed.port
        require(port is None or 1 <= port <= 65535, 'origin')
        return 'https://' + host + (':' + str(port) if port and port != 443 else '')
    except Failure:
        raise
    except (ValueError, UnicodeError, TypeError, AttributeError):
        raise Failure('origin') from None


def read_password(path):
    descriptor = None
    try:
        require(path is not None and not path.is_symlink(), 'password')
        before = path.lstat()
        require(stat.S_ISREG(before.st_mode) and stat.S_IMODE(before.st_mode) == 0o600, 'password')
        descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0))
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600
                and (before.st_dev, before.st_ino) == (metadata.st_dev, metadata.st_ino), 'password')
        with os.fdopen(descriptor, 'rb') as stream:
            descriptor = None
            raw = stream.read(4098)
        # Same newline convention as the API's _FILE settings; preserve other spaces.
        if raw.endswith(b'\n'):
            raw = raw[:-1]
            if raw.endswith(b'\r'):
                raw = raw[:-1]
        require(0 < len(raw) <= 4096, 'password')
        return raw.decode('utf-8')
    except Failure:
        raise
    except (OSError, ValueError, UnicodeError, AttributeError):
        raise Failure('password') from None
    finally:
        if descriptor is not None:
            os.close(descriptor)


def write_report(path, report, password_path=None):
    temporary = None
    try:
        # Do not let an output typo replace the password file, including hardlinks.
        if password_path is not None:
            require(path.resolve() != password_path.resolve(), 'report')
            if path.exists() and password_path.exists():
                require(not os.path.samefile(path, password_path), 'report')
        require(not path.is_symlink(), 'report')
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor, temporary = tempfile.mkstemp(prefix='.deployed-report-', dir=path.parent)
        with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
            json.dump(report, stream, indent=2)
            stream.write('\n')
        os.replace(temporary, path)
        temporary = None
    except Failure:
        raise
    except (OSError, ValueError, TypeError):
        raise Failure('report') from None
    finally:
        if temporary is not None:
            try:
                os.unlink(temporary)
            except OSError:
                pass


class Runner:
    def __init__(self, origin, username, password, transport=None):
        self.origin, self.username, self.password = origin, username, password
        self.http = transport if transport is not None else HTTP(origin)
        self.report = {'status': 'running', 'cleanup_status': 'not_required', 'cases': []}
        self.clients = []
        self.web_sessions = []
        self.uncertain_logins = 0
        self.original = None
        self.owned_books = []
        self.dirty = False
        self.user_id = None

    def record(self, identifier, status, reason=None):
        item = {'id': identifier, 'status': status}
        if reason:
            item['message'] = MESSAGES[reason]
        self.report['cases'].append(item)
        print(identifier + ': ' + status.upper(), flush=True)

    def case(self, identifier, check, *, cleanup=False):
        try:
            result = check()
        except (Exception, KeyboardInterrupt) as error:
            reason = error.reason if isinstance(error, Failure) else (
                'interrupted' if isinstance(error, KeyboardInterrupt) else 'internal')
            self.record(identifier, 'failed', reason)
            self.report['status'] = 'failed'
            if cleanup:
                self.report['cleanup_status'] = 'failed'
                return None
            raise Halt() from None
        self.record(identifier, 'passed')
        return result

    @staticmethod
    def bearer(token):
        return {'Authorization': 'Bearer ' + token}

    def web_headers(self, web, *, origin=False, csrf=False):
        headers = {'Cookie': 'rd_web_session=' + web['token']}
        if origin:
            headers['Origin'] = self.origin
        if csrf:
            headers['X-CSRF-Token'] = web['csrf']
        return headers

    def get_json(self, path, headers=None):
        response = self.http.request('GET', path, headers=headers)
        require(response[0] == 200)
        no_store(response)
        return value(response)

    def web_login(self):
        self.uncertain_logins += 1
        response = self.http.request('POST', '/admin/api/v1/auth/login',
                                     headers={'Origin': self.origin},
                                     data={'username': self.username, 'password': self.password})
        if response[0] in (400, 401, 403, 429):
            self.uncertain_logins -= 1
        parsed_cookies = []
        # Capture a usable cookie before checking body, status, or attributes, so
        # a regression in any of those checks still allows session cleanup.
        for line in response[1].get_all('Set-Cookie', []):
            cookies = SimpleCookie()
            cookies.load(line)
            morsel = cookies.get('rd_web_session')
            if morsel is not None and TOKEN.fullmatch(morsel.value):
                token = morsel.value
                web = {'token': token, 'csrf': hashlib.sha256(
                    ('rustdesk-web-csrf-v1:' + token).encode('ascii')).hexdigest()}
                self.web_sessions.append(web)
                parsed_cookies.append(morsel)
        if parsed_cookies and response[0] not in (400, 401, 403, 429):
            self.uncertain_logins -= 1
        require(response[0] == 200 and len(parsed_cookies) == 1)
        no_store(response)
        morsel = parsed_cookies[0]
        require(morsel['secure'] and morsel['httponly']
                and morsel['samesite'].lower() == 'lax'
                and morsel['path'] == '/admin/api/v1')
        body = value(response)
        web = self.web_sessions[-1]
        require(isinstance(body, dict) and 'access_token' not in body
                and body.get('csrf_token') == web['csrf'])
        user = body.get('user')
        require(isinstance(user, dict) and isinstance(user.get('id'), str)
                and bool(user['id']) and user.get('username') == self.username
                and user.get('role') == 'admin' and user.get('source') == 'local'
                and user.get('disabled') is False)
        self.user_id = user['id']
        return web

    def client_login(self):
        self.uncertain_logins += 1
        response = self.http.request('POST', '/api/login', data={
            'username': self.username, 'password': self.password, 'type': 'account',
            'id': 'controlled-smoke', 'uuid': uuid.uuid4().hex, 'autoLogin': False,
        })
        if response[0] in (400, 401, 403, 429):
            self.uncertain_logins -= 1
        body = value(response)
        token = body.get('access_token') if isinstance(body, dict) else None
        if isinstance(token, str) and TOKEN.fullmatch(token):
            self.clients.append(token)
            if response[0] not in (400, 401, 403, 429):
                self.uncertain_logins -= 1
        require(response[0] == 200 and isinstance(token, str) and TOKEN.fullmatch(token))
        no_store(response)
        require(body.get('type') == 'access_token' and isinstance(body.get('user'), dict)
                and body['user'].get('id') == self.user_id)
        return token

    def read_book(self, token):
        body = self.get_json('/api/ab', self.bearer(token))
        require(isinstance(body, dict) and isinstance(body.get('data'), str))
        try:
            snapshot = json.loads(body['data'])
        except (ValueError, TypeError):
            raise Failure('response') from None
        require(isinstance(snapshot, dict))
        return snapshot

    def empty_book_guard(self, first, second):
        original = self.read_book(first)
        require(original.get('peers') == [] and original.get('tags') == []
                and set(original).issubset({'peers', 'tags', 'tag_colors'}), 'empty')
        if 'tag_colors' in original:
            try:
                colors = json.loads(original['tag_colors'])
                valid = (isinstance(original['tag_colors'], str) and isinstance(colors, dict)
                         and all(type(color) is int and 0 <= color <= 0xffffffff
                                 for color in colors.values()))
            except (ValueError, TypeError):
                valid = False
            require(valid, 'empty')
        require(self.read_book(second) == original and self.read_book(first) == original, 'concurrent')
        self.original = copy.deepcopy(original)

    def write_book(self, token, snapshot, expected):
        require(self.original is not None)
        # Mark before sending: a transport failure can happen after a committed write.
        self.owned_books.extend([copy.deepcopy(snapshot), copy.deepcopy(expected)])
        self.dirty = True
        response = self.http.request('POST', '/api/ab', headers=self.bearer(token),
                                     data={'data': json.dumps(snapshot, ensure_ascii=False)})
        require(response[0] == 200)
        no_store(response)
        require(response[1].get('X-Rustdesk-Credential-Sync') == 'disabled; metadata-only')

    def restore_book(self):
        for token in self.clients:
            try:
                current = self.read_book(token)
            except (Exception, KeyboardInterrupt):
                continue
            require(current == self.original or current in self.owned_books, 'concurrent')
            try:
                response = self.http.request('POST', '/api/ab', headers=self.bearer(token),
                                             data={'data': json.dumps(self.original, ensure_ascii=False)})
                if response[0] == 200 and self.read_book(token) == self.original:
                    self.dirty = False
                    return
            except (Exception, KeyboardInterrupt):
                continue
        raise Failure('restore')

    def revoke_client(self, token):
        # Probe even if the logout response was lost after committing revocation.
        try:
            response = self.http.request('POST', '/api/logout', data={}, headers=self.bearer(token))
            logout_ok = response[0] in (200, 401)
        except (Exception, KeyboardInterrupt):
            logout_ok = True
        revoked = True
        for method, path in [('GET', '/api/ab'), ('POST', '/api/currentUser')]:
            try:
                response = self.http.request(method, path, data={} if method == 'POST' else None,
                                             headers=self.bearer(token))
                revoked = (response[0] == 401) and revoked
            except (Exception, KeyboardInterrupt):
                revoked = False
        require(logout_ok and revoked, 'revoke')

    def revoke_web(self, web):
        try:
            response = self.http.request('POST', '/admin/api/v1/auth/logout', data={},
                                         headers=self.web_headers(web, origin=True, csrf=True))
            logout_ok = response[0] in (204, 401)
        except (Exception, KeyboardInterrupt):
            logout_ok = True
        try:
            response = self.http.request('GET', '/admin/api/v1/me', headers=self.web_headers(web))
            require(logout_ok and response[0] == 401, 'revoke')
        except (Exception, KeyboardInterrupt):
            raise Failure('revoke') from None

    def cleanup(self):
        if self.clients or self.web_sessions or self.uncertain_logins or self.dirty:
            self.report['cleanup_status'] = 'passed'
        if self.dirty:
            self.case('BOOK_RESTORE', self.restore_book, cleanup=True)
        else:
            self.record('BOOK_RESTORE', 'skipped')
        for index, token in enumerate(self.clients, 1):
            self.case('CLIENT_LOGOUT_' + str(index), lambda token=token: self.revoke_client(token), cleanup=True)
        for index, web in enumerate(self.web_sessions, 1):
            self.case('WEB_LOGOUT_' + str(index), lambda web=web: self.revoke_web(web), cleanup=True)
        if self.uncertain_logins:
            self.case('UNTRACKED_LOGIN', lambda: require(False, 'uncertain'), cleanup=True)

    def checks(self):
        def health():
            response = self.http.request('GET', '/healthz')
            require(response[0] == 200 and value(response) == {'status': 'ok'})
        self.case('HEALTH', health)
        self.case('READY', lambda: require(self.http.request('GET', '/readyz')[0] == 200))

        def admin_html():
            response = self.http.request('GET', '/admin/')
            require(response[0] == 200 and response[1].get('Content-Type', '').lower().startswith('text/html')
                    and b'<html' in response[2].lower() and b'<script' in response[2].lower())
        self.case('ADMIN_HTML', admin_html)

        def redirect():
            response = self.http.request('GET', '/admin/', plain_http=True)
            target = urllib.parse.urlsplit(response[1].get('Location', ''))
            target_origin = normalized_origin(urllib.parse.urlunsplit((target.scheme, target.netloc, '', '', '')))
            require(response[0] in (301, 302, 303, 307, 308) and target_origin == self.origin
                    and target.path == '/admin/' and not target.query and not target.fragment)
        self.case('HTTP_REDIRECT', redirect)
        self.case('OIDC_CLIENT_DISABLED', lambda: require(self.get_json('/api/login-options') == []))
        self.case('OIDC_WEB_DISABLED', lambda: require(
            self.get_json('/admin/api/v1/auth/options') == {'oidc_enabled': False}))
        self.case('UNAUTHENTICATED_BOOK', lambda: require(self.http.request('GET', '/api/ab')[0] == 401))
        web = self.case('WEB_LOGIN', self.web_login)

        def me():
            body = self.get_json('/admin/api/v1/me', self.web_headers(web))
            require(isinstance(body, dict) and body.get('csrf_token') == web['csrf']
                    and isinstance(body.get('user'), dict) and body['user'].get('id') == self.user_id
                    and body['user'].get('role') == 'admin')
        self.case('WEB_ME', me)

        def admin_users():
            body = self.get_json('/admin/api/v1/users', self.web_headers(web))
            require(isinstance(body, dict) and isinstance(body.get('users'), list)
                    and any(isinstance(user, dict) and user.get('id') == self.user_id
                            and user.get('role') == 'admin' for user in body['users']))
        self.case('WEB_ADMIN_USERS', admin_users)

        def audit():
            query = urllib.parse.urlencode({'actor_id': self.user_id, 'action': 'login',
                                            'source': 'local', 'kind': 'web', 'result': 'success', 'limit': 20})
            body = self.get_json('/admin/api/v1/audit-events?' + query, self.web_headers(web))
            require(isinstance(body, dict) and isinstance(body.get('events'), list) and body['events'])
            for event in body['events']:
                require(isinstance(event, dict) and event.get('actor_id') == self.user_id
                        and event.get('action') == 'login' and event.get('source') == 'local'
                        and event.get('kind') == 'web' and event.get('result') == 'success')
        self.case('WEB_AUDIT_FILTER', audit)
        self.case('WEB_ORIGIN_GUARD', lambda: require(self.http.request(
            'POST', '/admin/api/v1/auth/logout', data={},
            headers=self.web_headers(web, csrf=True))[0] == 403))
        first = self.case('CLIENT_LOGIN_1', self.client_login)
        second = self.case('CLIENT_LOGIN_2', self.client_login)
        self.case('CLIENT_DISTINCT_SESSIONS', lambda: require(first != second))

        def scope():
            for path in ['/admin/api/v1/me', '/admin/api/v1/users', '/admin/api/v1/audit-events']:
                require(self.http.request('GET', path, headers=self.bearer(first))[0] == 401)
                require(self.http.request('GET', path, headers={'Cookie': 'rd_web_session=' + first})[0] == 401)
            require(self.http.request('GET', '/api/ab', headers=self.web_headers(web))[0] == 401)
        self.case('CLIENT_WEB_SESSION_ISOLATION', scope)

        def identities():
            for token in [first, second]:
                response = self.http.request('POST', '/api/currentUser', data={}, headers=self.bearer(token))
                require(response[0] == 200)
                body = value(response)
                require(isinstance(body, dict) and body.get('id') == self.user_id)
        self.case('CLIENT_IDENTITY', identities)
        self.case('BOOK_EMPTY_GUARD', lambda: self.empty_book_guard(first, second))
        marker = 'controlled-smoke-' + uuid.uuid4().hex
        expected = {'peers': [{'id': marker, 'alias': 'Smoke metadata one',
                               'platform': 'API smoke', 'tags': [marker]}],
                    'tags': [marker], 'tag_colors': json.dumps({marker: 4281558681})}
        payload = copy.deepcopy(expected)
        payload['peers'][0].update({'password': 'synthetic-never-store', 'hash': 'synthetic-never-store'})
        payload.update({'password': 'synthetic-never-store', 'hash': 'synthetic-never-store'})
        self.case('BOOK_WRITE_1', lambda: self.write_book(first, payload, expected))
        self.case('BOOK_SYNC_2_CREDENTIAL_STRIP', lambda: require(self.read_book(second) == expected))
        expected_two = copy.deepcopy(expected)
        expected_two['peers'][0]['alias'] = 'Smoke metadata two'
        payload_two = copy.deepcopy(expected_two)
        payload_two['peers'][0].update({'password': 'synthetic-never-store', 'hash': 'synthetic-never-store'})
        self.case('BOOK_WRITE_2', lambda: self.write_book(second, payload_two, expected_two))
        self.case('BOOK_SYNC_1_CREDENTIAL_STRIP', lambda: require(self.read_book(first) == expected_two))

    def run(self):
        try:
            self.checks()
        except Halt:
            pass
        except (Exception, KeyboardInterrupt) as error:
            self.record('RUN', 'failed', 'interrupted' if isinstance(error, KeyboardInterrupt) else 'internal')
            self.report['status'] = 'failed'
        finally:
            try:
                self.cleanup()
            except (Exception, KeyboardInterrupt):
                self.record('CLEANUP', 'failed', 'internal')
                self.report['status'] = 'failed'
                self.report['cleanup_status'] = 'failed'
            finally:
                self.password = None
        if self.report['status'] == 'running':
            self.report['status'] = 'passed'
        return self.report


def main(argv=None):
    parser = SafeParser(description=__doc__)
    parser.add_argument('--execute', action='store_true', help='authorize HTTP checks and temporary personal-book writes')
    parser.add_argument('--origin', help='HTTPS origin of the explicitly authorized deployment')
    parser.add_argument('--username', default='administrator')
    parser.add_argument('--password-file', type=Path)
    parser.add_argument('--report', type=Path, default=DEFAULT_REPORT, help='private, redacted JSON report output')
    args = None
    report = {'status': 'refused', 'cleanup_status': 'not_required', 'cases': []}
    report_path = DEFAULT_REPORT
    password_path = None
    try:
        args, unknown = parser.parse_known_args(argv)
        report_path, password_path = args.report, args.password_file
        require(not unknown, 'arguments')
        require(args.execute, 'execute')
        origin = normalized_origin(args.origin)
        require(isinstance(args.username, str) and 0 < len(args.username.encode('utf-8')) <= 128, 'arguments')
        # Establish a writable report before reading credentials or making requests.
        write_report(report_path, {'status': 'running', 'cleanup_status': 'not_required', 'cases': []}, password_path)
        password = read_password(password_path)
        runner = Runner(origin, args.username, password)
        report = runner.report
        password = None
        previous_term = signal.getsignal(signal.SIGTERM)
        def interrupted(_signal, _frame):
            raise KeyboardInterrupt()
        signal.signal(signal.SIGTERM, interrupted)
        try:
            report = runner.run()
        finally:
            signal.signal(signal.SIGTERM, previous_term)
    except (Exception, KeyboardInterrupt) as error:
        reason = error.reason if isinstance(error, Failure) else (
            'interrupted' if isinstance(error, KeyboardInterrupt) else 'internal')
        report['cases'].append({'id': 'PREFLIGHT', 'status': 'failed', 'message': MESSAGES[reason]})
        if args is not None and args.execute:
            report['status'] = 'failed'
        print('PREFLIGHT: FAILED', flush=True)
    try:
        write_report(report_path, report, password_path)
    except Failure:
        print('REPORT_WRITE: FAILED', file=sys.stderr, flush=True)
        return 1
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
