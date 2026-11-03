#!/usr/bin/env python3
"""M2 personal-book HTTP validation using isolated M0 SQL/real Keycloak fixtures.

Two independent protocol clients are simulated, not two official GUI clients.
Only this runner's API child is stopped/restarted. Existing dependencies must be
running; containers are never started here. Secrets/raw exceptions stay out of
reports. Detailed reports and API logs use the private .runtime directory.
"""
import argparse
import concurrent.futures
import copy
import datetime
import json
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request

try:
    from . import run_m0 as m0
except ImportError:
    import run_m0 as m0

require = m0.require
EMPTY = {"peers": [], "tags": []}
FAMILY_CASES = [
    "two-independent-sessions-empty-book",
    "add-chinese-edit-tag-colors-delete-clear",
    "metadata-only-credential-exclusion",
    "ownership-forged-user-ids",
    "invalid-json-shape-fields-atomic-rejection",
    "capacity-1000-accepted-1001-rejected",
    "body-over-2mib-rejected",
    "concurrent-whole-snapshots-atomic-last-commit",
    "restart-session-and-book-persistence",
    "real-child-offline-unsent-write-not-persisted",
    "revoked-session-read-write-401",
]
PLANNED_CASES = ["setup.isolated-database-cli-users-api"] + [
    source + "." + case for source in ["local", "oidc"] for case in FAMILY_CASES
] + ["capabilities.legacy-and-empty-team-routes"]


class Client:
    """Independent transport and bearer, matching the legacy snapshot protocol."""
    def __init__(self, login):
        self.token = login["access_token"]
        self.user_id = login["user"]["id"]
        self.opener = urllib.request.build_opener()

    def call(self, method, path="/api/ab", *, value=None, raw=None):
        headers = {"Authorization": "Bearer " + self.token}
        if value is not None:
            require(raw is None, "ambiguous HTTP test body")
            raw = json.dumps(value, ensure_ascii=False).encode()
        if raw is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(m0.BASE + path, data=raw, headers=headers, method=method)
        try:
            return m0.Response(self.opener.open(request, timeout=20))
        except urllib.error.HTTPError as error:
            return m0.Response(error)

    def read(self, query=""):
        response = self.call("GET", "/api/ab" + query)
        require(response.status == 200, "address book read must return 200")
        require(response.headers.get("Cache-Control") == "no-store", "book read must be no-store")
        envelope = response.json()
        require(isinstance(envelope.get("data"), str) and envelope.get("licensed_devices") == 0,
                "legacy address book envelope differs")
        value = json.loads(envelope["data"])
        require(isinstance(value, dict), "book snapshot must be an object")
        return value

    def write(self, snapshot, *, path="/api/ab", extra=None):
        body = {"data": json.dumps(snapshot, ensure_ascii=False)}
        body.update(extra or {})
        return self.call("POST", path, value=body)

    def save(self, snapshot):
        response = self.write(snapshot)
        require(response.status == 200, "valid snapshot write must return 200")
        require(response.headers.get("X-Rustdesk-Credential-Sync") == "disabled; metadata-only",
                "metadata-only capability header missing")


def snapshot(marker, count=2):
    return {"tags": [marker], "tag_colors": json.dumps({marker: 4281558681}, ensure_ascii=False),
            "peers": [{"id": marker + "-" + str(index), "alias": "设备-" + marker,
                       "platform": "Mac OS", "username": "用户", "hostname": "主机",
                       "tags": [marker]} for index in range(count)]}


class Runner(m0.Runner):
    def __init__(self, args):
        super().__init__(args)
        self.report.update({"milestone": "M2", "planned_cases": len(PLANNED_CASES),
                            "scope": "two independent HTTP protocol clients; isolated SQL and real Keycloak; no new official GUI two-client claim",
                            "requirements": ["RD-007", "RD-008", "RD-013"],
                            "concurrency_policy": "whole-snapshot replacement; last successful database commit wins; overlapping responses do not define commit order",
                            "limits": {"peers": 1000, "tags": 1000, "http_body_bytes": 2097152},
                            "observations": {}})

    def run_family(self, source):
        state = {}

        def case(name, function, prerequisite=False):
            passed = self.check(source + "." + name, function)
            if prerequisite:
                require(passed, source + " synchronization prerequisite failed")

        def sessions():
            login = (lambda: self.login_shape(self.local_login(self.locals[0]))) if source == "local" else (lambda: self.oidc_login("member"))
            first, second = Client(login()), Client(login())
            other = Client(self.login_shape(self.local_login(self.locals[1])))
            require(first.token != second.token and first.opener is not second.opener,
                    "two client sessions/transports must be independent")
            require(first.user_id == second.user_id and first.user_id != other.user_id,
                    "session identities do not match fixture ownership")
            require(first.read() == EMPTY and second.read() == EMPTY, "new user's book is not empty")
            state.update(first=first, second=second, other=other)
        case(FAMILY_CASES[0], sessions, prerequisite=True)
        first, second, other = (state[key] for key in ["first", "second", "other"])

        def lifecycle():
            first.save(snapshot("新增"))
            require(second.read() == snapshot("新增"), "added peers not visible to second client")
            edited = snapshot("中文标签")
            edited["peers"][0]["alias"] = "上海办公室－中文别名"
            edited["tag_colors"] = '{"中文标签":4294901760}'
            second.save(edited)
            require(first.read() == edited, "Chinese edit or tag colors did not roundtrip")
            edited["peers"].pop()
            first.save(edited)
            require(second.read() == edited, "peer deletion did not roundtrip")
            second.save(EMPTY)
            require(first.read() == EMPTY, "clear did not replace complete snapshot")
        case(FAMILY_CASES[1], lifecycle)

        def credentials():
            for credential in ["", "synthetic-remote-credential-must-not-persist"]:
                clean = snapshot("metadata")
                dirty = copy.deepcopy(clean)
                dirty.update(password=credential, hash=credential, unknown="discard")
                for peer in dirty["peers"]:
                    peer.update(password=credential, hash=credential, unknown="discard")
                first.save(dirty)
                require(second.read() == clean, "credential/unknown fields persisted or metadata changed")
        case(FAMILY_CASES[2], credentials)

        def isolation():
            own, foreign = snapshot("owner"), snapshot("other-owner")
            other.save(foreign)
            forged = copy.deepcopy(own)
            forged.update(user_id=other.user_id, userid=other.user_id, owner_id=other.user_id)
            forged["peers"][0]["user_id"] = other.user_id
            query = "?" + urllib.parse.urlencode({"user_id": other.user_id, "userid": other.user_id})
            response = first.write(forged, path="/api/ab" + query,
                                   extra={"user_id": other.user_id, "userid": other.user_id})
            require(response.status == 200, "owner write with ignored IDs failed")
            require(second.read(query) == own, "forged read IDs changed book ownership")
            require(other.read() == foreign, "forged write IDs modified another user's book")
        case(FAMILY_CASES[3], isolation)

        def rejection():
            baseline = snapshot("rejection-baseline")
            first.save(baseline)
            candidates = [
                (b'{"data":', 400),
                ({"data": "{broken"}, 400),
                ({"data": "[]"}, 400),
                ({"data": json.dumps({"tags": [], "peers": [{"alias": "missing-id"}]})}, 400),
            ]
            invalids = [
                {"tags": [], "peers": [{"id": ""}]},
                {"tags": [], "peers": [{"id": "i" * 257}]},
                {"tags": [], "peers": [{"id": "i", "alias": "中" * 1366}]},
                {"tags": ["t" * 257], "peers": []},
                {"tags": [], "peers": [{"id": "i", "tags": ["t" * 257]}]},
                {"tags": [], "peers": [{"id": "i", "platform": 3}]},
                {"tags": [], "peers": [], "tag_colors": "[]"},
                {"tags": [], "peers": [], "tag_colors": '{"bad":4294967296}'},
            ]
            candidates.extend(({"data": json.dumps(value)}, 400) for value in invalids)
            for body, status in candidates:
                response = first.call("POST", raw=body) if isinstance(body, bytes) else first.call("POST", value=body)
                require(response.status == status, "invalid JSON/field snapshot rejection status differs")
                require(second.read() == baseline, "failed snapshot request changed committed data")
            self.report["observations"][source + ".invalid_snapshots_checked"] = len(candidates)
        case(FAMILY_CASES[4], rejection)

        def capacity():
            boundary = {"tags": ["标签" + str(i) for i in range(1000)],
                        "peers": [{"id": str(i)} for i in range(1000)]}
            boundary["peers"][0].update(id="i" * 256, alias="a" * 4096, tags=["t" * 256])
            first.save(boundary)
            require(second.read() == boundary, "valid count/byte boundaries failed")
            for field, entry in [("peers", {"id": "1001"}), ("tags", "1001")]:
                too_many = copy.deepcopy(boundary)
                too_many[field].append(entry)
                require(first.write(too_many).status == 400, "1001-entry snapshot was accepted")
                require(second.read() == boundary, "1001-entry rejection changed committed data")
        case(FAMILY_CASES[5], capacity)

        def body_limit():
            baseline = snapshot("body-limit-baseline")
            first.save(baseline)
            # A valid small data snapshot plus an ignored envelope member ensures
            # 413 exercises the HTTP limit, not metadata field validation.
            body = json.dumps({"data": json.dumps(EMPTY), "padding": "p" * (2 * 1024 * 1024)}).encode()
            require(len(body) > 2 * 1024 * 1024, "oversize body fixture is too short")
            require(first.call("POST", raw=body).status == 413, "body over 2 MiB was not rejected")
            require(second.read() == baseline, "oversize request overwrote committed data")
        case(FAMILY_CASES[6], body_limit)

        def concurrent_snapshots():
            read_count = 0
            for round_number in range(3):
                baseline = snapshot("baseline-" + str(round_number), 80)
                left = snapshot("left-" + str(round_number), 100)
                right = snapshot("right-" + str(round_number), 120)
                first.save(baseline)
                barrier = threading.Barrier(3)
                # The observer needs its own transport; no opener is used by two
                # concurrent threads. Its bearer may safely match client B.
                observer = Client({"access_token": second.token, "user": {"id": second.user_id}})
                def write(client, value):
                    barrier.wait(timeout=10)
                    client.save(value)
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                    futures = [pool.submit(write, first, left), pool.submit(write, second, right)]
                    barrier.wait(timeout=10)
                    while True:
                        actual = observer.read()
                        read_count += 1
                        require(actual in [baseline, left, right], "observer saw mixed or partial snapshot")
                        if all(future.done() for future in futures):
                            break
                    for future in futures:
                        future.result()
                require(observer.read() in [left, right], "concurrent final book is not a complete successful snapshot")
                # Happens-after ordering gives an unambiguous final commit.
                winner = snapshot("last-after-both-" + str(round_number), 90)
                first.save(winner)
                require(second.read() == winner, "last non-overlapping successful commit did not win")
            self.report["observations"][source + ".concurrency"] = {"rounds": 3, "atomic_reads": read_count,
                "result": "overlap final is one complete accepted snapshot; subsequent successful commit replaces it"}
        case(FAMILY_CASES[7], concurrent_snapshots)

        def restart():
            saved = snapshot("restart-persisted")
            first.save(saved)
            self.stop()
            self.start()
            require(first.read() == saved and second.read() == saved, "restart lost committed book or valid client session")
        case(FAMILY_CASES[8], restart)

        def offline():
            saved = snapshot("online-before-stop")
            first.save(saved)
            child = self.proc
            self.stop()
            try:
                require(child is not None and child.poll() is not None, "owned API child did not stop")
                try:
                    first.write(snapshot("offline-unsent-must-not-persist"))
                except (urllib.error.URLError, OSError):
                    pass
                else:
                    raise m0.CheckFailure("stopped API still returned an HTTP response")
                try:
                    second.read()
                except (urllib.error.URLError, OSError):
                    pass
                else:
                    raise m0.CheckFailure("offline book read was disguised as a successful empty book")
            finally:
                self.start()
            require(second.read() == saved, "offline unsent snapshot was persisted or previous data lost")
            recovered = snapshot("online-after-restart")
            first.save(recovered)
            require(second.read() == recovered, "fresh write after offline recovery failed")
        case(FAMILY_CASES[9], offline)

        def revoked():
            saved = second.read()
            require(first.call("POST", "/api/logout", value={}).status == 200, "client logout failed")
            require(first.call("GET").status == 401, "revoked client read accepted")
            require(first.write(snapshot("revoked-write")).status == 401, "revoked client write accepted")
            require(second.read() == saved, "revoked write changed data or revoked unrelated session")
        case(FAMILY_CASES[10], revoked)
        self.last_client = second

    def capabilities(self):
        client = self.last_client
        require(client.call("POST", "/api/ab/personal", value={}).status == 404,
                "personal probe did not declare legacy fallback")
        for path in ["/api/device-group/accessible", "/api/users", "/api/peers"]:
            response = client.call("GET", path)
            require(response.status == 200 and response.json() == {"total": 0, "data": []},
                    "unsupported team directory capability differs")
        response = m0.request(m0.BASE + "/api/login-options")
        require(response.status == 200 and response.json() == ["oidc/company"], "enabled OIDC discovery differs")

    def run(self):
        try:
            if self.check(PLANNED_CASES[0], self.setup):
                self.run_family("local")
                self.run_family("oidc")
                self.check("capabilities.legacy-and-empty-team-routes", self.capabilities)
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
        print(json.dumps({"milestone": "M2", "cases": PLANNED_CASES, "count": len(PLANNED_CASES)}))
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
