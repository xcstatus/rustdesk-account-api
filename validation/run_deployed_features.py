#!/usr/bin/env python3
"""HTTPS smoke checks using one newly created, ordinary validation account.

The default is a preview: no requests, credential reads, or output files. To run:
  --execute --base https://accounts.example --admin-username administrator
  --admin-password-file /private/path/password --fixture-file /private/path/new.json
  --report-file /private/path/new-report.json [--keep-member-for-manual]

Both output files must be new. The fixture's existing parent directory must be
private (owned by this user, no group/other permissions). Files use mode 0600.
The fixture contains base/username/password/member_id for two real clients; never
share it or commit it. Only --keep-member-for-manual keeps the new member enabled
after successful checks and member cleanup. Otherwise the member is disabled.

Only this run's new member, its initially empty book, and newly issued sessions
are changed. Client logins omit device identity so no synthetic devices are
registered. No existing administrator book, roles, devices, or SQL are modified.
Cleanup clears the new book, revokes all of this member's sessions, verifies known
tokens are rejected, and logs out this run's admin session. Failed requests can
have committed: cleanup is best effort and any uncertainty fails the report.
System TLS verification is mandatory; redirects and environment proxies are off.
Reports and console output contain fixed case IDs/statuses/messages only.
"""

import argparse
import base64
import copy
import hashlib
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
import secrets
import signal
import stat
import sys
import urllib.parse

sys.dont_write_bytecode = True
try:
    from . import run_deployed as smoke
except ImportError:
    import run_deployed as smoke


require = smoke.require
Failure = smoke.Failure


class NewJSON:
    """Reserve a new file exclusively, then update only that owned descriptor."""

    def __init__(self, path, *, private_parent=False):
        descriptor = None
        try:
            require(path is not None and path.is_absolute(), 'report')
            parent = path.parent.stat()
            require(stat.S_ISDIR(parent.st_mode), 'report')
            if private_parent:
                require(parent.st_uid == os.geteuid()
                        and stat.S_IMODE(parent.st_mode) & 0o077 == 0, 'report')
            descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL
                                 | getattr(os, 'O_NOFOLLOW', 0), 0o600)
            os.fchmod(descriptor, 0o600)
            self.stream = os.fdopen(descriptor, 'w+', encoding='utf-8')
            descriptor = None
        except Failure:
            raise
        except (OSError, ValueError, TypeError, AttributeError):
            raise Failure('report') from None
        finally:
            if descriptor is not None:
                os.close(descriptor)

    def write(self, value):
        try:
            self.stream.seek(0)
            json.dump(value, self.stream, ensure_ascii=False, indent=2)
            self.stream.write('\n')
            self.stream.truncate()
            self.stream.flush()
            os.fsync(self.stream.fileno())
        except (OSError, ValueError, TypeError):
            raise Failure('report') from None

    def close(self):
        self.stream.close()


class Runner:
    def __init__(self, base, admin_username, admin_password, fixture, report, keep,
                 transport=None):
        self.base = base
        self.admin_username = admin_username
        self.admin_password = admin_password
        self.fixture_file = fixture
        self.report_file = report
        self.keep = keep
        self.http = transport if transport is not None else smoke.HTTP(base)
        self.fixture = {'base': base, 'username': 'client-validation-' + secrets.token_hex(12),
                        'password': secrets.token_urlsafe(32), 'member_id': None,
                        'account_state': 'not_created'}
        self.report = {'status': 'running', 'cleanup_status': 'not_required', 'cases': []}
        self.webs = []
        self.clients = []
        self.admin_login_uncertain = False
        self.create_attempted = False
        self.empty_verified = False
        self.dirty = False
        self.output_failed = False
        self.fixture_file.write(self.fixture)
        self.report_file.write(self.report)

    def case(self, identifier, check, *, cleanup=False):
        try:
            result = check()
        except (Exception, KeyboardInterrupt) as error:
            reason = error.reason if isinstance(error, Failure) else (
                'interrupted' if isinstance(error, KeyboardInterrupt) else 'internal')
            self.report['status'] = 'failed'
            if cleanup:
                self.report['cleanup_status'] = 'failed'
            self.record(identifier, 'failed', reason)
            if not cleanup:
                raise smoke.Halt() from None
            return False
        self.record(identifier, 'passed')
        return result

    def record(self, identifier, status, reason=None):
        item = {'id': identifier, 'status': status}
        if reason:
            item['message'] = smoke.MESSAGES[reason]
        self.report['cases'].append(item)
        try:
            print(identifier + ': ' + status.upper(), flush=True)
        except OSError:
            self.output_failed = True
            self.report['status'] = 'failed'
        try:
            self.report_file.write(self.report)
        except Failure:
            # An output failure must not interrupt subsequent session cleanup.
            self.output_failed = True
            self.report['status'] = 'failed'

    @staticmethod
    def bearer(token):
        return {'Authorization': 'Bearer ' + token}

    def web_headers(self, web=None, *, mutation=False):
        web = self.webs[-1] if web is None else web
        headers = {'Cookie': 'rd_web_session=' + web['token']}
        if mutation:
            headers.update({'Origin': self.base, 'X-CSRF-Token': web['csrf']})
        return headers

    def get(self, path, headers):
        response = self.http.request('GET', path, headers=headers)
        require(response[0] == 200)
        smoke.no_store(response)
        return smoke.value(response)

    def admin_login(self):
        self.admin_login_uncertain = True
        response = self.http.request('POST', '/admin/api/v1/auth/login',
                                     headers={'Origin': self.base}, data={
                                         'username': self.admin_username,
                                         'password': self.admin_password})
        if response[0] in (400, 401, 403, 429):
            self.admin_login_uncertain = False
        cookies = []
        for line in response[1].get_all('Set-Cookie', []):
            parsed = SimpleCookie()
            parsed.load(line)
            cookie = parsed.get('rd_web_session')
            if cookie is not None and smoke.TOKEN.fullmatch(cookie.value):
                self.webs.append({'token': cookie.value, 'csrf': hashlib.sha256(
                    ('rustdesk-web-csrf-v1:' + cookie.value).encode('ascii')).hexdigest()})
                cookies.append(cookie)
        if cookies:
            self.admin_login_uncertain = False
        require(response[0] == 200 and len(cookies) == 1)
        smoke.no_store(response)
        cookie = cookies[0]
        require(cookie['secure'] and cookie['httponly']
                and cookie['samesite'].lower() == 'lax'
                and cookie['path'] == '/admin/api/v1')
        body = smoke.value(response)
        require(isinstance(body, dict) and 'access_token' not in body
                and body.get('csrf_token') == self.webs[-1]['csrf'])
        user = body.get('user')
        require(isinstance(user, dict) and user.get('username') == self.admin_username
                and user.get('role') == 'admin' and user.get('disabled') is False)
        current = self.get('/admin/api/v1/me', self.web_headers())
        require(current.get('user') == user and current.get('csrf_token') == body['csrf_token'])

    def admin_guards(self):
        web = self.webs[-1]
        # Rejected logout requests exercise guards without creating extra users.
        for headers in [self.web_headers(),
                        {**self.web_headers(), 'Origin': self.base},
                        {**self.web_headers(), 'X-CSRF-Token': web['csrf']},
                        {**self.web_headers(mutation=True), 'X-CSRF-Token': 'incorrect'}]:
            require(self.http.request('POST', '/admin/api/v1/auth/logout',
                                     headers=headers, data={})[0] == 403)

    def created_user(self, user):
        require(isinstance(user, dict) and user.get('username') == self.fixture['username']
                and user.get('role') == 'member' and user.get('source') == 'local'
                and user.get('disabled') is False
                and isinstance(user.get('id'), str) and bool(user['id'])
                and len(user['id']) <= 256)
        self.fixture['member_id'] = user['id']
        self.fixture['account_state'] = 'running'
        self.fixture_file.write(self.fixture)

    def create_member(self):
        users = self.get('/admin/api/v1/users', self.web_headers())
        require(isinstance(users, dict) and isinstance(users.get('users'), list))
        require(all(user.get('username') != self.fixture['username'] for user in users['users']))
        self.create_attempted = True
        response = self.http.request('POST', '/admin/api/v1/users',
                                     headers=self.web_headers(mutation=True), data={
                                         'username': self.fixture['username'],
                                         'password': self.fixture['password'], 'role': 'member'})
        if response[0] == 201:
            # Capture ownership before checking ancillary response attributes.
            self.created_user(smoke.value(response).get('user'))
        require(response[0] == 201)
        smoke.no_store(response)

    def member_path(self):
        require(self.fixture['member_id'] is not None)
        return '/admin/api/v1/users/' + urllib.parse.quote(self.fixture['member_id'], safe='')

    def client_login(self):
        # Deliberately omit id, uuid, and deviceInfo: this creates no fake device.
        response = self.http.request('POST', '/api/login', data={
            'username': self.fixture['username'], 'password': self.fixture['password'],
            'type': 'account'})
        body = smoke.value(response)
        token = body.get('access_token') if isinstance(body, dict) else None
        if isinstance(token, str) and smoke.TOKEN.fullmatch(token):
            self.clients.append(token)
        require(response[0] == 200 and isinstance(token, str) and smoke.TOKEN.fullmatch(token))
        smoke.no_store(response)
        require(body.get('type') == 'access_token' and isinstance(body.get('user'), dict)
                and body['user'].get('id') == self.fixture['member_id']
                and body['user'].get('is_admin') is False)
        return token

    def read_book(self, token):
        body = self.get('/api/ab', self.bearer(token))
        require(isinstance(body, dict) and isinstance(body.get('data'), str))
        try:
            snapshot = json.loads(body['data'])
        except (TypeError, ValueError):
            raise Failure('response') from None
        require(isinstance(snapshot, dict))
        return snapshot

    def empty_guard(self):
        for token in self.clients:
            snapshot = self.read_book(token)
            require(snapshot.get('peers') == [] and snapshot.get('tags') == []
                    and set(snapshot).issubset({'peers', 'tags', 'tag_colors'}), 'empty')
        self.empty_verified = True

    def write_book(self, token, snapshot):
        require(self.empty_verified and self.fixture['member_id'] is not None)
        self.dirty = True  # A lost response may follow a committed write.
        response = self.http.request('POST', '/api/ab', headers=self.bearer(token),
                                     data={'data': json.dumps(snapshot, ensure_ascii=False)})
        require(response[0] == 200)
        smoke.no_store(response)
        require(response[1].get('X-Rustdesk-Credential-Sync') == 'enabled; personal-salted-hash')

    def credentials(self):
        first, second = self.clients
        synthetic_hash = base64.b64encode(secrets.token_bytes(32)).decode('ascii')
        expected = {'peers': [{'id': 'synthetic-validation-peer', 'alias': 'HTTPS validation',
                               'platform': 'API smoke', 'tags': ['validation'], 'hash': synthetic_hash}],
                    'tags': ['validation'], 'tag_colors': '{"validation":4281558681}'}
        upload = copy.deepcopy(expected)
        upload['peers'][0]['password'] = 'synthetic-plaintext-must-be-stripped'
        self.write_book(first, upload)
        require(self.read_book(second) == expected)
        updated = copy.deepcopy(expected)
        updated['peers'][0]['alias'] = 'HTTPS validation updated'
        self.write_book(second, updated)
        require(self.read_book(first) == updated)
        invalid = copy.deepcopy(updated)
        invalid['peers'][0].update({'hash': 'invalid-synthetic-hash', 'alias': 'must-not-persist'})
        response = self.http.request('POST', '/api/ab', headers=self.bearer(first),
                                     data={'data': json.dumps(invalid)})
        require(response[0] == 400)
        require(self.read_book(second) == updated)

    @staticmethod
    def no_uuid(value):
        if isinstance(value, dict):
            require(all('uuid' not in key.lower() for key in value))
            for item in value.values():
                Runner.no_uuid(item)
        elif isinstance(value, list):
            for item in value:
                Runner.no_uuid(item)

    def directories(self):
        for token in self.clients:
            headers = self.bearer(token)
            require(self.get('/api/peers?current=1&pageSize=1', headers) == {'total': 0, 'data': []})
            users = self.get('/api/users?current=1&pageSize=1', headers)
            require(isinstance(users, dict) and users.get('total') == 1
                    and isinstance(users.get('data'), list) and len(users['data']) == 1)
            user = users['data'][0]
            require(user.get('id') == self.fixture['member_id']
                    and user.get('name') == self.fixture['username'] and user.get('is_admin') is False)
            self.no_uuid(users)
            require(self.get('/api/users?current=2&pageSize=1', headers) == {'total': 1, 'data': []})
            require(self.get('/api/device-group/accessible', headers) == {'total': 0, 'data': []})
            require(self.http.request('GET', '/admin/api/v1/devices', headers=headers)[0] == 401)
        devices = self.get('/admin/api/v1/devices', self.web_headers())
        require(isinstance(devices, dict) and isinstance(devices.get('devices'), list))
        self.no_uuid(devices)

    def recover_member(self):
        if self.fixture['member_id'] is not None or not self.create_attempted:
            return
        users = self.get('/admin/api/v1/users', self.web_headers())
        matches = [user for user in users.get('users', [])
                   if user.get('username') == self.fixture['username']]
        require(len(matches) <= 1)
        if matches:
            self.created_user(matches[0])

    def clear_book(self):
        if not self.dirty:
            return
        def clear(token):
            response = self.http.request('POST', '/api/ab', headers=self.bearer(token),
                                         data={'data': '{"peers":[],"tags":[]}'})
            if response[0] == 200 and self.read_book(token) == {'peers': [], 'tags': []}:
                self.dirty = False
                return True
            return False

        for token in self.clients:
            try:
                if clear(token):
                    return
            except (Exception, KeyboardInterrupt):
                pass
        # Expired or unexpectedly revoked tokens must not prevent a cleanup retry.
        try:
            if clear(self.client_login()):
                return
        except (Exception, KeyboardInterrupt):
            pass
        raise Failure('restore')

    def revoke_members(self):
        response = self.http.request('DELETE', self.member_path() + '/sessions',
                                     headers=self.web_headers(mutation=True))
        require(response[0] == 204, 'revoke')

    def revoke_client(self, token):
        # Already-revoked tokens return 401; this also handles uncertain responses.
        try:
            self.http.request('POST', '/api/logout', data={}, headers=self.bearer(token))
        except (Exception, KeyboardInterrupt):
            pass
        for method, path in [('GET', '/api/ab'), ('POST', '/api/currentUser')]:
            require(self.http.request(method, path, headers=self.bearer(token),
                                      data={} if method == 'POST' else None)[0] == 401, 'revoke')

    def disable_member(self):
        response = self.http.request('PATCH', self.member_path(),
                                     headers=self.web_headers(mutation=True), data={'disabled': True})
        require(response[0] == 200)
        user = smoke.value(response).get('user')
        require(isinstance(user, dict) and user.get('id') == self.fixture['member_id']
                and user.get('disabled') is True and user.get('role') == 'member')
        self.fixture['account_state'] = 'disabled'
        self.fixture_file.write(self.fixture)

    def revoke_admin(self, web):
        try:
            self.http.request('POST', '/admin/api/v1/auth/logout', data={},
                              headers=self.web_headers(web, mutation=True))
        except (Exception, KeyboardInterrupt):
            pass
        require(self.http.request('GET', '/admin/api/v1/me',
                                  headers=self.web_headers(web))[0] == 401, 'revoke')

    def cleanup(self):
        self.report['cleanup_status'] = 'passed'
        if self.webs and self.create_attempted:
            self.case('RECOVER_NEW_MEMBER', self.recover_member, cleanup=True)
        if self.fixture['member_id'] is not None:
            self.case('CLEAR_NEW_MEMBER_BOOK', self.clear_book, cleanup=True)
        for index, token in enumerate(self.clients, 1):
            self.case('CLIENT_LOGOUT_' + str(index), lambda token=token: self.revoke_client(token), cleanup=True)
        if self.fixture['member_id'] is not None:
            # Covers a successful login whose response/token was lost as well.
            self.case('REVOKE_NEW_MEMBER_SESSIONS', self.revoke_members, cleanup=True)
            if self.keep and self.report['status'] == 'running' and self.report['cleanup_status'] == 'passed':
                self.fixture['account_state'] = 'enabled_for_manual'
                self.case('SAVE_MANUAL_MEMBER_FIXTURE', lambda: self.fixture_file.write(self.fixture), cleanup=True)
            else:
                self.case('DISABLE_NEW_MEMBER', self.disable_member, cleanup=True)
        for index, web in enumerate(self.webs, 1):
            self.case('ADMIN_LOGOUT_' + str(index), lambda web=web: self.revoke_admin(web), cleanup=True)
        if self.admin_login_uncertain:
            self.case('UNTRACKED_ADMIN_SESSION', lambda: require(False, 'uncertain'), cleanup=True)

    def run(self):
        try:
            self.case('ADMIN_WEB_LOGIN', self.admin_login)
            self.case('ADMIN_ORIGIN_CSRF', self.admin_guards)
            self.case('CREATE_NEW_MEMBER', self.create_member)
            first = self.case('MEMBER_CLIENT_LOGIN_1', self.client_login)
            second = self.case('MEMBER_CLIENT_LOGIN_2', self.client_login)
            self.case('DISTINCT_CLIENT_SESSIONS', lambda: require(first != second))
            self.case('NEW_MEMBER_EMPTY_BOOK', self.empty_guard)
            self.case('CREDENTIAL_SYNC_AND_INVALID_REJECTION', self.credentials)
            self.case('DIRECTORY_SCOPE_AND_SHAPE', self.directories)
        except smoke.Halt:
            pass
        except (Exception, KeyboardInterrupt):
            self.report['status'] = 'failed'
        finally:
            try:
                self.cleanup()
            except (Exception, KeyboardInterrupt):
                self.report['status'] = 'failed'
                self.report['cleanup_status'] = 'failed'
            self.admin_password = None
        if self.report['status'] == 'running':
            self.report['status'] = 'passed'
        if self.output_failed:
            self.report['status'] = 'failed'
        self.report_file.write(self.report)
        return self.report['status'] == 'passed'


def main(argv=None):
    parser = smoke.SafeParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--base')
    parser.add_argument('--admin-username', default='administrator')
    parser.add_argument('--admin-password-file', type=Path)
    parser.add_argument('--fixture-file', type=Path)
    parser.add_argument('--report-file', type=Path)
    parser.add_argument('--keep-member-for-manual', action='store_true')
    outputs = []
    report_output = None
    runner = None
    try:
        args, unknown = parser.parse_known_args(argv)
        require(not unknown, 'arguments')
        if not args.execute:
            print('PREVIEW: create one member; check HTTPS credential sync/directories; clear book; revoke sessions.')
            print('PREVIEW: --execute and new --fixture-file/--report-file paths are required for requests.')
            return 0
        base = smoke.normalized_origin(args.base)
        require(isinstance(args.admin_username, str)
                and 0 < len(args.admin_username.encode('utf-8')) <= 128, 'arguments')
        require(args.admin_password_file is not None and args.fixture_file is not None
                and args.report_file is not None, 'arguments')
        require(args.fixture_file.resolve() != args.report_file.resolve(), 'report')
        require(not os.path.lexists(args.fixture_file) and not os.path.lexists(args.report_file), 'report')
        fixture = NewJSON(args.fixture_file, private_parent=True)
        outputs.append(fixture)
        report = NewJSON(args.report_file)
        outputs.append(report)
        report_output = report
        report.write({'status': 'preflight', 'cleanup_status': 'not_required', 'cases': []})
        password = smoke.read_password(args.admin_password_file)
        runner = Runner(base, args.admin_username, password, fixture, report, args.keep_member_for_manual)
        password = None
        previous_term = signal.getsignal(signal.SIGTERM)
        def interrupted(_signal, _frame):
            raise KeyboardInterrupt()
        signal.signal(signal.SIGTERM, interrupted)
        try:
            return 0 if runner.run() else 1
        finally:
            signal.signal(signal.SIGTERM, previous_term)
    except (Exception, KeyboardInterrupt) as error:
        reason = error.reason if isinstance(error, Failure) else (
            'interrupted' if isinstance(error, KeyboardInterrupt) else 'internal')
        if report_output is not None:
            failed = runner.report if runner is not None else {
                'status': 'failed', 'cleanup_status': 'not_required', 'cases': []}
            failed['status'] = 'failed'
            failed['cases'].append({'id': 'PREFLIGHT_OR_OUTPUT', 'status': 'failed',
                                    'message': smoke.MESSAGES[reason]})
            try:
                report_output.write(failed)
            except Failure:
                pass
        print('PREFLIGHT_OR_OUTPUT: FAILED; ' + smoke.MESSAGES[reason], file=sys.stderr, flush=True)
        return 1
    finally:
        for output in outputs:
            try:
                output.close()
            except OSError:
                pass


if __name__ == '__main__':
    sys.exit(main())
