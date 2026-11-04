# 变更记录

本文记录本项目变更，不代表已经发布版本包或创建Git tag。应用manifest当前为`0.1.0`开发版本，交付事实以日期化验收与部署记录为准。

## 未发布

### 2026-11-05：PR门禁与非商业许可

- 增加main Ruleset配置：走PR、Rust/Web CI通过、解决审查讨论，禁止删除和强推；关闭自动合并，保留维护者决定。
- 增加CodeRabbit中文自动审查配置，包含草稿和bot PR，不自动批准或修复；App安装授权与首次实际审查仍待完成。
- 自有内容采用PolyForm Noncommercial 1.0.0，补充包许可元数据与第三方许可边界；RustDesk相关补丁保留适用的上游许可。
- 仅核验配置、文档和许可元数据，不变更生产、不重跑历史实机矩阵。

### 2026-11-04：仓库公开

- 按项目负责人要求，将`xcstatus/rustdesk-account-api`设为公开并同步维护文档；项目许可证另行确定。
- 核对已跟踪历史与发布边界；本次不合并既有PR，不变更生产或增加AI审查服务。

### 2026-11-03：GitHub私有交付与安全维护

- 初始化Git，创建`xcstatus/rustdesk-account-api`私有仓库，公开发布与许可证仍另行决定。
- 增加Dependabot、RustSec/npm每日扫描、失败Issue与Rust/Web CI，修复采用PR审核，不自动部署。
- 排除原始运行截图及私有运行资产；保留脱敏JSON验收证据。

### 2026-11-03：项目文档整理

- 建立[文档中心](docs/README.md)，区分当前维护手册、专项设计与历史证据。
- 整理技术路线、当前架构、业务逻辑、开发指南、运维手册和维护计划。
- 修正旧阶段状态与已完成的Windows双端验收、生产部署之间的表述差异。
- 增加项目维护入口AGENTS与CHANGELOG，补充私有secret、Python缓存及诊断产物的忽略规则。
- 保留早期架构讨论与原验收记录；没有新增功能、生产变更或功能重测。

## 0.1.0 开发基线（2026-10-31）

- 完成本地/OIDC账号、JIT、角色映射、client/Web会话与撤销、管理页面和账号审计。
- 支持SQLite/PostgreSQL及单实例Compose交付，保存[阶段验收](docs/m23-validation.md)与[首次部署](docs/deployment-2026-10-31.md)。
- schema4增加设备归属、本人设备目录和个人簿凭据静态加密同步，见[扩展验收](docs/device-credentials-validation.md)。
- 固定PR #734解决登录后的hbbs安全握手兼容，Mac/Windows完成远控与同步凭据实测，见[兼容记录](docs/rendezvous-compatibility.md)。
