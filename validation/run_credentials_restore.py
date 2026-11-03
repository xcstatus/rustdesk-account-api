#!/usr/bin/env python3
"""Opt-in isolated schema-4 credential restore checks; never touches existing projects.

Creates random projects, private synthetic secrets and new volumes. Stops its own
services on completion; retains every volume and fixture. No production secrets,
TLS acceptance, public ports, volume deletion, or raw subprocess output.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time
from urllib.parse import quote
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def fixture_environment():
    # Shell interpolation wins over --env-file in Compose. Inherited values must
    # never select a volume, secret, image, profile, env file, or remote daemon.
    prefixes = ('RD_', 'COMPOSE_', 'POSTGRES_', 'CADDY_', 'ACME_', 'DOCKER_')
    return {name: value for name, value in os.environ.items() if not name.startswith(prefixes)}


def run(command, check=True):
    result = subprocess.run(command, capture_output=True, env=fixture_environment())
    require(not check or result.returncode == 0, 'Fixture operation failed; raw output withheld.')
    return result


def docker(*arguments):
    return run(['docker', *arguments]).stdout


def inspect(kind, name):
    return json.loads(docker(kind, 'inspect', name))[0]


def require_local_docker():
    context = json.loads(docker('context', 'inspect'))
    require(len(context) == 1 and context[0].get('Endpoints', {}).get('docker', {}).get('Host', '').startswith('unix://'),
            'Restore checks require a local Unix-socket Docker context; no containers started.')


def private(path, value):
    with path.open('x', encoding='utf-8') as stream:
        stream.write(value)
    path.chmod(0o600)


def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def request(origin, path, data=None, token=None):
    headers = {'Content-Type': 'application/json', 'Origin': 'https://restore.test.invalid'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    req = urllib.request.Request(origin + path, data=None if data is None else json.dumps(data).encode(), headers=headers)
    with urllib.request.urlopen(req, timeout=15) as response:
        body = response.read()
        return json.loads(body) if body else None


def uid_check(work, project, backend, image_id, postgres_image, key):
    """Check a valid restored volume with the actual unprivileged API user."""
    directory = work / 'uid-fixture'
    directory.mkdir(mode=0o700)
    copy = directory / 'book_keys.json'
    private(copy, key.read_text())
    server = None
    try:
        if backend == 'postgres':
            server = project + '-uid-check'
            docker('run', '-d', '--name', server, '--network', 'none',
                       '-v', project + '-rotated:/var/lib/postgresql/data',
                       '-v', str(work / 'secrets/app_password') + ':/run/secrets/app_password:ro', postgres_image)
            for _ in range(60):
                probe = run(['docker', 'exec', server, 'pg_isready', '-h', '127.0.0.1', '-U', 'rustdesk_admin', '-d', 'rustdesk'], False)
                if probe.returncode == 0:
                    break
                time.sleep(1)
            else:
                raise RuntimeError('UID check database did not become ready.')
            password = docker('exec', server, 'cat', '/run/secrets/app_password').decode().removesuffix('\n')
            private(directory / 'database_url', 'postgres://rustdesk:' + quote(password, safe='') + '@127.0.0.1:5432/rustdesk\n')
        docker('run', '--rm', '--user', '0:0', '--network', 'none',
                   '-v', str(directory) + ':/fixture', '--entrypoint', '/bin/sh', image_id,
                   '-ec', 'chown 10001:10001 /fixture/*; chmod 600 /fixture/*')
        command = ['run', '--rm', '--user', '10001:10001', '--cap-drop', 'ALL',
                   '--security-opt', 'no-new-privileges:true', '--read-only', '--tmpfs', '/tmp',
                   '--network', 'container:' + server if server else 'none',
                   '-v', str(copy) + ':/run/secrets/book_keys:ro', '-e', 'RD_BOOK_KEYS_FILE=/run/secrets/book_keys']
        if backend == 'sqlite':
            command += ['-v', project + '-rotated:/app/data', '-e', 'RD_DATABASE_URL=sqlite:///app/data/accounts.sqlite']
        else:
            command += ['-v', str(directory / 'database_url') + ':/run/secrets/database_url:ro',
                        '-e', 'RD_DATABASE_URL_FILE=/run/secrets/database_url']
        docker(*command, image_id, 'check-credentials')
    finally:
        if server:
            docker('stop', '--time', '30', server)
            docker('rm', server)


def check_backend(args, backend, directory, report):
    project = 'rdcred-' + backend + '-' + uuid.uuid4().hex[:10]
    work = directory / backend
    work.mkdir(mode=0o700)
    secret = work / 'secrets'
    secret.mkdir(mode=0o700)
    key = secret / 'book_keys.json'
    v1 = base64.b64encode(secrets.token_bytes(32)).decode()
    v2 = base64.b64encode(secrets.token_bytes(32)).decode()
    private(key, json.dumps({'active_key': 'v1', 'keys': {'v1': v1}}))
    private(secret / 'service_book_keys.json', key.read_text())
    password, app_password = secrets.token_urlsafe(28), secrets.token_urlsafe(28)
    url = ('sqlite:///app/data/accounts.sqlite' if backend == 'sqlite' else
           'postgres://rustdesk:' + app_password + '@postgres:5432/rustdesk')
    for name, value in {'database_url': url, 'bootstrap_password': password,
                        'postgres_password': secrets.token_urlsafe(28), 'app_password': app_password}.items():
        private(secret / name, value + '\n')
    port = free_port()
    origin = 'http://127.0.0.1:' + str(port)
    env = {'RD_API_IMAGE': args.api_image, 'POSTGRES_IMAGE': args.postgres_image,
           'RD_DOMAIN': 'restore.test.invalid', 'RD_WEB_ORIGIN': 'https://restore.test.invalid',
           'RD_SECRETS_DIR': str(secret), 'RD_BOOK_KEYS_HOST_FILE': str(secret / 'service_book_keys.json'),
           'RD_DATA_VOLUME': project + '-data', 'RD_POSTGRES_VOLUME': project + '-pg', 'RD_CADDY_VOLUME': project + '-caddy'}
    env_file = work / 'compose.env'
    private(env_file, ''.join(k + '=' + v + '\n' for k, v in env.items()))
    overlay = work / 'fixture.yml'
    overlay.write_text('services:\n  caddy:\n    profiles: [disabled]\n  api:\n    ports: ["127.0.0.1:' + str(port) + ':21114"]\n')
    prefix = ['docker', 'compose', '-p', project, '--env-file', str(env_file),
              '-f', str(ROOT / 'deploy' / ('compose.' + backend + '.yml')),
              '-f', str(ROOT / 'deploy/compose.book-credentials.yml'), '-f', str(overlay)]

    def dc(*command):
        return run(prefix + list(command)).stdout

    common = [sys.executable, str(ROOT / 'deploy/ops.py'), '--backend', backend,
              '--project', project, '--env-file', str(env_file)]
    pg_flags = (['--pg-password-file', str(secret / 'postgres_password'),
                 '--app-password-file', str(secret / 'app_password')] if backend == 'postgres' else [])
    entry = {'backend': backend, 'project': project, 'cases': [], 'volumes_retained': True}
    report['backends'].append(entry)
    try:
        # Service copies follow real UID ownership; the operator's separate key
        # copy stays readable for ops.py preflight on Linux as well as macOS.
        docker('run', '--rm', '--user', '0:0', '--network', 'none', '-v', str(secret) + ':/secrets',
                   '--entrypoint', 'chown', args.api_image, '10001:10001', '/secrets/database_url',
                   '/secrets/bootstrap_password', '/secrets/service_book_keys.json')
        if backend == 'postgres':
            uid = docker('run', '--rm', '--network', 'none', '--entrypoint', 'id', args.postgres_image, '-u', 'postgres').decode().strip()
            require(uid.isdigit(), 'Invalid PostgreSQL UID.')
            docker('run', '--rm', '--user', '0:0', '--network', 'none', '-v', str(secret) + ':/secrets',
                       '--entrypoint', 'chown', args.api_image, uid + ':' + uid, '/secrets/postgres_password', '/secrets/app_password')
            dc('up', '-d', '--no-build', '--wait', 'postgres')
        dc('run', '--rm', '--no-deps', '-T', '-v', str(secret / 'bootstrap_password') + ':/run/secrets/bootstrap_password:ro',
           '-e', 'RD_BOOTSTRAP_PASSWORD_FILE=/run/secrets/bootstrap_password', 'api', 'bootstrap-admin', 'fixture_admin')
        dc('up', '-d', '--no-build', '--wait', 'api')
        api = inspect('container', dc('ps', '-q', 'api').decode().strip())
        require(api['Config']['User'] == '10001:10001' and api['HostConfig']['ReadonlyRootfs'], 'Fixture API hardening differs.')
        login = request(origin, '/api/login', {'username': 'fixture_admin', 'password': password, 'type': 'account',
                        'id': 'restore-fixture-id', 'uuid': 'restore-fixture-uuid',
                        'deviceInfo': {'os': 'Linux', 'name': 'restore-fixture', 'type': 'desktop'}})
        token = login['access_token']
        book = {'peers': [{'id': 'peer-restore', 'alias': 'before-backup',
                          'hash': base64.b64encode(secrets.token_bytes(32)).decode()}], 'tags': []}
        request(origin, '/api/ab', {'data': json.dumps(book)}, token)
        require(json.loads(request(origin, '/api/ab', token=token)['data']) == book, 'Source hash roundtrip failed.')
        backup = work / 'backup'
        run(common + ['--book-keys-file', str(key), 'backup', str(backup)])
        manifest = json.loads((backup / 'manifest.json').read_text())
        require(manifest['schema'] == 4 and manifest['credential_key_ids'] == ['v1'], 'Backup key inventory differs.')
        require(v1 not in (backup / 'manifest.json').read_text() and str(key) not in (backup / 'manifest.json').read_text(), 'Key leaked into manifest.')
        entry['cases'].append('schema4-ciphertext-backup-key-separated')
        target = project + '-restored'
        restore = common + ['--book-keys-file', str(key), 'restore', str(backup), '--target-volume', target] + pg_flags
        run(restore)
        entry['cases'].append('correct-key-authenticated-restore')
        require(run(restore, False).returncode != 0, 'Existing target volume accepted.')
        entry['cases'].append('existing-volume-refused')
        missing_target = project + '-missing-key'
        require(run(common + ['restore', str(backup), '--target-volume', missing_target] + pg_flags, False).returncode != 0, 'Missing key accepted.')
        require(run(['docker', 'volume', 'inspect', missing_target], False).returncode != 0, 'Missing key created a volume.')
        entry['cases'].append('missing-key-before-volume-refused')
        rings = [('retired', {'active_key': 'v2', 'keys': {'v2': v2}}, False),
                 ('wrong', {'active_key': 'v1', 'keys': {'v1': v2}}, False),
                 ('rotated', {'active_key': 'v2', 'keys': {'v1': v1, 'v2': v2}}, True)]
        for name, ring, success in rings:
            path = secret / (name + '.json')
            private(path, json.dumps(ring))
            result = run(common + ['--book-keys-file', str(path), 'restore', str(backup),
                                  '--target-volume', project + '-' + name] + pg_flags, False)
            require((result.returncode == 0) == success, 'Unexpected key restore result.')
            require(success or b'Restore complete' not in result.stdout, 'Failed restore reported success.')
            entry['cases'].append(name + '-key-' + ('restored' if success else 'refused'))
        uid_check(work, project, backend, report['image_id'], args.postgres_image, key)
        entry['cases'].append('uid10001-cap-drop-all-readonly-check-credentials')
        dc('stop')
        env['RD_DATA_VOLUME' if backend == 'sqlite' else 'RD_POSTGRES_VOLUME'] = target
        env_file.write_text(''.join(k + '=' + v + '\n' for k, v in env.items()))
        if backend == 'postgres':
            dc('up', '-d', '--no-build', '--force-recreate', '--wait', 'postgres')
        dc('up', '-d', '--no-build', '--force-recreate', '--wait', 'api')
        require(json.loads(request(origin, '/api/ab', token=token)['data']) == book, 'Restored session/book/hash differs.')
        entry['cases'].append('restored-session-book-hash-roundtrip')
        dc('stop', 'api')
        # Corrupt only the restored synthetic fixture, preserving the original backup.
        sql = "UPDATE book_credentials SET ciphertext='" + base64.b64encode(bytes(60)).decode() + "'"
        if backend == 'sqlite':
            docker('run', '--rm', '--user', '0:0', '--network', 'none', '-v', target + ':/data',
                       '--entrypoint', 'sqlite3', args.api_image, '/data/accounts.sqlite', sql)
        else:
            dc('exec', '-T', 'postgres', 'psql', '-U', 'rustdesk_admin', '-d', 'rustdesk', '-c', sql)
        tampered = work / 'tampered-backup'
        run(common + ['--book-keys-file', str(key), 'backup', str(tampered)])
        require(run(common + ['--book-keys-file', str(key), 'restore', str(tampered),
                              '--target-volume', project + '-tampered'] + pg_flags, False).returncode != 0, 'Tampered ciphertext accepted.')
        entry['cases'].append('tampered-ciphertext-restore-refused')
        entry['passed'] = True
    except Exception as error:
        entry.update(passed=False, failure_type=type(error).__name__)
        raise
    finally:
        run(prefix + ['stop'], False)
        (directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--execute', action='store_true', help='start only new isolated projects and retain fixtures/volumes')
    parser.add_argument('--api-image', default='rustdesk-account-api:0.1.0-devices')
    parser.add_argument('--postgres-image', default='postgres:16-alpine')
    parser.add_argument('--backend', choices=['sqlite', 'postgres', 'both'], default='both')
    parser.add_argument('--directory', type=Path, help='new private fixture directory; default is a new OS temporary directory')
    args = parser.parse_args()
    if not args.execute:
        print('No containers started. Pass --execute and a locally available compatible --api-image to run isolated checks.')
        return
    os.umask(0o077)
    require_local_docker()
    image = inspect('image', args.api_image)
    if args.backend != 'sqlite':
        inspect('image', args.postgres_image)
    if args.directory:
        args.directory.mkdir(mode=0o700, exist_ok=False)
        directory = args.directory.resolve()
    else:
        directory = Path(tempfile.mkdtemp(prefix='rdcred-')).resolve()
    report = {'api_image': args.api_image, 'image_id': image['Id'], 'backends': [],
              'fixture_api_user': '10001:10001', 'source_transport': 'loopback HTTP with HTTPS origin policy; TLS not exercised',
              'data_retained': True, 'inherited_deployment_environment': 'cleared for every subprocess',
              'docker_context_policy': 'local Unix socket required'}
    try:
        for backend in (['sqlite', 'postgres'] if args.backend == 'both' else [args.backend]):
            check_backend(args, backend, directory, report)
    except Exception:
        print('Credential restore checks failed; raw errors withheld. Private artifacts retained: ' + str(directory / 'report.json'), file=sys.stderr)
        sys.exit(1)
    print(json.dumps({'report': str(directory / 'report.json'), 'backends': report['backends']}))


if __name__ == '__main__':
    main()
