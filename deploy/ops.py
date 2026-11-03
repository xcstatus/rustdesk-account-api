#!/usr/bin/env python3
"""Stopped-writer backups and restore to a new Docker volume; redacted output only."""
import argparse
import base64
from contextlib import closing
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
SCHEMA_SQL = 'SELECT COALESCE(MAX(version),0) FROM account_schema_migrations'
KEY_IDS_SQL = 'SELECT DISTINCT key_id FROM book_credentials ORDER BY key_id'


class OperationError(Exception):
    pass


def run(command, *, stdin=None, stdout=None, env=None):
    # Raw Docker/SQL errors may contain configuration or credentials. Keep them private.
    result = subprocess.run(command, stdin=stdin, stdout=stdout or subprocess.PIPE,
                            stderr=subprocess.PIPE, check=False, env=env)
    if result.returncode:
        raise OperationError('Container operation failed; no credentials or raw output were emitted.')
    return result.stdout if stdout is None else b''


def docker(*args, **kwargs):
    return run(['docker', *args], **kwargs)


def checksum(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def inspect(kind, name):
    return json.loads(docker(kind, 'inspect', name))[0]


def new_volume(name, project):
    existing = docker('volume', 'ls', '--format', '{{.Name}}').decode().splitlines()
    if name in existing:
        raise OperationError('Restore refused: target volume already exists, including empty volumes.')
    operation_id = uuid.uuid4().hex
    docker('volume', 'create', '--label', 'rustdesk.restore-project=' + project,
           '--label', 'rustdesk.restore-id=' + operation_id, name)
    if inspect('volume', name).get('Labels', {}).get('rustdesk.restore-id') != operation_id:
        raise OperationError('Restore refused: target volume was concurrently created by another operation.')


def compose(args, *command, **kwargs):
    cmd = ['docker', 'compose', '--project-name', args.project,
           '--env-file', str(args.env_file.resolve()), '-f', str(HERE / ('compose.' + args.backend + '.yml'))]
    if args.oidc:
        cmd += ['-f', str(HERE / 'compose.oidc.yml')]
    if getattr(args, 'book_keys_file', None):
        cmd += ['-f', str(HERE / 'compose.book-credentials.yml')]
        kwargs['env'] = dict(os.environ, RD_BOOK_KEYS_HOST_FILE=str(args.book_keys_file.absolute()))
    return run(cmd + list(command), **kwargs)


def validate_key_file(path, required_ids):
    """Preflight only: authenticated decryption is performed by the API image."""
    if path is None:
        if required_ids:
            raise OperationError('Credential keys are required: supply --book-keys-file from the separate private key backup.')
        return
    invalid = 'Credential key file must be a private regular non-symlink 0600 key ring containing every required key.'
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, 'r', encoding='utf-8') as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600 or metadata.st_size > 65536:
                raise ValueError()

            def unique(pairs):
                result = {}
                for name, value in pairs:
                    if name in result:
                        raise ValueError()
                    result[name] = value
                return result

            raw = stream.read(65537)
            if len(raw.encode('utf-8')) > 65536:
                raise ValueError()
            config = json.loads(raw, object_pairs_hook=unique)
        if not isinstance(config, dict) or set(config) != {'active_key', 'keys'}:
            raise ValueError()
        keys = config['keys']
        if not isinstance(keys, dict) or not 1 <= len(keys) <= 16:
            raise ValueError()
        if not isinstance(config['active_key'], str) or config['active_key'] not in keys:
            raise ValueError()
        for key_id, encoded in keys.items():
            if not re.fullmatch(r'[A-Za-z0-9_.-]{1,64}', key_id) or not isinstance(encoded, str):
                raise ValueError()
            decoded = base64.b64decode(encoded, validate=True)
            if len(decoded) != 32 or base64.b64encode(decoded).decode('ascii') != encoded:
                raise ValueError()
        if not set(required_ids).issubset(keys):
            raise ValueError()
    except (OSError, ValueError, TypeError, UnicodeError):
        raise OperationError(invalid) from None


def sqlite_key_ids(db, schema):
    return [row[0] for row in db.execute(KEY_IDS_SQL)] if schema >= 4 else []


def required_key_ids(value):
    if (not isinstance(value, list) or len(value) > 16
            or any(not isinstance(key, str) or not re.fullmatch(r'[A-Za-z0-9_.-]{1,64}', key) for key in value)
            or len(set(value)) != len(value)):
        raise OperationError('Invalid credential key inventory; no key material was emitted.')
    return sorted(value)


def validate_restored_credentials(args, image_id, *, volume=None, pg_container=None):
    # Only the newly restored database is opened. Keep the source snapshot and
    # its original database untouched; the API connection may run migrations.
    command = ['run', '--rm', '--user', '0:0', '--read-only', '--tmpfs', '/tmp',
               '--network', ('container:' + pg_container) if pg_container else 'none']
    if args.book_keys_file:
        command += ['-v', str(args.book_keys_file.absolute()) + ':/run/secrets/book_keys:ro',
                    '-e', 'RD_BOOK_KEYS_FILE=/run/secrets/book_keys']
    if volume:
        docker(*command, '-v', volume + ':/app/data',
               '-e', 'RD_DATABASE_URL=sqlite:///app/data/accounts.sqlite', image_id, 'check-credentials')
    else:
        # Do not put a PostgreSQL password in argv/env, the manifest, or stdout.
        # The private transient file is removed after this isolated check.
        with tempfile.TemporaryDirectory(prefix='rustdesk-restore-') as directory:
            url_file = Path(directory) / 'database_url'
            password = docker('exec', pg_container, 'cat', '/run/secrets/app_password').decode().removesuffix('\n')
            with url_file.open('x', encoding='utf-8') as stream:
                stream.write('postgres://rustdesk:' + quote(password, safe='') + '@127.0.0.1:5432/rustdesk\n')
            url_file.chmod(0o600)
            docker(*command, '-v', str(url_file) + ':/run/secrets/database_url:ro',
                   '-e', 'RD_DATABASE_URL_FILE=/run/secrets/database_url', image_id, 'check-credentials')


def backup(args):
    args.directory.mkdir(parents=True, mode=0o700, exist_ok=False)
    # Stop only this project's API. Operators must also stop external CLI/DB writers.
    compose(args, 'stop', '--timeout', '30', 'api')
    ids = compose(args, 'ps', '--all', '-q', 'api').decode().splitlines()
    if len(ids) != 1:
        raise OperationError('Backup requires exactly one existing API container.')
    container = inspect('container', ids[0])
    if container['State']['Running']:
        raise OperationError('Backup refused: API is still running.')
    image_id = container['Image']
    image = inspect('image', image_id)
    snapshot = args.directory / ('accounts.sqlite' if args.backend == 'sqlite' else 'accounts.dump')
    with snapshot.open('xb') as stream:
        if args.backend == 'sqlite':
            volume = next((mount['Name'] for mount in container['Mounts']
                           if mount['Destination'] == '/app/data' and mount['Type'] == 'volume'), None)
            if not volume:
                raise OperationError('Backup requires the configured named data volume.')
            # SQLite .backup reads committed WAL pages too; never copy only the main DB.
            script = ("set -eu; test -f /source/accounts.sqlite; "
                      "sqlite3 /source/accounts.sqlite \".backup '/tmp/accounts.sqlite'\"; "
                      "test \"$(sqlite3 /tmp/accounts.sqlite 'PRAGMA integrity_check')\" = ok; "
                      'cat /tmp/accounts.sqlite')
            docker('run', '--rm', '--user', '0:0', '--network', 'none',
                   '-v', volume + ':/source', '--entrypoint', '/bin/sh', image_id,
                   '-c', script, stdout=stream)
        else:
            compose(args, 'exec', '-T', 'postgres', 'sh', '-ec',
                    'export PGPASSWORD="$(cat /run/secrets/postgres_password)"; '
                    'exec pg_dump -h 127.0.0.1 -U rustdesk_admin -d rustdesk -Fc --no-owner --no-acl', stdout=stream)
    snapshot.chmod(0o600)
    if args.backend == 'sqlite':
        with closing(sqlite3.connect('file:' + str(snapshot.resolve()) + '?mode=ro', uri=True)) as db:
            schema = db.execute(SCHEMA_SQL).fetchone()[0]
            key_ids = sqlite_key_ids(db, schema)
    else:
        raw = compose(args, 'exec', '-T', 'postgres', 'sh', '-ec',
                      'export PGPASSWORD="$(cat /run/secrets/postgres_password)"; '
                      'exec psql -h 127.0.0.1 -U rustdesk_admin -d rustdesk -At -c "$1"', 'sh', SCHEMA_SQL)
        schema = int(raw.strip())
        key_ids = []
        if schema >= 4:
            raw = compose(args, 'exec', '-T', 'postgres', 'sh', '-ec',
                          'export PGPASSWORD="$(cat /run/secrets/postgres_password)"; '
                          'exec psql -h 127.0.0.1 -U rustdesk_admin -d rustdesk -At -c "$1"', 'sh', KEY_IDS_SQL)
            key_ids = raw.decode().splitlines()
    key_ids = required_key_ids(key_ids)
    validate_key_file(args.book_keys_file, key_ids)
    manifest = {'format': 1, 'backend': args.backend, 'schema': schema,
                'created_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'image_id': image_id, 'image_ref': container['Config']['Image'],
                'repo_digests': image.get('RepoDigests') or [],
                'file': snapshot.name, 'sha256': checksum(snapshot), 'api_stopped': True}
    if schema >= 4:
        # IDs route ciphertext to its key; the keys and their file path are never archived here.
        manifest['credential_key_ids'] = key_ids
    if args.backend == 'postgres':
        pg_id = compose(args, 'ps', '-q', 'postgres').decode().strip()
        pg = inspect('container', pg_id)
        manifest.update({'database_image_id': pg['Image'],
                         'database_admin_role': 'rustdesk_admin', 'database_app_role': 'rustdesk',
                         'database_image_ref': pg['Config']['Image'],
                         'database_repo_digests': inspect('image', pg['Image']).get('RepoDigests') or []})
    (args.directory / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (args.directory / 'manifest.json').chmod(0o600)
    print('Backup complete; API remains stopped. Snapshot, schema, image identity and checksum recorded. '
          'Credential keys were not copied: retain a separate encrypted key backup and rehearse restore.')


def restore(args):
    manifest = json.loads((args.directory / 'manifest.json').read_text())
    expected_file = 'accounts.sqlite' if args.backend == 'sqlite' else 'accounts.dump'
    if manifest.get('format') != 1 or manifest.get('backend') != args.backend or manifest.get('file') != expected_file:
        raise OperationError('Restore refused: backup format or backend differs.')
    snapshot = args.directory / expected_file
    if checksum(snapshot) != manifest.get('sha256'):
        raise OperationError('Restore refused: snapshot checksum mismatch.')
    key_ids = required_key_ids(manifest.get('credential_key_ids', []))
    validate_key_file(args.book_keys_file, key_ids)
    image_id = manifest.get('image_id', '')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', image_id):
        raise OperationError('Restore refused: invalid recorded API image identity.')
    if args.backend == 'sqlite' or manifest.get('schema', 0) >= 4:
        inspect('image', image_id)  # Requires check-credentials before creating a volume.
    if args.backend == 'sqlite':
        with closing(sqlite3.connect('file:' + str(snapshot.resolve()) + '?mode=ro', uri=True)) as db:
            if db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                raise OperationError('Restore refused: SQLite integrity check failed.')
            if db.execute(SCHEMA_SQL).fetchone()[0] != manifest.get('schema'):
                raise OperationError('Restore refused: schema differs from manifest.')
            actual_ids = required_key_ids(sqlite_key_ids(db, manifest['schema']))
            if actual_ids != key_ids:
                raise OperationError('Restore refused: credential key inventory differs from manifest.')
            validate_key_file(args.book_keys_file, actual_ids)
    elif not args.pg_password_file or not args.pg_password_file.is_file():
        raise OperationError('Restore requires a private PostgreSQL password file.')
    elif not args.app_password_file or not args.app_password_file.is_file():
        raise OperationError('Restore requires a private PostgreSQL application password file.')
    new_volume(args.target_volume, args.project)
    if args.backend == 'sqlite':
        with snapshot.open('rb') as stream:
            docker('run', '--rm', '-i', '--user', '0:0', '--network', 'none',
                   '-v', args.target_volume + ':/restore', '--entrypoint', '/bin/sh', image_id,
                   '-ec', 'test -z "$(ls -A /restore)"; umask 077; '
                   'cat > /restore/accounts.sqlite; chown -R 10001:10001 /restore; '
                   'chmod 700 /restore; chmod 600 /restore/accounts.sqlite', stdin=stream)
        if manifest['schema'] >= 4:
            validate_restored_credentials(args, image_id, volume=args.target_volume)
    else:
        pg_restore(args, snapshot, manifest)
    print('Restore complete into a new volume; data retained. Select its volume name before starting the API.')


def pg_restore(args, snapshot, manifest):
    name = args.project + '-restore-' + uuid.uuid4().hex[:12]
    started = False
    try:
        docker('run', '-d', '--name', name, '--label', 'rustdesk.restore-project=' + args.project,
               '--network', 'none', '-e', 'POSTGRES_USER=rustdesk_admin', '-e', 'POSTGRES_DB=rustdesk',
               '-e', 'POSTGRES_PASSWORD_FILE=/run/secrets/postgres_password',
               '-v', str(args.pg_password_file.resolve()) + ':/run/secrets/postgres_password:ro',
               '-v', str(args.app_password_file.resolve()) + ':/run/secrets/app_password:ro',
               '-v', str(HERE / 'initdb/10-app-role.sh') + ':/docker-entrypoint-initdb.d/10-app-role.sh:ro',
               '-v', args.target_volume + ':/var/lib/postgresql/data',
               args.postgres_image or manifest.get('database_image_id') or 'postgres:16-alpine')
        started = True
        for _ in range(60):
            probe = subprocess.run(['docker', 'exec', name, 'pg_isready', '-h', '127.0.0.1', '-U', 'rustdesk_admin', '-d', 'rustdesk'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if probe.returncode == 0:
                break
            time.sleep(1)
        else:
            raise OperationError('Restore database did not become ready; target volume retained.')
        count = docker('exec', name, 'psql', '-U', 'rustdesk_admin', '-d', 'rustdesk', '-At', '-c',
                       "SELECT COUNT(*) FROM pg_tables WHERE schemaname='public'")
        if int(count.strip()) != 0:
            raise OperationError('Restore refused: target database is not empty.')
        with snapshot.open('rb') as stream:
            docker('exec', '-i', name, 'pg_restore', '-U', 'rustdesk_admin', '-d', 'rustdesk',
                   '--role=rustdesk', '--exit-on-error', '--single-transaction', '--no-owner', '--no-acl', stdin=stream)
        raw = docker('exec', name, 'psql', '-U', 'rustdesk_admin', '-d', 'rustdesk', '-At', '-c', SCHEMA_SQL)
        if int(raw.strip()) != manifest.get('schema'):
            raise OperationError('Restored schema differs from manifest; target volume retained for inspection.')
        if manifest['schema'] >= 4:
            raw = docker('exec', name, 'psql', '-U', 'rustdesk_admin', '-d', 'rustdesk', '-At', '-c', KEY_IDS_SQL)
            key_ids = required_key_ids(raw.decode().splitlines())
            if key_ids != manifest.get('credential_key_ids', []):
                raise OperationError('Restored credential key inventory differs from manifest; target volume retained.')
            validate_key_file(args.book_keys_file, key_ids)
            validate_restored_credentials(args, manifest['image_id'], pg_container=name)
    finally:
        if started:
            # Only this uniquely named helper is stopped/removed; no volume deletion.
            docker('stop', '--time', '30', name)
            docker('rm', name)


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend', choices=['sqlite', 'postgres'], required=True)
    parser.add_argument('--project', required=True)
    parser.add_argument('--env-file', type=Path, default=HERE / '.env')
    parser.add_argument('--oidc', action='store_true')
    parser.add_argument('--book-keys-file', type=Path,
                        help='private key ring; enables credential overlay, never copied into the database backup')
    sub = parser.add_subparsers(dest='action', required=True)
    save = sub.add_parser('backup')
    save.add_argument('directory', type=Path, help='new, non-existing backup directory')
    load = sub.add_parser('restore')
    load.add_argument('directory', type=Path)
    load.add_argument('--target-volume', required=True, help='new volume; any existing volume is refused')
    load.add_argument('--postgres-image', help='defaults to recorded PostgreSQL image identity')
    load.add_argument('--pg-password-file', type=Path)
    load.add_argument('--app-password-file', type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,62}', args.project):
        raise OperationError('Invalid project name.')
    if args.action == 'restore' and not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}', args.target_volume):
        raise OperationError('Invalid target volume name.')
    (backup if args.action == 'backup' else restore)(args)


if __name__ == '__main__':
    try:
        main()
    except OperationError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except (OSError, ValueError, KeyError, sqlite3.Error, StopIteration):
        print('Operation failed safely; API may remain stopped. Keep all artifacts; check configuration privately.', file=sys.stderr)
        sys.exit(1)
