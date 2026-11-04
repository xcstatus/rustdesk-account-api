# 项目文档

- `document_status: reviewed`；整理日期：2026-11-03；维护负责人：项目发起人。
- 本目录是项目知识的维护位置；根 [README](../README.md) 是项目入口。
- 当前实现依据本地源码；部署与实机结果依据 2026-10-31 的记录。本次整理没有重新连接服务器或执行测试。

## 按任务阅读

| 要解决的问题 | 首选文档 | 继续定位 |
|---|---|---|
| 项目做什么、哪些功能不做 | [PRD](PRD.md) | RD-*需求与D-*产品决策 |
| 为什么采用这套技术、后续如何演进 | [技术路线](technical-route.md) | [维护与待办](maintenance.md) |
| 系统如何组成、模块和数据如何关联 | [技术架构](architecture.md) | [业务逻辑](logic.md) |
| 登录、角色、同步、归属、撤销的具体规则 | [业务逻辑](logic.md) | [设备与凭据专项设计](device-credentials-design.md) |
| 如何开发、启动和选择检查 | [开发指南](development.md) | [API参数](../api/README.md)、[Web说明](../web/README.md) |
| 如何部署、升级、备份、恢复与排障 | [运维手册](operations.md) | [通用部署操作](../deploy/README.md)、[hbbs构建](../deploy/rustdesk-pr734/README.md) |
| 哪些环境已通过、有哪些限制 | [支持矩阵](release-notes.md) | 下方验收记录 |
| 漏洞扫描、修复PR与安全报告 | [安全维护](../SECURITY.md) | [Actions配置](../.github/workflows/) |
| 一次变更需要同步什么 | [维护流程](maintenance.md) | [变更记录](../CHANGELOG.md) |

## 当前交付基线

| 范围 | 状态与证据 |
|---|---|
| 账号与管理 | 本地账号、Web管理、独立client/Web会话、撤销、账号审计已实现；[M1](m1-validation.md)、[M2/M3](m23-validation.md) |
| 统一认证 | OIDC、用户自动创建、单claim精确角色映射已实现并通过测试Keycloak；默认拒绝，可显式member兜底；生产尚未启用 |
| 个人数据与设备 | 个人簿、凭据静态加密同步、登录关联归属、本人设备目录已实现；[扩展验收](device-credentials-validation.md) |
| 官方双端 | 用户在官方1.5.0 macOS/Windows完成目录、个人簿跨端重登、Mac登录后远控和Windows同步后免输密码连接；[人工记录](../validation/results/device-credentials-client-manual-2026-10-31.json) |
| 实例 | 已部署schema4账号API、共享PG和Traefik；hbbs使用固定PR #734；[扩展升级](device-credentials-deployment-2026-10-31.md)、[握手兼容](rendezvous-compatibility.md) |
| 后续范围 | 团队共享通讯录尚未实现；生产OIDC、异地备份和外部告警另行安排；仓库已公开，自有内容采用[非商业许可](../LICENSE)，第三方范围见[NOTICE](../NOTICE) |

文档整理完成不代表新增功能交付或对所有客户端平台兼容。测试账号最终收尾仍按人工记录单独保留，未当作已清理。

## 事实归属

| 信息 | 唯一维护位置 | 使用方式 |
|---|---|---|
| 需求与范围 | [PRD](PRD.md) | 需求保持RD-*编号，不在架构中另定产品范围 |
| 已采用的技术取舍 | [技术路线](technical-route.md) | 变化时记录替代关系与理由 |
| 当前结构与规则 | [架构](architecture.md)、[逻辑](logic.md) | 以源码核对；专项协议细节引用原设计 |
| 命令和配置契约 | [API](../api/README.md)、[Web](../web/README.md)、[部署](../deploy/README.md) | 概览文档引用，避免多份完整参数表 |
| 实例配置与操作入口 | [运维](operations.md) | 日期化部署记录保存操作当时事实 |
| 上游兼容来源 | [research](research.md)、[源码清单](../sources.lock.json)、[PR #734记录](rendezvous-compatibility.md) | 初始调研快照与实际部署PR分别标明，不能混为一个版本 |
| 验收证据 | 日期化报告和 `validation/results/` | 历史结果不覆盖，不能把略过项写作通过 |
| 后续工作和限制 | [维护计划](maintenance.md)、[支持矩阵](release-notes.md) | 建议优先级不等于批准实施 |

负责人为项目发起人。修改者同步受影响文档及证据；需求、外部发布和运维策略的取舍由负责人决定。文档生命周期使用`document_status`，功能和部署状态使用独立字段；`reviewed`表示一致性检查，不表示批准公开发布。

## 专项设计与历史证据

| 文档 | 定位 |
|---|---|
| [设备与凭据设计](device-credentials-design.md) | 官方hash协议、AAD、密钥、设备归属细节 |
| [握手兼容](rendezvous-compatibility.md) | 故障根因、PR #734固定提交、构建和实例回退 |
| [M0验收](m0-validation.md) | 首次本地/OIDC登录、个人簿与退出闭环 |
| [M1验收](m1-validation.md) | 账号、Web和会话扩展的阶段证据 |
| [M2/M3验收](m23-validation.md) | 同步专项、审计、双库运维演练 |
| [首次部署](deployment-2026-10-31.md) | schema3首次上线，后续升级不改写其快照 |
| [扩展验收](device-credentials-validation.md)、[扩展升级](device-credentials-deployment-2026-10-31.md) | schema4与双端实机结果 |
| [早期架构讨论](architecture-discussion-2026-10-31.md) | 归档的候选方案，不作为当前配置契约 |
| [文档任务简报](assets/document-brief.md) | 本次整理的来源、归属和范围 |

[macOS点击缺陷专项](macos-command-click-fix-plan.md)与[事件采集工具](../diagnostics/macos-command-click/README.md)是独立客户端排障材料，不属于账号服务正常运行的依赖。
