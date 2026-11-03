import test from "node:test";
import assert from "node:assert/strict";
import {
  AccountApi,
  ApiError,
  passwordError,
  usernameError,
} from "../src/api.ts";

test("requests use same-origin cookies and mutations use only the in-memory CSRF token", async () => {
  const originalFetch = globalThis.fetch;
  const requests: Array<{ url: string; init: RequestInit }> = [];
  globalThis.fetch = async (url, init) => {
    requests.push({ url: String(url), init: init! });
    return new Response(null, { status: 204 });
  };
  try {
    const api = new AccountApi(() =>
      assert.fail("unexpected unauthorized callback"),
    );
    api.setIdentity({
      user: {
        id: "u1",
        username: "admin",
        display_name: "",
        role: "admin",
        source: "local",
        disabled: false,
      },
      csrf_token: "test-token",
    });
    await api.request("/users");
    await api.request("/users/u1", "PATCH", { disabled: true });
    api.clearIdentity();
    await api.request("/auth/login", "POST", {
      username: "user",
      password: "password-for-test",
    });
    assert.equal(requests[0]!.url, "/admin/api/v1/users");
    for (const { init } of requests) {
      assert.equal(init.credentials, "same-origin");
      assert.equal(init.cache, "no-store");
      assert.equal(new Headers(init.headers).has("Authorization"), false);
    }
    assert.equal(
      new Headers(requests[0]!.init.headers).has("X-CSRF-Token"),
      false,
    );
    assert.equal(
      new Headers(requests[1]!.init.headers).get("X-CSRF-Token"),
      "test-token",
    );
    assert.equal(requests[1]!.init.body, '{"disabled":true}');
    assert.equal(
      new Headers(requests[2]!.init.headers).has("X-CSRF-Token"),
      false,
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("401 clears CSRF and calls the identity reset; denied mutations are never retried", async () => {
  const originalFetch = globalThis.fetch;
  let resets = 0;
  let calls = 0;
  const api = new AccountApi(() => {
    resets++;
  });
  api.setIdentity({
    user: {
      id: "u1",
      username: "admin",
      display_name: "",
      role: "admin",
      source: "local",
      disabled: false,
    },
    csrf_token: "test-token",
  });
  globalThis.fetch = async () => {
    calls++;
    return Response.json({ error: "expired" }, { status: 401 });
  };
  try {
    await assert.rejects(
      api.request("/sessions", "DELETE"),
      (reason: unknown) => reason instanceof ApiError && reason.status === 401,
    );
    assert.equal(resets, 1);
    assert.equal(calls, 1);
    globalThis.fetch = async (_url, init) => {
      assert.equal(new Headers(init!.headers).has("X-CSRF-Token"), false);
      return Response.json({ message: "admin required" }, { status: 403 });
    };
    await assert.rejects(
      api.request("/users", "POST", {}),
      /权限不足：admin required/,
    );
    assert.equal(resets, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("transport and malformed successful responses show errors instead of success", async () => {
  const originalFetch = globalThis.fetch;
  const api = new AccountApi(() => {});
  try {
    globalThis.fetch = async () => {
      throw new TypeError("offline");
    };
    await assert.rejects(api.request("/me"), /无法连接服务/);
    globalThis.fetch = async () => new Response("not-json");
    await assert.rejects(api.request("/me"), /无法识别的数据/);
    globalThis.fetch = async () =>
      new Response("gateway error", { status: 502 });
    await assert.rejects(api.request("/me"), /HTTP 502/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("validation counts UTF-8 bytes and rejects username whitespace", () => {
  assert.ok(passwordError("a".repeat(11)));
  assert.equal(passwordError("中".repeat(4)), "");
  assert.equal(passwordError("a".repeat(4096)), "");
  assert.ok(passwordError("a".repeat(4097)));
  assert.ok(usernameError(" user"));
  assert.ok(usernameError("user "));
  assert.ok(usernameError(""));
  assert.equal(usernameError("a".repeat(128)), "");
  assert.ok(usernameError("中".repeat(43)));
});
