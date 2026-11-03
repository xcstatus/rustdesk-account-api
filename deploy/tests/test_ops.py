"""Deployment security boundaries; uses only newly generated temporary fixtures."""
import base64
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parents[1]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


ops = module('deploy_ops', 'ops.py')
init_keys = module('init_book_keys', 'init-book-keys.py')
IMAGE = 'sha256:' + 'a' * 64


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.key = self.root / 'book_keys.json'
        init_keys.create(self.key)


class KeyTests(Fixture):
    def test_generator_cli_does_not_emit_key_or_overwrite(self):
        path = self.root / 'cli.json'
        command = [sys.executable, str(HERE / 'init-book-keys.py'), '--file', str(path)]
        first = subprocess.run(command, capture_output=True)
        self.assertEqual(first.returncode, 0)
        material = path.read_bytes()
        encoded = json.loads(material)['keys']['v1'].encode()
        self.assertNotIn(encoded, first.stdout + first.stderr)
        second = subprocess.run(command, capture_output=True)
        self.assertNotEqual(second.returncode, 0)
        self.assertNotIn(encoded, second.stdout + second.stderr)
        self.assertEqual(path.read_bytes(), material)

    def test_generation_private_random_and_never_overwrites(self):
        first = self.key.read_bytes()
        config = json.loads(first)
        self.assertEqual(stat.S_IMODE(self.key.stat().st_mode), 0o600)
        self.assertEqual(config['active_key'], 'v1')
        self.assertEqual(len(base64.b64decode(config['keys']['v1'], validate=True)), 32)
        with self.assertRaises(FileExistsError):
            init_keys.create(self.key)
        self.assertEqual(self.key.read_bytes(), first)
        second = self.root / 'second.json'
        init_keys.create(second)
        self.assertNotEqual(json.loads(second.read_text())['keys']['v1'], config['keys']['v1'])
        link = self.root / 'link'
        link.symlink_to(self.root / 'absent')
        with self.assertRaises(FileExistsError):
            init_keys.create(link)
        self.assertFalse((self.root / 'absent').exists())

    def test_preflight_rejects_missing_old_keys_permissions_symlink_fifo(self):
        ops.validate_key_file(None, [])
        ops.validate_key_file(self.key, ['v1'])
        with self.assertRaises(ops.OperationError):
            ops.validate_key_file(None, ['v1'])
        with self.assertRaises(ops.OperationError):
            ops.validate_key_file(self.key, ['retired-v0'])
        for mode in [0o644, 0o640, 0o400, 0o1600]:
            self.key.chmod(mode)
            with self.assertRaises(ops.OperationError):
                ops.validate_key_file(self.key, ['v1'])
        self.key.chmod(0o600)
        link = self.root / 'link'
        link.symlink_to(self.key)
        fifo = self.root / 'fifo'
        os.mkfifo(fifo, 0o600)
        for path in [link, fifo, self.root]:
            with self.assertRaises(ops.OperationError):
                ops.validate_key_file(path, ['v1'])

    def test_preflight_rejects_duplicate_invalid_and_oversized_keys(self):
        encoded = json.loads(self.key.read_text())['keys']['v1']
        for value in [
            '{"active_key":"v1","keys":{"v1":"' + encoded + '","v1":"' + encoded + '"}}',
            json.dumps({'active_key': 'v2', 'keys': {'v1': encoded}}),
            json.dumps({'active_key': 'v1', 'keys': {'v1': 'private-invalid-value'}}),
            ' ' * 65537,
        ]:
            self.key.write_text(value)
            with self.assertRaises(ops.OperationError) as caught:
                ops.validate_key_file(self.key, ['v1'])
            self.assertNotIn(encoded, str(caught.exception))
            self.assertNotIn('private-invalid-value', str(caught.exception))


class RestoreTests(Fixture):
    def setUp(self):
        super().setUp()
        self.directory = self.root / 'backup'
        self.directory.mkdir()
        self.snapshot = self.directory / 'accounts.sqlite'
        with contextlib.closing(sqlite3.connect(self.snapshot)) as db, db:
            db.executescript('CREATE TABLE account_schema_migrations(version INTEGER);'
                             'INSERT INTO account_schema_migrations VALUES(4);'
                             'CREATE TABLE book_credentials(key_id TEXT);'
                             "INSERT INTO book_credentials VALUES('v1');")
        self.manifest = {'format': 1, 'backend': 'sqlite', 'schema': 4, 'file': self.snapshot.name,
                         'sha256': ops.checksum(self.snapshot), 'image_id': IMAGE, 'credential_key_ids': ['v1']}
        self.write_manifest()
        self.args = types.SimpleNamespace(directory=self.directory, backend='sqlite', project='test-restore',
                                          target_volume='test-new-volume', book_keys_file=self.key)

    def write_manifest(self):
        (self.directory / 'manifest.json').write_text(json.dumps(self.manifest))

    def test_restore_missing_keys_and_inventory_tamper_refused_before_volume(self):
        with patch.object(ops, 'new_volume') as new, patch.object(ops, 'inspect'):
            self.args.book_keys_file = None
            with self.assertRaises(ops.OperationError):
                ops.restore(self.args)
            self.args.book_keys_file = self.key
            self.manifest['credential_key_ids'] = []
            self.write_manifest()
            with self.assertRaises(ops.OperationError):
                ops.restore(self.args)
            new.assert_not_called()

    def test_restore_authentication_failure_never_reports_success(self):
        def docker(*command, **kwargs):
            if command[-1] == 'check-credentials':
                self.assertIn(IMAGE, command)
                self.assertIn('none', command)
                self.assertIn(self.args.target_volume + ':/app/data', command)
                self.assertIn(str(self.key) + ':/run/secrets/book_keys:ro', command)
                raise ops.OperationError('authentication failed (redacted)')
        output = io.StringIO()
        with patch.object(ops, 'new_volume'), patch.object(ops, 'inspect'), patch.object(ops, 'docker', side_effect=docker) as run:
            with contextlib.redirect_stdout(output), self.assertRaises(ops.OperationError):
                ops.restore(self.args)
            self.assertEqual(run.call_count, 2)
        self.assertNotIn('Restore complete', output.getvalue())
        self.assertEqual(ops.checksum(self.snapshot), self.manifest['sha256'])

    def test_backup_manifest_excludes_key_values_and_paths(self):
        destination = self.root / 'new-backup'
        args = types.SimpleNamespace(directory=destination, backend='sqlite', book_keys_file=self.key)
        container = {'State': {'Running': False}, 'Image': IMAGE, 'Config': {'Image': 'recorded-api'},
                     'Mounts': [{'Destination': '/app/data', 'Type': 'volume', 'Name': 'source-data'}]}
        def compose(_args, *command, **kwargs):
            return b'container-id\n' if command[0] == 'ps' else b''
        def docker(*command, **kwargs):
            kwargs['stdout'].write(self.snapshot.read_bytes())
        with patch.object(ops, 'compose', side_effect=compose), patch.object(ops, 'docker', side_effect=docker), \
                patch.object(ops, 'inspect', side_effect=[container, {}]), contextlib.redirect_stdout(io.StringIO()):
            ops.backup(args)
        raw = (destination / 'manifest.json').read_text()
        self.assertEqual(json.loads(raw)['credential_key_ids'], ['v1'])
        self.assertNotIn(str(self.key), raw)
        self.assertNotIn(json.loads(self.key.read_text())['keys']['v1'], raw)
        self.assertEqual(sorted(p.name for p in destination.iterdir()), ['accounts.sqlite', 'manifest.json'])

    def test_pg_check_uses_private_transient_url_and_removes_it(self):
        password = self.root / 'app_password'
        password.write_text('test-password:@/\n')
        password.chmod(0o600)
        self.args.app_password_file = password
        observed = []
        def docker(*command, **kwargs):
            if command[:3] == ('exec', 'isolated-helper', 'cat'):
                return password.read_bytes()
            self.assertEqual(command[-1], 'check-credentials')
            self.assertIn('container:isolated-helper', command)
            self.assertNotIn('test-password', ' '.join(command))
            mount = next(value for value in command if value.endswith(':/run/secrets/database_url:ro'))
            transient = Path(mount.removesuffix(':/run/secrets/database_url:ro'))
            observed.append(transient)
            self.assertEqual(stat.S_IMODE(transient.stat().st_mode), 0o600)
            self.assertIn('test-password%3A%40%2F@127.0.0.1', transient.read_text())
            raise ops.OperationError('authentication failed (redacted)')
        with patch.object(ops, 'docker', side_effect=docker), self.assertRaises(ops.OperationError):
            ops.validate_restored_credentials(self.args, IMAGE, pg_container='isolated-helper')
        self.assertEqual(len(observed), 1)
        self.assertFalse(observed[0].exists())


@unittest.skipUnless(shutil.which('docker'), 'Docker Compose unavailable')
class ComposeTests(unittest.TestCase):
    def test_backend_oidc_traefik_combinations_retain_secrets_and_hardening(self):
        env = dict(os.environ, RD_DOMAIN='test.invalid', RD_WEB_ORIGIN='https://test.invalid',
                   RD_DATA_VOLUME='test-data', RD_POSTGRES_VOLUME='test-postgres', RD_CADDY_VOLUME='test-caddy',
                   RD_OIDC_ISSUER='https://idp.invalid', RD_OIDC_CLIENT_ID='test-client',
                   RD_TRAEFIK_NETWORK='test-proxy', RD_DATABASE_NETWORK='test-database',
                   RD_TRAEFIK_CERTRESOLVER='test-resolver', RD_BOOK_KEYS_HOST_FILE='/tmp/test-book-keys.json')
        for backend in ['sqlite', 'postgres', 'external-postgres']:
            for oidc in [False, True]:
                for traefik in ([False, True] if backend == 'external-postgres' else [False]):
                    with self.subTest(backend=backend, oidc=oidc, traefik=traefik):
                        files = ['compose.' + backend + '.yml']
                        if oidc:
                            files += ['compose.oidc.yml']
                        if traefik:
                            files += ['compose.traefik.yml']
                        files += ['compose.book-credentials.yml']
                        command = ['docker', 'compose', '-p', 'test-config']
                        for filename in files:
                            command += ['-f', str(HERE / filename)]
                        result = subprocess.run(command + ['config', '--format', 'json'], env=env, capture_output=True)
                        self.assertEqual(result.returncode, 0, 'Compose configuration failed (raw output withheld)')
                        config = json.loads(result.stdout)
                        api = config['services']['api']
                        self.assertEqual(api['environment']['RD_BOOK_KEYS_FILE'], '/run/secrets/book_keys')
                        mounted = {item['source'] for item in api['secrets']}
                        self.assertTrue({'database_url', 'book_keys'}.issubset(mounted))
                        self.assertEqual('oidc_client_secret' in mounted, oidc)
                        self.assertEqual(api['user'], '10001:10001')
                        self.assertTrue(api['read_only'])
                        self.assertEqual(config['secrets']['book_keys']['file'], '/tmp/test-book-keys.json')


if __name__ == '__main__':
    unittest.main()
