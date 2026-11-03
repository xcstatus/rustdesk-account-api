"""Prevent shell configuration from escaping an isolated restore fixture."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('credential_restore_runner', ROOT / 'validation/run_credentials_restore.py')
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class EnvironmentIsolationTests(unittest.TestCase):
    def test_docker_and_ops_subprocesses_cannot_inherit_deployment_overrides(self):
        poison = {
            'RD_DATA_VOLUME': 'outside-data', 'RD_POSTGRES_VOLUME': 'outside-pg',
            'RD_SECRETS_DIR': '/outside/secrets', 'RD_BOOK_KEYS_HOST_FILE': '/outside/key',
            'RD_API_IMAGE': 'outside-api', 'RD_DATABASE_URL': 'outside-url',
            'RD_WEB_ORIGIN': 'https://outside.invalid', 'RD_NEW_SETTING': 'future-override',
            'POSTGRES_IMAGE': 'outside-db', 'CADDY_IMAGE': 'outside-proxy', 'ACME_EMAIL': 'outside-email',
            'COMPOSE_FILE': '/outside/compose.yml', 'COMPOSE_ENV_FILES': '/outside/.env',
            'COMPOSE_PROJECT_NAME': 'outside-project', 'COMPOSE_PROFILES': 'standalone-caddy',
            'DOCKER_HOST': 'tcp://outside.invalid:2375', 'DOCKER_CONTEXT': 'outside-context',
        }
        completed = subprocess.CompletedProcess([], 0, stdout=b'', stderr=b'')
        with patch.dict(os.environ, poison), patch.object(runner.subprocess, 'run', return_value=completed) as execute:
            runner.run(['docker', 'compose', '--env-file', '/new-fixture/compose.env', 'up'])
            runner.run(['python3', str(ROOT / 'deploy/ops.py'), '--backend', 'sqlite', 'backup', '/new-fixture/backup'])
            runner.docker('run', '--rm', 'fixture-image', 'check-credentials')
        self.assertEqual(execute.call_count, 3)
        for call in execute.call_args_list:
            environment = call.kwargs['env']
            for name in poison:
                self.assertNotIn(name, environment)
            self.assertIn('PATH', environment)

    def test_remote_docker_context_is_refused_before_mutation(self):
        with patch.object(runner, 'docker', return_value=json.dumps([
            {'Endpoints': {'docker': {'Host': 'tcp://outside.invalid:2375'}}}
        ]).encode()) as docker:
            with self.assertRaises(RuntimeError):
                runner.require_local_docker()
            docker.assert_called_once_with('context', 'inspect')
        with patch.object(runner, 'docker', return_value=json.dumps([
            {'Endpoints': {'docker': {'Host': 'unix:///local/docker.sock'}}}
        ]).encode()):
            runner.require_local_docker()


if __name__ == '__main__':
    unittest.main()
