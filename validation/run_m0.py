#!/usr/bin/env python3
"""Real loopback HTTP/Keycloak/SQL validation; no official client GUI claim.

Only stdlib. Secrets stay in .runtime (0600); reports contain case names/statuses,
never credentials, tokens, browser URLs, callback codes or raw server responses.
"""
import argparse
import datetime
import html.parser
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
RUNTIME = HERE / ".runtime"
BASE = "http://127.0.0.1:21114"
IDP = "http://127.0.0.1:18080"
COMPOSE = ["docker", "compose", "-p", "rustdesk-m0", "--env-file", str(RUNTIME / ".env"), "-f", str(HERE / "compose.yaml")]
PLANNED_CASES = [
    "setup.isolated-database-cli-users-api", "local.wrong-password", "local.correct-password-two-devices",
    "local.book-sync-isolation-metadata-invalid-snapshot", "local.logout-all-protected-routes-401",
    "oidc.options", "oidc.code-pkce-pendingexact-wrongbinding-invalidstate",
    "oidc.member-real-browser-jit-repeat-single-use", "oidc.book-sync-isolation-metadata-invalid-snapshot",
    "oidc.logout-all-protected-routes-401", "oidc.admin-wins-both-groups", "oidc.unmatched-default-deny",
    "oidc.configurable-member-fallback-policy-revocation", "oidc.bad-exchange-never-member-fallback",
    "oidc.restore-deny-removes-existing-admission",
]


def private_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
        json.dump(value, f, indent=2)
        f.write("\n")


def prepare():
    RUNTIME.mkdir(mode=0o700, exist_ok=True)
    if (RUNTIME / "fixture.json").exists():
        print(json.dumps({"prepare": "reused", "fixture": ".runtime/fixture.json"}))
        return
    suffix = secrets.token_hex(5)
    fixture = {"realm": "rustdesk-m0-" + suffix, "client_id": "company", "client_secret": secrets.token_urlsafe(32),
               "pg_password": secrets.token_urlsafe(32), "admin_username": "bootstrap-" + suffix,
               "admin_password": secrets.token_urlsafe(32),
               "users": {role: {"username": role + "-" + suffix, "password": secrets.token_urlsafe(24)}
                         for role in ["admin", "member", "denied"]}}
    realm = {"realm": fixture["realm"], "enabled": True, "registrationAllowed": False,
             "loginWithEmailAllowed": False, "sslRequired": "none", "groups": [
                 {"name": "rustdesk-admins"}, {"name": "rustdesk-users"}, {"name": "unmatched"}],
             "clients": [{"clientId": "company", "enabled": True, "protocol": "openid-connect",
                          "publicClient": False, "secret": fixture["client_secret"],
                          "standardFlowEnabled": True, "directAccessGrantsEnabled": False,
                          "redirectUris": [BASE + "/api/oidc/callback"],
                          "attributes": {"pkce.code.challenge.method": "S256"},
                          "protocolMappers": [{"name": "groups", "protocol": "openid-connect",
                              "protocolMapper": "oidc-group-membership-mapper", "consentRequired": False,
                              "config": {"full.path": "false", "id.token.claim": "true", "access.token.claim": "true",
                                         "userinfo.token.claim": "true", "claim.name": "groups"}}]}], "users": []}
    for role, user in fixture["users"].items():
        realm["users"].append({"username": user["username"], "enabled": True, "emailVerified": True,
                               "firstName": "M0", "lastName": role, "email": user["username"] + "@example.invalid",
                               "credentials": [{"type": "password", "value": user["password"], "temporary": False}],
                               "groups": {"admin": ["/rustdesk-admins", "/rustdesk-users"],
                                          "member": ["/rustdesk-users"], "denied": ["/unmatched"]}[role]})
    private_json(RUNTIME / "realm.json", realm)
    private_json(RUNTIME / "fixture.json", fixture)
    with os.fdopen(os.open(RUNTIME / ".env", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as f:
        f.write("KC_ADMIN_USERNAME=" + fixture["admin_username"] + "\nKC_ADMIN_PASSWORD=" + fixture["admin_password"]
                + "\nPG_PASSWORD=" + fixture["pg_password"] + "\n")
    print(json.dumps({"prepare": "created", "fixture": ".runtime/fixture.json", "secret_permissions": "0600"}))


class Response:
    def __init__(self, response):
        self.status = response.code
        self.headers = response.headers
        self.body = response.read()
        self.url = response.geturl()

    def json(self):
        return json.loads(self.body)


def request(url, data=None, token=None, opener=None, form=False):
    headers = {}
    if token:
        headers["Authorization"] = "Bearer " + token
    if data is not None:
        headers["Content-Type"] = "application/x-www-form-urlencoded" if form else "application/json"
        data = urllib.parse.urlencode(data).encode() if form else json.dumps(data).encode()
    req = urllib.request.Request(url, data=data, headers=headers)
    try:
        return Response((opener or urllib.request.build_opener()).open(req, timeout=20))
    except urllib.error.HTTPError as e:
        return Response(e)


class LoginForm(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.action = None
        self.fields = {}
        self.inside = False

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "form" and attrs.get("id") == "kc-form-login":
            self.action = attrs.get("action")
            self.inside = True
        if tag == "input" and self.inside and attrs.get("name") and attrs.get("type") == "hidden":
            self.fields[attrs["name"]] = attrs.get("value", "")

    def handle_endtag(self, tag):
        if tag == "form":
            self.inside = False


class LoopbackCookiePolicy(http.cookiejar.DefaultCookiePolicy):
    def return_ok_secure(self, cookie, request):
        # Keycloak 26 marks its loopback login cookies Secure even on this HTTP
        # development origin. Model the browser's local secure-context exception
        # only for the exact fixture endpoint; never send them to another host.
        if request.type == "http" and request.host == "127.0.0.1:18080":
            return True
        return super().return_ok_secure(cookie, request)


class CheckFailure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise CheckFailure(message)


class Runner:
    def __init__(self, args):
        self.args = args
        self.fixture = json.loads((RUNTIME / "fixture.json").read_text())
        self.namespace = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d%H%M%S") + "_" + secrets.token_hex(4)
        self.directory = RUNTIME / (args.database + "_" + self.namespace)
        self.directory.mkdir(mode=0o700)
        self.report = {"database": args.database, "namespace": self.namespace,
                       "scope": "real Keycloak authorization-code + PKCE via HTTP forms/cookies; no official client GUI",
                       "cases": [], "status": "running"}
        self.proc = None
        self.log = None
        self.env = os.environ.copy()
        self.binary = str(Path(args.binary).resolve())

    def check(self, name, func):
        start = time.monotonic()
        try:
            func()
            result = {"case": name, "status": "pass"}
        except CheckFailure as e:
            result = {"case": name, "status": "fail", "detail": str(e)}
        except Exception as e:
            # urllib errors/tracebacks can expose callback codes or credentials.
            result = {"case": name, "status": "error", "detail": type(e).__name__}
        result["seconds"] = round(time.monotonic() - start, 3)
        self.report["cases"].append(result)
        print(json.dumps(result), flush=True)
        return result["status"] == "pass"

    def cli(self, *args, extra=None):
        env = self.env.copy()
        env.update(extra or {})
        result = subprocess.run([self.binary, *args], cwd=self.directory, env=env, capture_output=True, timeout=45)
        require(result.returncode == 0, "API CLI returned nonzero (raw output withheld)")

    def setup(self):
        require(Path(self.binary).is_file(), "API binary absent; build api first")
        self.report["api_binary_sha256"] = hashlib.sha256(Path(self.binary).read_bytes()).hexdigest()
        with socket.socket() as sock:
            require(sock.connect_ex(("127.0.0.1", 21114)) != 0, "API port already occupied; no existing process touched")
        if self.args.database == "sqlite":
            db = "sqlite://" + str(self.directory / "accounts.sqlite") + "?mode=rwc"
        else:
            name = "m0_" + self.namespace
            result = subprocess.run(COMPOSE + ["exec", "-T", "postgres", "psql", "-U", "rustdesk_m0", "-d", "postgres", "-v", "ON_ERROR_STOP=1"],
                                    input=("CREATE DATABASE " + name + ";\n").encode(), capture_output=True, timeout=30)
            require(result.returncode == 0, "Dedicated PostgreSQL database creation failed")
            db = "postgres://rustdesk_m0:" + self.fixture["pg_password"] + "@127.0.0.1:15432/" + name
            version = subprocess.run(COMPOSE + ["exec", "-T", "postgres", "psql", "-U", "rustdesk_m0", "-d", "postgres", "-Atc", "SHOW server_version"],
                                     capture_output=True, timeout=10)
            require(version.returncode == 0, "PostgreSQL version query failed")
            self.report["postgres_server_version"] = version.stdout.decode().strip()
        self.env.update({"RD_DATABASE_URL": db, "RD_LISTEN": "127.0.0.1:21114",
                         "RD_WEB_DIR": str(ROOT / "web" / "dist"),
                         "RD_OIDC_ISSUER": IDP + "/realms/" + self.fixture["realm"],
                         "RD_OIDC_CLIENT_ID": self.fixture["client_id"], "RD_OIDC_CLIENT_SECRET": self.fixture["client_secret"],
                         "RD_OIDC_REDIRECT_URI": BASE + "/api/oidc/callback", "RD_OIDC_CLAIM_PATH": "groups",
                         "RD_OIDC_ADMIN_VALUES": "rustdesk-admins", "RD_OIDC_MEMBER_VALUES": "rustdesk-users",
                         "RD_OIDC_UNMATCHED": "deny", "RD_ALLOW_HTTP_LOOPBACK": "true"})
        self.locals = [{"username": "local_" + role + "_" + self.namespace, "password": secrets.token_urlsafe(24)}
                       for role in ["member", "other"]]
        private_json(self.directory / "local-fixture.json", self.locals)
        for user in self.locals:
            self.cli("create-local", user["username"], extra={"RD_BOOTSTRAP_PASSWORD": user["password"]})
        self.start()

    def start(self, fallback="deny"):
        self.env["RD_OIDC_UNMATCHED"] = fallback
        self.log = os.fdopen(os.open(self.directory / ("api-" + fallback + ".log"), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600), "ab")
        self.proc = subprocess.Popen([self.binary, "serve"], cwd=self.directory, env=self.env, stdout=self.log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                raise CheckFailure("API exited during startup (raw log withheld)")
            try:
                if request(BASE + "/healthz").status == 200:
                    return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(.25)
        raise CheckFailure("API health timeout")

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.send_signal(signal.SIGINT)
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        self.proc = None
        if self.log:
            self.log.close()
            self.log = None

    def local_login(self, user, password=None):
        return request(BASE + "/api/login", {"username": user["username"],
                       "password": user["password"] if password is None else password, "type": "account"})

    def login_shape(self, response):
        require(response.status == 200, "login status must be 200")
        require(response.headers.get("Cache-Control") == "no-store", "session response must be no-store")
        value = response.json()
        require(value.get("type") == "access_token" and bool(value.get("access_token")), "direct session shape missing")
        require(isinstance(value.get("user", {}).get("info"), dict), "user.info object missing")
        require(bool(value["user"].get("id")) and bool(value["user"].get("name")), "stable id/name missing")
        return value

    def auth_start(self):
        binding = {"id": "m0-" + secrets.token_hex(6), "uuid": secrets.token_hex(16)}
        response = request(BASE + "/api/oidc/auth", dict(binding, op="company", deviceInfo={"os": "HTTP-validation"}, apiDomain="https://untrusted.invalid"))
        require(response.status == 200, "OIDC auth start must return 200")
        require(response.headers.get("Cache-Control") == "no-store", "OIDC auth must be no-store")
        value = response.json()
        require(bool(value.get("code")) and bool(value.get("url")), "OIDC start shape missing")
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(value["url"]).query)
        require(query.get("response_type") == ["code"], "authorization code response_type missing")
        require(query.get("code_challenge_method") == ["S256"] and bool(query.get("code_challenge")), "PKCE S256 missing")
        require("openid" in query.get("scope", [""])[0].split(), "openid scope missing")
        require(bool(query.get("nonce")) and bool(query.get("state")), "nonce/state missing")
        require(query.get("redirect_uri") == [BASE + "/api/oidc/callback"], "untrusted apiDomain changed callback")
        return dict(binding, **value)

    def poll(self, attempt, **override):
        data = {k: attempt[k] for k in ["id", "uuid", "code"]}
        data.update(override)
        return request(BASE + "/api/oidc/auth-query?" + urllib.parse.urlencode(data))

    def browser(self, attempt, role):
        jar = http.cookiejar.CookieJar(policy=LoopbackCookiePolicy())
        opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
        response = request(attempt["url"], opener=opener)
        require(response.status == 200, "Keycloak login page unavailable")
        form = LoginForm()
        form.feed(response.body.decode())
        require(bool(form.action), "Keycloak password login form absent")
        action = urllib.parse.urljoin(response.url, form.action)
        require(urllib.parse.urlsplit(action).netloc == "127.0.0.1:18080", "unexpected form target")
        user = self.fixture["users"][role]
        response = request(action, dict(form.fields, username=user["username"], password=user["password"], credentialId=""), opener=opener, form=True)
        require(response.url.startswith(BASE + "/api/oidc/callback?"), "browser did not reach fixed callback")
        require(response.headers.get("Cache-Control") == "no-store", "callback must be no-store")
        require(b"access_token" not in response.body, "callback exposed bearer")
        return response

    def oidc_login(self, role, denied=False):
        attempt = self.auth_start()
        response = self.browser(attempt, role)
        require(response.status == (400 if denied else 200), "unexpected callback outcome for mapped role")
        result = self.poll(attempt)
        if denied:
            require(result.status == 400 and "access_token" not in result.json(), "unmatched identity received session")
            return None
        value = self.login_shape(result)
        require(self.poll(attempt).status == 400, "poll credential replay accepted")
        require(request(response.url).status == 400, "callback replay accepted")
        return value

    def book(self, token):
        response = request(BASE + "/api/ab", token=token)
        require(response.status == 200, "address book read failed")
        outer = response.json()
        require(isinstance(outer.get("data"), str) and "licensed_devices" in outer, "legacy book envelope missing")
        return json.loads(outer["data"])

    def sync_book(self, first, second, isolated):
        snapshot = {"tags": ["m0"], "peers": [{"id": "123456789", "alias": self.namespace,
                    "platform": "Mac OS", "tags": ["m0"], "hash": "nonempty-sensitive-fixture", "password": "sensitive-fixture"}],
                    "hash": "top-secret-fixture", "tag_colors": '{"m0":4294901760}'}
        before = self.book(isolated)
        response = request(BASE + "/api/ab", {"data": json.dumps(snapshot)}, token=first)
        require(response.status == 200, "snapshot write failed")
        require(response.headers.get("X-Rustdesk-Credential-Sync") == "disabled; metadata-only", "credential exclusion header missing")
        actual = self.book(second)
        expected = {"tags": snapshot["tags"], "peers": [{k: v for k, v in snapshot["peers"][0].items() if k not in ["hash", "password"]}], "tag_colors": snapshot["tag_colors"]}
        require(actual == expected, "metadata roundtrip differs or sensitive fields persisted")
        require(self.book(isolated) == before, "another user's book changed")
        expected["peers"][0]["alias"] += "-second-device"
        require(request(BASE + "/api/ab", {"data": json.dumps(expected)}, token=second).status == 200, "second-device write failed")
        require(self.book(first) == expected, "second-device write invisible to first")
        require(request(BASE + "/api/ab", {"data": '{"tags":[],"peers":[{"alias":"invalid"}]}'}, token=first).status == 400,
                "invalid snapshot must return 400")
        require(self.book(first) == expected, "invalid snapshot overwrote existing book")

    def revoke(self, token):
        require(request(BASE + "/api/logout", {}, token=token).status == 200, "logout failed")
        for path, data in [("/api/currentUser", {}), ("/api/ab", None), ("/api/ab", {"data": '{"tags":[],"peers":[]}'}),
                           ("/api/ab/personal", {}), ("/api/device-group/accessible", None), ("/api/users", None),
                           ("/api/peers", None), ("/api/logout", {})]:
            require(request(BASE + path, data, token=token).status == 401, "revoked session accepted on " + path)

    def local_cases(self):
        def wrong():
            require(self.local_login(self.locals[0], "wrong-password").status == 401, "wrong local password accepted")
        self.check("local.wrong-password", wrong)
        tokens = []
        def correct():
            tokens.extend(self.login_shape(self.local_login(self.locals[0]))["access_token"] for _ in range(2))
            tokens.append(self.login_shape(self.local_login(self.locals[1]))["access_token"])
            require(tokens[0] != tokens[1], "separate logins reused bearer")
            response = request(BASE + "/api/currentUser", {}, token=tokens[0])
            require(response.status == 200, "currentUser rejected valid local token")
            require(request(BASE + "/api/ab/personal", {}, token=tokens[0]).status == 404, "personal probe must trigger legacy fallback")
        if self.check("local.correct-password-two-devices", correct):
            self.check("local.book-sync-isolation-metadata-invalid-snapshot", lambda: self.sync_book(*tokens))
            self.check("local.logout-all-protected-routes-401", lambda: self.revoke(tokens[0]))
            self.local_isolated = tokens[2]
        else:
            raise CheckFailure("local login prerequisite failed")

    def oidc_cases(self):
        def options():
            response = request(BASE + "/api/login-options")
            require(response.status == 200 and response.json() == ["oidc/company"], "OIDC option discovery differs")
        self.check("oidc.options", options)
        def pending():
            attempt = self.auth_start()
            response = self.poll(attempt)
            require(response.status == 200 and response.json() == {"error": "No authed oidc is found"}, "pending response differs")
            require(self.poll(attempt, uuid="wrong-binding").status == 400, "wrong UUID binding accepted")
            require(self.poll(attempt, id="wrong-binding").status == 400, "wrong ID binding accepted")
            require(request(BASE + "/api/oidc/callback?state=invalid&code=invalid").status == 400, "invalid state accepted")
            require(self.poll(attempt).json() == {"error": "No authed oidc is found"}, "bad binding/state consumed original attempt")
        self.check("oidc.code-pkce-pendingexact-wrongbinding-invalidstate", pending)
        members = []
        def member():
            members.extend([self.oidc_login("member"), self.oidc_login("member")])
            require(members[0]["user"]["id"] == members[1]["user"]["id"], "JIT identity unstable across login")
            require(not members[0]["user"]["is_admin"], "member mapped as admin")
        if self.check("oidc.member-real-browser-jit-repeat-single-use", member):
            self.check("oidc.book-sync-isolation-metadata-invalid-snapshot", lambda: self.sync_book(members[0]["access_token"], members[1]["access_token"], self.local_isolated))
            self.check("oidc.logout-all-protected-routes-401", lambda: self.revoke(members[0]["access_token"]))
        def admin():
            value = self.oidc_login("admin")
            require(value["user"]["is_admin"] is True, "admin did not win over member group")
        self.check("oidc.admin-wins-both-groups", admin)
        self.check("oidc.unmatched-default-deny", lambda: self.oidc_login("denied", denied=True))
        def fallback():
            self.stop()
            self.start("member")
            value = self.oidc_login("denied")
            require(value["user"]["is_admin"] is False, "fallback must map member")
            if members:
                require(request(BASE + "/api/currentUser", {}, token=members[-1]["access_token"]).status == 401,
                        "policy change did not revoke prior OIDC token")
            self.fallback_token = value["access_token"]
        if self.check("oidc.configurable-member-fallback-policy-revocation", fallback):
            def bad_exchange():
                attempt = self.auth_start()
                state = urllib.parse.parse_qs(urllib.parse.urlsplit(attempt["url"]).query)["state"][0]
                callback = BASE + "/api/oidc/callback?" + urllib.parse.urlencode({"state": state, "code": "invalid-fixture-code"})
                require(request(callback).status == 400, "invalid authorization code accepted under member fallback")
                require(self.poll(attempt).status == 400, "protocol failure received fallback session")
                require(request(callback).status == 400, "failed callback replay accepted")
            self.check("oidc.bad-exchange-never-member-fallback", bad_exchange)
            def deny_again():
                self.stop()
                self.start("deny")
                require(request(BASE + "/api/currentUser", {}, token=self.fallback_token).status == 401,
                        "restoring deny retained fallback session")
                self.oidc_login("denied", denied=True)
            self.check("oidc.restore-deny-removes-existing-admission", deny_again)

    def run(self):
        try:
            if self.check("setup.isolated-database-cli-users-api", self.setup):
                self.local_cases()
                self.oidc_cases()
        except CheckFailure as e:
            self.report["cases"].append({"case": "suite.prerequisite", "status": "fail", "detail": str(e)})
        except Exception as e:
            self.report["cases"].append({"case": "suite.unexpected", "status": "error", "detail": type(e).__name__})
        finally:
            self.stop()
            seen = {c["case"] for c in self.report["cases"]}
            self.report["cases"].extend({"case": name, "status": "not_run", "detail": "prerequisite did not complete"}
                                        for name in PLANNED_CASES if name not in seen)
            self.report["status"] = "pass" if self.report["cases"] and all(c["status"] == "pass" for c in self.report["cases"]) else "fail"
            self.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            private_json(self.directory / "report.json", self.report)
            print(json.dumps({"status": self.report["status"], "report": str((self.directory / "report.json").relative_to(ROOT)),
                              "cases": len(self.report["cases"])}), flush=True)
        return 0 if self.report["status"] == "pass" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("prepare")
    run = sub.add_parser("run")
    run.add_argument("--database", choices=["sqlite", "postgres"], required=True)
    run.add_argument("--binary", default=str(ROOT / "api/target/debug/rustdesk-account-api"))
    args = parser.parse_args()
    if args.command == "prepare":
        prepare()
        return 0
    if not (RUNTIME / "fixture.json").is_file():
        parser.error("run prepare first")
    return Runner(args).run()


if __name__ == "__main__":
    sys.exit(main())
