#!/usr/bin/env python3
"""Isolated M3 Compose HTTPS/backup checks. Default: static only; --execute starts own projects.

Images must already exist. Never builds, removes volumes, prints raw subprocess output,
or touches an existing project. Data and private test credentials are retained.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'deploy'))
import ops  # noqa: E402


class CheckFailure(Exception):
    pass


def require(value, message):
    if not value:
        raise CheckFailure(message)


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


class Runner:
    def __init__(self, args, backend):
        self.args, self.backend = args, backend
        self.project = 'rdm3-' + uuid.uuid4().hex[:12]
        self.directory = ROOT / 'validation/.runtime' / self.project
        self.directory.mkdir(mode=0o700, parents=True)
        self.secrets = self.directory / 'secrets'
        self.secrets.mkdir(mode=0o700)
        self.password = secrets.token_urlsafe(30)
        pg_password = secrets.token_urlsafe(36)
        pg_app_password = secrets.token_urlsafe(36)
        for name, value in {
            'database_url': ('sqlite:///app/data/accounts.sqlite' if backend == 'sqlite' else
                             'postgres://rustdesk:' + pg_app_password + '@postgres:5432/rustdesk'),
            'postgres_password': pg_password,
            'app_password': pg_app_password,
            'bootstrap_password': self.password,
            'oidc_client_secret': '',
        }.items():
            (self.secrets / name).write_text(value + '\n')
            (self.secrets / name).chmod(0o600)
        self.https_port = port()
        self.env = {
            'RD_DOMAIN': 'localhost', 'RD_WEB_ORIGIN': 'https://localhost:' + str(self.https_port),
            'CADDY_TLS_DIRECTIVE': 'tls internal', 'RD_HTTP_BIND': '127.0.0.1:' + str(port()),
            'RD_HTTPS_BIND': '127.0.0.1:' + str(self.https_port),
            'RD_API_IMAGE': args.api_image, 'CADDY_IMAGE': args.caddy_image,
            'POSTGRES_IMAGE': args.postgres_image, 'RD_SECRETS_DIR': str(self.secrets),
            'RD_DATA_VOLUME': self.project + '-data', 'RD_POSTGRES_VOLUME': self.project + '-postgres',
            'RD_CADDY_VOLUME': self.project + '-caddy',
            'RD_LOGIN_WINDOW_SECONDS': '2', 'RD_LOGIN_PER_ACCOUNT': '3',
            'RD_LOGIN_GLOBAL': '100',
        }
        self.env_file = self.directory / 'compose.env'
        self.write_env()
        self.report = {'milestone': 'M3', 'backend': backend, 'project': self.project,
                       'mode': 'runtime' if args.execute else 'static', 'cases': [],
                       'data_retained': True, 'oidc_scope': 'not exercised; run M0/M1/M2 separately'}
        self.started = False

    def write_env(self):
        self.env_file.write_text(''.join(k + '=' + v + '\n' for k, v in self.env.items()))
        self.env_file.chmod(0o600)

    def command(self, *cmd, check=True):
        prefix = ['docker', 'compose', '-p', self.project, '--env-file', str(self.env_file),
                  '-f', str(ROOT / 'deploy' / ('compose.' + self.backend + '.yml'))]
        result = subprocess.run(prefix + list(cmd), capture_output=True)
        if check:
            require(result.returncode == 0, 'Compose operation failed (raw output withheld)')
        return result

    def record(self, name):
        self.report['cases'].append({'name': name, 'passed': True})
        print(self.backend + ': ' + name + ': PASS', flush=True)

    def api_container(self):
        return self.command('ps', '--all', '-q', 'api').stdout.decode().strip()

    def call(self, path, data=None, token=None, extra_headers=None):
        headers = {'Content-Type': 'application/json', 'Origin': self.env['RD_WEB_ORIGIN']}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        headers.update(extra_headers or {})
        req = urllib.request.Request(self.env['RD_WEB_ORIGIN'] + path,
                                     data=None if data is None else json.dumps(data).encode(), headers=headers)
        try:
            response = urllib.request.urlopen(req, context=self.tls, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        return response.status, response.headers, response.read()

    def wait_ready(self):
        for _ in range(60):
            try:
                if self.call('/readyz')[0] == 200:
                    return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(1)
        raise CheckFailure('HTTPS readiness did not recover')

    def check_pg_roles(self):
        def query(sql):
            return self.command('exec', '-T', 'postgres', 'psql', '-U', 'rustdesk_admin',
                                '-d', 'rustdesk', '-At', '-c', sql).stdout.decode().strip()
        require(query("SELECT rolsuper,rolcreatedb,rolcreaterole,rolreplication,rolbypassrls "
                      "FROM pg_roles WHERE rolname='rustdesk'") == 'f|f|f|f|f',
                'PostgreSQL application role has elevated privileges')
        require(query("SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='rustdesk'") == 'rustdesk',
                'PostgreSQL application does not own its database')
        require(query("SELECT count(*) FROM pg_tables WHERE schemaname='public' AND tableowner<>'rustdesk'") == '0',
                'PostgreSQL application does not own restored tables')
        self.command('exec', '-T', 'postgres', 'psql', '-U', 'rustdesk', '-d', 'rustdesk',
                     '-v', 'ON_ERROR_STOP=1', '-c',
                     'BEGIN; CREATE TABLE m3_ownership_probe(id int); DROP TABLE m3_ownership_probe; ROLLBACK;')

    def run(self):
        self.command('config', '-q')
        self.record('compose-config')
        if not self.args.execute:
            return
        require(not self.command('ps', '--all', '-q').stdout.strip(), 'Project unexpectedly already exists')
        for image in [self.args.api_image, self.args.caddy_image] + ([self.args.postgres_image] if self.backend == 'postgres' else []):
            ops.inspect('image', image)
        self.report['images'] = {
            image: {'id': ops.inspect('image', image)['Id'],
                    'repo_digests': ops.inspect('image', image).get('RepoDigests') or []}
            for image in [self.args.api_image, self.args.caddy_image] +
            ([self.args.postgres_image] if self.backend == 'postgres' else [])
        }
        # Local Compose file secrets preserve host permissions, so set real image ownership.
        ops.docker('run', '--rm', '--user', '0:0', '--network', 'none',
                   '-v', str(self.secrets) + ':/secrets', '--entrypoint', '/bin/sh', self.args.api_image,
                   '-ec', 'chown 10001:10001 /secrets/database_url /secrets/bootstrap_password /secrets/oidc_client_secret')
        self.started = True
        if self.backend == 'postgres':
            pg_uid = ops.docker('run', '--rm', '--network', 'none', '--entrypoint', 'id',
                                self.args.postgres_image, '-u', 'postgres').decode().strip()
            require(pg_uid.isdigit(), 'PostgreSQL UID could not be resolved')
            ops.docker('run', '--rm', '--user', '0:0', '--network', 'none',
                       '-v', str(self.secrets) + ':/secrets', '--entrypoint', 'chown', self.args.api_image,
                       pg_uid + ':' + pg_uid, '/secrets/postgres_password', '/secrets/app_password')
            self.command('up', '-d', '--no-build', '--wait', '--wait-timeout', '90', 'postgres')
        self.command('run', '--rm', '--no-deps', '-T', '-v',
                     str(self.secrets / 'bootstrap_password') + ':/run/secrets/bootstrap_password:ro',
                     '-e', 'RD_BOOTSTRAP_PASSWORD_FILE=/run/secrets/bootstrap_password',
                     'api', 'bootstrap-admin', 'm3_admin')
        self.command('up', '-d', '--no-build', '--wait', '--wait-timeout', '120')
        caddy_id = self.command('ps', '-q', 'caddy').stdout.decode().strip()
        ca = self.directory / 'caddy-root.crt'
        for _ in range(30):
            result = subprocess.run(['docker', 'cp', caddy_id + ':/data/caddy/pki/authorities/local/root.crt', str(ca)], capture_output=True)
            if result.returncode == 0:
                break
            time.sleep(1)
        require(ca.is_file(), 'Caddy local CA unavailable')
        self.tls = ssl.create_default_context(cafile=str(ca))
        self.wait_ready()
        if self.backend == 'postgres':
            self.check_pg_roles()
            self.record('postgres-app-role-no-elevation-database-table-ownership-ddl')
        require(self.call('/healthz')[0] == 200, 'Liveness failed')
        require(self.call('/admin/')[0] == 200, 'Built Web files unavailable')
        api = ops.inspect('container', self.api_container())
        require(api['Config']['User'] == '10001:10001' and api['HostConfig']['ReadonlyRootfs'], 'API hardening differs')
        require(not api['HostConfig']['PortBindings'], 'API unexpectedly publishes a port')
        self.record('https-ca-ready-web-nonroot-readonly-private-api')
        status, headers, body = self.call('/admin/api/v1/auth/login', {'username': 'm3_admin', 'password': self.password})
        require(status == 200, 'HTTPS Web login failed')
        cookie = headers.get('Set-Cookie', '').lower()
        require('secure' in cookie and 'httponly' in cookie and 'samesite=lax' in cookie, 'HTTPS cookie flags missing')
        web_cookie = headers['Set-Cookie'].split(';', 1)[0]
        status, _, body = self.call('/api/login', {'username': 'm3_admin', 'password': self.password, 'type': 'account'})
        require(status == 200, 'Client login failed')
        token = json.loads(body)['access_token']
        snapshot = {'peers': [{'id': 'm3-device-001', 'alias': 'before-backup', 'platform': 'Linux',
                               'tags': ['m3-tag']}], 'tags': ['m3-tag'],
                    'tag_colors': json.dumps({'m3-tag': 4278190335})}
        require(self.call('/api/ab', {'data': json.dumps(snapshot)}, token)[0] == 200,
                'Address book snapshot write failed')
        def read_book():
            status, _, body = self.call('/api/ab', token=token)
            require(status == 200, 'Address book read failed')
            return json.loads(json.loads(body)['data'])
        def read_audit():
            status, _, body = self.call('/admin/api/v1/audit-events?limit=200',
                                       extra_headers={'Cookie': web_cookie})
            require(status == 200, 'Audit snapshot query failed')
            page = json.loads(body)
            require(page['next_before_id'] is None, 'Fixture audit unexpectedly needs pagination')
            return page['events']
        require(read_book() == snapshot, 'Address book snapshot content differs')
        self.command('stop', '--timeout', '30', 'api')
        stopped = ops.inspect('container', self.api_container())
        require(stopped['State']['ExitCode'] == 0, 'SIGTERM exit was not clean')
        callback_marker = secrets.token_urlsafe(24)
        require(self.call('/api/oidc/callback?code=' + callback_marker + '&state=' + callback_marker)[0] == 502,
                'Stopped API did not produce an upstream failure')
        self.command('up', '-d', '--no-build', '--wait', 'api')
        self.wait_ready()
        require(self.call('/api/currentUser', {}, token)[0] == 200, 'Session did not survive restart')
        require(read_book() == snapshot, 'Address book did not survive restart')
        self.record('secure-cookie-sigterm-restart-session-persistence')
        wrong = {'username': 'm3_limit_probe', 'password': 'incorrect-test-password', 'type': 'account'}
        for _ in range(3):
            require(self.call('/api/login', wrong)[0] == 401, 'Limiter rejected before account budget')
        require(self.call('/api/login', wrong)[0] == 429, 'Account limiter did not reject exhausted budget')
        require(self.call('/admin/api/v1/auth/login', {'username': wrong['username'],
                                                      'password': wrong['password']})[0] == 429,
                'Web and client login did not share account limiter')
        require(self.call('/api/login', wrong, extra_headers={
            'Forwarded': 'for=198.51.100.22;proto=https', 'X-Forwarded-For': '198.51.100.23',
            'X-Real-IP': '198.51.100.24'})[0] == 429, 'Proxy headers bypassed account limiter')
        time.sleep(2.1)
        require(self.call('/api/login', wrong)[0] == 401, 'Account limiter did not recover after window')
        require(self.call('/api/login', {'username': 'm3_admin', 'password': self.password,
                                         'type': 'account'})[0] == 200, 'Correct login failed after limiter window')
        self.record('account-limiter-proxy-header-resistance-and-window-recovery')
        if self.backend == 'postgres':
            self.command('stop', '--timeout', '30', 'postgres')
            try:
                require(self.call('/healthz')[0] == 200 and self.call('/readyz')[0] == 503,
                        'DB outage did not separate liveness/readiness')
            finally:
                self.command('up', '-d', '--no-build', '--wait', 'postgres')
        else:
            def set_schema(before, after):
                ops.docker('run', '--rm', '--user', '0:0', '--network', 'none',
                           '-v', self.env['RD_DATA_VOLUME'] + ':/data', '--entrypoint', 'sqlite3',
                           self.args.api_image, '/data/accounts.sqlite',
                           'UPDATE account_schema_migrations SET version=' + str(after) + ' WHERE version=' + str(before))
            set_schema(4, 40)
            try:
                require(self.call('/healthz')[0] == 200 and self.call('/readyz')[0] == 503,
                        'Schema fault did not separate liveness/readiness')
            finally:
                set_schema(40, 4)
        self.wait_ready()
        self.record('readiness-fault-and-recovery')
        before_audit = read_audit()
        require(bool(before_audit) and all(isinstance(event['id'], int) for event in before_audit),
                'Audit snapshot is missing event IDs')
        backup = self.directory / 'backup'
        common = [sys.executable, str(ROOT / 'deploy/ops.py'), '--backend', self.backend,
                  '--project', self.project, '--env-file', str(self.env_file)]
        ops.run(common + ['backup', str(backup)])
        # Deliberately add post-snapshot data before restoring: exercise snapshot rollback.
        self.command('up', '-d', '--no-build', '--wait', 'api')
        self.wait_ready()
        after_snapshot = {'peers': [{'id': 'm3-device-after-backup', 'alias': 'after-backup'}], 'tags': []}
        require(self.call('/api/ab', {'data': json.dumps(after_snapshot)}, token)[0] == 200,
                'Post-backup mutation failed')
        require(read_book() == after_snapshot, 'Post-backup content did not change')
        require(self.call('/api/login', {'username': 'm3_after_backup_failure',
                                         'password': 'incorrect-test-password', 'type': 'account'})[0] == 401,
                'Post-backup audit fixture failed')
        require(read_audit() != before_audit, 'Post-backup audit did not change')
        self.command('stop', '--timeout', '30', 'api')
        target = self.project + '-restored'
        restore = common + ['restore', str(backup), '--target-volume', target]
        if self.backend == 'postgres':
            restore += ['--postgres-image', self.args.postgres_image,
                        '--pg-password-file', str(self.secrets / 'postgres_password'),
                        '--app-password-file', str(self.secrets / 'app_password')]
        ops.run(restore)
        repeated = subprocess.run(restore, capture_output=True)
        require(repeated.returncode != 0, 'Repeated restore did not refuse existing target')
        # Capture old-container evidence before force-recreate removes its Docker logs.
        previous_logs = self.command('logs', '--no-color', 'api', 'caddy').stdout
        self.command('stop', '--timeout', '30')
        self.env['RD_DATA_VOLUME' if self.backend == 'sqlite' else 'RD_POSTGRES_VOLUME'] = target
        self.write_env()
        self.command('up', '-d', '--no-build', '--force-recreate', '--wait', '--wait-timeout', '120')
        self.wait_ready()
        require(self.call('/api/currentUser', {}, token)[0] == 200, 'Restored session invalid')
        require(read_book() == snapshot, 'Restored address book differs from backup or includes later writes')
        require(read_audit() == before_audit, 'Restored audit IDs/content differ from backup or include later events')
        if self.backend == 'postgres':
            self.check_pg_roles()
            self.report['restored_application_role_ownership_verified'] = True
        self.report['restore_snapshot'] = {'audit_event_ids': [event['id'] for event in before_audit],
                                           'audit_events_equal': True, 'address_book_equal': True,
                                           'post_backup_changes_removed': True}
        logs = previous_logs + self.command('logs', '--no-color', 'api', 'caddy').stdout
        require(all(value.encode() not in logs for value in [self.password, token, callback_marker]),
                'Secret or callback query found in service logs')
        self.record('backup-restore-session-book-audit-snapshot-rollback-overwrite-refusal-redacted-logs')

    def finish(self):
        if self.started and not self.args.keep_running:
            self.command('stop', '--timeout', '30', check=False)
        if self.started:
            self.report['containers'] = []
            for container_id in self.command('ps', '--all', '-q', check=False).stdout.decode().splitlines():
                try:
                    state = ops.inspect('container', container_id)
                    self.report['containers'].append({
                        'service': state['Config'].get('Labels', {}).get('com.docker.compose.service'),
                        'stopped': not state['State']['Running'], 'exit_code': state['State']['ExitCode']})
                except (ops.OperationError, ValueError, KeyError):
                    self.report['container_state_unavailable'] = True
        (self.directory / 'report.json').write_text(json.dumps(self.report, indent=2) + '\n')
        print('Private artifacts retained: ' + str(self.directory.relative_to(ROOT)))


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend', choices=['sqlite', 'postgres', 'both'], default='both')
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--keep-running', action='store_true')
    parser.add_argument('--api-image', default='rustdesk-account-api:0.1.0-devices')
    parser.add_argument('--caddy-image', default='caddy:2-alpine')
    parser.add_argument('--postgres-image', default='postgres:16-alpine')
    args = parser.parse_args()
    for backend in (['sqlite', 'postgres'] if args.backend == 'both' else [args.backend]):
        runner = Runner(args, backend)
        try:
            runner.run()
            runner.report['passed'] = True
        except (CheckFailure, ops.OperationError, OSError, ValueError, KeyError, urllib.error.URLError):
            runner.report['passed'] = False
            print(backend + ': FAIL (raw output withheld)', file=sys.stderr)
            return 1
        finally:
            runner.finish()
    return 0


if __name__ == '__main__':
    sys.exit(main())
