<script setup lang="ts">
import { onBeforeUnmount, onMounted, reactive, watch } from "vue";
import type { AccountApi, AuditAction, AuditFilter, AuditResult, AuditSource } from "./api";
import { createAuditFeed } from "./audit";
import type { AuditFeedState } from "./audit";

const props = defineProps<{ api: AccountApi }>();
const emit = defineEmits<{ forbidden: [] }>();
const state = reactive<AuditFeedState>({ events: [], nextBeforeId: null, loading: false, error: "" });
const form = reactive({ source: "" as AuditSource | "", result: "" as AuditResult | "", action: "" as AuditAction | "", target: "", since: "", until: "" });
const feed = createAuditFeed(props.api, state, () => emit("forbidden"));
const actions: Record<AuditAction, string> = {
  create_user: "创建账号", bootstrap: "初始化管理员", update_user: "更新账号",
  change_password: "修改密码", reset_password: "重置密码", oidc_jit: "公司账号首次准入",
  oidc_mapping: "公司账号映射", policy_change: "配置变更", login: "登录",
  logout: "退出登录", revoke_session: "撤销会话", revoke_all: "撤销全部会话", prune_audit: "清理审计记录",
};
const sources: Record<AuditSource, string> = { local: "本地", oidc: "公司 OIDC", system: "系统" };
const kinds = { client: "客户端", web: "Web", cli: "命令行", unknown: "未知" };
const reasons: Record<string, string> = {
  invalid_request: "请求无效", rate_limited: "请求过于频繁", invalid_origin: "来源校验失败",
  oidc_state: "登录状态校验失败", oidc_provider: "身份平台异常", oidc_verification: "身份校验失败",
  oidc_expired: "登录流程已过期", session_denied: "会话被拒绝", invalid_credentials: "凭据无效",
};
function applyFilters() {
  feed.clear();
  const since = form.since ? Math.floor(new Date(form.since).getTime() / 1000) : undefined;
  const until = form.until ? Math.floor(new Date(form.until).getTime() / 1000) : undefined;
  if ((since !== undefined && !Number.isFinite(since)) || (until !== undefined && !Number.isFinite(until)) || (since !== undefined && until !== undefined && since > until)) {
    state.error = "请选择有效的时间范围，开始时间不能晚于结束时间。";
    return;
  }
  const filter: AuditFilter = { limit: 50, since, until };
  if (form.source) filter.source = form.source;
  if (form.result) filter.result = form.result;
  if (form.action) filter.action = form.action;
  if (form.target.trim()) filter.target_id = form.target.trim();
  void feed.reset(filter);
}
function resetFilters() {
  Object.assign(form, { source: "", result: "", action: "", target: "", since: "", until: "" });
  applyFilters();
}
function role(value: string | null) {
  return value === "admin" ? "管理员" : value === "member" ? "成员" : value === "none" ? "无角色" : "—";
}
function flag(value: boolean | null, yes: string, no: string) {
  return value === null ? "—" : value ? yes : no;
}
function date(timestamp: number) {
  return new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false });
}
// Editing a filter invalidates the current page immediately, including in-flight requests.
watch(form, () => feed.clear(), { flush: "sync" });
onMounted(applyFilters);
onBeforeUnmount(feed.clear);
</script>

<template>
  <section class="panel audit-panel" aria-labelledby="audit-title">
    <div class="section-heading wrap">
      <div><h2 id="audit-title">账号审计记录</h2><p class="muted small">按发生时间倒序显示账号与登录操作，时间使用本机时区。</p></div>
      <span class="badge">仅管理员可查看</span>
    </div>
    <form class="audit-form" @submit.prevent="applyFilters">
      <div class="audit-filters">
        <label>来源<select v-model="form.source"><option value="">全部来源</option><option v-for="(label, value) in sources" :key="value" :value="value">{{ label }}</option></select></label>
        <label>结果<select v-model="form.result"><option value="">全部结果</option><option value="success">成功</option><option value="failure">失败</option></select></label>
        <label>操作<select v-model="form.action"><option value="">全部操作</option><option v-for="(label, value) in actions" :key="value" :value="value">{{ label }}</option></select></label>
        <label>目标账号 ID<input v-model="form.target" placeholder="输入完整账号 ID" maxlength="128" /></label>
        <label>开始时间<input v-model="form.since" type="datetime-local" /></label>
        <label>结束时间<input v-model="form.until" type="datetime-local" /></label>
      </div>
      <div class="actions"><button class="button primary" type="submit">查询</button><button class="button ghost" type="button" @click="resetFilters">重置筛选</button></div>
    </form>
    <p v-if="state.error" class="banner error audit-error" role="alert">{{ state.error }}</p>
    <p v-if="state.loading && !state.events.length" class="empty" role="status">正在加载审计记录…</p>
    <p v-else-if="!state.events.length && !state.error" class="empty">暂无记录；可调整筛选条件后查询。</p>
    <ol v-if="state.events.length" class="audit-events">
      <li v-for="item in state.events" :key="item.id" class="audit-event">
        <div class="audit-event-heading">
          <div><strong>{{ actions[item.action] || item.action }}</strong><span class="badge" :class="item.result === 'success' ? 'success' : 'danger'">{{ item.result === "success" ? "成功" : "失败" }}</span></div>
          <time :datetime="new Date(item.timestamp * 1000).toISOString()">{{ date(item.timestamp) }}</time>
        </div>
        <dl class="audit-details">
          <div><dt>操作主体 ID</dt><dd class="mono">{{ item.actor_id || "—" }}</dd></div>
          <div><dt>目标账号 ID</dt><dd class="mono">{{ item.target_id || "—" }}</dd></div>
          <div><dt>来源 / 入口</dt><dd>{{ sources[item.source] }} / {{ kinds[item.kind] }}</dd></div>
          <div><dt>角色变化</dt><dd>{{ role(item.before_role) }} → {{ role(item.after_role) }}</dd></div>
          <div><dt>准入变化</dt><dd>{{ flag(item.before_admitted, "已准入", "未准入") }} → {{ flag(item.after_admitted, "已准入", "未准入") }}</dd></div>
          <div><dt>停用变化</dt><dd>{{ flag(item.before_disabled, "已停用", "正常") }} → {{ flag(item.after_disabled, "已停用", "正常") }}</dd></div>
          <div><dt>撤销会话数</dt><dd>{{ item.revoked_count }}</dd></div>
          <div><dt>配置版本</dt><dd class="mono">{{ item.policy_version || "—" }}</dd></div>
          <div v-if="item.reason"><dt>原因</dt><dd>{{ reasons[item.reason] || item.reason }}</dd></div>
        </dl>
      </li>
    </ol>
    <div v-if="state.events.length" class="audit-pagination">
      <span class="muted small">已显示 {{ state.events.length }} 条</span>
      <button v-if="state.nextBeforeId !== null" class="button secondary" :disabled="state.loading" @click="feed.loadMore">{{ state.loading ? "正在加载…" : "加载更多" }}</button>
      <span v-else class="muted small">已加载全部匹配记录</span>
    </div>
  </section>
</template>

<style scoped>
.audit-filters { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 16px; }
.audit-filters label + label { margin-top: 0; }
.audit-form { padding-bottom: 24px; border-bottom: 1px solid #e6ede7; }
.audit-error { margin-top: 24px; }
.audit-events { list-style: none; padding: 0; margin: 0; }
.audit-event { padding: 24px 0; border-bottom: 1px solid #e6ede7; }
.audit-event-heading { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 12px; }
.audit-event-heading .badge { margin-left: 10px; }
.audit-event-heading time { color: #73817b; font-size: 12px; }
.audit-details { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 16px; margin: 20px 0 0; }
.audit-details dt { color: #829087; font-size: 12px; }
.audit-details dd { margin: 4px 0 0; font-size: 13px; overflow-wrap: anywhere; }
.audit-pagination { display: flex; justify-content: space-between; align-items: center; gap: 12px; padding-top: 20px; }
@media (max-width: 950px) { .audit-filters, .audit-details { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@media (max-width: 700px) { .audit-filters, .audit-details { grid-template-columns: 1fr; } .audit-pagination { flex-wrap: wrap; } }
</style>
