import type { AccountApi, AuditEvent, AuditFilter } from "./api";

export interface AuditFeedState {
  events: AuditEvent[];
  nextBeforeId: number | null;
  loading: boolean;
  error: string;
}

// State is supplied by the view so request generations also work with Vue proxies.
export function createAuditFeed(
  api: Pick<AccountApi, "listAuditEvents">,
  state: AuditFeedState,
  onForbidden: () => void = () => {},
) {
  let generation = 0;
  let filter: AuditFilter = {};
  function clear() {
    generation++;
    state.events = [];
    state.nextBeforeId = null;
    state.loading = false;
    state.error = "";
  }
  async function load(append: boolean) {
    const version = generation;
    state.loading = true;
    state.error = "";
    const query = { ...filter };
    if (append && state.nextBeforeId !== null) query.before_id = state.nextBeforeId;
    try {
      const page = await api.listAuditEvents(query);
      if (version !== generation) return;
      state.events = append ? [...state.events, ...page.events] : page.events;
      state.nextBeforeId = page.next_before_id;
    } catch (reason) {
      if (version !== generation) return;
      const status = reason && typeof reason === "object" && "status" in reason
        ? reason.status : undefined;
      if (status === 401 || status === 403) clear();
      state.error = reason instanceof Error ? reason.message : "审计记录加载失败，请重试。";
      if (status === 403) onForbidden();
    } finally {
      if (version === generation) state.loading = false;
    }
  }
  return {
    clear,
    reset(next: AuditFilter) {
      clear();
      filter = { ...next };
      return load(false);
    },
    loadMore() {
      if (state.loading || state.nextBeforeId === null) return Promise.resolve();
      return load(true);
    },
  };
}
