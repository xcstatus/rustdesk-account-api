import test from "node:test";
import assert from "node:assert/strict";
import { AccountApi, ApiError } from "../src/api.ts";
import type { AuditEvent, AuditFilter, AuditPage } from "../src/api.ts";
import { createAuditFeed } from "../src/audit.ts";
import type { AuditFeedState } from "../src/audit.ts";

function state(): AuditFeedState {
  return { events: [], nextBeforeId: null, loading: false, error: "" };
}
function page(id: number, next: number | null = null): AuditPage {
  return { events: [{ id } as AuditEvent], next_before_id: next };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("audit query encodes IDs and filters and uses Web cookies without mutation credentials", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    const parsed = new URL(String(url), "https://test.invalid");
    assert.equal(parsed.pathname, "/admin/api/v1/audit-events");
    assert.equal(parsed.searchParams.get("target_id"), "u &/?=+中文");
    assert.equal(parsed.searchParams.get("source"), "oidc");
    assert.equal(parsed.searchParams.get("result"), "failure");
    assert.equal(parsed.searchParams.get("action"), "login");
    assert.equal(parsed.searchParams.get("before_id"), "123");
    assert.equal(parsed.searchParams.get("since"), "0");
    assert.equal(parsed.searchParams.get("until"), "456");
    assert.equal(parsed.searchParams.has("actor_id"), false);
    assert.equal(init!.credentials, "same-origin");
    assert.equal(init!.cache, "no-store");
    assert.equal(init!.method, "GET");
    assert.equal(new Headers(init!.headers).has("Authorization"), false);
    assert.equal(new Headers(init!.headers).has("X-CSRF-Token"), false);
    return Response.json(page(1));
  };
  try {
    await new AccountApi(() => {}).listAuditEvents({ target_id: "u &/?=+中文", actor_id: "", source: "oidc", result: "failure", action: "login", before_id: 123, since: 0, until: 456 });
  } finally { globalThis.fetch = original; }
});

test("new filters and clearing a view reject stale responses", async () => {
  const pending = [deferred<AuditPage>(), deferred<AuditPage>(), deferred<AuditPage>()];
  const queries: AuditFilter[] = [];
  const view = state();
  const feed = createAuditFeed({ listAuditEvents(query) { queries.push(query); return pending[queries.length - 1]!.promise; } }, view);
  const old = feed.reset({ source: "local" });
  const current = feed.reset({ target_id: "current" });
  pending[1]!.resolve(page(10, 10));
  await current;
  pending[0]!.resolve(page(99, 99));
  await old;
  assert.deepEqual(view.events.map(item => item.id), [10]);
  assert.equal(view.nextBeforeId, 10);
  const more = feed.loadMore();
  assert.deepEqual(queries[2], { target_id: "current", before_id: 10 });
  feed.clear(); // Navigation, logout and role changes dispose the panel.
  pending[2]!.resolve(page(9));
  await more;
  assert.deepEqual(view.events, []);
  assert.equal(view.nextBeforeId, null);
  assert.equal(view.loading, false);
});

test("cursor pagination appends once and retains the filter", async () => {
  const view = state();
  const queries: AuditFilter[] = [];
  const feed = createAuditFeed({ async listAuditEvents(query) { queries.push(query); return queries.length === 1 ? page(20, 20) : page(19); } }, view);
  await feed.reset({ source: "system", limit: 50 });
  await Promise.all([feed.loadMore(), feed.loadMore()]);
  assert.deepEqual(queries, [{ source: "system", limit: 50 }, { source: "system", limit: 50, before_id: 20 }]);
  assert.deepEqual(view.events.map(item => item.id), [20, 19]);
  await feed.loadMore();
  assert.equal(queries.length, 2);
});

test("401 clears audit data and identity; 403 clears audit data and reports lost permission", async () => {
  const original = globalThis.fetch;
  let resets = 0;
  let forbidden = 0;
  const view = state();
  const api = new AccountApi(() => { resets++; });
  const feed = createAuditFeed(api, view, () => { forbidden++; });
  try {
    for (const status of [401, 403]) {
      globalThis.fetch = async () => Response.json(page(2, 2));
      await feed.reset({});
      assert.equal(view.events.length, 1);
      globalThis.fetch = async () => Response.json({ error: "denied" }, { status });
      await feed.loadMore();
      assert.deepEqual(view.events, []);
      assert.equal(view.nextBeforeId, null);
      assert.equal(view.loading, false);
      assert.ok(view.error);
    }
    assert.equal(resets, 1);
    assert.equal(forbidden, 1);
  } finally { globalThis.fetch = original; }
});

test("an obsolete failed request does not replace the current view error", async () => {
  const pending = deferred<AuditPage>();
  const view = state();
  let calls = 0;
  let forbidden = 0;
  const feed = createAuditFeed({ async listAuditEvents() { return ++calls === 1 ? pending.promise : page(7); } }, view, () => { forbidden++; });
  const old = feed.reset({});
  await feed.reset({ source: "oidc" });
  pending.reject(new ApiError(403, "old request"));
  await old;
  assert.deepEqual(view.events.map(item => item.id), [7]);
  assert.equal(view.error, "");
  assert.equal(forbidden, 0);
});
