#!/usr/bin/env python3
"""Start an isolated API fixture for an explicitly authorized manual client test."""
import argparse
import datetime
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time

from run_m0 import BASE, ROOT, RUNTIME, Runner, private_json


def timestamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save_manifest(path, manifest):
    descriptor, name = tempfile.mkstemp(prefix=".manual-client-", suffix=".tmp", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w") as f:
            json.dump(manifest, f, indent=2)
            f.write("\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def interrupted(_signum, _frame):
    raise KeyboardInterrupt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(ROOT / "api/target/debug/rustdesk-account-api"))
    args = parser.parse_args()
    args.database = "sqlite"
    manifest_path = RUNTIME / "manual-client.json"
    if not (RUNTIME / "fixture.json").is_file():
        print("Run validation/run_m0.py prepare and start its Keycloak first.", file=sys.stderr)
        return 1
    runner = Runner(args)
    manifest = None
    signal.signal(signal.SIGTERM, interrupted)
    try:
        # setup refuses an occupied API port, creates a fresh SQLite directory and
        # users through the CLI, then starts only its own API child process.
        runner.setup()
        manifest = {"state": "ready", "ready": True, "started_at": timestamp(),
                    "run_namespace": runner.namespace, "api_url": BASE,
                    "database_directory": str(runner.directory),
                    "launcher_pid": os.getpid(), "api_pid": runner.proc.pid,
                    "local_user": runner.locals[0],
                    "idp_member_user": runner.fixture["users"]["member"],
                    "idp_realm": runner.fixture["realm"],
                    "api_binary_sha256": runner.report["api_binary_sha256"]}
        if manifest_path.exists():
            # Preserve the previous manifest rather than losing prior runtime data.
            private_json(runner.directory / "previous-manual-client.json", json.loads(manifest_path.read_text()))
        save_manifest(manifest_path, manifest)
        private_json(runner.directory / "manual-client.json", manifest)
        print(json.dumps({"credential_file": str(manifest_path), "api_url": BASE, "ready": True}), flush=True)
        while runner.proc.poll() is None:
            time.sleep(.5)
        manifest.update(state="api_exited", ready=False)
        print("Manual fixture API exited; inspect its private runtime log.", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        if manifest is not None:
            manifest.update(state="stopped", ready=False)
        return 0
    except Exception:
        # Never print raw exceptions, HTTP responses or environment data.
        if manifest is not None:
            manifest.update(state="failed", ready=False)
        print("Manual fixture setup failed; port must be free and Keycloak ready. Inspect private runtime logs.", file=sys.stderr)
        return 1
    finally:
        runner.stop()
        if manifest is not None:
            manifest["stopped_at"] = timestamp()
            try:
                current = json.loads(manifest_path.read_text())
                if current.get("run_namespace") == runner.namespace:
                    save_manifest(manifest_path, manifest)
            except (OSError, ValueError):
                pass
            save_manifest(runner.directory / "manual-client.json", manifest)


if __name__ == "__main__":
    sys.exit(main())
