<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, watch } from "vue";
import type { AccountApi, User } from "./api";
import { createDeviceManager } from "./devices";
import type { Device, DeviceState } from "./devices";

const props = defineProps<{ api: AccountApi; user: User }>();
const emit = defineEmits<{ forbidden: [] }>();
const state = reactive<DeviceState>({ devices: [], users: [], owners: {}, loading: false, savingId: "", error: "", notice: "" });
const manager = createDeviceManager(props.api, state, () => emit("forbidden"));
const isAdmin = computed(() => props.user.role === "admin");
const pending = computed(() => state.loading || !!state.savingId);
const accounts = computed(() => [...state.users].sort((a, b) => a.username.localeCompare(b.username, "zh-CN")));

function ownerName(device: Device) {
  if (!device.owner_user_id) return "未归属";
  return device.owner_username || device.owner_user_id;
}
function formatDate(timestamp: number) {
  const date = new Date(timestamp * 1000);
  return timestamp > 0 && Number.isFinite(date.getTime())
    ? date.toLocaleString("zh-CN", { hour12: false }) : "—";
}
function save(device: Device) {
  void manager.save(device.id, state.owners[device.id] || null);
}

watch(() => [props.user.id, props.user.role, props.user.disabled, props.user.admitted], () => { void manager.reset(props.user); }, { immediate: true, flush: "sync" });
onBeforeUnmount(manager.clear);
</script>

<template>
  <section class="panel device-panel" aria-labelledby="devices-title">
    <div class="section-heading wrap">
      <div>
        <h2 id="devices-title">{{ isAdmin ? "设备管理" : "我的设备" }} <span class="count">{{ state.devices.length }}</span></h2>
        <p class="muted small">设备归属用于账号下的设备管理；连接仍按被控端设置验证。</p>
      </div>
      <button class="button ghost" :disabled="pending" @click="manager.refresh">{{ state.loading ? "正在刷新…" : "刷新" }}</button>
    </div>
    <p v-if="state.error" class="banner error" role="alert">{{ state.error }}</p>
    <p v-if="state.notice" class="banner notice" role="status">{{ state.notice }}</p>
    <p v-if="state.loading && !state.devices.length" class="empty" role="status">正在加载设备…</p>
    <p v-else-if="!state.devices.length && !state.error" class="empty">在 RustDesk 客户端登录账号后，会显示关联设备。</p>
    <div v-if="state.devices.length" class="table-scroll">
      <table class="devices-table">
        <thead><tr><th>设备</th><th>平台</th><th>归属账号</th><th>最近登录时间</th><th v-if="isAdmin">调整归属</th></tr></thead>
        <tbody>
          <tr v-for="device in state.devices" :key="device.id">
            <td><strong>{{ device.device_name || "未命名设备" }}</strong><span class="table-id mono">{{ device.rustdesk_id }}</span></td>
            <td>{{ device.platform || "—" }}</td>
            <td><span>{{ ownerName(device) }}</span><span v-if="device.owner_user_id === user.id" class="badge tiny">本人</span></td>
            <td>{{ formatDate(device.last_login_at) }}</td>
            <td v-if="isAdmin">
              <form class="owner-form" @submit.prevent="save(device)">
                <label class="owner-select">
                  <span class="sr-only">{{ device.device_name || device.rustdesk_id }}的归属账号</span>
                  <select v-model="state.owners[device.id]" :disabled="pending">
                    <option value="">解除归属</option>
                    <option v-if="device.owner_user_id && !state.users.some(account => account.id === device.owner_user_id)" :value="device.owner_user_id">{{ ownerName(device) }}（当前归属）</option>
                    <option v-for="account in accounts" :key="account.id" :value="account.id">{{ account.username }}{{ account.disabled ? "（已停用）" : "" }}</option>
                  </select>
                </label>
                <button class="button secondary" type="submit" :disabled="pending || state.owners[device.id] === (device.owner_user_id || '')">{{ state.savingId === device.id ? "正在保存…" : "保存" }}</button>
              </form>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p class="helper device-note">首次登录会关联新设备；退出账号后保留归属。{{ isAdmin ? "已有设备可在此调整或解除归属。" : "已有设备需要调整归属时，请联系管理员。" }}</p>
  </section>
</template>

<style scoped>
.devices-table { min-width: 650px; }
.devices-table td { overflow-wrap: anywhere; }
.owner-form { display: flex; align-items: center; gap: 8px; }
.owner-select { min-width: 150px; max-width: 230px; flex: 1; }
.owner-select select, .owner-form .button { margin-top: 0; }
.device-note { margin-top: 20px; }
@media (max-width: 700px) { .owner-select { min-width: 130px; } }
</style>
