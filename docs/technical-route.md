# 技术路线与已采用决策

- `document_status: reviewed`；`technical_route_status: implemented_baseline`；整理：2026-11-03。
- 负责人：项目发起人；依据：[PRD](PRD.md)、当前manifest与锁文件、[调研](research.md)及2026-10-31验收记录。

## 项目定位

为单个团队或自己与朋友提供轻量自托管账号服务，兼容未修改的官方RustDesk客户端。通过API Server地址接入，独立提供账号、个人簿、统一认证和管理能力；远控仍通过RustDesk会合、中继与客户端完成。

账号API与管理Web独立开发。已拉取的官方与社区仓库用于核实兼容行为，未采用社区API整体裁剪路线。源码已公开，自有内容采用[非商业许可](../LICENSE)，第三方范围见[NOTICE](../NOTICE)；公共镜像、版本发布与对外托管服务另行安排。

## 已采用的决策

| 编号 | 决策 | 原因与代价 | 实现或证据 |
|---|---|---|---|
| T-01 | Rust + Tokio + Axum + SQLx | 用户确认Rust，采用异步HTTP模块化单体；兼容与迁移由本项目维护 | [Cargo.toml](../api/Cargo.toml)、[架构](architecture.md) |
| T-02 | Vue 3 + TypeScript + Vite | 独立管理页面，构建产物由API同源提供；开发需先准备Web产物 | [package.json](../web/package.json)、[Web说明](../web/README.md) |
| T-03 | 仅SQLite与PostgreSQL | 轻量单实例与已有PG两条部署路径；两套迁移保持业务一致 | [迁移](../api/migrations/)、RD-016 |
| T-04 | 单进程模块化单体 | 当前规模不引入微服务、Redis或消息队列；进程内状态限制横向扩展 | [架构](architecture.md)、[开发](development.md) |
| T-05 | 本地账号与一个OIDC提供方并存 | 本地应急管理员可恢复管理；OIDC按issuer+sub建档，避免同名身份误合并 | RD-017～019、[逻辑](logic.md) |
| T-06 | 单claim静态精确角色映射 | admin/member两种角色，命中admin优先；默认拒绝，可显式member兜底，复杂规则后移 | PRD D-10、[API配置](../api/README.md) |
| T-07 | 沿用官方个人簿与目录协议 | 当前使用Legacy整本快照回退；官方hash用于跨端凭据同步；需接受旧快照覆盖限制 | RD-007/020～022、[专项设计](device-credentials-design.md) |
| T-08 | 凭据服务端AES-256-GCM静态加密 | 避免数据库明文保存可用hash；API持钥可解密，恢复必须有对应密钥，不是端到端加密 | [credentials.rs](../api/src/credentials.rs)、[专项设计](device-credentials-design.md) |
| T-09 | Compose + HTTPS代理交付 | 通用配置支持Caddy；现有实例复用Traefik与共享PG，减少独立基础设施 | [运维](operations.md)、[部署包](../deploy/README.md) |
| T-10 | 固定PR #734构建hbbs | 修复官方1.5.0登录后安全握手互等；增加上游版本维护责任，保留hbbr及原身份 | [兼容记录](rendezvous-compatibility.md) |

T-*是技术决策索引，不替代PRD的需求编号。依赖精确版本以[Cargo.lock](../api/Cargo.lock)及[package-lock.json](../web/package-lock.json)为准，不以本文作为“最新版本”清单。

## 兼容路线

1. 固定目标客户端版本，读实际请求和解析方式，维护协议适配层。
2. 自动化证明协议、持久化与异常处理；官方客户端证明实际用户流程，两者分别留证。
3. 协议故障沿账号API、客户端、hbbs、hbbr和目标设备分层定位，不用伪造空响应或删除账号token掩盖不兼容。
4. 当前目标为官方1.5.0，双端人工范围见[支持矩阵](release-notes.md)。更新客户端或hbbs时按变更路径选检查，不默认扩大完整平台矩阵。

PR #734的选择依据是2026-10-31固定提交，不自动跟随远端PR或“latest”镜像。此次整理未查询它在2026-11-03的合并状态。初始调研仓库记录在`sources.lock.json`，实际PR部署版本记录在专项文档，两者用途不同。

## 保留与未采用的路线

| 路线 | 当前处理 |
|---|---|
| Go/Java后端或整体裁剪社区API | 前期已比较，Rust独立实现已定；原讨论保留在[归档](architecture-discussion-2026-10-31.md) |
| 多租户、MySQL、公开自主注册 | 不属于当前产品范围 |
| 按用户/用户组强制限制远控 | 用户已取消，不从“我的设备”或通讯录权限推导远控授权 |
| 团队共享通讯录 | 已确认放第二阶段，需另行设计共享协议与读写权限 |
| 多claim/表达式映射、LDAP、目录事件同步 | 后续候选，未承诺实现 |
| 独立hbbs握手补丁 | 已被T-10取代，不叠加到PR构建 |

## 演进顺序

近期维护先保持现有单团队闭环可重现：代码与文档纳入明确版本管理、保存构建输入、处理测试账号收尾与运维责任。下一阶段按实际需要接入生产OIDC、完善异地备份/告警，再进入已确认的团队共享通讯录。

以上是维护排序建议，不是新功能开发授权或工期承诺。具体待办、负责人和完成条件集中在[维护计划](maintenance.md)，避免在多份文档中重复排期。
