export interface User {
  id: string;
  username: string;
  display_name: string;
  role: "admin" | "member" | "";
  source: "local" | "oidc";
  disabled: boolean;
  admitted?: boolean;
}

export interface SessionInfo {
  id: string;
  user_id: string;
  kind: "client" | "web";
  created_at: number;
  expires_at: number;
  revoked: boolean;
  current: boolean;
}

export interface Identity {
  user: User;
  csrf_token: string;
}

export type AuditSource = "local" | "oidc" | "system";
export type AuditKind = "client" | "web" | "cli" | "unknown";
export type AuditResult = "success" | "failure";
export type AuditAction =
  | "create_user" | "bootstrap" | "update_user" | "change_password"
  | "reset_password" | "oidc_jit" | "oidc_mapping" | "policy_change"
  | "login" | "logout" | "revoke_session" | "revoke_all" | "prune_audit";
export interface AuditEvent {
  id: number;
  timestamp: number;
  actor_id: string | null;
  target_id: string | null;
  session_id: string | null;
  source: AuditSource;
  kind: AuditKind;
  action: AuditAction;
  result: AuditResult;
  reason: string | null;
  before_role: string | null;
  after_role: string | null;
  before_admitted: boolean | null;
  after_admitted: boolean | null;
  before_disabled: boolean | null;
  after_disabled: boolean | null;
  policy_version: string;
  revoked_count: number;
}
export interface AuditFilter {
  before_id?: number;
  limit?: number;
  since?: number;
  until?: number;
  actor_id?: string;
  target_id?: string;
  source?: AuditSource;
  kind?: AuditKind;
  action?: AuditAction;
  result?: AuditResult;
}
export interface AuditPage {
  events: AuditEvent[];
  next_before_id: number | null;
}

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

// HttpOnly cookies stay in the browser; the CSRF token lives only in this instance.
export class AccountApi {
  private csrfToken = "";
  private onUnauthorized: () => void;

  constructor(onUnauthorized: () => void) {
    this.onUnauthorized = onUnauthorized;
  }
  setIdentity(identity: Identity) {
    if (!identity.csrf_token || !identity.user?.id)
      throw new Error("服务端身份响应不完整，请重新登录。");
    this.csrfToken = identity.csrf_token;
  }
  clearIdentity() {
    this.csrfToken = "";
  }

  listAuditEvents(filter: AuditFilter): Promise<AuditPage> {
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(filter)) {
      if (value !== undefined && value !== "") query.set(key, String(value));
    }
    return this.request<AuditPage>(`/audit-events?${query.toString()}`);
  }

  async request<T>(path: string, method = "GET", body?: unknown): Promise<T> {
    const headers = new Headers({ Accept: "application/json" });
    if (body !== undefined) headers.set("Content-Type", "application/json");
    if (method !== "GET" && this.csrfToken)
      headers.set("X-CSRF-Token", this.csrfToken);
    let response: Response;
    try {
      response = await fetch(`/admin/api/v1${path}`, {
        method,
        headers,
        credentials: "same-origin",
        cache: "no-store",
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
    } catch {
      throw new Error("无法连接服务，请检查网络后重试。");
    }
    if (!response.ok) {
      if (response.status === 401) {
        this.clearIdentity();
        this.onUnauthorized();
      }
      let detail = "";
      try {
        const data: unknown = await response.json();
        if (data && typeof data === "object") {
          const value = data as Record<string, unknown>;
          if (typeof value.message === "string") detail = value.message;
          else if (typeof value.error === "string") detail = value.error;
        }
      } catch {
        /* An HTML or empty error response still gets a useful status message. */
      }
      const message =
        response.status === 403
          ? `权限不足${detail ? `：${detail}` : "，请联系管理员。"}`
          : response.status === 401
            ? "登录信息无效或会话已失效，请重新登录。"
            : detail || `操作失败（HTTP ${response.status}），请稍后重试。`;
      throw new ApiError(response.status, message);
    }
    if (response.status === 204) return undefined as T;
    try {
      return (await response.json()) as T;
    } catch {
      throw new Error("服务端返回了无法识别的数据，请刷新后重试。");
    }
  }
}

export function passwordError(password: string): string {
  const bytes = new TextEncoder().encode(password).length;
  return bytes < 12 || bytes > 4096
    ? "密码须为 12–4096 字节（中文字符通常占 3 字节）。"
    : "";
}

export function usernameError(username: string): string {
  if (!username || username !== username.trim())
    return "用户名不能为空或含首尾空格。";
  return new TextEncoder().encode(username).length > 128
    ? "用户名不能超过 128 字节。"
    : "";
}
