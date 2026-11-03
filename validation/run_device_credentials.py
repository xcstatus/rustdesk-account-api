#!/usr/bin/env python3
"""Real isolated HTTP + existing Keycloak validation for devices/credentials.

Only this suite's API processes/databases are changed. Private fixtures and logs
stay under feature-* (0700/0600). Public reports contain no reusable credentials,
device UUIDs, callback URLs/codes, raw HTTP/SQL responses, or raw exceptions.
No official RustDesk client GUI is exercised by this script.
"""
import argparse
import base64
import csv
import datetime
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import socket
import sqlite3
import subprocess
import sys
import time

sys.dont_write_bytecode = True
try:
    from . import run_m0 as m0, run_m1 as m1
except ImportError:
    import run_m0 as m0
    import run_m1 as m1

require = m0.require
BASE = m0.BASE
PLANNED = [
    "setup.private-keys-isolated-database-real-api",
    "local.distinct-device-logins-session-binding",
    "credentials.cross-token-hash-roundtrip-user-isolation",
    "credentials.database-encryption-metadata-excludes-secrets",
    "credentials.invalid-snapshot-preserves-both-tables",
    "credentials.unpadded-standard-base64-roundtrip",
    "credentials.snapshot-clear-omitted-empty-removed",
    "devices.client-directory-isolation-pagination-privacy",
    "devices.web-directory-admin-member-scope",
    "devices.assignment-admin-csrf-session-purpose-required",
    "devices.invalid-owner-no-partial-writes",
    "devices.transfer-revokes-bound-sessions-transaction-facts",
    "devices.unassign-and-login-does-not-reclaim",
    "oidc.real-keycloak-jit-poll-device-binding-repeat",
    "logout.token-401-ownership-retained",
    "restart.same-keys-decrypts-existing-credentials",
    "restart.missing-key-config-refused-data-retained",
    "restart.wrong-key-material-refused-data-retained",
    "restart.unavailable-key-id-refused-data-retained",
    "restart.correct-keys-restores-readiness-and-data",
]


class Runner(m1.Runner):
    def __init__(self, args, database):
        self.args = argparse.Namespace(**{**vars(args), "database": database})
        self.fixture = json.loads((m0.RUNTIME / "fixture.json").read_text())
        self.namespace = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d%H%M%S") + "_" + secrets.token_hex(4)
        self.directory = m0.RUNTIME / ("feature-" + database + "-" + self.namespace)
        self.directory.mkdir(mode=0o700)
        self.binary = str(Path(args.binary).resolve())
        self.report = {"database": database, "namespace": self.namespace, "status": "running", "cases": [],
                       "planned_cases": len(PLANNED),
                       "scope": "real loopback HTTP, existing Keycloak authorization-code/PKCE forms, direct SQL evidence; no official client GUI",
                       "synthetic_hash": "standard base64 of 32 random synthetic bytes; not a production credential",
                       "limitations": ["HTTP form/cookie driver is not an official client GUI", "existing Keycloak fixture uses loopback HTTP", "transaction evidence compares committed state and rejected writes; no crash injection"]}
        self.env = os.environ.copy()
        for key in list(self.env):
            if key.startswith("RD_"):
                self.env.pop(key)
        self.proc = self.log = None
        self.pg_name = None
        self.users = {}
        self.devices = {name: {"id": "feature-" + name + "-" + secrets.token_hex(5), "uuid": secrets.token_hex(16)}
                        for name in ["a1", "a2", "b1", "oidc"]}
        self.hash_a = base64.b64encode(secrets.token_bytes(32)).decode()
        self.hash_b = base64.b64encode(secrets.token_bytes(32)).decode()
        self.key_path = self.directory / "book-keys.json"
        self.key_config = {"active_key": "feature-v1", "keys": {"feature-v1": base64.b64encode(secrets.token_bytes(32)).decode()}}
        m0.private_json(self.key_path, self.key_config)

    def pg(self, sql, database=None):
        result = subprocess.run(m0.COMPOSE + ["exec", "-T", "postgres", "psql", "-U", "rustdesk_m0", "-d",
                                database or self.pg_name or "postgres", "-v", "ON_ERROR_STOP=1", "--csv", "-t", "-P", "null=__FEATURE_NULL__"],
                                input=sql.encode(), capture_output=True, timeout=30)
        require(result.returncode == 0, "isolated PostgreSQL operation failed (raw output withheld)")
        return [["" if value == "__FEATURE_NULL__" else value for value in row]
                for row in csv.reader(io.StringIO(result.stdout.decode()))]

    def sql(self, query):
        if self.args.database == "postgres":
            return self.pg(query)
        with sqlite3.connect(self.directory / "accounts.sqlite") as connection:
            return [["" if value is None else str(value) for value in row] for row in connection.execute(query).fetchall()]

    def setup(self):
        require(Path(self.binary).is_file(), "API binary absent")
        self.report["api_binary_sha256"] = hashlib.sha256(Path(self.binary).read_bytes()).hexdigest()
        self.report["web_index_sha256"] = hashlib.sha256((m0.ROOT / "web/dist/index.html").read_bytes()).hexdigest()
        require(self.key_path.stat().st_mode & 0o777 == 0o600, "key file permission differs from 0600")
        with socket.socket() as sock:
            require(sock.connect_ex(("127.0.0.1", 21114)) != 0, "API fixture port occupied; existing process untouched")
        if self.args.database == "sqlite":
            database = "sqlite://" + str(self.directory / "accounts.sqlite") + "?mode=rwc"
            self.report["sqlite_version"] = sqlite3.sqlite_version
        else:
            name = "feature_" + self.namespace
            self.pg("CREATE DATABASE " + name + ";", "postgres")
            self.pg_name = name
            database = "postgres://rustdesk_m0:" + self.fixture["pg_password"] + "@127.0.0.1:15432/" + name
            self.report["postgres_server_version"] = self.pg("SHOW server_version;", "postgres")[0][0]
        self.env.update({"RD_DATABASE_URL": database, "RD_LISTEN": "127.0.0.1:21114", "RD_WEB_ORIGIN": BASE,
                         "RD_WEB_DIR": str(m0.ROOT / "web/dist"), "RD_WEB_SESSION_TTL_SECONDS": str(m1.WEB_TTL),
                         "RD_LOGIN_PER_ACCOUNT": "100", "RD_BOOK_KEYS_FILE": str(self.key_path),
                         "RD_OIDC_ISSUER": m0.IDP + "/realms/" + self.fixture["realm"],
                         "RD_OIDC_CLIENT_ID": self.fixture["client_id"], "RD_OIDC_CLIENT_SECRET": self.fixture["client_secret"],
                         "RD_OIDC_REDIRECT_URI": BASE + "/api/oidc/callback", "RD_OIDC_CLAIM_PATH": "groups",
                         "RD_OIDC_ADMIN_VALUES": "rustdesk-admins", "RD_OIDC_MEMBER_VALUES": "rustdesk-users",
                         "RD_OIDC_UNMATCHED": "deny", "RD_ALLOW_HTTP_LOOPBACK": "true"})
        for name in ["a", "b", "admin"]:
            user = {"username": "feature_" + name + "_" + self.namespace, "password": secrets.token_urlsafe(24)}
            self.cli("create-local", user["username"], "--role", "admin" if name == "admin" else "member",
                     extra={"RD_BOOTSTRAP_PASSWORD": user["password"]})
            self.users[name] = user
        m0.private_json(self.directory / "fixture.json", {"users": self.users, "devices": self.devices,
                                                         "hashes": [self.hash_a, self.hash_b],
                                                         "environment": {key: value for key, value in self.env.items() if key.startswith("RD_")}})
        self.start()
        require(self.client("GET", "/readyz").status == 200, "API readiness failed")
        self.web = {name: self.web_login(user) for name, user in self.users.items()}
        for name, session in self.web.items():
            self.users[name]["id"] = session.user["id"]

    def client(self, method, path, token=None, data=None):
        return m1.http(method, path, data=data, headers={"Authorization": "Bearer " + token} if token else {})

    def device_login(self, who, device):
        response = self.client("POST", "/api/login", data={**self.devices[device], "username": self.users[who]["username"],
            "password": self.users[who]["password"], "type": "account",
            "deviceInfo": {"os": "Linux", "name": "Feature " + device, "type": "desktop"}})
        return self.login_shape(response)["access_token"]

    def snapshot(self, hash_value=None):
        return {"peers": [{"id": "synthetic-peer", "alias": "工作机", "platform": "Mac OS", "tags": ["work"],
                           **({"hash": hash_value} if hash_value is not None else {})}], "tags": ["work"]}

    def write(self, token, value, status=200):
        response = self.client("POST", "/api/ab", token, {"data": json.dumps(value)})
        require(response.status == status, "snapshot HTTP status differs")
        require(response.headers.get("Cache-Control") == "no-store", "snapshot cache policy differs")
        if status == 200:
            require(response.headers.get("X-Rustdesk-Credential-Sync") == "enabled; personal-salted-hash", "enabled credential header absent")

    def private(self, value):
        raw = json.dumps(value)
        require("uuid" not in raw.lower(), "device response exposed UUID field")
        require(all(device["uuid"] not in raw for device in self.devices.values()), "device response exposed private binding")

    def state(self):
        return [self.sql(query) for query in ["SELECT user_id,snapshot FROM books ORDER BY user_id",
            "SELECT user_id,peer_id,key_id,nonce,ciphertext FROM book_credentials ORDER BY user_id,peer_id"]]

    def logins(self):
        self.tokens = {"a1": self.device_login("a", "a1"), "a1repeat": self.device_login("a", "a1"),
                       "a2": self.device_login("a", "a2"), "b1": self.device_login("b", "b1")}
        require(len(set(self.tokens.values())) == 4, "separate logins reused sessions")
        rows = self.sql("SELECT d.rustdesk_id,d.owner_user_id,count(s.id) FROM devices d LEFT JOIN sessions s ON s.device_id=d.id GROUP BY d.rustdesk_id,d.owner_user_id")
        require(len(rows) == 3, "repeat device login inserted duplicate device")
        expected = {self.devices[k]["id"]: (self.users["b" if k == "b1" else "a"]["id"], "2" if k == "a1" else "1") for k in ["a1", "a2", "b1"]}
        require({r[0]: (r[1], r[2]) for r in rows} == expected, "device ownership/session SQL binding differs")

    def roundtrip(self):
        upload = self.snapshot(self.hash_a)
        upload["peers"][0]["password"] = "synthetic-never-returned"
        upload["user_id"] = self.users["b"]["id"]
        self.write(self.tokens["a1"], upload)
        require(self.book(self.tokens["a2"]) == self.snapshot(self.hash_a), "credential did not cross own tokens or unsafe fields returned")
        require(self.book(self.tokens["b1"]) == {"peers": [], "tags": []}, "another user's book leaked")
        self.write(self.tokens["b1"], self.snapshot(self.hash_b))
        require(self.book(self.tokens["a2"]) == self.snapshot(self.hash_a), "same peer ID crossed account boundary")

    def encrypted(self):
        metadata, sealed = self.state()
        require(len(metadata) == 3 and len(sealed) == 2, "expected personal metadata and credential rows absent")
        for row in metadata:
            require("hash" not in row[1] and "password" not in row[1], "metadata retained secret fields")
        require(all(len(row[3]) == 16 and len(row[4]) == 80 for row in sealed), "sealed credential shape differs")
        raw = json.dumps([metadata, sealed])
        require(self.hash_a not in raw and self.hash_b not in raw, "database retained reusable clear credential")

    def invalid(self):
        before = self.state()
        for invalid in ["invalid", base64.b64encode(secrets.token_bytes(31)).decode(), 17,
                        base64.b64encode(bytes([255]) * 32).decode().replace("/", "_")]:
            self.write(self.tokens["a2"], self.snapshot(invalid), 400)
            require(self.state() == before, "rejected hash changed metadata or credentials")
        duplicate = self.snapshot(self.hash_a)
        duplicate["peers"].append(duplicate["peers"][0].copy())
        self.write(self.tokens["a2"], duplicate, 400)
        require(self.state() == before, "duplicate peer changed committed book")

    def unpadded(self):
        value = self.snapshot(self.hash_a.rstrip("="))
        self.write(self.tokens["a2"], value)
        require(self.book(self.tokens["a1"]) == value, "unpadded standard base64 not preserved")

    def clear(self):
        for peers in [self.snapshot()["peers"], self.snapshot("")["peers"], []]:
            self.write(self.tokens["a2"], self.snapshot(self.hash_a))
            self.write(self.tokens["a2"], {"peers": peers, "tags": []})
            require(all("hash" not in peer for peer in self.book(self.tokens["a1"])["peers"]), "snapshot clear left credential readable")
            require(self.sql("SELECT count(*) FROM book_credentials WHERE user_id='" + self.users["a"]["id"] + "'")[0][0] == "0", "snapshot clear retained sealed row")
            require(self.book(self.tokens["b1"]) == self.snapshot(self.hash_b), "clear changed another account")
        self.write(self.tokens["a2"], self.snapshot(self.hash_a))

    def directories(self):
        for who, token, names in [("a", self.tokens["a2"], ["a1", "a2"]), ("b", self.tokens["b1"], ["b1"])]:
            response = self.client("GET", "/api/peers", token)
            require(response.status == 200 and response.headers.get("Cache-Control") == "no-store", "directory read/cache policy differs")
            value = response.json()
            self.private(value)
            require({r["id"] for r in value["data"]} == {self.devices[k]["id"] for k in names}, "client directory escaped ownership scope")
            require(all(r["user"] == self.users[who]["id"] for r in value["data"]), "directory owner differs")
            for page in range(1, len(names) + 2):
                paged = self.client("GET", "/api/peers?current=" + str(page) + "&pageSize=1", token).json()
                require(paged["total"] == len(names) and len(paged["data"]) == (1 if page <= len(names) else 0), "directory pagination differs")
            users = self.client("GET", "/api/users", token).json()
            require(users["total"] == 1 and users["data"][0]["id"] == self.users[who]["id"], "client users escaped current user")

    def web_directory(self):
        all_devices = self.web["admin"].call("GET", "/devices").json()
        self.private(all_devices)
        require(len(all_devices["devices"]) == 3, "admin directory omitted device")
        for who, count in [("a", 2), ("b", 1)]:
            value = self.web[who].call("GET", "/devices").json()
            self.private(value)
            require(len(value["devices"]) == count and all(d["owner_user_id"] == self.users[who]["id"] for d in value["devices"]), "web member scope differs")
        self.target = next(d for d in all_devices["devices"] if d["rustdesk_id"] == self.devices["a1"]["id"])
        self.target_path = "/devices/" + self.target["id"]

    def guards(self):
        data = {"owner_user_id": self.users["b"]["id"]}
        require(self.web["a"].call("PATCH", self.target_path, data).status == 403, "member assigned device")
        for kwargs in [{"csrf": False}, {"origin": None}, {"origin": "https://untrusted.invalid"}, {"csrf": "incorrect"}]:
            require(self.web["admin"].call("PATCH", self.target_path, data, **kwargs).status == 403, "assignment accepted invalid CSRF/origin")
        require(self.client("GET", "/admin/api/v1/devices", self.tokens["a1"]).status == 401, "client bearer authorized Web directory")

    def device_state(self):
        return [self.sql(q) for q in ["SELECT id,owner_user_id FROM devices ORDER BY id", "SELECT id,revoked,device_id FROM sessions ORDER BY id", "SELECT actor_id,device_id,action,old_owner_id,new_owner_id,revoked_count FROM device_events ORDER BY id"]]

    def invalid_owner(self):
        before = self.device_state()
        response = self.web["admin"].call("PATCH", self.target_path, {"owner_user_id": "missing-synthetic-owner"})
        require(response.status == 404, "missing owner status differs")
        require(self.device_state() == before, "rejected assignment partially changed owner, sessions or events")

    def transfer(self):
        response = self.web["admin"].call("PATCH", self.target_path, {"owner_user_id": self.users["b"]["id"]})
        require(response.status == 200 and response.json()["device"]["owner_user_id"] == self.users["b"]["id"], "transfer owner differs")
        self.private(response.json())
        for key in ["a1", "a1repeat"]:
            for method, path in [("GET", "/api/ab"), ("GET", "/api/peers"), ("POST", "/api/currentUser")]:
                require(self.client(method, path, self.tokens[key], {} if method == "POST" else None).status == 401, "transferred device token remains usable")
        require(self.client("GET", "/api/ab", self.tokens["a2"]).status == 200, "transfer revoked another device")
        require(self.web["a"].call("GET", "/devices").status == 200, "transfer revoked Web session")
        events = self.sql("SELECT actor_id,device_id,action,old_owner_id,new_owner_id,revoked_count FROM device_events ORDER BY id")
        require(events == [[self.users["admin"]["id"], self.target["id"], "assign", self.users["a"]["id"], self.users["b"]["id"], "2"]], "transaction device event facts differ")
        require(self.sql("SELECT count(*) FROM sessions WHERE device_id='" + self.target["id"] + "' AND revoked=1")[0][0] == "2", "SQL revoked session count differs")
        require(self.client("GET", "/api/peers", self.tokens["b1"]).json()["total"] == 2, "new owner client directory missed transfer")

    def unassign(self):
        require(self.web["admin"].call("PATCH", self.target_path, {"owner_user_id": None}).status == 200, "unassignment failed")
        self.device_login("a", "a1")
        require(self.sql("SELECT owner_user_id FROM devices WHERE id='" + self.target["id"] + "'") == [[""]], "login reclaimed explicitly unassigned device")
        events = self.sql("SELECT action,old_owner_id,new_owner_id,revoked_count FROM device_events ORDER BY id")
        require(events[-1] == ["unassign", self.users["b"]["id"], "", "0"], "unassignment event differs")

    def oidc(self):
        device = self.devices["oidc"]
        identities = []
        for repeat in range(2):
            response = self.client("POST", "/api/oidc/auth", data={**device, "op": "company"})
            require(response.status == 200, "OIDC auth start failed")
            attempt = {**device, **response.json()}
            require(self.poll(attempt, uuid="incorrect-synthetic-binding").status == 400, "wrong OIDC device binding accepted")
            require(self.browser(attempt, "member").status == 200, "real Keycloak callback failed")
            before = self.sql("SELECT count(*) FROM devices WHERE rustdesk_id='" + device["id"] + "'")[0][0]
            require(before == str(repeat != 0 and 1 or 0), "device created before token poll")
            response = self.poll(attempt)
            require(response.status == 200, "OIDC poll failed")
            value = response.json()
            require(value.get("type") == "access_token" and value.get("access_token") and value.get("user", {}).get("id"), "OIDC token/identity missing")
            # OIDC JIT may intentionally have an empty legacy name; identity ID is stable.
            identities.append(value["user"]["id"])
            require(self.poll(attempt).status == 400, "OIDC poll replay accepted")
            self.oidc_token = value["access_token"]
            peers = self.client("GET", "/api/peers", self.oidc_token).json()
            self.private(peers)
            require(peers["total"] == 1 and peers["data"][0]["id"] == device["id"] and peers["data"][0]["user"] == identities[-1], "OIDC device not visible to its owner")
        require(identities[0] == identities[1], "OIDC repeated JIT identity changed")
        require(self.sql("SELECT count(*) FROM users WHERE source='oidc'")[0][0] == "1", "OIDC repeat duplicated user")
        rows = self.sql("SELECT d.owner_user_id,count(s.id) FROM devices d JOIN sessions s ON s.device_id=d.id WHERE d.rustdesk_id='" + device["id"] + "' GROUP BY d.owner_user_id")
        require(rows == [[identities[0], "2"]], "OIDC repeated device/session binding differs")

    def logout_owned(self):
        before = self.sql("SELECT id,owner_user_id FROM devices ORDER BY id")
        for token in [self.oidc_token, self.tokens["b1"]]:
            self.revoke(token)
            for path in ["/api/peers", "/api/users"]:
                require(self.client("GET", path, token).status == 401, "logged-out token authorized directory")
        require(self.sql("SELECT id,owner_user_id FROM devices ORDER BY id") == before, "logout removed persistent device ownership")
        self.tokens["b1"] = self.device_login("b", "b1")

    def restart_read(self):
        self.saved_state = self.state()
        self.stop()
        self.start()
        require(self.book(self.tokens["a2"]) == self.snapshot(self.hash_a), "same-key restart lost own credentials")
        require(self.book(self.tokens["b1"]) == self.snapshot(self.hash_b), "same-key restart lost isolated credentials")
        require(self.state() == self.saved_state, "same-key restart changed persisted data")

    def refused(self, mode):
        self.stop()
        env = self.env.copy()
        if mode == "missing":
            env.pop("RD_BOOK_KEYS_FILE", None)
        else:
            path = self.directory / (mode + "-keys.json")
            config = {"active_key": "feature-v1" if mode == "wrong" else "feature-v2",
                      "keys": {"feature-v1" if mode == "wrong" else "feature-v2": base64.b64encode(secrets.token_bytes(32)).decode()}}
            m0.private_json(path, config)
            env["RD_BOOK_KEYS_FILE"] = str(path)
        with os.fdopen(os.open(self.directory / ("refused-" + mode + ".log"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as log:
            process = subprocess.Popen([self.binary, "serve"], cwd=self.directory, env=env, stdout=log, stderr=subprocess.STDOUT)
            try:
                result = process.wait(timeout=12)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
                raise m0.CheckFailure("unusable credential key configuration accepted startup")
        require(result != 0, "unusable key startup did not fail")
        require(self.state() == self.saved_state, "refused startup changed credential or metadata data")
        with socket.socket() as sock:
            require(sock.connect_ex(("127.0.0.1", 21114)) != 0, "refused API unexpectedly listening")

    def restore(self):
        self.start()
        require(self.client("GET", "/readyz").status == 200, "correct-key restore readiness failed")
        self.cli("check-credentials")
        require(self.book(self.tokens["a2"]) == self.snapshot(self.hash_a), "correct-key restore cannot decrypt")
        require(self.state() == self.saved_state, "restore changed preserved data")

    def run(self):
        functions = [self.setup, self.logins, self.roundtrip, self.encrypted, self.invalid, self.unpadded, self.clear,
                     self.directories, self.web_directory, self.guards, self.invalid_owner, self.transfer, self.unassign,
                     self.oidc, self.logout_owned, self.restart_read, lambda: self.refused("missing"),
                     lambda: self.refused("wrong"), lambda: self.refused("unavailable"), self.restore]
        try:
            for name, function in zip(PLANNED, functions):
                if not self.check(name, function):
                    # Preserve dependent-state validity; report all remaining cases explicitly.
                    for remaining in PLANNED[len(self.report["cases"]):]:
                        self.report["cases"].append({"case": remaining, "status": "blocked", "detail": "earlier prerequisite failed"})
                    break
        finally:
            self.stop()
            self.report["cleanup"] = {"own_api_stopped": True, "private_fixture_retained": True,
                                      "containers_changed": False, "isolated_postgres_database_dropped": None}
            if self.pg_name:
                try:
                    self.pg("DROP DATABASE " + self.pg_name + ";", "postgres")
                    self.report["cleanup"]["isolated_postgres_database_dropped"] = True
                except Exception:
                    self.report["cleanup"]["isolated_postgres_database_dropped"] = False
            passed = sum(case["status"] == "pass" for case in self.report["cases"])
            self.report.update({"passed": passed, "status": "pass" if passed == len(PLANNED) and self.report["cleanup"]["isolated_postgres_database_dropped"] is not False else "fail"})
            path = m0.HERE / "results" / ("device-credentials-" + self.args.database + "-" + self.namespace + ".json")
            path.write_text(json.dumps(self.report, indent=2) + "\n")
            print(json.dumps({"report": str(path.relative_to(m0.ROOT)), "status": self.report["status"], "passed": passed, "planned": len(PLANNED)}), flush=True)
        return self.report["status"] == "pass"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", choices=["sqlite", "postgres", "both"], default="both")
    parser.add_argument("--binary", default=str(m0.ROOT / "api/target/debug/rustdesk-account-api"))
    args = parser.parse_args()
    success = True
    for database in (["sqlite", "postgres"] if args.database == "both" else [args.database]):
        success = Runner(args, database).run() and success
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
