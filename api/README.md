# RustDesk 账号 API

Rust + Tokio + Axum + SQLx 的独立实现，现包含账号、OIDC、client/Web会话、个人簿及凭据同步、设备归属与账号审计，数据库schema4。部署和官方1.5.0 Mac/Windows功能验收已记录，具体范围见[支持矩阵](../docs/release-notes.md)与[扩展验收](../docs/device-credentials-validation.md)。默认监听本机，容器通过显式配置开放内部监听。新维护者先读[开发指南](../docs/development.md)，本页维护CLI、参数与接口契约；2026-11-03仅整理文档，未重测或巡检。

```sh
cargo build --locked --manifest-path api/Cargo.toml
cargo test --locked --manifest-path api/Cargo.toml
cargo clippy --locked --manifest-path api/Cargo.toml --all-targets -- -D warnings
```

完整测试环境的生成与运行命令见 [validation](../validation/README.md)。测试使用真实Keycloak、生成的账号和独立数据库，不需要现有RustDesk账号。

## 命令与配置

在项目根目录运行二进制或`cargo run --manifest-path api/Cargo.toml -- <command>`：

- `bootstrap-admin <username>`：首次初始化本地管理员；密码由`RD_BOOTSTRAP_PASSWORD`提供，成功后持久关闭同一入口。升级旧库已有本地管理员时也视作已初始化。
- `create-local <username> --role admin|member`：可信运维CLI建号及离线管理员恢复，密码也由`RD_BOOTSTRAP_PASSWORD`提供。该命令要求宿主机/数据库访问权限，不是公开初始化API。日常账号管理使用Web页面。
- `disable-user <user_id>`：停用并撤销所有会话；不能停用最后有效本地管理员。
- `migrate`：连接数据库并应用前向迁移后退出。
- `ready-check`：迁移后检查schema、数据库可写与Web产物；失败退出非0。
- `prune-audit --retention-days 90 --batch 1000`：清理一批严格早于保留期的审计事件。
- `serve`：启动账号API与Web静态页面；缺少Web构建或就绪失败时拒绝启动。

没有内置默认账号；本地密码统一要求12～4096字节，用户名非空、无首尾空格且不超过128字节。OIDC账号按issuer+sub自动建档，不生成本地密码，不按邮箱或同名合并。若丢失管理员访问，拥有数据库访问权限的运维人员可备份数据后用`create-local ... --role admin`建立新恢复账号，登录核验后处理旧账号。

| 环境变量 | 含义或默认 |
|---|---|
| `RD_DATABASE_URL` | 默认`sqlite://data/accounts.sqlite`；仅SQLite、PostgreSQL |
| `RD_LISTEN` | 默认`127.0.0.1:21114`；非loopback需`RD_ALLOW_NON_LOOPBACK=true`且HTTPS Web origin |
| `RD_ALLOW_NON_LOOPBACK` | 默认关闭；反向代理容器访问时显式`true`，API仍只在内部网络公开 |
| `RD_DATABASE_URL_FILE` / `RD_BOOTSTRAP_PASSWORD_FILE` / `RD_OIDC_CLIENT_SECRET_FILE` | 对应秘密文件；与同名非FILE变量互斥，仅去除一个末尾换行；空OIDC secret为public client |
| `RD_LOGIN_WINDOW_SECONDS` / `RD_LOGIN_PER_ACCOUNT` / `RD_LOGIN_GLOBAL` / `RD_LOGIN_MAX_KEYS` | 默认60 / 10 / 300 / 1024，Web与client共享账号/进程限流，不信任代理IP头，重启重置 |
| `RD_AUDIT_RETENTION_DAYS` | 默认90；0永久；每小时最多10批，每批1000条，失败记录通用错误 |
| `RD_BOOK_KEYS_FILE` | 可选0600密钥JSON文件，启用个人簿hash静态加密；存量密文缺/错钥拒绝，详见[扩展设计](../docs/device-credentials-design.md) |
| `RD_SESSION_TTL_SECONDS` | 默认2592000秒，正整数，可撤销 |
| `RD_WEB_ORIGIN` | 默认当前loopback监听地址的HTTP origin；校验浏览器写请求Origin，HTTPS origin设置Secure Cookie；只能为HTTPS或开发用loopback HTTP |
| `RD_WEB_SESSION_TTL_SECONDS` | 默认28800秒（8小时），范围1～2592000；与客户端会话独立 |
| `RD_WEB_DIR` | 默认`web/dist`，相对启动工作目录；Web构建产物路径 |
| `RD_OIDC_ISSUER` | 配置后启用OIDC；未配置则login-options返回空数组 |
| `RD_OIDC_CLIENT_ID` / `RD_OIDC_CLIENT_SECRET` | IdP应用标识；机密客户端提供secret |
| `RD_OIDC_REDIRECT_URI` | 固定登记的本服务`/api/oidc/callback`地址 |
| `RD_OIDC_CLAIM_PATH` | ID Token中的可信角色声明路径，默认`groups`；嵌套键以`.`分隔 |
| `RD_OIDC_ADMIN_VALUES` / `RD_OIDC_MEMBER_VALUES` | 逗号分隔的精确匹配值；同时匹配时admin优先 |
| `RD_OIDC_UNMATCHED` | 默认`deny`；可显式`member`，不提供admin兜底 |
| `RD_ALLOW_HTTP_LOOPBACK` | 仅显式`true`允许OIDC测试使用loopback HTTP；其他URL要求HTTPS |

启用member兜底意味着该IdP中认证成功且未被本地停用的未匹配用户也能进入。角色声明类型错误、认证与协议故障不会触发兜底。准入、角色与会话策略变更采用单实例停机重启；新配置初始化时先撤销已有OIDC会话，进行中的登录也不能跨进程恢复。

## Web账号管理

先按[Web说明](../web/README.md)在`web/`执行`npm ci`、`npm run build`，再从项目根目录启动API。默认管理地址为`http://127.0.0.1:21114/admin/`。在其他工作目录启动时显式设置`RD_WEB_DIR`为构建产物目录。Vite开发模式需把`RD_WEB_ORIGIN`设为实际浏览器origin；Web与OIDC回调使用相同hostname，确保临时绑定Cookie可回传。

Web与RustDesk客户端凭据分别持久化为`web`/`client`用途，不能互换。浏览器使用host-only、HttpOnly、SameSite=Lax Cookie，HTTPS时Secure；写操作同时检查Origin和`X-CSRF-Token`。`GET /admin/api/v1/me`返回本人信息与会话绑定的CSRF值，不返回原始凭据。密码重置/修改会撤销所有既有Web和客户端会话，包括当前改密会话。

| 管理路径前缀`/admin/api/v1` | 行为 |
|---|---|
| `GET /auth/options`、`POST /auth/login`、`POST /auth/oidc`、`POST /auth/logout` | 本地/OIDC浏览器登录与退出；登录/发起OIDC也要求正确Origin |
| `GET /me`、`POST /me/password` | 本人资料与本地改密；OIDC用户在IdP管理密码 |
| `GET/POST /users`、`PATCH /users/{id}` | 管理员列表、创建、启停和本地角色维护 |
| `POST /users/{id}/password` | 管理员重置本地密码；OIDC目标返回409 |
| `GET/DELETE /sessions`、`DELETE /sessions/{id}` | 本人会话列表、全部撤销或选择撤销；admin可按ID撤销其他用户会话 |
| `GET/DELETE /users/{id}/sessions` | 仅管理员查看或撤销目标用户全部会话 |
| `GET /devices`、`PATCH /devices/{id}` | 本人设备；admin列全部并按`owner_user_id`分配/解除，变更撤销关联client会话 |
| `GET /audit-events` | 仅管理员查询账号审计；倒序ID游标分页与来源/入口/操作/结果/时间/主体/目标筛选 |

管理员写入在事务内复核当前权限；最后有效本地管理员受保护，OIDC真实降权不受此保护。人工停用保留个人簿，重新启用OIDC后仍需IdP重新认证准入。会话列表返回公共UUID，不暴露凭据摘要。v1升级v2保留既有client会话，旧会话创建时间以迁移时间回填；v3新增审计表与索引，不修改历史事件补造旧记录；旧M0/M1二进制不能打开v3库。升级前备份，回退恢复旧快照及对应旧镜像。

## 兼容接口

| 路径 | 行为 |
|---|---|
| `POST /api/login` | 本地账号登录，返回本服务access_token/type/user |
| `POST /api/currentUser` | Bearer会话；直接返回本人对象，含`info:{}` |
| `POST /api/logout` | 撤销本次会话，200空响应；重放旧token被拒绝 |
| `GET /api/login-options` | 未启用OIDC为`[]`，启用为`["oidc/company"]` |
| `POST /api/oidc/auth` | 发起`op=company`浏览器认证，返回code/url |
| `GET /api/oidc/auth-query` | 绑定id/uuid的短期轮询；成功单次领取本服务token |
| `GET /api/oidc/callback` | 服务端换码验证；页面不包含token |
| `POST /api/ab/personal` | 认证通过后404触发Legacy；无效会话401 |
| `GET/POST /api/ab` | 外层`data`是JSON字符串，内层为整本个人簿快照 |
| `GET /api/device-group/accessible`、`/api/users`、`/api/peers` | 设备组为空，users为本人，peers为本人归属设备，分页返回 |
| `GET /healthz` | 进程存活状态 |
| `GET /readyz` | 2秒内检查schema 4、数据库写路径及Web产物；探针回滚，故障503 |

个人簿保存ID、别名、平台、主机/用户显示信息、标签与标签色；原文`password`和未知字段剔除。配置`RD_BOOK_KEYS_FILE`时，官方`hash`单独AES-GCM加密保存并同账号跨端返回，响应头为`enabled; personal-salted-hash`；未配置且无存量密文时为`disabled; metadata-only`。hash是可用于远控认证的摘要，API持钥可解密，非端到端加密。缺/空hash或移除peer会删除对应凭据；非法非空hash拒绝整簿。每用户最多1000个联系人，整本快照最后成功提交生效，无效快照不覆盖已有数据。

账号停用、OIDC降权与失去准入会撤销相应会话；普通退出仅撤销当前会话。随机token仅保存SHA256摘要，密码使用Argon2id。SQLite/PG各有版本化迁移；写事务串行化，通讯录读写在同一事务重验有效client身份，避免停用后延迟写入；不以此宣称已达到生产并发目标。

当前Web包含账号、密码、会话、审计查询与设备管理/我的设备。Compose、HTTPS、备份恢复和升级回退说明见[部署手册](../deploy/README.md)，实测边界见[M2/M3记录](../docs/m23-validation.md)。IdP事件同步或自动JWKS轮换不在本轮；JWKS刷新需要重启。官方客户端实机和跨平台结果单独记录，不以HTTP集成测试代替。


## 审计契约

`GET /admin/api/v1/audit-events`接受`before_id`、`limit`（默认50，1～200）、`since/until`（Unix秒，闭区间）、`actor_id/target_id`、`action/source/kind/result`。返回`{events,next_before_id}`，下页沿用相同过滤条件和游标；并发新增事件不会插入旧游标页。未知或非法参数400，普通成员403，client凭据不可作为Web Cookie使用。

事件包括`id/timestamp`、内部主体与目标ID、公共会话UUID、来源、入口、动作、结果、固定原因码、角色/准入/停用前后值、当前配置版本及实际新撤销会话数。管理员动作source为操作者认证来源；CLI/system动作为system。OIDC建档/映射在身份验证后记录，尚未准入身份只保留不可用档案。失败轮询不是新的登录，不重复记录已失败回调。

成功的账号、密码、会话、准入和策略变更与审计在同一事务提交；审计存储失败时回滚。未知主体登录失败不保存原始用户名。事件模型没有密码、hash、token、摘要、OIDC code/state/nonce/claims、subject、IP或任意metadata字段。账号审计不包含远控连接/文件传输内容；清理不删除历史备份。

`check-credentials`会在迁移后验证配置及全部已有凭据，不输出秘密。schema4升级前备份；回退需旧库和旧镜像。设备关联取认证登录中的id/uuid；旧会话需重登，不通过匿名sysinfo认领。归属不授予或限制远控权限。
