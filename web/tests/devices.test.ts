import test from "node:test";
import assert from "node:assert/strict";
import { AccountApi } from "../src/api.ts";
import type { User } from "../src/api.ts";
import { createDeviceManager } from "../src/devices.ts";
import type { Device, DeviceState } from "../src/devices.ts";

const admin: User = { id: "admin", username: "admin", display_name: "", role: "admin", source: "local", disabled: false };
const member: User = { ...admin, id: "member", username: "member", role: "member" };
function state(): DeviceState {
  return { devices: [], users: [], owners: {}, loading: false, savingId: "", error: "", notice: "" };
}
function device(owner: string | null = member.id): Device {
  return { id: "device &/中文", rustdesk_id: "123456", owner_user_id: owner, owner_username: owner, platform: "Linux", device_name: "工作电脑", device_type: "", last_login_at: 1 };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

test("members request only devices, display only their own devices, and cannot submit ownership changes", async () => {
  const original = globalThis.fetch;
  const paths: string[] = [];
  globalThis.fetch = async (url) => {
    paths.push(String(url));
    return Response.json({ devices: [device(), { ...device(admin.id), id: "other" }] });
  };
  try {
    const view = state();
    const manager = createDeviceManager(new AccountApi(() => {}), view);
    await manager.reset(member);
    await manager.save(device().id, admin.id);
    assert.deepEqual(paths, ["/admin/api/v1/devices"]);
    assert.deepEqual(view.devices.map(item => item.owner_user_id), [member.id]);
    assert.deepEqual(view.users, []);
  } finally { globalThis.fetch = original; }
});

test("ownership saves encode IDs, serialize mutations, and reread authoritative data", async () => {
  const original = globalThis.fetch;
  const mutation = deferred<Response>();
  const requests: Array<{ path: string; method: string; body: unknown }> = [];
  let saved = false;
  globalThis.fetch = async (url, init) => {
    const path = String(url);
    requests.push({ path, method: init!.method!, body: init!.body });
    if (init!.method === "PATCH") return mutation.promise;
    return Response.json(path.endsWith("/users") ? { users: [admin, member] } : { devices: [device(saved ? null : member.id)] });
  };
  try {
    const view = state();
    const manager = createDeviceManager(new AccountApi(() => {}), view);
    await manager.reset(admin);
    const saving = manager.save(device().id, null);
    await manager.save(device().id, admin.id);
    await manager.refresh();
    assert.equal(requests.length, 3);
    assert.equal(view.savingId, device().id);
    saved = true;
    mutation.resolve(Response.json({ device: device(null) }));
    await saving;
    assert.deepEqual(requests[2], { path: `/admin/api/v1/devices/${encodeURIComponent(device().id)}`, method: "PATCH", body: '{"owner_user_id":null}' });
    assert.equal(requests.length, 5);
    assert.equal(view.devices[0]!.owner_user_id, null);
    assert.equal(view.owners[device().id], "");
    assert.equal(view.savingId, "");
    assert.ok(view.notice);
  } finally { globalThis.fetch = original; }
});

test("identity changes and view disposal reject stale device and account responses", async () => {
  const original = globalThis.fetch;
  const oldDevices = deferred<Response>();
  const oldUsers = deferred<Response>();
  globalThis.fetch = async (url) => String(url).endsWith("/users") ? oldUsers.promise : oldDevices.promise;
  try {
    const view = state();
    const manager = createDeviceManager(new AccountApi(() => {}), view);
    const old = manager.reset(admin);
    globalThis.fetch = async () => Response.json({ devices: [device()] });
    await manager.reset(member);
    oldUsers.resolve(Response.json({ users: [admin, member] }));
    oldDevices.resolve(Response.json({ devices: [device(admin.id)] }));
    await old;
    assert.deepEqual(view.users, []);
    assert.equal(view.devices[0]!.owner_user_id, member.id);
    const pending = deferred<Response>();
    globalThis.fetch = async () => pending.promise;
    const refreshing = manager.refresh();
    manager.clear();
    pending.resolve(Response.json({ devices: [device()] }));
    await refreshing;
    assert.deepEqual(view.devices, []);
    assert.deepEqual(view.owners, {});
    assert.equal(view.loading, false);
  } finally { globalThis.fetch = original; }
});

test("401 and 403 during a save clear all private data without retrying", async () => {
  const original = globalThis.fetch;
  try {
    for (const status of [401, 403]) {
      let resets = 0;
      let forbidden = 0;
      let mutations = 0;
      const view = state();
      globalThis.fetch = async (url, init) => {
        if (init!.method === "PATCH") {
          mutations++;
          return Response.json({ error: "denied" }, { status });
        }
        return Response.json(String(url).endsWith("/users") ? { users: [member] } : { devices: [device()] });
      };
      const manager = createDeviceManager(new AccountApi(() => { resets++; }), view, () => { forbidden++; });
      await manager.reset(admin);
      await manager.save(device().id, null);
      assert.deepEqual(view.devices, []);
      assert.deepEqual(view.users, []);
      assert.deepEqual(view.owners, {});
      assert.equal(view.savingId, "");
      assert.equal(view.loading, false);
      assert.ok(view.error);
      assert.equal(mutations, 1);
      assert.equal(resets, status === 401 ? 1 : 0);
      assert.equal(forbidden, status === 403 ? 1 : 0);
    }
  } finally { globalThis.fetch = original; }
});

test("a disposed ownership request cannot repopulate data or trigger a follow-up reload", async () => {
  const original = globalThis.fetch;
  const mutation = deferred<Response>();
  let reads = 0;
  globalThis.fetch = async (url, init) => {
    if (init!.method === "PATCH") return mutation.promise;
    reads++;
    return Response.json(String(url).endsWith("/users") ? { users: [member] } : { devices: [device()] });
  };
  try {
    const view = state();
    const manager = createDeviceManager(new AccountApi(() => {}), view);
    await manager.reset(admin);
    const saving = manager.save(device().id, null);
    manager.clear();
    mutation.resolve(Response.json({ device: device(null) }));
    await saving;
    assert.equal(reads, 2);
    assert.deepEqual(view.devices, []);
    assert.deepEqual(view.users, []);
    assert.equal(view.notice, "");
    assert.equal(view.savingId, "");
  } finally { globalThis.fetch = original; }
});
