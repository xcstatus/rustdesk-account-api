# 项目维护入口

本项目为单团队RustDesk账号API及管理Web。项目资料从[README](README.md)和[文档中心](docs/README.md)进入；需求唯一来源是[PRD](docs/PRD.md)。

## 工作范围

- 业务实现：`api/`与`web/`；部署：`deploy/`；验证资产：`validation/`。
- `upstream/`、`references/`是独立参考仓库；`fixes/`、`patches/`、`diagnostics/`有各自专项用途，不作为账号API实现随意改动。
- 修改前按现有resolver和祖先指令处理目标路径；独立checkout分别解析。

## 维护原则

- 结构读[架构](docs/architecture.md)，行为读[业务逻辑](docs/logic.md)，运行读[运维](docs/operations.md)，开发读[开发指南](docs/development.md)。按当前问题读取必要材料。
- 数据库仅SQLite/PostgreSQL；业务模型或迁移变更同步两库。保留版本化前向迁移，不覆盖历史迁移与验收事实。
- 账号数据权限、设备目录归属与远控授权分别处理；不因目录可见推导远控许可，不擅自恢复已取消的远控ACL需求。
- 保留兼容来源的固定版本。更新上游、修改生产、发布、清理测试账号或设备时，按本次任务授权判断，不从历史开发授权无限扩大范围。
- 检查按风险选择：文档只检查链接/事实；代码运行直接相关用例；兼容变更才补受影响实机流程。不默认重跑全部矩阵或连接生产。
- `.runtime`、数据库、secrets、凭据、密钥、备份和原始诊断输出不进入公开文档或构建上下文；清理前确认恢复与构建依赖。
- 修改同步相关文档；当前手册与日期化历史记录分开。维护待办与变更记录分别见[维护计划](docs/maintenance.md)和[CHANGELOG](CHANGELOG.md)。
