# RustDesk 账号 API 服务

面向单个团队的轻量自托管账号服务，使用Rust独立开发。官方RustDesk客户端通过配置API Server地址接入，提供账号登录、OIDC统一认证、个人通讯录与连接凭据同步、设备归属、管理Web和账号审计。

## 当前基线

- 后端：Rust、Tokio、Axum、SQLx；管理端：Vue 3、TypeScript、Vite。
- 数据库：仅SQLite和PostgreSQL；单实例部署，不需要Redis或消息队列。
- 统一认证：OIDC用户自动创建与简版角色映射已实现，默认拒绝，可配置普通成员兜底；生产暂未接入IdP。
- 设备与同步：schema4已部署；官方1.5.0的macOS/Windows双端完成个人簿同步、设备目录、登录后远控和跨端免输密码连接。
- 远控兼容：hbbs采用固定PR #734构建，hbbr保留原服务；不实现按用户/用户组限制远控。

文档整理于2026-11-03；上述上线与实测依据2026-10-31记录，本次未重新巡检生产。仓库于2026-11-04公开，项目许可证尚未确定。

## 项目入口

| 阅读目的 | 文档 |
|---|---|
| 全部文档及当前/历史证据导航 | **[项目文档中心](docs/README.md)** |
| 目标、范围、需求与验收 | [PRD](docs/PRD.md) |
| 技术选型与演进方向 | [技术路线](docs/technical-route.md) |
| 组件、模块、数据与安全边界 | [技术架构](docs/architecture.md) |
| 登录、角色、同步、归属与审计规则 | [业务逻辑](docs/logic.md) |
| 本地启动、源码导航与必要检查 | [开发指南](docs/development.md) |
| 部署、备份、恢复、升级与排障 | [运维手册](docs/operations.md) |
| 维护流程、已知待办和交接 | [维护计划](docs/maintenance.md) |
| 实测范围和发布限制 | [支持矩阵](docs/release-notes.md) |
| 自动漏洞扫描与修复流程 | [安全维护](SECURITY.md) |
| 持续变更 | [CHANGELOG](CHANGELOG.md) |

新开发者从开发指南开始；接手服务器从运维手册开始。参数和具体命令继续由[API说明](api/README.md)、[Web说明](web/README.md)及[部署说明](deploy/README.md)维护。

## 目录

```text
api/          Rust账号API、两库迁移和测试
web/          Vue管理页面和前端测试
deploy/      Compose、镜像、配置初始化、备份恢复与hbbs构建
docs/        当前设计、维护手册、专项与历史证据
validation/  可复现验证脚本及脱敏结果
upstream/    官方参考源码，不是本项目业务实现
references/  社区参考源码
patches/     专项补丁与已停用备选
diagnostics/ 客户端问题诊断材料
```

参考源码的初始版本见[sources.lock.json](sources.lock.json)，实际hbbs部署版本见[PR #734记录](docs/rendezvous-compatibility.md)。参考仓库保留各自历史与许可证，不把它们整体纳入账号API镜像。

项目使用Git管理，远端为[GitHub公开仓库](https://github.com/xcstatus/rustdesk-account-api)。私有运行资产与源码的边界见[维护计划](docs/maintenance.md)。
