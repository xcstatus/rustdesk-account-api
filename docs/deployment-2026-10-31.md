# me-aliyun 部署记录

- `document_status: reviewed`
- `deployment_status: deployed_verified`
- 日期：2026-10-31；责任人：项目发起人；实施与核验：Codex。
- 授权来源：用户确认实际部署，指定 SSH 别名 `me-aliyun`、域名 `desk.166198.xyz`、同服务器共享 PostgreSQL 容器，暂不接入 OIDC。
- 本记录只描述该实例；历史验收范围仍以 [M2/M3 记录](m23-validation.md)为准。

> 历史记录：本文保留首次schema3上线证据。服务器随后已改为共享PG集中备份，旧`aliyun-backup-check.py`入口退役；当前升级前快照应使用`/home/migu/app/usr/postgres/backup.py backup`，应用配置和密钥仍单独备份。最新设备/凭据扩展另行记录，不覆盖本文历史结果。

## 已确认环境

| 项目 | 实测结果 |
|---|---|
| 系统 | Ubuntu 24.04.3 LTS，Linux x86_64；2 CPU、约 1.6 GiB 内存 |
| Docker / Compose | 28.3.3 / v2.39.1 |
| HTTPS 入口 | 已有 Traefik v3.6，`traefik-network`，`web` / `websecure`，`letsencrypt` resolver |
| 数据库 | 已有 `postgres:16-alpine` 容器，运行版本 16.10，`database-network`；未开放数据库公网端口 |
| 数据隔离 | 新库及 owner 均为 `rustdesk_accounts`；角色非超级用户，无建库、建角色、复制或绕过 RLS 权限；撤销该库 PUBLIC 连接权限 |
| 数据库传输 | 同宿主 Docker 内网，现有 PG 未启用 TLS；本实例明确使用 `sslmode=disable`。移至其他服务器时须重新配置并验证 TLS |
| 实例目录 | `/home/migu/app/usr/rustdesk-accounts`，权限 `0700` |
| Secret | `.secrets/database_url` 与 `.secrets/bootstrap_password` 为 `0600`、UID/GID `10001:10001`；不在 Compose 环境值或本文中保存密码 |
| 初始管理员 | `administrator`；随机密码保存在服务器 `credentials/administrator-password`，`0600`、由部署用户读取 |
| OIDC | 本实例禁用；代码及历史测试覆盖保留 |

使用[外部 PG 基础配置](../deploy/compose.external-postgres.yml)和 [Traefik overlay](../deploy/compose.traefik.yml)，实例额外设置 `cpus: 0.75`、`mem_limit: 384m`、`pids_limit: 128`。API 以 UID 10001、只读根文件系统运行，无宿主机端口映射。Caddy 未启用，现有 Traefik、PG 和 hbbs/hbbr 容器不重建。

同域名现有 `/ws/id`、`/ws/relay` 路由使用 `rustdesk-https-tls`；实例 override 将新路由设为 `rustdesk-https-tls@file`，保持该域名的 TLS options 一致。跨 provider 引用和同域名规则见 [Traefik 3.6 TLS options](https://doc.traefik.io/traefik/v3.6/reference/routing-configuration/http/tls/tls-options/)。账号路由关闭 access logs / tracing，配置依据见 [Traefik router observability](https://doc.traefik.io/traefik/reference/routing-configuration/http/routing/observability/)。

## 部署前基线

通过系统信任链访问域名成功；`/admin/`、`/healthz`、`/readyz` 均为 404。已有 `/ws/id`、`/ws/relay` 请求均为 502，此为上线前已有状态，本次账号服务不负责修复这两条转发链路。

## 上线与验收

2026-10-31 首次部署完成。应用专用角色执行 schema 3 迁移、首次管理员初始化均成功；公网 HTTPS 下 `/admin/`、`/healthz`、`/readyz` 返回 200，HTTP 请求跳转 HTTPS。系统信任链验证通过，观测为 TLS 1.3，证书有效期至 2027-01-11；复用已有 Traefik 证书，不将本次请求验证写成一次新 ACME 签发或续期演练。

| 证据 | 实测结果 |
|---|---|
| [HTTP smoke](../validation/results/deployment-smoke-2026-10-31.json) | 26 项通过：Web 登录与 Cookie、Origin、账号/审计读取、客户端登录、双会话通讯录同步、凭据剥离及退出后 401 |
| [备份恢复](../validation/results/deployment-backup-2026-10-31.json) | 8 项通过：停写备份、归档读取、独立新库恢复、六表行数/完整内容摘要一致、审计序列状态一致、专用 owner、清理临时库、重启 healthy |
| [最终状态](../validation/results/deployment-state-2026-10-31.json) | 1 个账号、0 有效会话、0 非空通讯录、0 遗留恢复库；表及序列 owner 均正确；API healthy、无宿主端口 |
| 原有服务 | Traefik、PostgreSQL、hbbs/hbbr 的启动时间仍为 2026-10-19，本次未重启它们 |

HTTP smoke 在尚未交付初始密码、通讯录为空的维护窗口执行，结束后恢复完整原簿并撤销本轮三个会话。它模拟官方协议请求，不增加官方客户端 GUI 实机支持矩阵；历史 macOS 1.5.0 实机证据仍归 M0。当前 `/ws/id`、`/ws/relay` 仍返回部署前已有的 502，独立账号路由不负责修复该链路。

本次运行采用 384 MiB / 0.75 CPU / 128 PIDs 上限；空闲状态曾观测约 1.4 MiB 容器内存记账。该值是一次空闲快照，不是峰值、负载测试或容量保证。

## 构建指纹

- 镜像：`rustdesk-account-api:0.1.0-m3-amd64`，平台 `linux/amd64`。
- Image ID：`sha256:1458fb963eaa91950c1da14ccaca011db22933ed0d5d069a5538bb5904596fd8`；无公共 registry 发布或 RepoDigest。
- 容器二进制 SHA-256：`085188852a15d5726ef63efb44d11aa10417a3bd84ae25ac6c6999c59ab5d1b8`。
- Web index SHA-256：`2db55369e1558277566bde3d5252d17f0566ae63d87b1560b74c25968629aaee`，与既有本机 `web/dist` 一致。
- 镜像 gzip 归档：45,344,898 bytes，SHA-256 `6a45b0e735adda4772fea89bede902aff71fa781fcf6b1274552d3878268383b`；服务器保存于 `releases/initial-amd64-image.tar.gz`。
- [源码清单](../validation/results/deployment-source-manifest-2026-10-31.json)与[汇总](../validation/results/deployment-2026-10-31.json)绑定该次构建和验证；M2/M3 的旧指纹保持独立。

首次标准构建因依赖下载缓慢主动取消，运行时和 Web 阶段缓存保留。最终通过临时 Dockerfile 复用上一轮 Rust 构建的 Cargo registry，并采用 `cargo build --release --locked --offline`；第一次离线尝试发现缺少 amd64 依赖，补齐 `cpufeatures 0.3.1`、`curve25519-dalek-derive 0.1.1`、`fiat-crypto 0.2.9`、`zerocopy-derive 0.8.63` 且逐一验证 `Cargo.lock` checksum 后构建成功。源码和锁文件未因部署改动；最终镜像已在目标 amd64 主机实际运行并通过上述 HTTP 验收。

## 交接与操作

- 管理端：<https://desk.166198.xyz/admin/>；用户名 `administrator`。
- 初始密码由服务器部署用户读取：`ssh me-aliyun 'cat /home/migu/app/usr/rustdesk-accounts/credentials/administrator-password'`。本文和脱敏结果不包含密码。
- 官方客户端 API Server：`https://desk.166198.xyz`；ID Server、Relay、Key 按现有 RustDesk 配置。
- 常用操作：登录服务器后进入实例目录，执行 `./accountctl ps`、`./accountctl logs --tail 100 api`；`./accountctl up -d --no-build --pull never --wait api` 启动已保存镜像。
- 首次上线撤回：`./accountctl stop api`；保留数据库、secret、镜像及备份。现有共享 PG、代理和中继服务继续由原部署管理。

本次备份保存在服务器 `backups/initial-20261031T034155Z/database.dump`，14,368 bytes，SHA-256 `4699ea0247d78bd3432edd9dc9902bb06f794350289be016dc940eedcee9ad5d`；同目录私有 `manifest.json` 记录镜像、schema、内容摘要和校验结果。归档和目录分别为 `0600` / `0700`。当时的验证入口 `python3 aliyun-backup-check.py` 曾暂停本服务、创建一份新备份、恢复到随机新库验证、清理自己的临时库并重启 API。该入口已退役，不再用于当前备份；当前以共享PG维护目录为准。

后续升级先保存对应旧镜像和停写备份，核对 manifest；恢复到独立目标库后再切换 secret URL。禁止把 schema 3 库直接交给 M0/M1 旧镜像，也不在共享原库上执行覆盖恢复。当前只验证同版本快照恢复，没有做历史旧版 Linux 镜像的上线回退。

外部共享 PG 不使用现有 `deploy/ops.py --backend postgres`，该脚本只适用于内置 PG 配置。此实例当前由共享PG统一入口备份（动态包含所有非模板数据库）；恢复验证使用独立容器，不能覆盖共享数据库或其他业务。

## 当前边界

首次部署时未配置生产 OIDC、定时/异地备份或外部告警；随后共享PG已另行配置每日备份，应用密钥仍由本服务独立保管。现有备份位于同机，不能应对整机磁盘丢失。生产客户端 GUI 实机复验、其他客户端平台、持续负载和证书续期仍需独立验收。没有发布公共镜像或选择项目开源许可证。
