#!/usr/bin/env python3
"""M1 real HTTP validation against the existing M0 Keycloak/PostgreSQL fixture.

Run M0 separately for the original RustDesk protocol regression. This script only
starts/stops its own loopback API child and creates isolated SQL fixtures. It does
not start containers. Reports never contain passwords, tokens, authorization URLs,
callback codes, raw HTTP responses or raw exceptions.
"""
import argparse
import datetime
import http.cookiejar as cookiejar
import http.cookies as cookies
import json
import secrets
import sys
import urllib.error
import urllib.parse
import urllib.request

try:
    from . import run_m0 as m0
except ImportError:
    import run_m0 as m0

BASE = m0.BASE
PREFIX = "/admin/api/v1"
ORIGIN = BASE
WEB_TTL = 28800
PLANNED_CASES = [
    "setup.isolated-database-cli-users-api",
    "web.options-local-login-cookie-me",
    "web.login-origin-json-wrong-password",
    "web.client-session-purpose-isolation",
    "users.create-list-conflict-input-boundaries",
    "users.member-admin-routes-forbidden",
    "web.all-mutations-origin-csrf",
    "users.local-role-promotion-demotion",
    "users.local-disable-enable-revokes-all-kinds",
    "users.local-admin-password-reset",
    "users.local-self-password-change",
    "sessions.shape-scope-selected-and-admin-revocation",
    "sessions.self-all-revocation",
    "oidc.web-real-browser-pkce-cookie-callback-replay",
    "oidc.web-missing-wrong-browser-binding",
    "oidc.web-client-attempt-purpose-isolation",
    "oidc.provider-managed-role-password-conflicts",
    "oidc.disable-enable-requires-fresh-admission",
    "users.missing-targets",
    "web.logout-cookie-revocation",
]
require = m0.require


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def http(method, path, *, data=None, headers=None, opener=None, form=False,
         content_type="application/json"):
    """Do not infer methods or follow redirects; keep secrets out of exceptions."""
    url = path if path.startswith("http://") or path.startswith("https://") else BASE + path
    request_headers = dict(headers or {})
    if data is not None:
        request_headers["Content-Type"] = "application/x-www-form-urlencoded" if form else content_type
        data = urllib.parse.urlencode(data).encode() if form else json.dumps(data).encode()
    req = urllib.request.Request(url, data=data, headers=request_headers, method=method)
    try:
        return m0.Response((opener or urllib.request.build_opener(NoRedirect())).open(req, timeout=20))
    except urllib.error.HTTPError as error:
        return m0.Response(error)


def cookie_value(response, name, *, path, max_age):
    matches = []
    for line in response.headers.get_all("Set-Cookie", []):
        parsed = cookies.SimpleCookie()
        parsed.load(line)
        if name in parsed:
            matches.append(parsed[name])
    require(len(matches) == 1, "expected exactly one authentication cookie")
    cookie = matches[0]
    require(bool(cookie["httponly"]), "authentication cookie must be HttpOnly")
    require(cookie["samesite"].lower() == "lax", "authentication cookie must use SameSite=Lax")
    require(cookie["path"] == path, "authentication cookie path differs")
    require(cookie["max-age"] == str(max_age), "authentication cookie lifetime differs")
    require(not cookie["domain"], "authentication cookie must be host-only")
    return cookie.value


class Browser:
    def __init__(self):
        self.jar = cookiejar.CookieJar(policy=m0.LoopbackCookiePolicy())
        self.opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(self.jar))

    def call(self, method, path, **kwargs):
        return http(method, path, opener=self.opener, **kwargs)

    def token(self):
        tokens = [cookie.value for cookie in self.jar if cookie.name == "rd_web_session"]
        require(len(tokens) == 1, "browser Web session cookie missing")
        return tokens[0]


class WebSession:
    def __init__(self, browser, user, csrf):
        self.browser = browser
        self.user = user
        self.csrf = csrf

    def call(self, method, path, data=None, *, origin=ORIGIN, csrf=True):
        headers = {}
        if method not in ["GET", "HEAD"]:
            if origin is not None:
                headers["Origin"] = origin
            if csrf:
                headers["X-CSRF-Token"] = self.csrf if csrf is True else csrf
        return self.browser.call(method, PREFIX + path, data=data, headers=headers)


class Runner(m0.Runner):
    def __init__(self, args):
        super().__init__(args)
        self.report.update({"milestone": "M1", "scope": "real loopback HTTP Web management and Keycloak forms/cookies; run M0 independently",
                            "planned_cases": len(PLANNED_CASES), "web_session_ttl_seconds": WEB_TTL})

    def setup(self):
        self.report["test_login_limit_override"] = {"per_account": 100, "reason": "functional suite reuses fixture identities; limiter is verified separately by operations tests"}
        self.env.update({"RD_WEB_ORIGIN": ORIGIN, "RD_WEB_SESSION_TTL_SECONDS": str(WEB_TTL),
                         "RD_LOGIN_PER_ACCOUNT": "100",
                         "RD_WEB_DIR": str(m0.ROOT / "web/dist")})
        super().setup()
        # Reuse the existing Runner fixtures regardless of their current CLI default role.
        self.admin_account = None
        for account in self.locals:
            session = self.web_login(account)
            account["id"] = session.user["id"]
            account["role"] = session.user["role"]
            if session.user["role"] == "admin" and self.admin_account is None:
                self.admin_account = account
                self.admin = session
            else:
                require(session.call("POST", "/auth/logout", {}).status == 204, "fixture probe logout failed")
        if self.admin_account is None:
            self.admin_account = {"username": "local_admin_" + self.namespace, "password": secrets.token_urlsafe(24)}
            self.cli("create-local", self.admin_account["username"], "--role", "admin",
                     extra={"RD_BOOTSTRAP_PASSWORD": self.admin_account["password"]})
            self.admin = self.web_login(self.admin_account)
            self.admin_account["id"] = self.admin.user["id"]
        require(self.admin.user["role"] == "admin", "CLI local admin role missing")
        # Password reset/change and revocation flows use separate accounts to avoid
        # exhausting the per-user login limiter during a fast suite run.
        self.accounts = {}
        for purpose in ["managed", "sessions", "selfchange"]:
            account = {"username": "m1_" + purpose + "_" + self.namespace, "password": secrets.token_urlsafe(24)}
            self.cli("create-local", account["username"], "--role", "member",
                     extra={"RD_BOOTSTRAP_PASSWORD": account["password"]})
            self.accounts[purpose] = account
        m0.private_json(self.directory / "m1-local-fixture.json", {"admin": self.admin_account, **self.accounts})

    def shape(self, response, browser):
        require(response.status == 200, "Web login/me must return 200")
        require(response.headers.get("Cache-Control") == "no-store", "Web authentication must be no-store")
        value = response.json()
        require(set(value) == {"user", "csrf_token"}, "Web authentication response shape differs")
        user = value["user"]
        require(isinstance(user, dict) and bool(user.get("id")) and bool(user.get("username")), "Web user identity missing")
        require(user.get("role") in ["admin", "member"] and user.get("source") in ["local", "oidc"], "Web user role/source missing")
        require(isinstance(value["csrf_token"], str) and bool(value["csrf_token"]), "CSRF token missing")
        require(all(key not in user for key in ["password", "password_hash", "token", "digest"]), "Web user exposed a secret field")
        require(b"access_token" not in response.body, "Web response exposed client bearer")
        return WebSession(browser, user, value["csrf_token"])

    def web_login(self, account, password=None):
        browser = Browser()
        response = browser.call("POST", PREFIX + "/auth/login",
                                data={"username": account["username"], "password": account["password"] if password is None else password},
                                headers={"Origin": ORIGIN})
        cookie_value(response, "rd_web_session", path=PREFIX, max_age=WEB_TTL)
        return self.shape(response, browser)

    def client_login(self, account):
        return self.login_shape(self.local_login(account))["access_token"]

    def client_valid(self, token, expected=200):
        require(m0.request(BASE + "/api/currentUser", {}, token=token).status == expected,
                "client authentication status differs")

    def web_revoked(self, session):
        require(session.call("GET", "/me").status == 401, "revoked Web session remains usable")

    def listed_users(self):
        response = self.admin.call("GET", "/users")
        require(response.status == 200, "admin user list failed")
        users = response.json().get("users")
        require(isinstance(users, list), "user list envelope missing")
        require(all(all(key not in user for key in ["password", "password_hash", "token", "digest"]) for user in users),
                "user list exposed secret fields")
        return users

    def sessions(self, session, user_id=None):
        path = "/sessions" if user_id is None else "/users/" + user_id + "/sessions"
        response = session.call("GET", path)
        require(response.status == 200, "session list failed")
        values = response.json().get("sessions")
        require(isinstance(values, list), "session list envelope missing")
        fields = {"id", "user_id", "kind", "created_at", "expires_at", "revoked", "current"}
        for item in values:
            require(set(item) == fields, "session metadata differs or secret field exposed")
            require(item["kind"] in ["web", "client"], "unknown session kind")
            require(isinstance(item["revoked"], bool) and isinstance(item["current"], bool), "session flags must be boolean")
            require(item["user_id"] == (user_id or session.user["id"]), "another user's session leaked")
        return values

    def local_cases(self):
        def options_login():
            response = http("GET", PREFIX + "/auth/options")
            require(response.status == 200 and response.json() == {"oidc_enabled": True}, "Web auth options differ")
            self.member = self.web_login(self.locals[0])
            me = self.shape(self.member.call("GET", "/me"), self.member.browser)
            require(me.user == self.member.user and me.csrf == self.member.csrf, "me identity/CSRF changed")
            require(self.member.browser.token() != self.member.csrf, "CSRF token equals session secret")
        require(self.check("web.options-local-login-cookie-me", options_login), "Web authentication prerequisite failed")

        def login_guards():
            body = {"username": self.locals[0]["username"], "password": self.locals[0]["password"]}
            for origin in [None, "https://attacker.example"]:
                headers = {} if origin is None else {"Origin": origin}
                require(http("POST", PREFIX + "/auth/login", data=body, headers=headers).status == 403, "login accepted missing/wrong Origin")
                require(http("POST", PREFIX + "/auth/oidc", data={}, headers=headers).status == 403, "OIDC start accepted missing/wrong Origin")
            require(http("POST", PREFIX + "/auth/login", data=body, headers={"Origin": ORIGIN}, content_type="text/plain").status == 415,
                    "login accepted a non-JSON request")
            body["password"] = "incorrect-password-" + secrets.token_hex(8)
            require(http("POST", PREFIX + "/auth/login", data=body, headers={"Origin": ORIGIN}).status == 401, "wrong Web password accepted")
            require(http("GET", PREFIX + "/me").status == 401, "anonymous Web me accepted")
        self.check("web.login-origin-json-wrong-password", login_guards)

        def purposes():
            client = self.client_login(self.locals[0])
            self.client_valid(client)
            web = self.member.browser.token()
            require(http("GET", PREFIX + "/me", headers={"Authorization": "Bearer " + web}).status == 401, "Web bearer accepted without cookie")
            require(http("GET", PREFIX + "/me", headers={"Cookie": "rd_web_session=" + client}).status == 401, "client token accepted as Web cookie")
            self.client_valid(web, 401)
            require(http("GET", "/api/ab", headers={"Cookie": "rd_web_session=" + web}).status == 401, "Web cookie accepted by client route")
        self.check("web.client-session-purpose-isolation", purposes)

        def create():
            self.created = {"username": "m1_created_" + self.namespace, "password": secrets.token_urlsafe(24)}
            body = dict(self.created, role="member")
            response = self.admin.call("POST", "/users", body)
            require(response.status == 201, "local account creation must return 201")
            self.created["id"] = response.json()["user"]["id"]
            require(self.admin.call("POST", "/users", body).status == 409, "duplicate local account accepted")
            listed = next((user for user in self.listed_users() if user["id"] == self.created["id"]), None)
            require(listed is not None and listed["source"] == "local" and listed["admitted"] is True and listed["disabled"] is False,
                    "created local user metadata differs")
            for username, password, role in [
                (" leading", self.created["password"], "member"), ("trailing ", self.created["password"], "member"),
                ("", self.created["password"], "member"), ("u" * 129, self.created["password"], "member"),
                ("short_" + self.namespace, "a" * 11, "member"),
                ("long_" + self.namespace, "é" * 2049, "member"),
                ("role_" + self.namespace, self.created["password"], "owner"),
            ]:
                require(self.admin.call("POST", "/users", {"username": username, "password": password, "role": role}).status == 400,
                        "invalid local account input accepted")
            for username, password in [("b" * 128, "a" * 12), ("max_" + self.namespace, "é" * 2048)]:
                require(self.admin.call("POST", "/users", {"username": username, "password": password, "role": "member"}).status == 201,
                        "valid username/password byte boundary rejected")
            self.managed = self.web_login(self.accounts["managed"])
            self.accounts["managed"]["id"] = self.managed.user["id"]
        require(self.check("users.create-list-conflict-input-boundaries", create), "management fixture prerequisite failed")
        target = "/users/" + self.managed.user["id"]

        def member_forbidden():
            for method, path, data in [
                ("GET", "/users", None), ("POST", "/users", {"username": self.created["username"], "password": self.created["password"], "role": "member"}),
                ("PATCH", target, {"disabled": True}), ("POST", target + "/password", {"new_password": secrets.token_urlsafe(24)}),
                ("GET", target + "/sessions", None), ("DELETE", target + "/sessions", None),
            ]:
                require(self.managed.call(method, path, data).status == 403, "member accepted by administrator route")
        self.check("users.member-admin-routes-forbidden", member_forbidden)

        def csrf():
            current = next(item["id"] for item in self.sessions(self.admin) if item["current"])
            for method, path, data in [
                ("POST", "/users", {"username": "csrf_" + self.namespace, "password": secrets.token_urlsafe(24), "role": "member"}),
                ("PATCH", target, {"disabled": True}), ("POST", target + "/password", {"new_password": secrets.token_urlsafe(24)}),
                ("POST", "/me/password", {"current_password": self.admin_account["password"], "new_password": secrets.token_urlsafe(24)}),
                ("DELETE", "/sessions", None), ("DELETE", "/sessions/" + current, None),
                ("DELETE", target + "/sessions", None), ("POST", "/auth/logout", {}),
            ]:
                for origin, csrf_value in [(ORIGIN, False), (ORIGIN, "invalid-csrf"), (None, True), ("https://attacker.example", True)]:
                    require(self.admin.call(method, path, data, origin=origin, csrf=csrf_value).status == 403,
                            "mutation accepted missing/wrong Origin or CSRF")
            require(self.admin.call("GET", "/me").status == 200 and self.managed.call("GET", "/me").status == 200,
                    "rejected CSRF mutation changed account/session state")
        self.check("web.all-mutations-origin-csrf", csrf)

        def roles():
            require(self.admin.call("PATCH", target, {"role": "admin"}).status == 200, "local promotion failed")
            require(self.managed.call("GET", "/users").status == 200, "promoted user lacks admin access")
            require(self.admin.call("PATCH", target, {"role": "member"}).status == 200, "local demotion failed")
            self.web_revoked(self.managed)
            self.managed = self.web_login(self.accounts["managed"])
            require(self.managed.user["role"] == "member", "demoted role did not persist")
        self.check("users.local-role-promotion-demotion", roles)

        def disable():
            self.managed = self.web_login(self.accounts["managed"])
            client = self.client_login(self.accounts["managed"])
            require(self.admin.call("PATCH", target, {"disabled": True}).status == 200, "local disable failed")
            self.web_revoked(self.managed)
            self.client_valid(client, 401)
            require(self.local_login(self.accounts["managed"]).status == 401, "disabled local client login accepted")
            require(http("POST", PREFIX + "/auth/login", data={k: self.accounts["managed"][k] for k in ["username", "password"]},
                         headers={"Origin": ORIGIN}).status == 401, "disabled local Web login accepted")
            require(self.admin.call("PATCH", target, {"disabled": False}).status == 200, "local enable failed")
            self.web_revoked(self.managed)
            self.managed = self.web_login(self.accounts["managed"])
            self.client_valid(self.client_login(self.accounts["managed"]))
        self.check("users.local-disable-enable-revokes-all-kinds", disable)

        def reset():
            old = dict(self.accounts["managed"])
            client = self.client_login(old)
            replacement = secrets.token_urlsafe(24)
            require(self.admin.call("POST", target + "/password", {"new_password": replacement}).status == 204, "admin password reset failed")
            self.web_revoked(self.managed)
            self.client_valid(client, 401)
            require(self.local_login(old).status == 401, "reset retained old client password")
            self.accounts["managed"]["password"] = replacement
            self.managed = self.web_login(self.accounts["managed"])
            self.client_valid(self.client_login(self.accounts["managed"]))
        self.check("users.local-admin-password-reset", reset)

        def change():
            account = self.accounts["selfchange"]
            session = self.web_login(account)
            client = self.client_login(account)
            replacement = secrets.token_urlsafe(24)
            require(session.call("POST", "/me/password", {"current_password": "wrong-password-123", "new_password": replacement}).status == 401,
                    "self password change accepted wrong current password")
            require(session.call("GET", "/me").status == 200, "wrong-password attempt revoked valid session")
            require(session.call("POST", "/me/password", {"current_password": account["password"], "new_password": replacement}).status == 204,
                    "self password change failed")
            self.web_revoked(session)
            self.client_valid(client, 401)
            require(self.local_login(account).status == 401, "self change retained old password")
            account["password"] = replacement
            self.web_login(account)
            self.client_valid(self.client_login(account))
        self.check("users.local-self-password-change", change)

    def session_cases(self):
        def selected():
            account = self.accounts["sessions"]
            session = self.web_login(account)
            other = self.web_login(account)
            client = self.client_login(account)
            values = self.sessions(session)
            require(sum(item["current"] for item in values) == 1, "current session flag differs")
            require(any(item["kind"] == "web" and item["current"] for item in values), "current Web session missing")
            clients = [item for item in values if item["kind"] == "client" and not item["revoked"]]
            require(len(clients) == 1, "active client metadata missing")
            active = next(item for item in values if item["current"])
            require(active["expires_at"] - active["created_at"] == WEB_TTL, "Web session SQL lifetime differs")
            admin_id = next(item["id"] for item in self.sessions(self.admin) if item["current"])
            require(session.call("DELETE", "/sessions/" + admin_id).status == 403, "member revoked another user's session")
            require(session.call("DELETE", "/sessions/" + clients[0]["id"]).status == 204, "selected own session revoke failed")
            self.client_valid(client, 401)
            require(session.call("GET", "/me").status == 200, "selected revoke affected another session")
            other_id = next(item["id"] for item in self.sessions(other) if item["current"])
            require(self.admin.call("DELETE", "/sessions/" + other_id).status == 204, "admin selected session revoke failed")
            self.web_revoked(other)
            managed = self.sessions(self.admin, session.user["id"])
            require(any(item["revoked"] for item in managed), "revoked metadata not retained")
            require(self.admin.call("DELETE", "/users/" + session.user["id"] + "/sessions").status == 204,
                    "administrator target session revoke failed")
            self.web_revoked(session)
        self.check("sessions.shape-scope-selected-and-admin-revocation", selected)

        def all_own():
            account = self.accounts["sessions"]
            first, second = self.web_login(account), self.web_login(account)
            client = self.client_login(account)
            require(first.call("DELETE", "/sessions").status == 204, "self all-session revoke failed")
            self.web_revoked(first)
            self.web_revoked(second)
            self.client_valid(client, 401)
            require(self.admin.call("GET", "/me").status == 200, "self all-session revoke affected another user")
        self.check("sessions.self-all-revocation", all_own)

    def web_oidc_start(self):
        browser = Browser()
        response = browser.call("POST", PREFIX + "/auth/oidc", data={}, headers={"Origin": ORIGIN})
        require(response.status == 200, "Web OIDC start failed")
        require(response.headers.get("Cache-Control") == "no-store", "Web OIDC start must be no-store")
        binding = cookie_value(response, "rd_web_oidc", path="/api/oidc/callback", max_age=180)
        value = response.json()
        require(set(value) == {"url"} and isinstance(value["url"], str), "Web OIDC must expose only authorization URL")
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(value["url"]).query)
        require(query.get("response_type") == ["code"] and query.get("code_challenge_method") == ["S256"], "Web OIDC code/PKCE missing")
        require(bool(query.get("state")) and bool(query.get("nonce")) and bool(query.get("code_challenge")), "Web OIDC state/nonce/challenge missing")
        require("openid" in query.get("scope", [""])[0].split(), "Web OIDC openid scope missing")
        require(query.get("redirect_uri") == [BASE + "/api/oidc/callback"], "Web OIDC callback differs")
        require(urllib.parse.urlsplit(value["url"]).netloc == "127.0.0.1:18080", "Web OIDC authorization target differs")
        return {"browser": browser, "url": value["url"], "state": query["state"][0], "binding": binding}

    def browser_callback_url(self, attempt, role="member"):
        """Authenticate with real Keycloak forms, pausing before API callback."""
        browser = attempt["browser"]
        response = browser.call("GET", attempt["url"])
        for _ in range(12):
            if response.status not in [301, 302, 303, 307, 308]:
                break
            target = urllib.parse.urljoin(response.url, response.headers.get("Location", ""))
            require(urllib.parse.urlsplit(target).netloc == "127.0.0.1:18080", "unexpected pre-login redirect target")
            response = browser.call("GET", target)
        require(response.status == 200, "Keycloak login page unavailable")
        form = m0.LoginForm()
        form.feed(response.body.decode())
        require(bool(form.action), "Keycloak login form missing")
        action = urllib.parse.urljoin(response.url, form.action)
        require(urllib.parse.urlsplit(action).netloc == "127.0.0.1:18080", "unexpected Keycloak form target")
        account = self.fixture["users"][role]
        response = browser.call("POST", action, data=dict(form.fields, username=account["username"],
                                password=account["password"], credentialId=""), form=True)
        for _ in range(12):
            require(response.status in [301, 302, 303, 307, 308], "Keycloak login did not redirect")
            location = response.headers.get("Location")
            require(bool(location), "Keycloak redirect target missing")
            target = urllib.parse.urljoin(response.url, location)
            parsed = urllib.parse.urlsplit(target)
            if parsed.netloc == "127.0.0.1:21114":
                require(parsed.path == "/api/oidc/callback", "Keycloak redirected to another API route")
                return target
            require(parsed.netloc == "127.0.0.1:18080", "unexpected post-login redirect target")
            response = browser.call("GET", target)
        raise m0.CheckFailure("Keycloak redirect limit exceeded")

    def finish_web_oidc(self, attempt, callback):
        response = attempt["browser"].call("GET", callback)
        require(response.status == 303 and response.headers.get("Location") == BASE + "/admin/", "Web OIDC callback must redirect 303 to admin")
        require(response.headers.get("Cache-Control") == "no-store", "Web callback must be no-store")
        token = cookie_value(response, "rd_web_session", path=PREFIX, max_age=WEB_TTL)
        cookie_value(response, "rd_web_oidc", path="/api/oidc/callback", max_age=0)
        require(b"access_token" not in response.body and token.encode() not in response.body, "Web callback body exposed session")
        require(token not in response.headers["Location"] and "?" not in response.headers["Location"], "Web redirect exposed credential/query")
        landing = attempt["browser"].call("GET", response.headers["Location"])
        require(landing.status == 200, "admin landing page unavailable")
        require(token.encode() not in landing.body and b"access_token" not in landing.body, "admin HTML exposed session")
        return self.shape(attempt["browser"].call("GET", PREFIX + "/me"), attempt["browser"])

    def oidc_cases(self):
        def complete():
            attempt = self.web_oidc_start()
            callback = self.browser_callback_url(attempt)
            self.oidc_member = self.finish_web_oidc(attempt, callback)
            require(self.oidc_member.user["role"] == "member" and self.oidc_member.user["source"] == "oidc", "OIDC Web member role/source differs")
            require(attempt["browser"].call("GET", callback).status == 400, "Web callback replay accepted")
            self.client_valid(self.oidc_member.browser.token(), 401)
        require(self.check("oidc.web-real-browser-pkce-cookie-callback-replay", complete), "Web OIDC prerequisite failed")

        def binding():
            attempt = self.web_oidc_start()
            callback = self.browser_callback_url(attempt)
            require(http("GET", callback).status == 400, "Web callback accepted without browser binding")
            require(http("GET", callback, headers={"Cookie": "rd_web_oidc=" + secrets.token_hex(32)}).status == 400,
                    "Web callback accepted wrong browser binding")
            require(http("GET", PREFIX + "/me").status == 401, "unbound callback created a Web session")
            session = self.finish_web_oidc(attempt, callback)
            require(session.user["id"] == self.oidc_member.user["id"], "repeated OIDC identity changed stable id")
            require(attempt["browser"].call("GET", callback).status == 400, "bound callback replay accepted")
        self.check("oidc.web-missing-wrong-browser-binding", binding)

        def purposes():
            attempt = self.web_oidc_start()
            for candidate in [attempt["state"], attempt["binding"]]:
                require(self.poll({"id": "web", "uuid": "web", "code": candidate}).status == 400,
                        "client polling accepted Web attempt material")
            client_attempt = self.auth_start()
            require(self.poll(client_attempt).json() == {"error": "No authed oidc is found"}, "client pending protocol differs")
            client_browser = {"browser": Browser(), "url": client_attempt["url"]}
            callback = self.browser_callback_url(client_browser)
            response = client_browser["browser"].call("GET", callback)
            require(response.status == 200 and b"access_token" not in response.body, "client callback contract differs")
            require(not any(cookie.name == "rd_web_session" for cookie in client_browser["browser"].jar), "client callback created Web session")
            require(client_browser["browser"].call("GET", PREFIX + "/me").status == 401, "client browser authenticated to Web")
            client_value = self.login_shape(self.poll(client_attempt))
            require(client_value["user"]["id"] == self.oidc_member.user["id"], "Web/client OIDC identity split")
            self.oidc_client = client_value["access_token"]
            require(self.poll(client_attempt).status == 400, "client poll replay accepted")
            require(http("GET", PREFIX + "/me", headers={"Cookie": "rd_web_session=" + self.oidc_client}).status == 401,
                    "OIDC client session accepted as Web cookie")
            # Complete Web attempt after exercising client polling to prove that
            # probing it through the wrong adapter did not consume the attempt.
            callback = self.browser_callback_url(attempt)
            session = self.finish_web_oidc(attempt, callback)
            require(session.user["id"] == self.oidc_member.user["id"], "Web attempt failed after client poll probes")
        self.check("oidc.web-client-attempt-purpose-isolation", purposes)

        def managed():
            target = "/users/" + self.oidc_member.user["id"]
            require(self.admin.call("PATCH", target, {"role": "admin"}).status == 409, "local management changed OIDC role")
            require(self.admin.call("POST", target + "/password", {"new_password": secrets.token_urlsafe(24)}).status == 409,
                    "local management changed OIDC password")
            require(self.oidc_member.call("POST", "/me/password", {"current_password": "irrelevant-password", "new_password": secrets.token_urlsafe(24)}).status == 409,
                    "OIDC self password change accepted")
        self.check("oidc.provider-managed-role-password-conflicts", managed)

        def enable():
            target = "/users/" + self.oidc_member.user["id"]
            require(self.admin.call("PATCH", target, {"disabled": True}).status == 200, "OIDC disable failed")
            self.web_revoked(self.oidc_member)
            self.client_valid(self.oidc_client, 401)
            require(all(item["revoked"] for item in self.sessions(self.admin, self.oidc_member.user["id"])), "OIDC disable retained active sessions")
            denied = self.web_oidc_start()
            callback = self.browser_callback_url(denied)
            response = denied["browser"].call("GET", callback)
            require(response.status == 303 and response.headers.get("Location") == BASE + "/admin/?login=failed", "disabled OIDC login was admitted")
            require(denied["browser"].call("GET", PREFIX + "/me").status == 401, "disabled OIDC identity obtained Web session")
            require(self.admin.call("PATCH", target, {"disabled": False}).status == 200, "OIDC re-enable failed")
            user = next(user for user in self.listed_users() if user["id"] == self.oidc_member.user["id"])
            require(user["disabled"] is False and user["admitted"] is False, "OIDC enable bypassed fresh admission policy")
            self.web_revoked(self.oidc_member)
            self.client_valid(self.oidc_client, 401)
            fresh = self.web_oidc_start()
            self.oidc_member = self.finish_web_oidc(fresh, self.browser_callback_url(fresh))
            user = next(user for user in self.listed_users() if user["id"] == self.oidc_member.user["id"])
            require(user["admitted"] is True, "fresh OIDC login did not restore admission")
        self.check("oidc.disable-enable-requires-fresh-admission", enable)

    def final_cases(self):
        def missing():
            absent = "00000000-0000-4000-8000-" + secrets.token_hex(6)
            target = "/users/" + absent
            for method, path, data in [
                ("PATCH", target, {"disabled": True}), ("POST", target + "/password", {"new_password": secrets.token_urlsafe(24)}),
                ("GET", target + "/sessions", None), ("DELETE", target + "/sessions", None),
                ("DELETE", "/sessions/" + absent, None),
            ]:
                require(self.admin.call(method, path, data).status == 404, "missing user/session must return 404")
        self.check("users.missing-targets", missing)

        def logout():
            session = self.web_login(self.locals[1])
            client = self.client_login(self.locals[1])
            saved_token = session.browser.token()
            response = session.call("POST", "/auth/logout", {})
            require(response.status == 204, "Web logout failed")
            cookie_value(response, "rd_web_session", path=PREFIX, max_age=0)
            self.web_revoked(session)
            require(http("GET", PREFIX + "/me", headers={"Cookie": "rd_web_session=" + saved_token}).status == 401,
                    "logout only cleared browser cookie without revoking session")
            self.client_valid(client)
            require(self.admin.call("GET", "/me").status == 200, "logout affected another user")
        self.check("web.logout-cookie-revocation", logout)

    def run(self):
        try:
            if self.check("setup.isolated-database-cli-users-api", self.setup):
                self.local_cases()
                self.session_cases()
                self.oidc_cases()
                self.final_cases()
        except m0.CheckFailure as error:
            self.report["cases"].append({"case": "suite.prerequisite", "status": "fail", "detail": str(error)})
        except Exception as error:
            self.report["cases"].append({"case": "suite.unexpected", "status": "error", "detail": type(error).__name__})
        finally:
            try:
                self.stop()
            except Exception as error:
                self.report["cases"].append({"case": "suite.cleanup", "status": "error", "detail": type(error).__name__})
            seen = {case["case"] for case in self.report["cases"]}
            self.report["cases"].extend({"case": name, "status": "not_run", "detail": "prerequisite did not complete"}
                                        for name in PLANNED_CASES if name not in seen)
            self.report["status"] = "pass" if all(case["status"] == "pass" for case in self.report["cases"]) else "fail"
            self.report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            m0.private_json(self.directory / "report.json", self.report)
            print(json.dumps({"status": self.report["status"], "report": str((self.directory / "report.json").relative_to(m0.ROOT)),
                              "cases": len(self.report["cases"])}), flush=True)
        return 0 if self.report["status"] == "pass" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["run", "list-cases"])
    parser.add_argument("--database", choices=["sqlite", "postgres"])
    parser.add_argument("--binary", default=str(m0.ROOT / "api/target/debug/rustdesk-account-api"))
    args = parser.parse_args()
    if args.command == "list-cases":
        print(json.dumps({"milestone": "M1", "cases": PLANNED_CASES, "count": len(PLANNED_CASES)}))
        return 0
    if args.database is None:
        parser.error("run requires --database sqlite|postgres")
    if not (m0.RUNTIME / "fixture.json").is_file():
        parser.error("existing validation/.runtime/fixture.json required; prepare/start M0 dependencies first")
    return Runner(args).run()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except Exception as error:
        print(json.dumps({"status": "error", "case": "suite.startup-or-report", "detail": type(error).__name__}), flush=True)
        sys.exit(1)
