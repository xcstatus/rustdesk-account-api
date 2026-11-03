# M1 账号管理闭环

- 日期：2026-10-31；`document_status: drafted`；`m1_validation_status: passed`。
- 负责人及最终确认：项目发起人；实现、测试与证据整理：Codex。
- 来源：E-14；权威需求见[PRD](PRD.md) RD-002～RD-006、RD-011、RD-017～RD-019及相关管理入口。
- 长期归属：本项目`docs/`；代码：`api/`、`web/`；可复现资产：`validation/`。

M0提供官方客户端兼容基线，本轮补齐账号管理与基础Web入口。完整账号审计、生产TLS/反向代理信任、容量和备份恢复验收仍属后续阶段，服务保持loopback监听。

| 用例 | 验收范围 | 验证方式 |
|---|---|---|
| M1-01 初始化 | 无固定密码；bootstrap只成功一次；旧库管理员升级后不重新开放；可信CLI恢复独立说明 | 双库Store测试、CLI |
| M1-02 账号管理 | 管理员建号/启停/本地角色/重置；成员越权失败；最后本地管理员保护；OIDC密码/角色不被手工覆盖 | 双库Store与HTTP |
| M1-03 改密撤销 | 旧密码失败；所有旧客户端/Web会话失效；旧密码验证后延迟签发被拒；写入时复核当前actor | Store竞态回归、HTTP |
| M1-04 Web安全 | Cookie HttpOnly/SameSite/Secure策略；client/web用途隔离；Origin与CSRF；会话列表不泄露摘要；跨用户撤销失败 | Rust路由测试、HTTP与页面验证 |
| M1-05 Web OIDC | 真实Keycloak表单、state/nonce/PKCE与browser cookie绑定；回调签发Web会话；Web尝试不可客户端轮询；重放拒绝 | 单元测试、真实HTTP与页面验证 |
| M1-06 迁移与兼容 | SQLite/PG v1→v2保留用户/个人簿/旧client会话，回填公共UUID；原M0协议继续通过 | 双库Store测试、M0 HTTP回归 |
| M1-07 Web页面 | 中文账号/密码/会话页面；普通成员与管理员入口正确；失败反馈、401回登录；密码不留浏览器持久存储 | TypeScript、前端测试、构建与浏览器验证 |

Web使用同源`/admin/api/v1`与独立Cookie；OIDC与客户端共享同一提供方服务及回调，临时绑定Cookie覆盖`/api/oidc/callback`。默认Web会话8小时，修改密码撤销当前会话后须重新登录。管理员停用账号保留个人簿；重新启用OIDC只清除本地停用状态，需再次通过IdP认证与准入。

## 结果与证据

本轮开发与验证完成。脱敏用例、构建指纹、前端产物指纹和环境清理见[M1结果](../validation/results/m1-2026-10-31.json)；复现方式见[验证说明](../validation/README.md)。

| 检查 | 结果 |
|---|---|
| Rust测试（启用专用PostgreSQL的`DATABASE_URL`） | 19/19；含SQLite与PG的Store/管理合同、并发初始化、旧库迁移与权限撤销 |
| Clippy全目标、`-D warnings`；`cargo fmt --check` | 通过 |
| Vue/TypeScript | 前端4项测试、类型检查与生产构建通过；Node 22.22.1，依赖锁定 |
| M1 SQLite＋真实Keycloak 26.8.1 | 最终构建20/20 HTTP用例通过 |
| M1 PostgreSQL 16.15＋真实Keycloak | 最终构建20/20 HTTP用例通过 |
| M0协议回归 | 同一最终构建，两库各15/15 HTTP用例通过 |
| 一次性管理员CLI | 首次成功，第二次非零退出；不重新开放初始化 |
| 实际浏览器页面 | 6项检查通过；本地管理员登录/列表/目标会话面板、退出、真实Web OIDC、成员入口隔离、刷新保持及退出 |

浏览器由Codex通过CUA操作本机内置浏览器，连接独立SQLite测试API和真实Keycloak。管理员与OIDC成员各产生一个Web会话，最终均已撤销，浏览器测试库有效会话为0。账号/密码变更由HTTP和Rust套件验证，浏览器检查未执行密码修改或重置；详见[浏览器记录](../validation/results/m1-browser-2026-10-31.json)。原始管理页面及OIDC成员截图仅保留于本地`validation/results/`，不纳入GitHub仓库。

最终API SHA256为`9e2d0583916bb336c92f50fa73a896fa99f2b10ed185011efad7a3815cfd8b8d`。归档发现最早SQLite通过轮次的构建指纹不同，已用最终构建补跑20项；补跑首次因Keycloak重启后尚未就绪而启动失败，确认发现端点就绪后重试通过。两次未计入最终通过矩阵的记录和原因保留于结果JSON，不覆盖原始运行记录。

独立只读代码复核未发现已确认的P1/P2问题，重点检查事务内授权、改密后延迟签发、Web/client隔离、OIDC回调绑定、迁移保留和静态资源目录边界；该审查不替代上述运行验证。

本轮未再次操作RustDesk官方客户端；M0原实机证据保持历史归属，当前构建以两库HTTP协议回归补证。未扩大为Windows/Linux/移动端、生产IdP、生产TLS/容量或备份恢复验收。

测试API及`rustdesk-m0`项目的Keycloak/PostgreSQL已停止，确认21114、18080、15432三个loopback端口关闭；数据库、fixture和数据卷保留，未改RustDesk客户端配置或其他容器/虚拟机。当前仍为本机开发交付，账号审计与生产运维工作待后续完成。
