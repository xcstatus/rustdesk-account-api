#!/usr/bin/env python3
"""Real HTTP audit validation using the existing isolated M1 SQL/Keycloak runner.

Requires existing M0 dependencies and a built API. Starts/stops only its own API
child. No container startup, raw response/exception, password, token or browser
URL is emitted. Reports use the private validation/.runtime run directory.
"""
import argparse
import datetime
import json
import secrets
import sys
import urllib.parse

try:
    from . import run_m1 as m1
except ImportError:
    import run_m1 as m1

m0 = m1.m0
require = m0.require
PATH = "/audit-events"
FIELDS = {"id", "timestamp", "actor_id", "target_id", "session_id", "source", "kind", "action",
          "result", "reason", "before_role", "after_role", "before_admitted", "after_admitted",
          "before_disabled", "after_disabled", "policy_version", "revoked_count"}
ACTIONS = {"create_user", "bootstrap", "update_user", "change_password", "reset_password", "oidc_jit",
           "oidc_mapping", "policy_change", "login", "logout", "revoke_session", "revoke_all", "prune_audit"}
REASONS = {None, "invalid_request", "rate_limited", "invalid_origin", "oidc_state", "oidc_provider",
           "oidc_verification", "oidc_expired", "session_denied", "invalid_credentials"}
PLANNED_CASES = [
    "setup.isolated-database-cli-users-api",
    "audit.access-schema-member-client-denied",
    "audit.local-client-web-login-success-failure",
    "audit.local-client-web-logout",
    "audit.admin-create-role-transitions",
    "audit.admin-disable-enable-reset",
    "audit.self-password-change",
    "audit.selected-and-all-session-revocation",
    "audit.oidc-real-jit-repeat-mapping",
    "audit.oidc-unmatched-admission-denial",
    "audit.oidc-invalid-state-and-exchange",
    "audit.policy-restart-and-oidc-role-remapping",
    "audit.filters-keyset-pagination-boundaries",
    "audit.persisted-events-and-secret-exclusion",
]


class Runner(m1.Runner):
    def __init__(self, args):
        self.sensitive = set()
        super().__init__(args)
        self.report.update({"milestone": "audit", "planned_cases": len(PLANNED_CASES),
                            "scope": "real HTTP local/Web/client events and real Keycloak code+PKCE; isolated SQL; protocol simulation",
                            "observations": {}})
        self.remember(self.fixture["client_secret"], self.fixture["pg_password"],
                      self.fixture["admin_password"], self.fixture["admin_username"])
        for account in self.fixture["users"].values():
            self.remember(account["username"], account["password"])

    def remember(self, *values):
        self.sensitive.update(value for value in values if isinstance(value, str) and len(value) >= 8)

    def login_shape(self, response):
        value = super().login_shape(response)
        self.remember(value["access_token"])
        return value

    def shape(self, response, browser):
        session = super().shape(response, browser)
        self.remember(browser.token(), session.csrf)
        return session

    def web_login(self, account, password=None):
        self.remember(account["username"], account["password"], password)
        return super().web_login(account, password)

    def auth_start(self):
        attempt = super().auth_start()
        self.remember(attempt["url"], attempt["code"])
        for values in urllib.parse.parse_qs(urllib.parse.urlsplit(attempt["url"]).query).values():
            self.remember(*values)
        return attempt

    def browser(self, attempt, role):
        response = super().browser(attempt, role)
        self.remember(response.url)
        for values in urllib.parse.parse_qs(urllib.parse.urlsplit(response.url).query).values():
            self.remember(*values)
        return response

    def web_oidc_start(self):
        attempt = super().web_oidc_start()
        self.remember(attempt["url"], attempt["state"], attempt["binding"])
        return attempt

    def browser_callback_url(self, attempt, role="member"):
        callback = super().browser_callback_url(attempt, role)
        self.remember(callback)
        for values in urllib.parse.parse_qs(urllib.parse.urlsplit(callback).query).values():
            self.remember(*values)
        return callback

    def setup(self):
        super().setup()
        for account in self.locals:
            self.remember(account["username"], account["password"])
        for account in self.accounts.values():
            self.remember(account["username"], account["password"])
        self.watermark = self.latest_id()
        self.report["observations"]["post_setup_event_watermark"] = self.watermark

    def query(self, **filters):
        query = urllib.parse.urlencode(filters)
        response = self.admin.call("GET", PATH + ("?" + query if query else ""))
        require(response.status == 200, "admin audit query must return 200")
        require(response.headers.get("Cache-Control") == "no-store", "audit response must be no-store")
        for value in self.sensitive:
            require(value.encode() not in response.body, "audit response exposed sensitive fixture material")
        page = response.json()
        require(set(page) == {"events", "next_before_id"} and isinstance(page["events"], list),
                "audit page envelope differs")
        ids = []
        for event in page["events"]:
            require(set(event) == FIELDS, "audit event fields differ or arbitrary metadata exposed")
            require(isinstance(event["id"], int) and event["id"] > 0 and isinstance(event["timestamp"], int),
                    "audit event identity/time is invalid")
            require(event["source"] in {"local", "oidc", "system"}
                    and event["kind"] in {"web", "client", "cli", "unknown"}
                    and event["action"] in ACTIONS and event["result"] in {"success", "failure"}
                    and event["reason"] in REASONS, "audit enum vocabulary differs")
            for field in ["actor_id", "target_id", "session_id"]:
                require(event[field] is None or (isinstance(event[field], str) and len(event[field]) == 36),
                        "audit identifier is not an opaque stable ID")
            for field in ["before_role", "after_role"]:
                require(event[field] in {None, "none", "admin", "member"}, "audit role value differs")
            for field in ["before_admitted", "after_admitted", "before_disabled", "after_disabled"]:
                require(event[field] is None or type(event[field]) is bool, "audit state must be boolean or null")
            require(isinstance(event["policy_version"], str) and isinstance(event["revoked_count"], int)
                    and event["revoked_count"] >= 0, "audit policy/revocation metadata differs")
            ids.append(event["id"])
        require(ids == sorted(set(ids), reverse=True), "audit events must have unique descending IDs")
        cursor = page["next_before_id"]
        require(cursor is None or (ids and cursor == ids[-1]), "audit next cursor does not match final event")
        return page

    def latest_id(self):
        events = self.query(limit=1)["events"]
        return events[0]["id"] if events else 0

    def events(self, after_id=0, **filters):
        result = []
        cursor = None
        for _ in range(100):
            page = self.query(limit=200, **filters, **({"before_id": cursor} if cursor else {}))
            result.extend(event for event in page["events"] if event["id"] > after_id)
            cursor = page["next_before_id"]
            if cursor is None or cursor <= after_id:
                return result
        raise m0.CheckFailure("audit pagination exceeded bounded run fixture")

    def one(self, action, mark, *, where=None, **filters):
        values = self.events(mark, action=action, **filters)
        if where:
            values = [event for event in values if all(event[key] == value for key, value in where.items())]
        require(len(values) == 1, "expected exactly one matching new audit event")
        return values[0]

    def access(self):
        require(self.events(), "setup produced no persisted audit events")
        self.member = self.web_login(self.accounts["sessions"])
        require(self.member.call("GET", PATH).status == 403, "member read administrator audit events")
        client = self.client_login(self.accounts["sessions"])
        require(m1.http("GET", m1.PREFIX + PATH, headers={"Authorization": "Bearer " + client}).status == 401,
                "client bearer accessed Web audit endpoint")
        require(m1.http("GET", m1.PREFIX + PATH).status == 401, "anonymous audit query accepted")

    def local_logins(self):
        account = self.accounts["managed"]
        mark = self.latest_id()
        self.local_web = self.web_login(account)
        account["id"] = self.local_web.user["id"]
        self.local_client = self.client_login(account)
        for kind in ["web", "client"]:
            event = self.one("login", mark, target_id=account["id"], source="local", kind=kind, result="success")
            require(event["actor_id"] == account["id"] and event["session_id"] is not None,
                    "successful local login lacks actor/session identity")
        wrong = "audit-wrong-password-" + secrets.token_hex(12)
        self.remember(wrong)
        mark = self.latest_id()
        require(self.local_login(account, wrong).status == 401, "wrong client password accepted")
        require(m1.http("POST", m1.PREFIX + "/auth/login", data={"username": account["username"], "password": wrong},
                        headers={"Origin": m1.ORIGIN}).status == 401, "wrong Web password accepted")
        for kind in ["web", "client"]:
            event = self.one("login", mark, source="local", kind=kind, result="failure")
            require(event["reason"] == "invalid_credentials" and event["actor_id"] is None
                    and event["target_id"] is None, "failed login leaked identity or omitted stable reason")

    def logouts(self):
        mark = self.latest_id()
        require(m0.request(m0.BASE + "/api/logout", {}, token=self.local_client).status == 200, "client logout failed")
        require(self.local_web.call("POST", "/auth/logout", {}).status == 204, "Web logout failed")
        for kind in ["client", "web"]:
            event = self.one("logout", mark, source="local", kind=kind, target_id=self.local_web.user["id"])
            require(event["actor_id"] == event["target_id"] and event["session_id"] is not None
                    and event["revoked_count"] == 1, "logout event identity/count differs")

    def create_roles(self):
        account = {"username": "audit_created_" + self.namespace, "password": secrets.token_urlsafe(24)}
        self.remember(account["username"], account["password"])
        mark = self.latest_id()
        response = self.admin.call("POST", "/users", dict(account, role="member"))
        require(response.status == 201, "audit target creation failed")
        account["id"] = response.json()["user"]["id"]
        self.created = account
        event = self.one("create_user", mark, target_id=account["id"], actor_id=self.admin.user["id"], kind="web")
        require(event["before_role"] is None and event["after_role"] == "member"
                and event["after_admitted"] is True, "create-user event state differs")
        for before, after in [("member", "admin"), ("admin", "member")]:
            mark = self.latest_id()
            require(self.admin.call("PATCH", "/users/" + account["id"], {"role": after}).status == 200,
                    "audit role transition failed")
            event = self.one("update_user", mark, target_id=account["id"], actor_id=self.admin.user["id"])
            require(event["before_role"] == before and event["after_role"] == after, "role event before/after differs")

    def disable_reset(self):
        account = self.created
        client = self.client_login(account)
        for disabled in [True, False]:
            mark = self.latest_id()
            require(self.admin.call("PATCH", "/users/" + account["id"], {"disabled": disabled}).status == 200,
                    "audit enable/disable operation failed")
            event = self.one("update_user", mark, target_id=account["id"], actor_id=self.admin.user["id"])
            require(event["before_disabled"] is (not disabled) and event["after_disabled"] is disabled
                    and event["after_admitted"] is (not disabled), "disable event state differs")
            if disabled:
                require(event["revoked_count"] >= 1, "disable event omitted revoked count")
                self.client_valid(client, 401)
        client = self.client_login(account)
        replacement = secrets.token_urlsafe(24)
        self.remember(replacement)
        mark = self.latest_id()
        require(self.admin.call("POST", "/users/" + account["id"] + "/password", {"new_password": replacement}).status == 204,
                "audit password reset failed")
        event = self.one("reset_password", mark, target_id=account["id"], actor_id=self.admin.user["id"])
        require(event["revoked_count"] >= 1, "reset event omitted revoked count")
        self.client_valid(client, 401)
        account["password"] = replacement

    def self_password(self):
        account = self.accounts["selfchange"]
        session = self.web_login(account)
        replacement = secrets.token_urlsafe(24)
        self.remember(replacement)
        mark = self.latest_id()
        require(session.call("POST", "/me/password", {"current_password": account["password"], "new_password": replacement}).status == 204,
                "audit self-password change failed")
        event = self.one("change_password", mark, actor_id=session.user["id"], target_id=session.user["id"])
        require(event["source"] == "local" and event["kind"] == "web" and event["revoked_count"] >= 1,
                "self-password event source/count differs")
        self.web_revoked(session)
        account["password"] = replacement

    def revocations(self):
        session = self.web_login(self.accounts["sessions"])
        self.client_login(self.accounts["sessions"])
        selected = next(item for item in self.sessions(session) if item["kind"] == "client" and not item["revoked"])
        mark = self.latest_id()
        require(self.admin.call("DELETE", "/sessions/" + selected["id"]).status == 204, "audit selected revoke failed")
        event = self.one("revoke_session", mark, actor_id=self.admin.user["id"], target_id=session.user["id"])
        require(event["session_id"] == selected["id"] and event["revoked_count"] == 1, "selected-revoke event differs")
        mark = self.latest_id()
        require(session.call("DELETE", "/sessions").status == 204, "audit all-session revoke failed")
        event = self.one("revoke_all", mark, actor_id=session.user["id"], target_id=session.user["id"])
        require(event["revoked_count"] >= 1 and event["kind"] == "web", "all-revoke event count differs")
        self.web_revoked(session)

    def oidc_jit(self):
        mark = self.latest_id()
        value = self.oidc_login("member")
        self.oidc_id, self.oidc_token = value["user"]["id"], value["access_token"]
        event = self.one("oidc_jit", mark, target_id=self.oidc_id, source="oidc")
        require(event["before_role"] is None and event["after_role"] == "member"
                and event["after_admitted"] is True and bool(event["policy_version"]), "OIDC JIT state differs")
        self.one("login", mark, target_id=self.oidc_id, source="oidc", kind="client", result="success")
        mark = self.latest_id()
        repeated = self.oidc_login("member")
        require(repeated["user"]["id"] == self.oidc_id, "OIDC repeat identity changed")
        event = self.one("oidc_mapping", mark, target_id=self.oidc_id, source="oidc")
        require(event["before_role"] == event["after_role"] == "member"
                and event["before_admitted"] is True and event["after_admitted"] is True,
                "OIDC repeat mapping state differs")
        mark = self.latest_id()
        attempt = self.web_oidc_start()
        session = self.finish_web_oidc(attempt, self.browser_callback_url(attempt))
        require(session.user["id"] == self.oidc_id, "OIDC Web/client identity differs")
        self.one("login", mark, target_id=self.oidc_id, source="oidc", kind="web", result="success")

    def admission_denied(self):
        mark = self.latest_id()
        self.oidc_login("denied", denied=True)
        event = self.one("oidc_jit", mark, source="oidc", where={"after_admitted": False})
        require(event["after_role"] == "none" and event["target_id"] is not None, "denied OIDC JIT state differs")
        failure = self.one("login", mark, source="oidc", result="failure", target_id=event["target_id"])
        require(failure["reason"] == "session_denied", "OIDC admission failure reason differs")

    def oidc_failures(self):
        mark = self.latest_id()
        require(m0.request(m0.BASE + "/api/oidc/callback?state=invalid-audit-state&code=invalid-audit-code").status == 400,
                "invalid OIDC state accepted")
        self.one("login", mark, source="oidc", result="failure", where={"reason": "oidc_state"})
        attempt = self.auth_start()
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(attempt["url"]).query)["state"][0]
        callback = m0.BASE + "/api/oidc/callback?" + urllib.parse.urlencode({"state": state, "code": "invalid-audit-exchange"})
        self.remember(callback, "invalid-audit-exchange")
        mark = self.latest_id()
        require(m0.request(callback).status == 400, "invalid OIDC code exchange accepted")
        self.one("login", mark, source="oidc", result="failure", kind="client", where={"reason": "oidc_verification"})
        failures = self.events(mark, action="login", source="oidc", result="failure")
        require(len(failures) == 1, "failed OIDC callback recorded duplicate authentication failures")
        failure_ids = [event["id"] for event in failures]
        # Retrieving the already failed attempt, including replay after consumption,
        # is not another authentication attempt and must not inflate audit counts.
        for _ in range(2):
            require(self.poll(attempt).status == 400, "failed OIDC attempt polling must return 400")
            observed = self.events(mark, action="login", source="oidc", result="failure")
            require([event["id"] for event in observed] == failure_ids,
                    "failed OIDC attempt polling/replay duplicated authentication failure events")
        self.report["observations"]["failed_oidc_polling"] = {"callback_failure_events": 1,
                                                                  "polls_including_replay": 2, "additional_failure_events": 0}

    def policy_mapping(self):
        original = self.env["RD_OIDC_ADMIN_VALUES"]
        previous_version = self.one("oidc_jit", self.watermark, target_id=self.oidc_id)["policy_version"]
        try:
            for configured, before_role, after_role in [("rustdesk-users", "member", "admin"), (original, "admin", "member")]:
                mark = self.latest_id()
                self.stop()
                self.env["RD_OIDC_ADMIN_VALUES"] = configured
                self.start()
                event = self.one("policy_change", mark, source="system")
                require(event["policy_version"] != previous_version and event["kind"] == "unknown",
                        "policy restart omitted changed policy version")
                require(event["revoked_count"] >= 1, "policy change omitted OIDC session revocation count")
                previous_version = event["policy_version"]
                self.client_valid(self.oidc_token, 401)
                mark = self.latest_id()
                value = self.oidc_login("member")
                self.oidc_token = value["access_token"]
                require(value["user"]["id"] == self.oidc_id and value["user"]["is_admin"] is (after_role == "admin"),
                        "OIDC policy remapping outcome differs")
                event = self.one("oidc_mapping", mark, target_id=self.oidc_id)
                require(event["before_role"] == before_role and event["after_role"] == after_role
                        and event["policy_version"] == previous_version, "OIDC mapping omitted before/after policy state")
        finally:
            if self.env["RD_OIDC_ADMIN_VALUES"] != original:
                self.stop()
                self.env["RD_OIDC_ADMIN_VALUES"] = original
                self.start()

    def filters_pagination(self):
        events = self.events(self.watermark, target_id=self.created["id"], action="update_user", source="local", kind="web", result="success")
        require(len(events) == 4, "filtered management event count differs")
        timestamp = events[0]["timestamp"]
        selected = self.events(self.watermark, target_id=self.created["id"], actor_id=self.admin.user["id"],
                               action="update_user", source="local", kind="web", result="success", since=timestamp, until=timestamp)
        require(selected and all(event["timestamp"] == timestamp for event in selected), "inclusive timestamp/actor filters differ")
        expected = [event["id"] for event in self.events()]
        ids, cursor = [], None
        for _ in range(100):
            page = self.query(limit=2, **({"before_id": cursor} if cursor else {}))
            ids.extend(event["id"] for event in page["events"])
            cursor = page["next_before_id"]
            if cursor is None:
                break
        require(ids == expected and len(ids) == len(set(ids)), "keyset pagination omitted or duplicated audit events")
        for invalid in [{"limit": 0}, {"limit": 201}, {"before_id": 0}, {"since": 2, "until": 1},
                        {"actor_id": "a" * 129}, {"action": "arbitrary"}, {"source": "arbitrary"},
                        {"kind": "arbitrary"}, {"result": "arbitrary"}, {"unknown": "field"}]:
            response = self.admin.call("GET", PATH + "?" + urllib.parse.urlencode(invalid))
            require(response.status == 400, "invalid audit filter accepted")
        self.report["observations"]["keyset_pagination"] = {"page_limit": 2, "events": len(ids), "duplicates": 0}

    def persistence_secrets(self):
        before = self.events()
        self.stop()
        self.start()
        after = self.events()
        require(after == before, "unchanged-policy restart changed or lost persisted audit history")
        require(all(event["id"] > self.watermark for event in self.events(self.watermark)), "run event watermark isolation failed")
        self.report["observations"]["persisted_events"] = len(after)
        self.report["observations"]["secret_exclusion"] = "every successful audit response scanned against known fixture credentials, sessions and OIDC URL/code material"

    def run(self):
        cases = [self.access, self.local_logins, self.logouts, self.create_roles, self.disable_reset,
                 self.self_password, self.revocations, self.oidc_jit, self.admission_denied,
                 self.oidc_failures, self.policy_mapping, self.filters_pagination, self.persistence_secrets]
        try:
            if self.check(PLANNED_CASES[0], self.setup):
                for name, function in zip(PLANNED_CASES[1:], cases):
                    # Account-creating prerequisites must finish before dependent
                    # operations; later cases still record isolated failures.
                    passed = self.check(name, function)
                    if function in [self.local_logins, self.create_roles, self.oidc_jit]:
                        require(passed, "audit fixture prerequisite did not complete")
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
        print(json.dumps({"milestone": "audit", "cases": PLANNED_CASES, "count": len(PLANNED_CASES)}))
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
