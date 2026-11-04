# 首版交付范围与支持矩阵

- `document_status: drafted`；`account_service_status: m23_verified`；整理：2026-11-03；运行与实机证据截至2026-10-31。
- 负责人：项目发起人；维护位置：本仓库；需求唯一来源：[PRD](PRD.md)。
- 验收依据：[M0](m0-validation.md)、[M1](m1-validation.md)、[M2/M3](m23-validation.md)、[实际部署](deployment-2026-10-31.md)。

当前已具备单团队小范围部署所需的账号、OIDC、个人通讯录、基础管理页面、账号审计及双库运维包。已在 `me-aliyun` / `desk.166198.xyz` 完成首次实例部署，使用同机共享PG独立库，暂不启用OIDC；26项线上HTTP检查和8项备份恢复检查通过。后续schema4与PR #734已部署并通过双端功能验收，见下文。生产IdP仍未指定；共享PG已纳入集中每日备份，计划实际触发与保留由PG维护入口负责。自动异地备份及外部告警尚无验收记录；本次未重新巡检线上。

| 对象 | 已有证据 | 支持边界 |
|---|---|---|
| 官方客户端 | 历史M0 macOS arm64 1.5.0 build 68；扩展由用户在macOS 27/Windows 11官方1.5.0验证登录、目录、个人簿跨端重登、Mac远控和Windows同步后免输密码连接 | 扩展系统/版本为用户报告；Linux/移动端未实机覆盖；仅对应本次测试设备，见[扩展验收](device-credentials-validation.md) |
| 管理Web | Vue 3/TypeScript，实际CUA浏览器账号与审计页面检查 | 实际验收使用ChatGPT in-app browser；其他浏览器尚无独立矩阵 |
| SQLite | 锁定SQLx SQLite构建，持久化、v1/v2→v3、审计故障回滚、HTTPS及备份恢复 | 单实例，本地可靠磁盘；不部署到共享网络文件系统 |
| PostgreSQL | 本机16.15及实际共享实例16.10；后者通过HTTP与独立库备份恢复、应用owner验证 | 实测PG16；其他主版本需另行验证，不支持MySQL |
| OIDC | Keycloak 26.8.1，真实授权码/PKCE、JIT、准入与静态映射、Web及客户端适配 | 生产IdP逐项配置；无自动目录同步、IdP事件注销或自动JWKS刷新 |
| 容器 | Linux arm64本机演练；Linux amd64在Ubuntu24.04/已有Traefik实际运行、公网HTTPS与共享PG恢复验证 | 复用既有证书；新ACME签发/续期、多实例、持续负载及历史旧Linux镜像回退未验证 |

历史M0～M3及首次生产镜像仅同步个人簿元数据；最新扩展配置私有密钥后支持官方hash同步，原文password仍剔除，详见[设计](device-credentials-design.md)。扩展双库/管理Web/密文恢复及schema4线上升级已独立记录，官方1.5.0双机功能实测已通过；登录后的握手兼容使用固定[PR #734](rendezvous-compatibility.md)。每簿最多1000条，HTTP请求体最大2MiB。并发采用最后数据库成功提交的整本快照生效，不合并旧快照。账号撤销只保护本API，不承诺断开远控。团队共享与远控连接/文件传输审计仍在后续；按用户/用户组限制远控已取消。

默认客户端会话30天、Web8小时，可配置。账号审计90天、0永久，不采集IP；容器日志10MB×5轮转。共享登录限流默认每账号10次/60秒、进程300次/60秒、1024个账号key。部署者按实际团队调整；100账号/200有效会话的容量假设尚无负载SLA证据。

升级为schema3后不可直接运行旧M0/M1二进制。两库已验证前向迁移和新卷快照恢复；回退必须配合升级前快照与保留的旧镜像。当前演练验证了恢复后回到快照状态，没有执行一个历史Linux旧发布镜像的完整上线回退。操作说明见[部署手册](../deploy/README.md)。

本项目独立实现位于`api/`、`web/`；协议调研来源和准确提交保存在[sources.lock.json](../sources.lock.json)、[research.md](research.md)。官方RustDesk客户端/服务端参考仓库保留原AGPL-3.0，社区参考API/Web保留原MIT声明；参考checkout不进入本服务镜像构建上下文。Rust/npm依赖分别由`api/Cargo.lock`、`web/package-lock.json`锁定。

用户已授权公开源码并为自有内容采用[PolyForm Noncommercial 1.0.0](../LICENSE)，第三方与补丁例外见[NOTICE](../NOTICE)。公共镜像与版本发布尚未执行；分发镜像/二进制前整理实际依赖的许可、署名及源码提供义务，并复核版本和支持范围。源码公开不扩大已有功能验收结论。

设备与凭据代码使用schema4，回退必须恢复升级前库和旧镜像；密文恢复需要独立保留的对应key ring，数据库备份本身不足以恢复凭据。

当前实例的设备与凭据扩展已部署，详见[扩展升级](device-credentials-deployment-2026-10-31.md)。共享PG当前集中备份入口归服务器PG运维目录，旧服务级脚本已退役；key ring及应用文件另有本轮私有备份。
