<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { AccountApi, ApiError, passwordError, usernameError } from "./api";
import type { Identity, SessionInfo, User } from "./api";
import AuditPanel from "./AuditPanel.vue";
import DevicePanel from "./DevicePanel.vue";

const user = ref<User | null>(null);
const booting = ref(true);
const oidcEnabled = ref(false);
const optionsLoaded = ref(false);
const busy = ref(false);
const error = ref("");
const notice = ref("");
const tab = ref<"account" | "devices" | "users" | "audit">("account");
const sessions = ref<SessionInfo[]>([]);
const sessionsLoading = ref(false);
const users = ref<User[]>([]);
const usersLoading = ref(false);
const search = ref("");
const target = ref<User | null>(null);
const targetSessions = ref<SessionInfo[]>([]);
const targetLoading = ref(false);
let identityVersion = 0;
let targetVersion = 0;
const login = reactive({ username: "", password: "" });
const password = reactive({ current: "", next: "", confirm: "" });
const create = reactive({ username: "", password: "", role: "member" });
const resetPassword = ref("");
const resetConfirm = ref("");
const resetError = ref("");
const resetNotice = ref("");
const api = new AccountApi(() => clearIdentity());
const isAdmin = computed(() => user.value?.role === "admin");
const filteredUsers = computed(() => {
  const query = search.value.trim().toLowerCase();
  return users.value.filter((item) =>
    `${item.username} ${item.display_name} ${item.id}`
      .toLowerCase()
      .includes(query),
  );
});
const activeSessions = computed(() => sessions.value.filter(revocable).length);

function clearSensitive() {
  login.password = "";
  password.current = password.next = password.confirm = "";
  create.password = "";
  resetPassword.value = resetConfirm.value = "";
}
function clearResetFeedback() {
  resetError.value = "";
  resetNotice.value = "";
}
function closeTarget() {
  targetVersion++;
  target.value = null;
  targetSessions.value = [];
  targetLoading.value = false;
  resetPassword.value = resetConfirm.value = "";
  clearResetFeedback();
}
function clearIdentity() {
  identityVersion++;
  api.clearIdentity();
  user.value = null;
  sessions.value = [];
  sessionsLoading.value = false;
  users.value = [];
  usersLoading.value = false;
  tab.value = "account";
  closeTarget();
  clearSensitive();
}
function acceptIdentity(identity: Identity) {
  api.setIdentity(identity);
  identityVersion++;
  user.value = identity.user;
  if (identity.user.role !== "admin") {
    users.value = [];
    tab.value = "account";
    closeTarget();
  }
}
async function auditForbidden() {
  tab.value = "account";
  users.value = [];
  closeTarget();
  const version = identityVersion;
  try {
    const identity = await api.request<Identity>("/me");
    if (version === identityVersion) acceptIdentity(identity);
  } catch (reason) {
    error.value = message(reason);
  }
}
function message(reason: unknown) {
  return reason instanceof Error ? reason.message : "操作失败，请重试。";
}
async function perform(action: () => Promise<void>) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  notice.value = "";
  try {
    await action();
  } catch (reason) {
    error.value = message(reason);
  } finally {
    busy.value = false;
  }
}
async function loadOptions() {
  const result = await api.request<{ oidc_enabled: boolean }>("/auth/options");
  oidcEnabled.value = result.oidc_enabled;
  optionsLoaded.value = true;
}
async function initialize() {
  booting.value = true;
  error.value = "";
  try {
    try {
      await loadOptions();
    } catch (reason) {
      error.value = message(reason);
    }
    try {
      acceptIdentity(await api.request<Identity>("/me"));
      await loadSessions();
    } catch (reason) {
      if (!(reason instanceof ApiError && reason.status === 401)) throw reason;
    }
  } catch (reason) {
    error.value = message(reason);
  } finally {
    booting.value = false;
  }
}
async function signIn() {
  const validation = usernameError(login.username);
  if (validation || !login.password) {
    error.value = validation || "请输入密码。";
    return;
  }
  // Remove the password from reactive form state before awaiting the network.
  const payload = { username: login.username, password: login.password };
  login.password = "";
  await perform(async () => {
    try {
      acceptIdentity(
        await api.request<Identity>("/auth/login", "POST", payload),
      );
    } finally {
      payload.password = "";
    }
    await loadSessions();
  });
}
async function signInOidc() {
  clearSensitive();
  await perform(async () => {
    const result = await api.request<{ url: string }>("/auth/oidc", "POST", {});
    const destination = new URL(result.url, window.location.origin);
    if (!["http:", "https:"].includes(destination.protocol))
      throw new Error("登录地址无效，请联系管理员。");
    window.location.assign(destination.href);
  });
}
async function signOut() {
  await perform(async () => {
    await api.request("/auth/logout", "POST");
    clearIdentity();
    notice.value = "已退出当前 Web 会话。";
  });
}
async function loadSessions() {
  const version = identityVersion;
  sessionsLoading.value = true;
  try {
    const result = await api.request<{ sessions: SessionInfo[] }>("/sessions");
    if (version === identityVersion) sessions.value = result.sessions;
  } finally {
    if (version === identityVersion) sessionsLoading.value = false;
  }
}
async function refreshSessions() {
  await perform(loadSessions);
}
async function revokeSession(session: SessionInfo, targetView = false) {
  if (
    !window.confirm(
      `确认撤销此${session.kind === "web" ? " Web" : "客户端"}会话？${session.current ? "当前页面将退出登录。" : "对应客户端账号需重新登录。"}`,
    )
  )
    return;
  await perform(async () => {
    await api.request(`/sessions/${encodeURIComponent(session.id)}`, "DELETE");
    if (session.current) {
      clearIdentity();
      notice.value = "当前会话已撤销，请重新登录。";
      return;
    }
    notice.value = "会话已撤销。";
    if (targetView && target.value) await loadTargetSessions(target.value);
    else await loadSessions();
  });
}
async function revokeAll() {
  if (
    !window.confirm(
      "确认退出本账号的全部 Web 和客户端会话？当前页面也将退出登录。",
    )
  )
    return;
  await perform(async () => {
    await api.request("/sessions", "DELETE");
    clearIdentity();
    notice.value = "全部会话已撤销，请重新登录。";
  });
}
async function changePassword() {
  const validation = passwordError(password.next);
  if (!password.current || validation || password.next !== password.confirm) {
    error.value = !password.current
      ? "请输入当前密码。"
      : validation || "两次新密码不一致。";
    return;
  }
  const payload = {
    current_password: password.current,
    new_password: password.next,
  };
  password.current = password.next = password.confirm = "";
  await perform(async () => {
    try {
      await api.request("/me/password", "POST", payload);
    } finally {
      payload.current_password = payload.new_password = "";
    }
    clearIdentity();
    notice.value = "密码已修改，全部会话已失效，请使用新密码登录。";
  });
}
async function loadUsers() {
  const version = identityVersion;
  usersLoading.value = true;
  try {
    const result = await api.request<{ users: User[] }>("/users");
    if (version === identityVersion && isAdmin.value)
      users.value = result.users;
  } finally {
    if (version === identityVersion) usersLoading.value = false;
  }
}
async function openUsers() {
  tab.value = "users";
  await perform(loadUsers);
}
async function createUser() {
  const validation =
    usernameError(create.username) || passwordError(create.password);
  if (validation) {
    error.value = validation;
    return;
  }
  const payload = {
    username: create.username,
    password: create.password,
    role: create.role,
  };
  create.password = "";
  await perform(async () => {
    try {
      await api.request("/users", "POST", payload);
    } finally {
      payload.password = "";
    }
    create.username = "";
    notice.value = "本地账号已创建。";
    await loadUsers();
  });
}
async function updateUser(
  item: User,
  patch: { disabled?: boolean; role?: string },
) {
  const operation =
    patch.disabled !== undefined
      ? patch.disabled
        ? "停用"
        : "启用"
      : `切换为${patch.role === "admin" ? "管理员" : "成员"}`;
  if (
    !window.confirm(
      `确认${operation}账号「${item.username}」？${patch.disabled ? "已有会话将失效。" : ""}`,
    )
  )
    return;
  await perform(async () => {
    const result = await api.request<{ user: User }>(
      `/users/${encodeURIComponent(item.id)}`,
      "PATCH",
      patch,
    );
    if (item.id === user.value?.id) {
      user.value = result.user;
      if (result.user.disabled) {
        clearIdentity();
        notice.value = "账号已停用，已退出登录。";
        return;
      }
      if (result.user.role !== "admin") {
        clearIdentity();
        notice.value = "角色已更新，全部会话已失效，请重新登录。";
        return;
      }
    }
    notice.value = "账号已更新。";
    await loadUsers();
  });
}
async function loadTargetSessions(item: User) {
  const version = identityVersion;
  const selection = targetVersion;
  targetLoading.value = true;
  try {
    const result = await api.request<{ sessions: SessionInfo[] }>(
      `/users/${encodeURIComponent(item.id)}/sessions`,
    );
    if (
      version === identityVersion &&
      selection === targetVersion &&
      target.value?.id === item.id
    )
      targetSessions.value = result.sessions;
  } finally {
    if (selection === targetVersion) targetLoading.value = false;
  }
}
async function openTarget(item: User) {
  closeTarget();
  target.value = item;
  await perform(() => loadTargetSessions(item));
}
async function revokeTargetAll() {
  const item = target.value;
  if (
    !item ||
    !window.confirm(
      `确认撤销「${item.username}」的全部会话？对应 Web 和客户端均需重新登录。`,
    )
  )
    return;
  await perform(async () => {
    await api.request(
      `/users/${encodeURIComponent(item.id)}/sessions`,
      "DELETE",
    );
    if (item.id === user.value?.id) {
      clearIdentity();
      notice.value = "全部会话已撤销，请重新登录。";
    } else {
      notice.value = "目标账号的全部会话已撤销。";
      await loadTargetSessions(item);
    }
  });
}
async function resetTargetPassword() {
  const item = target.value;
  if (busy.value || !item || item.source !== "local") return;
  const selection = targetVersion;
  clearResetFeedback();
  const validation = passwordError(resetPassword.value);
  if (validation || resetPassword.value !== resetConfirm.value) {
    resetError.value = validation || "两次新密码不一致。";
    return;
  }
  if (
    !window.confirm(
      `确认重置「${item.username}」的密码？该账号的全部会话将失效。`,
    )
  )
    return;
  const payload = { new_password: resetPassword.value };
  resetPassword.value = resetConfirm.value = "";
  busy.value = true;
  error.value = "";
  notice.value = "";
  try {
    try {
      await api.request(
        `/users/${encodeURIComponent(item.id)}/password`,
        "POST",
        payload,
      );
    } finally {
      payload.new_password = "";
    }
    if (item.id === user.value?.id) {
      clearIdentity();
      notice.value = "密码已重置，请使用新密码登录。";
    } else if (selection === targetVersion && target.value?.id === item.id) {
      resetNotice.value = "密码已重置，目标账号的全部会话已失效。";
      await loadTargetSessions(item);
    }
  } catch (reason) {
    if (selection === targetVersion && target.value?.id === item.id) {
      resetError.value = resetNotice.value
        ? `会话列表刷新失败：${message(reason)}`
        : message(reason);
    } else if (reason instanceof ApiError && reason.status === 401) {
      error.value = message(reason);
    }
  } finally {
    busy.value = false;
  }
}
function roleName(role: string) {
  return role === "admin"
    ? "管理员"
    : role === "member"
      ? "成员"
      : "未分配角色";
}
function formatDate(seconds: number) {
  return Number.isFinite(seconds)
    ? new Date(seconds * 1000).toLocaleString("zh-CN", { hour12: false })
    : "—";
}
function sessionState(item: SessionInfo) {
  return item.revoked
    ? "已撤销"
    : item.expires_at <= Date.now() / 1000
      ? "已过期"
      : "有效";
}
function revocable(item: SessionInfo) {
  return !item.revoked && item.expires_at > Date.now() / 1000;
}

onMounted(() => {
  const url = new URL(window.location.href);
  const oidcFailed = url.searchParams.get("login") === "failed";
  if (oidcFailed) {
    url.searchParams.delete("login");
    window.history.replaceState(null, "", url.pathname + url.search + url.hash);
  }
  void initialize().then(() => {
    if (oidcFailed) {
      error.value = [
        "公司账号登录失败，请重试或联系管理员。",
        error.value,
      ].filter(Boolean).join(" ");
    }
  });
});
</script>

<template>
  <div class="app-shell">
    <header class="site-header">
      <a class="brand" href="/admin/" aria-label="RustDesk 账号管理首页"
        ><span class="brand-mark" aria-hidden="true">R</span
        ><span>RustDesk <small>账号管理</small></span></a
      >
      <div v-if="user" class="header-user">
        <span>{{ user.display_name || user.username }}</span
        ><button class="button ghost" :disabled="busy" @click="signOut">
          退出登录
        </button>
      </div>
      <span v-else class="header-label">安全访问 · 统一身份</span>
    </header>

    <main>
      <div v-if="error" class="banner error" role="alert">
        <strong>操作未完成</strong><span>{{ error }}</span
        ><button class="dismiss" aria-label="关闭错误提示" @click="error = ''">
          ×
        </button>
      </div>
      <div v-if="notice" class="banner notice" role="status">
        <span>{{ notice }}</span
        ><button class="dismiss" aria-label="关闭提示" @click="notice = ''">
          ×
        </button>
      </div>
      <div v-if="busy" class="operation-status" role="status">
        正在处理，请稍候…
      </div>
      <div v-if="booting" class="empty panel" role="status">
        正在检查登录状态…
      </div>

      <section v-else-if="!user" class="login-layout">
        <div class="login-intro">
          <span class="eyebrow">ACCOUNT CENTER</span>
          <h1>管理你的<br />账号与登录会话。</h1>
          <p>
            查看账号信息，<br />在不同设备间管理账号与登录会话。
          </p>
          <div class="intro-note">
            <span class="status-dot"></span>账号与会话统一管理
          </div>
        </div>
        <div class="panel login-panel">
          <span class="eyebrow">欢迎回来</span>
          <h2>登录账号</h2>
          <p class="muted">使用本地账号，或通过公司身份平台登录。</p>
          <form @submit.prevent="signIn">
            <fieldset :disabled="busy">
              <label
                >用户名<input
                  v-model="login.username"
                  name="username"
                  autocomplete="username"
                  required
                  autofocus
                  placeholder="输入本地用户名" /></label
              ><label
                >密码<input
                  v-model="login.password"
                  name="password"
                  type="password"
                  autocomplete="current-password"
                  required
                  placeholder="输入密码" /></label
              ><button class="button primary full" type="submit">
                {{ busy ? "正在处理…" : "本地账号登录" }}
              </button>
            </fieldset>
          </form>
          <template v-if="oidcEnabled"
            ><div class="divider">或</div>
            <button
              class="button secondary full"
              :disabled="busy"
              @click="signInOidc"
            >
              使用公司账号登录 <span aria-hidden="true">↗</span>
            </button></template
          >
          <p v-if="!optionsLoaded" class="helper">
            暂时无法获取公司登录配置。<button
              class="text-button"
              :disabled="busy"
              @click="perform(loadOptions)"
            >
              重新加载
            </button>
          </p>
          <p class="helper login-help">
            公司账号的密码和角色由公司身份平台（IdP）管理。
          </p>
        </div>
      </section>

      <template v-else>
        <div class="page-heading">
          <div>
            <span class="eyebrow">ACCOUNT CENTER</span>
            <h1>{{ tab === "account" ? "我的账号" : tab === "devices" ? (isAdmin ? "设备管理" : "我的设备") : tab === "users" ? "用户管理" : "审计记录" }}</h1>
            <p class="muted">
              {{
                tab === "account"
                  ? "管理个人身份与登录会话。"
                  : tab === "devices"
                    ? "查看关联设备与账号归属。"
                  : tab === "users"
                    ? "维护本地账号，控制账号访问与会话。"
                    : "查看账号操作与登录结果。"
              }}
            </p>
          </div>
          <span class="badge" :class="isAdmin ? 'accent' : ''">{{
            roleName(user.role)
          }}</span>
        </div>
        <nav class="tabs" aria-label="账号管理导航">
          <button
            :class="{ active: tab === 'account' }"
            @click="tab = 'account'"
          >
            我的账号</button>
          <button :class="{ active: tab === 'devices' }" :disabled="busy" @click="tab = 'devices'">{{ isAdmin ? "设备管理" : "我的设备" }}</button>
          <button
            v-if="isAdmin"
            :class="{ active: tab === 'users' }"
            :disabled="busy"
            @click="openUsers"
          >
            用户管理
          </button>
          <button
            v-if="isAdmin"
            :class="{ active: tab === 'audit' }"
            :disabled="busy"
            @click="tab = 'audit'"
          >
            审计记录
          </button>
        </nav>

        <AuditPanel v-if="tab === 'audit' && isAdmin" :key="user.id" :api="api" @forbidden="auditForbidden" />
        <DevicePanel v-if="tab === 'devices'" :key="user.id" :api="api" :user="user" @forbidden="auditForbidden" />

        <template v-if="tab === 'account'">
          <div class="account-grid">
            <section class="panel">
              <div class="section-heading">
                <h2>身份信息</h2>
                <span
                  class="badge"
                  :class="user.disabled ? 'danger' : 'success'"
                  >{{ user.disabled ? "已停用" : "正常" }}</span
                >
              </div>
              <dl class="identity-list">
                <div>
                  <dt>显示名称</dt>
                  <dd>{{ user.display_name || "未设置" }}</dd>
                </div>
                <div>
                  <dt>用户名</dt>
                  <dd>{{ user.username }}</dd>
                </div>
                <div>
                  <dt>账号 ID</dt>
                  <dd class="mono">{{ user.id }}</dd>
                </div>
                <div>
                  <dt>角色</dt>
                  <dd>{{ roleName(user.role) }}</dd>
                </div>
                <div>
                  <dt>账号来源</dt>
                  <dd>
                    {{ user.source === "local" ? "本地账号" : "公司 OIDC" }}
                  </dd>
                </div>
              </dl>
            </section>
            <section class="panel">
              <h2>账号密码</h2>
              <template v-if="user.source === 'local'"
                ><p class="muted small">
                  修改后将退出本账号的全部 Web 和客户端会话。
                </p>
                <form @submit.prevent="changePassword">
                  <fieldset :disabled="busy">
                    <label
                      >当前密码<input
                        v-model="password.current"
                        type="password"
                        autocomplete="current-password"
                        required
                    /></label>
                    <div class="two-columns">
                      <label
                        >新密码<input
                          v-model="password.next"
                          type="password"
                          autocomplete="new-password"
                          required /></label
                      ><label
                        >确认新密码<input
                          v-model="password.confirm"
                          type="password"
                          autocomplete="new-password"
                          required
                      /></label>
                    </div>
                    <p class="helper">密码须为 12–4096 字节。</p>
                    <button class="button secondary" type="submit">
                      修改密码
                    </button>
                  </fieldset>
                </form></template
              >
              <div v-else class="idp-note">
                <span class="badge accent">公司身份平台</span>
                <p>
                  此账号通过 OIDC
                  登录。密码和角色由公司身份平台（IdP）管理，请联系公司管理员修改。
                </p>
              </div>
            </section>
          </div>
          <section class="panel sessions-panel">
            <div class="section-heading wrap">
              <div>
                <h2>
                  我的会话 <span class="count">{{ activeSessions }}</span>
                </h2>
                <p class="muted small">不认识的会话可以立即撤销。</p>
              </div>
              <div class="actions">
                <button
                  class="button ghost"
                  :disabled="busy || sessionsLoading"
                  @click="refreshSessions"
                >
                  刷新</button
                ><button
                  class="button danger-outline"
                  :disabled="busy"
                  @click="revokeAll"
                >
                  全部退出
                </button>
              </div>
            </div>
            <p v-if="sessionsLoading" class="empty" role="status">
              正在加载会话…
            </p>
            <p v-else-if="!sessions.length" class="empty">暂无会话记录。</p>
            <div v-else class="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>会话</th>
                    <th>创建时间</th>
                    <th>到期时间</th>
                    <th>状态</th>
                    <th class="right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="item in sessions" :key="item.id">
                    <td>
                      <strong>{{
                        item.kind === "web" ? "Web 浏览器" : "RustDesk 客户端"
                      }}</strong
                      ><span v-if="item.current" class="badge accent tiny"
                        >当前</span
                      ><span class="table-id mono">{{ item.id }}</span>
                    </td>
                    <td>{{ formatDate(item.created_at) }}</td>
                    <td>{{ formatDate(item.expires_at) }}</td>
                    <td>{{ sessionState(item) }}</td>
                    <td class="right">
                      <button
                        class="text-button danger-text"
                        :disabled="busy || !revocable(item)"
                        @click="revokeSession(item)"
                      >
                        撤销
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>

        <template v-if="tab === 'users' && isAdmin">
          <section class="panel create-panel">
            <div>
              <h2>创建本地账号</h2>
              <p class="muted small">密码和用户名规则由后端最终校验。</p>
            </div>
            <form @submit.prevent="createUser">
              <fieldset class="create-fields" :disabled="busy">
                <label
                  >用户名<input
                    v-model="create.username"
                    autocomplete="off"
                    required
                    placeholder="无首尾空格" /></label
                ><label
                  >初始密码<input
                    v-model="create.password"
                    type="password"
                    autocomplete="new-password"
                    required
                    placeholder="12–4096 字节" /></label
                ><label
                  >角色<select v-model="create.role">
                    <option value="member">成员</option>
                    <option value="admin">管理员</option>
                  </select></label
                ><button class="button primary" type="submit">创建账号</button>
              </fieldset>
            </form>
          </section>
          <section class="panel">
            <div class="section-heading wrap">
              <div>
                <h2>
                  用户列表 <span class="count">{{ users.length }}</span>
                </h2>
                <p class="muted small">
                  最后一个可用本地管理员的保护由后端执行。
                </p>
              </div>
              <div class="actions">
                <label class="search-label"
                  ><span class="sr-only">搜索用户</span
                  ><input
                    v-model="search"
                    type="search"
                    placeholder="搜索用户名、名称或 ID" /></label
                ><button
                  class="button ghost"
                  :disabled="busy || usersLoading"
                  @click="perform(loadUsers)"
                >
                  刷新
                </button>
              </div>
            </div>
            <p v-if="usersLoading" class="empty" role="status">正在加载用户…</p>
            <p v-else-if="!filteredUsers.length" class="empty">
              {{ search ? "没有匹配的用户。" : "暂无用户。" }}
            </p>
            <div v-else class="table-scroll">
              <table class="users-table">
                <thead>
                  <tr>
                    <th>用户</th>
                    <th>来源</th>
                    <th>角色</th>
                    <th>状态 / 准入</th>
                    <th class="right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="item in filteredUsers" :key="item.id">
                    <td>
                      <strong>{{ item.display_name || item.username }}</strong
                      ><span v-if="item.id === user.id" class="badge tiny"
                        >本人</span
                      ><span class="table-id">{{ item.username }}</span
                      ><span class="table-id mono">{{ item.id }}</span>
                    </td>
                    <td>
                      {{ item.source === "local" ? "本地" : "公司 OIDC" }}
                    </td>
                    <td>
                      <span>{{ roleName(item.role) }}</span
                      ><span v-if="item.source === 'oidc'" class="table-id"
                        >由 IdP 管理</span
                      ><button
                        v-else
                        class="text-button role-switch"
                        :disabled="busy"
                        @click="
                          updateUser(item, {
                            role: item.role === 'admin' ? 'member' : 'admin',
                          })
                        "
                      >
                        切换为{{ item.role === "admin" ? "成员" : "管理员" }}
                      </button>
                    </td>
                    <td>
                      <span
                        class="badge"
                        :class="item.disabled ? 'danger' : 'success'"
                        >{{ item.disabled ? "已停用" : "正常" }}</span
                      ><span
                        v-if="item.admitted !== undefined"
                        class="table-id"
                        >{{ item.admitted ? "已准入" : "未准入" }}</span
                      >
                    </td>
                    <td class="right">
                      <div class="row-actions">
                        <button
                          class="text-button"
                          :disabled="busy"
                          @click="openTarget(item)"
                        >
                          管理</button
                        ><button
                          class="text-button"
                          :class="{ 'danger-text': !item.disabled }"
                          :disabled="busy"
                          @click="
                            updateUser(item, { disabled: !item.disabled })
                          "
                        >
                          {{ item.disabled ? "启用" : "停用" }}
                        </button>
                      </div>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>

          <section
            v-if="target"
            class="panel target-panel"
            aria-labelledby="target-title"
          >
            <div class="section-heading wrap">
              <div>
                <span class="eyebrow">账号详情</span>
                <h2 id="target-title">
                  {{ target.display_name || target.username }}
                </h2>
                <p class="muted small mono">{{ target.id }}</p>
              </div>
              <button
                class="button ghost"
                :disabled="busy"
                @click="closeTarget"
              >
                关闭详情
              </button>
            </div>
            <div v-if="target.source === 'local'" class="reset-section">
              <h3>重置本地密码</h3>
              <p class="muted small">重置后，该账号的全部会话将失效。</p>
              <form @submit.prevent="resetTargetPassword">
                <fieldset class="reset-fields" :disabled="busy">
                  <label
                    >新密码<input
                      v-model="resetPassword"
                      type="password"
                      autocomplete="new-password"
                      aria-describedby="reset-password-help"
                      @input="clearResetFeedback"
                      required /></label
                  ><label
                    >确认新密码<input
                      v-model="resetConfirm"
                      type="password"
                      autocomplete="new-password"
                      aria-describedby="reset-password-help"
                      @input="clearResetFeedback"
                      required /></label
                  ><button class="button secondary" type="submit">
                    重置密码
                  </button>
                </fieldset>
                <p id="reset-password-help" class="helper">
                  密码须为 12–4096 字节，两次输入须一致。
                </p>
                <div
                  v-if="resetError"
                  class="banner error"
                  role="alert"
                >
                  <span>{{ resetError }}</span>
                </div>
                <div v-if="resetNotice" class="banner notice" role="status">
                  <span>{{ resetNotice }}</span>
                </div>
              </form>
            </div>
            <p v-else class="idp-note">
              公司账号的密码和角色由 IdP 管理，请到公司身份平台修改。
            </p>
            <div class="section-heading wrap">
              <h3>目标账号会话</h3>
              <div class="actions">
                <button
                  class="button ghost"
                  :disabled="busy || targetLoading"
                  @click="perform(() => loadTargetSessions(target!))"
                >
                  刷新</button
                ><button
                  class="button danger-outline"
                  :disabled="busy"
                  @click="revokeTargetAll"
                >
                  撤销全部会话
                </button>
              </div>
            </div>
            <p v-if="targetLoading" class="empty" role="status">
              正在加载会话…
            </p>
            <p v-else-if="!targetSessions.length" class="empty">
              暂无会话记录。
            </p>
            <div v-else class="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>会话</th>
                    <th>创建时间</th>
                    <th>到期时间</th>
                    <th>状态</th>
                    <th class="right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="item in targetSessions" :key="item.id">
                    <td>
                      <strong>{{
                        item.kind === "web" ? "Web 浏览器" : "RustDesk 客户端"
                      }}</strong
                      ><span v-if="item.current" class="badge accent tiny"
                        >当前</span
                      ><span class="table-id mono">{{ item.id }}</span>
                    </td>
                    <td>{{ formatDate(item.created_at) }}</td>
                    <td>{{ formatDate(item.expires_at) }}</td>
                    <td>{{ sessionState(item) }}</td>
                    <td class="right">
                      <button
                        class="text-button danger-text"
                        :disabled="busy || !revocable(item)"
                        @click="revokeSession(item, true)"
                      >
                        撤销
                      </button>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>
      </template>
    </main>
    <footer>RustDesk · 账号与访问管理</footer>
  </div>
</template>
