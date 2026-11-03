# 运维手册

- `document_status: reviewed`；`operations_baseline: local_records_consolidated`。
- 整理日期：2026-11-03；维护责任：项目发起人及相应应用、共享数据库维护者。
- 本文是运维入口；参数、完整部署步骤归 [部署包](../deploy/README.md)，线上事实归对应实例记录。
- 实例画像均来自 **2026-10-31 快照**，本轮只整理本地记录，没有重新检查线上状态、执行测试或连接服务器。
- 适用单实例账号 API；不把文档整理视为新部署、恢复演练或平台验收。契约见 [document-brief](assets/document-brief.md)。

## 先区分部署类型

| 对象 | 入口与责任 | 不可混用的边界 |
|---|---|---|
| 通用新部署 | [deploy/README.md](../deploy/README.md)：SQLite、内置 PG、外部 PG；默认 Caddy，可选 Traefik/OIDC/凭据 overlay | 每个实例固定 project、env、卷和完整 Compose 文件组合 |
| 账号实例 | `/home/migu/app/usr/rustdesk-accounts` 中的 `./accountctl` | 外部共享 PG + Traefik，不启动内置 PG 或 Caddy |
| 远控实例 | `/home/migu/app/usr/rustdesk` 的原 OSS Compose + PR #734 overlay | 独立 Compose 项目，不能把账号 overlay 加到这里 |
| 共享 PG | `/home/migu/app/usr/postgres` 的集中备份手册与 `backup.py` | 备份包含多个业务库，由共享 PG 维护者负责；应用不得自行覆盖恢复 |

通用部署从仓库根目录执行文档步骤，实例命令从对应服务器实例目录执行。执行前确认所在目录、project 和配置来源；`accountctl` 以服务器维护版本为准，不根据名称猜测其 project 参数。

新部署依次完成镜像固定、非秘密 `.env`、独立数据卷、secret 权限、所选数据库、迁移、一次性管理员初始化和 HTTPS 验证。直接沿用 [部署包的首次初始化](../deploy/README.md#secrets-与首次初始化)；外部 PG 必须改用其 [专用流程](../deploy/README.md#外部-postgresql不启用-oidc)，不能运行内置 PG 初始化器。

每次重建必须保留该实例全部 overlay，尤其密钥与 TLS options；不要把 README 的基础 SQLite/PG 简写复制到既有实例。新实例的 OIDC 是可选能力，`me-aliyun` 快照中未启用。

## 现有实例画像（2026-10-31）

| 项目 | 记录中的状态 |
|---|---|
| 主机 / 域名 | `me-aliyun` / `desk.166198.xyz`；Ubuntu 24.04.3、Linux amd64 |
| 账号 API | schema 4；`rustdesk-account-api:0.1.0-devices-amd64`；管理入口 `https://desk.166198.xyz/admin/` |
| 客户端 API Server | `https://desk.166198.xyz`；ID Server、Relay、Key 沿用原远控配置 |
| 数据库 | 同宿主 PG 16.10；独立库与 owner `rustdesk_accounts`，应用角色无集群管理权限 |
| PG 网络 | `database-network`，无数据库公网端口；现有内网连接为 `sslmode=disable` |
| HTTPS | 现有 Traefik，`traefik-network`；同域名 TLS options `rustdesk-https-tls@file` |
| API 隔离 | UID/GID 10001、只读根目录、无宿主 API 端口；384 MiB、0.75 CPU、128 PIDs |
| 账号功能 | 本地账号、Web、个人簿凭据同步和设备目录已部署；生产 OIDC 禁用 |
| hbbs / hbbr | hbbs 为固定 PR #734 构建；hbbr 保留原镜像与配置 |

依据：[首次部署](deployment-2026-10-31.md)、[schema 4 升级](device-credentials-deployment-2026-10-31.md)、[握手兼容记录](rendezvous-compatibility.md)。首次部署的 schema 3 镜像与“0 有效会话/0 非空簿”是历史收尾，不是后续人工验收后的现状。

内网明文 PG 是该实例的已记录选择，不能复制为跨主机方案。迁移到其他网络时重新配置并验证数据库 TLS、CA 与 hostname，参考部署包外部 PG 的 `sslmode=verify-full` 要求。

## 日常健康与日志

以下为后续运维使用的查看命令，本轮未执行。账号命令须在账号实例目录运行：

```sh
./accountctl ps
./accountctl logs --tail 100 api
curl --fail --silent --show-error https://desk.166198.xyz/healthz
curl --fail --silent --show-error https://desk.166198.xyz/readyz
```

`/healthz` 只表示进程存活；`/readyz` 检查 schema 4、数据库写路径和 Web 产物，写探针回滚。数据库故障可表现为前者 200、后者 503。Compose 使用 readiness 判定 healthy，unhealthy 本身不会触发自动重启；先定位数据库、密钥或构建产物问题。

`ready-check`、`check-credentials` 等 CLI 都先连接数据库，可能执行前向迁移，不能当成任意版本可运行的只读探针。日常先看 HTTP、容器状态和脱敏日志。

API 请求日志只记录方法、匹配路由、状态和耗时。Docker 日志配置为每服务 `10m × 5` 轮转；账号 Traefik router 关闭 access logs/tracing。新增采集器不得记录完整 URL、query、Cookie、Authorization 或请求体，故障日志也应脱敏。

账号审计在数据库中，默认保留 90 天；自动清理不等于清除历史备份。手动 `prune-audit` 会删除事件，应先确认留存要求，再按 [部署包审计流程](../deploy/README.md#健康日志与审计)执行；Traefik 实例临时 CLI 仍须禁用路由注册。

## 管理员与会话处置

日常从 `/admin/` 管理账号、密码、会话、设备与审计，接口语义见 [API 管理说明](../api/README.md#web账号管理)。不要用数据库改行或重置数据库解决管理员访问问题。

| 场景 | 正确入口与影响 |
|---|---|
| 全新库首次管理员 | `bootstrap-admin <username>`，按部署包临时挂载密码 secret；初始化成功后不可重复使用 |
| 丢失全部管理员访问 | 可信宿主机/数据库维护者先备份，再用 `create-local recovery-admin --role admin` 建立新应急管理员 |
| 本地密码重置/本人改密 | Web 操作；撤销目标用户既有 Web 与 client 会话，包括本人改密会话 |
| OIDC 密码 | 在 IdP 管理，本地密码重置对 OIDC 目标返回 409；本实例尚未启用 |
| 停用账号 | Web，或受信 CLI `disable-user <user_id>`；撤销其所有会话，保留个人簿 |
| 撤销会话 | Web 本人会话页；管理员可撤销目标用户会话或按公共 UUID 撤销指定会话 |
| 调整设备归属 | Web 设备管理；同事务撤销该设备关联的 client 会话，未绑定旧会话另按用户/会话处置 |

最后一个有效本地管理员不能被停用或降权。恢复后先确认长期管理员可登录，再撤销应急会话并停用临时账号；不能先关闭唯一的本地恢复通道。

实例初始管理员为 `administrator`，凭据归服务器私有 `credentials/` 和密码管理器管理；本文不复制值。正常建号走 Web，应急 CLI 需要可信主机与数据库权限。

实例应急创建示例：**会写数据库并可能迁移 schema**。先停写备份，确认镜像兼容；在账号实例目录用可信编辑器准备新的 `.secrets/recovery_password`，权限 `0600`、UID/GID `10001:10001`，密码不得经 argv、shell history 或日志传入。随后使用完整入口：

```sh
./accountctl run --rm --no-deps -l traefik.enable=false \
  -v "$PWD/.secrets/recovery_password:/run/secrets/recovery_password:ro" \
  -e RD_BOOTSTRAP_PASSWORD_FILE=/run/secrets/recovery_password \
  api create-local recovery-admin --role admin
```

`accountctl` 必须继续加载外部 PG、Traefik、实例和 book-credentials 四份配置；临时 CLI 的 `-l traefik.enable=false` 防止注册成线上后端。维护结束按私有密钥管理流程转存或处置临时文件。

账号退出、停用或撤销 token 作用于账号 API，会影响后续同步/认证；**不保证断开已有远控连接**。设备归属不构成远控 ACL，被控端密码或确认仍由既有远控机制负责。

## 升级前置与版本指纹

升级前先定义维护窗口、停写范围、恢复目标和可接受的数据回退点。保留旧镜像、完整 Compose/env、secret、全部 key ring 和数据库快照；先在独立目标演练，不原地降级。

| 需记录 | 记录位置与核验方法 |
|---|---|
| API / Web | 镜像 tag、实际 Image ID/RepoDigest、平台、二进制/Web SHA-256；[schema 4 发布报告](../validation/results/device-credentials-deployment-2026-10-31.json) |
| 数据库 | schema、数据库镜像实际版本/digest、owner、备份 manifest 与 SHA-256 |
| 应用文件 | Compose 文件清单与顺序、非秘密 env、secret 权限/归属、key ID；秘密归加密私有备份 |
| hbbs | PR head/base、子模块、Cargo.lock SHA、镜像 ID、启动开关与原身份挂载；[兼容记录](rendezvous-compatibility.md) |

账号快照 Image ID 为 `sha256:6e16c4ff737d4b590a6195b49705cb1dc560e3b3a2f0a748422824b797660237`。这是本地构建镜像，无公共 registry 发布；tag 本身不能证明内容相同。完整指纹以发布报告为准，不用首次 M3 的旧 ID 代替。

API 连接会自动迁移。schema 4 不能交给旧 M3/schema 3 镜像打开；移除密钥 overlay 也不能恢复 metadata-only 模式。新发布先核对目标 schema 和全部密文 key，再执行迁移/重建。

实例凭据检查入口如下，**可能前向迁移数据库**；只在维护窗口使用已匹配镜像，不作为例行无副作用检查：

```sh
./accountctl run --rm --no-deps -l traefik.enable=false api check-credentials
```

该检查验证全部已有密文，不输出 key/hash/密文。正式 API 的 UID10001 必须能读取实际密钥挂载；CLI/helper 的 root 检查不能替代此权限验证。重建仍使用实例原 `accountctl` 和完整配置，不能只加载基础 Compose。

## 备份责任与完整恢复材料

| 数据类别 | 责任与入口 | 必须一起保留 |
|---|---|---|
| 当前共享 PG | 共享 PG 维护者；`/home/migu/app/usr/postgres` 的统一手册与 `backup.py backup` | 集中备份集、校验/恢复证据、镜像/schema/owner 信息 |
| 通用 SQLite | 应用维护者；[停写备份与新卷恢复](../deploy/README.md#停写备份与新卷恢复)的 `ops.py --backend sqlite` | 合并已提交 WAL 的 `.backup`、integrity check、manifest、原镜像 |
| 通用内置 PG | 应用维护者；部署包的 `ops.py --backend postgres` | 自有 PG 归档、两个角色对应 secret、PG 镜像与新卷恢复材料 |
| 应用配置/secret | 应用维护者独立加密备份 | Compose/env、数据库连接 secret、必要 OIDC secret、key ring、恢复说明 |
| 远控数据与身份 | 远控维护者停 hbbs 后一致归档 | 原 Compose/overlay、镜像、数据、`id_ed25519` 和公钥；与账号密钥分开 |

**当前外部共享 PG 禁用 `deploy/ops.py --backend postgres`。** 脚本固定加载内置 `compose.postgres.yml` 并操作内置 PG 服务/新卷，不接受外部 URL，也不支持当前实例完整 overlay。`aliyun-backup-check.py` 已退役，不再作为当前备份入口。

共享 PG 集中备份范围动态包含所有非模板数据库；使用前先读服务器父级指令和共享 PG 当日维护规则，由其维护者协调。`backup.py backup` 会创建数据库备份；应用维护窗口仍需停止账号 API 及所有人工 CLI/其他写者，不能仅假定脚本识别所有写者。不得自行增加按服务 dump、计划任务或 prune。

2026-10-31 记录中，schema 4 升级前后集中备份均做过隔离恢复；升级后集 `set-20261031T053144Z-9a708b` 核验 149 张表。共享 PG 每日计划、保留策略和计划实际触发证据归共享 PG 目录，不以一次人工成功代替计划验收。

通用 `ops.py backup` 成功后 API 保持停止；恢复只接受从未存在的新卷，失败目标保留排查。SQLite 不直接复制活跃主库文件，不放在共享网络文件系统。含密文时备份需提供 `--book-keys-file` 作 ID/格式预检，实际 key 值仍须通过隔离恢复的解密检查。

数据库备份包含 `devices`、`book_credentials`、`device_events`，但**不包含应用加密 key**。账号 hash 可被持钥服务解密，不是端到端加密；快照及密钥都属于敏感材料。两者独立加密保存，并在私有记录中关联。

key ring 文件为非软链普通 JSON、`0600`、实际 API UID 可读。轮换须新 key ID、原子替换和重建加载；旧行不因切换 active key 自动重加密。只要当前库或保留期内任一备份引用旧 ID，就保留原 key；不要覆盖旧值或删除旧 ID。

已有 `with-book-keys.tar.gz` 与本机私有副本的记录只证明当时归档存在，不构成自动异地备份。应用秘密不进镜像、源码清单、公开 manifest、工单或本文；同机数据库备份也不能抵御整机磁盘丢失。

## 恢复与回滚

恢复是从可信快照重建可用数据；回滚是撤回发布，需要数据库、应用镜像及配置组成同一兼容版本。仅换旧镜像不是数据库回滚。

1. 停止目标 API 和全部 CLI 写者，保存当前失败状态及新数据，不删除原库、卷或 key。
2. 校验快照 manifest、SHA-256、schema、对应镜像和全部历史 key ID；备份必须来自可信来源，校验和不能替代来源认证。
3. 通用 SQLite/内置 PG 按部署包恢复到全新卷；当前共享 PG 由其维护者恢复到独立目标，不能覆盖共享原库或 PGDATA。
4. 在隔离目标核对 schema、owner、账号/会话/簿/设备/审计与凭据解密；挂载正确 key ring，用匹配镜像验证。
5. 切换前用正式 UID 与完整配置确认密钥可读。共享 PG 切换本 API 的私有连接 secret；通用卷部署按部署包切换目标卷，保留代理与其他服务。
6. 启动后检查 HTTPS readiness、Web 登录和必要业务，再决定恢复流量；失败时保留目标另行排查，下一次恢复另选新目标。

若撤回 schema 3→4 升级，必须取升级前旧 schema 快照、旧镜像及 `before-upgrade` 配置；保留新库与新 key。回退丢失快照后的新增写入，如何补偿由维护计划决定，不会自动合并。

2026-10-31 已验证同版本隔离恢复；未做生产降级演练。禁止 `docker compose down -v`、覆盖共享 PG 原目录、删除原身份密钥等方式“重置”故障。

## hbbs PR #734 的维护与撤回

采用固定 PR head `ef2166f7760905a18b6588f0882d633d3688ad2f`，子模块 `229b904508364c8997aad0fb5af57effac859f60`；2026-10-31 查询时未合并，按独立构建维护。具体固定值与构建材料见 [兼容记录](rendezvous-compatibility.md)及 [PR 部署包](../deploy/rustdesk-pr734/README.md)，不自动跟随 PR head。

- 镜像 `rustdesk-hbbs-pr734:ef2166f7-amd64`；ID `sha256:f9d74b60499cd4f84c3a5ae8edef62aa15178c1140a85e4d38d2f68542a4bd25`。
- 开关固定为 `SECURE_TCP=Y`、`IPV6_PUNCH=N`、`WEBRTC=N`、`UDP_PUNCH=N`。
- 实际 overlay：远控实例内 `releases/pr734-ef2166f7/compose.override.yml`，必须与原 Compose 同时加载；它不是账号 API overlay。
- 沿用 31116、原 `/root` 挂载及 hbbr；`-k _` 从原私钥读取服务器身份，客户端 Key 保持原值。
- [入口脚本](../deploy/rustdesk-pr734/entrypoint.sh)在原公私钥文件缺失时拒绝启动；先排查挂载，不能生成新钥绕过。

任何切换都先核对原公私钥匹配、备份配置和停止 hbbs 后的一致数据，再仅重建 hbbs。不要重建 hbbr、账号 API、PG 或代理来完成 hbbs 切换，也不要把生产私钥加入构建上下文。

原配置、镜像和一致数据归档记录在 `/home/migu/app/usr/rustdesk/backups/pre-pr734-20261031T082523Z/`。其 `rollback.sh` 是**生产写操作**，按原配置仅重建 hbbs；会撤回兼容修复，登录后的握手互等可能复现。先审查保存的脚本及当日配置再由远控维护者执行；不要凭本文拼接缺失原 Compose 的重建命令。

常规 hbbs 撤回继续保留原挂载；数据归档只在确有恢复需要时另行处理，不能自动覆盖后续数据。PR #734 不校验账号 token、不增加用户组或远控 ACL。

## 故障定位与处置边界

| 症状 | 先看哪条链路 | 处置重点 |
|---|---|---|
| `/admin/`、登录或同步不可用 | Traefik → 账号 API → PG/key/Web | 对照 healthz/readyz、API 状态、路由 TLS options、数据库与密钥权限；不先重启整机 |
| healthz 200 / readyz 503 | 账号服务依赖 | schema、数据库写路径与 Web 产物；避免旧镜像 CLI 触发不兼容迁移 |
| 启动拒绝、凭据解密失败 | 账号 key ring / 数据快照 | 权限、历史 ID、正确 key 与匹配备份；保留原文件，不删 overlay 或清空密文 |
| 跨端凭据未出现 | 账号同步与客户端状态 | 同一账号、个人簿保存/刷新、凭据开关及认证；目录可见不等于拥有远控权限 |
| 登录账号后 `Failed to secure tcp: deadline has elapsed` | 客户端 → hbbs 安全握手 | 核对固定 hbbs 镜像、SECURE_TCP、原身份与挂载；该历史问题发生在目标密码认证之前 |
| ID/中继连接失败 | hbbs/hbbr、端口、防火墙、客户端 Key | 由远控项目定位；账号 API ready 不代表远控链路可用 |
| `/ws/id`、`/ws/relay` 502 | 既有 WebSocket 代理链路 | 首次上线前已有的历史现象；本轮及账号升级未宣称修复，另案核验 |
| 撤销会话后已有远控仍继续 | 账号与远控授权边界 | 从被控端或远控机制处置活动连接；账号会话撤销不承诺即时断控 |

## 已验证边界与后续运维项

2026-10-31 的证据分别覆盖：首次 schema 3 上线 26 项 HTTP / 8 项备份恢复；schema 4 的 16 项 HTTPS 检查和集中备份隔离恢复；PR #734 的 Linux/amd64 构建、18 项 `key_exchange` 通过，3 项需旧二进制的 `compat` 对照按约定 ignored。

官方 1.5.0 的 macOS/Windows 本次双机人工结果包括设备目录、个人簿跨端与重登；切换 hbbs 后用户确认“mac连接成功， windows免密成功”。它仅覆盖本次设备，不扩展为全平台支持矩阵。旧独立补丁 17 项结果不计入 PR #734 证据。

- 应用 key/config 的加密异地备份、统一恢复责任与定期恢复演练仍需落实；数据库每日计划的实际运行状态由共享 PG 维护者持续核验。
- 外部告警、证书续期验证、容量与持续负载、主机故障恢复、生产 OIDC、其他客户端平台仍需独立安排。
- 官方客户端最终退出/配置恢复尚未报告，测试账号与设备归属保持记录中的现状，收尾须另行确认，不擅自清理。
- 生产降级、跨主机数据库 TLS 与完整灾难恢复不在已通过范围；镜像未公开发布，公开方式与许可证待定。

证据入口：[首次部署](deployment-2026-10-31.md)、[设备/凭据升级](device-credentials-deployment-2026-10-31.md)、[人工验收](../validation/results/device-credentials-client-manual-2026-10-31.json)、[PR 验证](../validation/results/rendezvous-pr734-validation-2026-10-31.json)。后续运维发现新事实时更新对应实例记录，再同步本入口。
