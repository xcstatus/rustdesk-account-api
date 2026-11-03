import type { AccountApi, User } from "./api";

export interface Device {
  id: string;
  rustdesk_id: string;
  owner_user_id: string | null;
  owner_username: string | null;
  platform: string;
  device_name: string;
  device_type: string;
  last_login_at: number;
}

export interface DeviceState {
  devices: Device[];
  users: User[];
  owners: Record<string, string>;
  loading: boolean;
  savingId: string;
  error: string;
  notice: string;
}

// Request generations keep a disposed or superseded view from receiving private data.
export function createDeviceManager(
  api: Pick<AccountApi, "request">,
  state: DeviceState,
  onForbidden: () => void = () => {},
) {
  let generation = 0;
  let actor: Pick<User, "id" | "role"> | null = null;

  function clear() {
    generation++;
    actor = null;
    state.devices = [];
    state.users = [];
    state.owners = {};
    state.loading = false;
    state.savingId = "";
    state.error = "";
    state.notice = "";
  }

  function failed(reason: unknown, version: number) {
    if (version !== generation) return;
    const status = reason && typeof reason === "object" && "status" in reason
      ? reason.status : undefined;
    if (status === 401 || status === 403) clear();
    state.error = reason instanceof Error ? reason.message : "设备信息加载失败，请重试。";
    if (status === 403) onForbidden();
  }

  async function load(version: number): Promise<boolean> {
    const currentActor = actor;
    if (!currentActor) return false;
    state.loading = true;
    state.error = "";
    try {
      const [result, accounts] = await Promise.all([
        api.request<{ devices: Device[] }>("/devices"),
        currentActor.role === "admin"
          ? api.request<{ users: User[] }>("/users")
          : Promise.resolve({ users: [] as User[] }),
      ]);
      if (version !== generation) return false;
      if (!Array.isArray(result?.devices) || !Array.isArray(accounts?.users))
        throw new Error("服务端返回了无法识别的设备信息，请刷新后重试。");
      state.devices = currentActor.role === "admin" ? result.devices
        : result.devices.filter((device) => device.owner_user_id === currentActor.id);
      state.users = accounts.users;
      state.owners = Object.fromEntries(state.devices.map((device) => [device.id, device.owner_user_id ?? ""]));
      return true;
    } catch (reason) {
      failed(reason, version);
      return false;
    } finally {
      if (version === generation) state.loading = false;
    }
  }

  function reset(user: Pick<User, "id" | "role">) {
    clear();
    actor = { id: user.id, role: user.role };
    return load(generation);
  }

  return {
    clear,
    reset,
    refresh() {
      if (!actor || state.loading || state.savingId) return Promise.resolve(false);
      return reset(actor);
    },
    async save(id: string, ownerId: string | null) {
      if (actor?.role !== "admin" || state.loading || state.savingId) return;
      const device = state.devices.find((item) => item.id === id);
      if (!device || ownerId === device.owner_user_id) return;
      state.error = "";
      state.notice = "";
      if (ownerId !== null && !state.users.some((user) => user.id === ownerId)) {
        state.error = "请选择列表中的账号后保存。";
        return;
      }
      const version = generation;
      state.savingId = id;
      try {
        await api.request<{ device: Device }>(`/devices/${encodeURIComponent(id)}`, "PATCH", { owner_user_id: ownerId });
        if (version !== generation) return;
        state.devices = [];
        state.owners = {};
        if (await load(version)) state.notice = "设备归属已更新。";
        else if (version === generation) state.error = `设备归属已保存，但列表刷新失败。${state.error}`;
      } catch (reason) {
        failed(reason, version);
      } finally {
        if (version === generation) state.savingId = "";
      }
    },
  };
}
