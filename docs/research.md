# 源码调研与兼容证据

- 首次调研：2026-10-30；补充OIDC调查：2026-10-31；`document_status: drafted`。
- `source_research_status: source_review_only`；本文记录静态证据，运行验证另见[M0记录](m0-validation.md)。
- 产品约束见 [PRD](PRD.md)，技术建议见 [架构讨论稿](architecture.md)。精确仓库版本以 [sources.lock.json](../sources.lock.json) 为唯一清单。

## 源码来源

| 角色 | 本地目录 | 固定提交 | 顶层许可 |
|---|---|---|---|
| 官方客户端 | `upstream/rustdesk` | [`33a154a`](https://github.com/rustdesk/rustdesk/commit/33a154a5d7502419381dc601aff1e4b0c660054d) | AGPL-3.0，见仓内 `LICENCE` |
| 官方 OSS 服务端 | `upstream/rustdesk-server` | [`873230e`](https://github.com/rustdesk/rustdesk-server/commit/873230e35a8805de7fd057e7eb763c46f7a174ef) | AGPL-3.0，见仓内 `LICENSE` |
| 社区 Go API | `references/rustdesk-api` | [`c5687e1`](https://github.com/lejianwen/rustdesk-api/commit/c5687e150615bfb00d9d105f0883a2802750a8ad) | MIT，见仓内 `LICENSE` |
| 社区 Vue 管理端 | `references/rustdesk-api-web` | [`3998c2a`](https://github.com/lejianwen/rustdesk-api-web/commit/3998c2a9213fcd047252776d0f0db33e6717026c) | MIT，见仓内 `LICENSE` |

均为浅克隆、保留独立 Git 仓库，工作区没有上游源码修改。官方两仓 `hbb_common` 分别初始化到父仓指定的 `fae47e0`、`69cea8d`，准确值见清单。

已另取得官方客户端 tag `1.5.0`（`fada664df7a294d1d1a9ca3e7cd3637069122f17`），保持工作区仍为上述 master 快照。1.5.0 是调研当日 [官方 latest 发布](https://github.com/rustdesk/rustdesk/releases/tag/1.5.0)；OSS server latest 是 [1.1.16](https://github.com/rustdesk/rustdesk-server/releases/tag/1.1.16)，本地 server master 不能视作 1.1.16 的相同源码。

对客户端 `user_model.dart`、`ab_model.dart`、`hbbs.dart`、`login.dart`、`peer_model.dart`、`http_service.dart` 执行 `1.5.0..HEAD` 定点比对，没有差异；`src/hbbs_http/sync.rs` 有策略应用代码差异，所调查的 heartbeat/sysinfo 请求响应段没有差异。此结论只覆盖这些代码，不能替代发布二进制联调。

## 证据索引

| ID | 类别 | 已确认内容 | 来源 |
|---|---|---|---|
| E-01 | `confirmed_fact` | 单团队、无多租户；兼容官方客户端、配置API地址；先比较技术栈；共享通讯录第二阶段；独立开发轻量服务。原“后续开源”意向已因最新讨论转为待定 | 用户本次指令与答复，2026-10-30～31 |
| E-02 | `external_reference` | 官方 OSS 的信令/中继职责，以及 Pro API 的区分 | [官方自托管文档](https://rustdesk.com/docs/en/self-host/)；[固定版本环境说明第55行](https://github.com/rustdesk/rustdesk-server/blob/873230e35a8805de7fd057e7eb763c46f7a174ef/docs/environment-variables.md#L55) |
| E-03 | `implementation_fact` | 账号登录、用户状态、token 与退出的客户端解析契约 | 下节 C-01～C-03 |
| E-04 | `implementation_fact` | 个人通讯录的新旧探测与读写契约 | 下节 C-04～C-06 |
| E-05 | `implementation_fact` | 无 Bearer 的后台设备上报，不属于登录前置调用 | 下节 C-07 |
| E-06 | `implementation_fact` | Go 参考项目的功能、存储与复用风险 | 下节社区项目调查 |
| E-07 | `external_reference` | Rust/Python 候选的自述及许可状态 | 下节其他候选；本轮未本地审计 |
| E-08 | `confirmed_fact` | 数据库只支持SQLite与PostgreSQL；接入OIDC/OAuth 2.0统一认证，纳入首期；具体IdP未指定 | 用户新增约束与协议答复，2026-10-31 |
| E-09 | `external_reference` | OIDC以OAuth 2.0为基础提供身份层；支持发现与ID Token验证；OAuth安全规范指导授权码流程保护 | [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html)、[OIDC Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html)、[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html) |
| E-10 | `confirmed_fact` | 后端使用Rust；未将具体框架和前端方案视为用户已确认 | 用户技术栈答复，2026-10-31 |
| E-11 | `confirmed_fact` | 用户提出自动创建用户、自动映射角色目标；最新分期与默认策略以E-12为准 | 用户身份自动化要求，2026-10-31 |
| E-12 | `confirmed_fact` | 角色自动映射复杂度高时允许不放首期；未匹配默认拒绝，并允许配置普通成员兜底。没有要求将全部映射能力无条件后移 | 用户分期与无匹配策略答复，2026-10-31 |
| E-13 | `confirmed_fact` | 全部按推荐方案推进，并执行本地登录、OIDC登录、个人通讯录读写和退出撤销验证；授权进入M0实现与运行验证 | 用户执行指令，2026-10-31 |
| E-14 | `confirmed_fact` | M0通过后要求进入下一阶段开发；本轮按PRD的M1账号闭环推进，包括基础Web账号入口与相应安全验证 | 用户后续开发请求，2026-10-31；具体实现拆分属于研发方案 |

## 客户端契约证据

源码路径相对于 `upstream/rustdesk/`，链接固定到调研提交。以下均为 `implementation_fact`。

| ID | 调用 | 可确认行为 | 精确证据 |
|---|---|---|---|
| C-01 | `POST /api/login` | 请求含 `username,password,id,uuid,autoLogin,type`，type=`account`，另有 `deviceInfo`；password 为输入原文，无客户端 MD5/SHA256。成功响应需 `type: access_token`、`access_token` 和 user 对象 | [`login.dart:842`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/common/widgets/login.dart#L842)、[`hbbs.dart:155`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/common/hbbs/hbbs.dart#L155)、[`user_model.dart:193`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/user_model.dart#L193) |
| C-02 | `POST /api/currentUser` | Bearer token，body `{id,uuid}`，响应直接为 user；400 清账号/token，401 还清通讯录/group。User 识别 name、display_name、email、is_admin 等；status=0 disabled、-1 unverified | [`user_model.dart:53`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/user_model.dart#L53)、[`hbbs.dart:36`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/common/hbbs/hbbs.dart#L36) |
| C-03 | `POST /api/logout` | Bearer token，body `{id,uuid}`；2秒超时后客户端仍执行本地退出。服务端撤销必须独立验证，不能用UI退出代替撤销验收 | [`user_model.dart:170`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/user_model.dart#L170)、[认证头 `common.dart:2729`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/common.dart#L2729) |
| C-04 | `POST /api/ab/personal` | 空body探测；404 才明确回退 LegacyAb；200 读取 `{guid}`，其他错误停止本轮拉取 | [`AbModel._getPersonalAbGuid`，`ab_model.dart:262`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L262) |
| C-05 | Legacy `GET/POST /api/ab` | GET 外层 `data` 为JSON字符串，另含 `licensed_devices`；内层为 tags、peers、tag_colors 等。POST 同样是 `data` 包裹的整本快照；200空body可成功 | [`LegacyAb.pullAbImpl:1008`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L1008)、[`pushAb:1055`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L1055)、[序列化第1309行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L1309) |
| C-06 | 新版 Ab | 除 personal 外还调用 settings、shared/profiles、peers分页、tags及联系人/标签增删改；写操作的成功格式不能任意包裹 | [`ab_model.dart:1432`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L1432)、[写操作第1548行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L1548)、[响应解析第2002行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/ab_model.dart#L2002) |
| C-07 | `POST /api/sysinfo`、`POST /api/heartbeat` | 独立后台循环，无Bearer；含id/uuid等上报。sysinfo成功标记 `SYSINFO_UPDATED`；心跳可能带连接观察数据。iOS排除该循环 | [`start_hbbs_sync_async`，`sync.rs:87`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/src/hbbs_http/sync.rs#L87) |
| C-08 | `GET /api/login-options` | 登录框立即请求，未提供第三方登录时可回200 `[]`；404、空body或null会显示错误/重试区，不能当成空列表 | [`user_model.dart:240`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/user_model.dart#L240)、[`login.dart:740`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/common/widgets/login.dart#L740) |
| C-09 | `GET /api/device-group/accessible`、`GET /api/users`、`GET /api/peers` | 登录后group面板启用时顺序请求，带Bearer和分页参数；不提供团队目录时可分别回200 `{"total":0,"data":[]}`；空body/null不符合列表解析 | [`group_model.dart:103`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/group_model.dart#L103)、[第160行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/group_model.dart#L160)、[第224行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/flutter/lib/models/group_model.dart#L224) |
| C-10 | `POST /api/oidc/auth`、`GET /api/oidc/auth-query` | 发起请求、浏览器认证与轮询领取本服务会话，准确形状见下方补充；此适配协议不等同于本服务对IdP的OIDC协议 | [`account.rs:160`](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/src/hbbs_http/account.rs#L160)、[轮询第179行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/src/hbbs_http/account.rs#L179)、[状态处理第291行](https://github.com/rustdesk/rustdesk/blob/33a154a5d7502419381dc601aff1e4b0c660054d/src/hbbs_http/account.rs#L291) |

P0 建议优先实现 C-04 的合法404回退及 C-05 旧版单个人簿。它是官方1.5.0源码仍包含的路径，不意味着兼容所有旧版本或提供 Pro 多通讯录。是否采用此路径，需 M0 实机确认。无效会话应先返回认证失败，不能把401伪装为404来触发回退。

旧版个人簿明确携带远控凭据派生值 `hash`：`LegacyAb._serialize` 使用 `toCustomJson(includingHash: true)`（`ab_model.dart:1309`），`peer_model.dart:70` 序列化元数据及hash，不输出password。它与账号登录原文password不同，也不能当成普通备注记录到日志。历史P0仅同步元数据；最新E-17已确认扩展hash同步，取代D-07旧目标，见末尾新证据和独立验收。

C-08/C-09 应纳入最小兼容实现，避免登录或团队页出现无谓报错。固定未提供的列表可以返回协议定义的空集合；真实数据库或认证错误不能伪装为空集合。`group_model.dart` 的所述调用也已与1.5.0定点核对。

### OIDC客户端适配补充（C-10）

以下为固定源码事实，`src/hbbs_http/account.rs`已定点与tag1.5.0比对无差异：

- 入口发现：`GET /api/login-options`可返回 `["oidc/company"]`，客户端去掉前缀，以`company`作为op。另支持`common-oidc/`后接包含name/icon的JSON数组字符串。见`user_model.dart:240`、`login.dart:905`。
- 发起：`POST /api/oidc/auth`包含`op,id,uuid,deviceInfo,apiDomain`，响应直接为`{"code":"…","url":"https://…"}`（`account.rs:25`、`:160`）。此code是客户端轮询凭据，不应混同IdP授权码。
- 浏览器与轮询：客户端打开url，然后请求`GET /api/oidc/auth-query?code=…&id=…&uuid=…`；源码按约1秒间隔等待、总超时180秒。等待态识别`{"error":"No authed oidc is found"}`；其他字符串错误会终止。见`account.rs:17`、`:179`、`:291`，`login.dart:309`、`:399`。
- 成功：直接返回`{"access_token":"本服务token","type":"access_token","user":{"name":"…","info":{}}}`；Rust的`AuthBody` / `UserPayload`要求这些字段，尤其user.info不能因普通账号路径能省略就从此路径删除。见`account.rs:79`、`:99`。`body`包装是客户端HTTP工具内部细节，服务端不增加此层。
- 官方这条路径靠轮询收取结果，没有要求IdP直接回调本机客户端。IdP回调路由、换码和校验由本服务设计；不据此宣称客户端可直接配置任意OIDC提供方。管理Web复用IdP及身份映射是本项目产品要求，尚未实现或验证。

### 统一认证标准与设计边界（E-09）

OIDC Core定义了OAuth 2.0之上的身份层及ID Token验证，Discovery定义提供方元数据获取。建议使用授权码流程、PKCE S256及state/nonce，验证签名、issuer、audience和有效期，按issuer+sub识别外部身份；回调地址固定登记。上述是本项目设计要求，参考[Core身份与Token验证](https://openid.net/specs/openid-connect-core-1_0.html#IDTokenValidation)、[Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html)、[OAuth安全最佳实践](https://www.rfc-editor.org/rfc/rfc9700.html)，不能当作客户端已经实现或服务已经通过验证。

## 社区项目调查

以下路径相对于 `references/rustdesk-api/`，均为 `implementation_fact`；只做静态检查，没有复现安全问题或运行服务。

- 路由：`http/router/api.go:24`、`:76`、`:107`，覆盖登录、OIDC、设备上报、审计、旧/新版通讯录；`http/router/admin.go:22` 包含账号、设备、分组、通讯录集合、OAuth、日志、分享及服务命令，范围大于本项目首期。
- 数据库：`cmd/apimain.go:146` 支持 SQLite/MySQL/PostgreSQL，`conf/config.yaml:26` 默认 SQLite；Redis 并非硬依赖（`cmd/apimain.go:127`）。未配置 cache 时不是默认启用 file cache。OAuth 临时状态在 `service/oauth.go:63` 的 `sync.Map`，重启丢失。
- 密码：`utils/password.go:10` 使用 bcrypt，并兼容历史 MD5密码校验后升级。新项目不应因此引入历史密码格式。
- Token：`service/user.go:89` 在未设置 jwt.key 时按 `MD5(username+time)` 生成 token；配置 key 时使用 JWT。`:97` 存数据库，`:197` 登出删除，`:280` 改密清理。若基于该项目开发，需优先替换可预测 token 生成方式，再验证撤销和重启行为。
- 管理权限：`http/router/admin.go:51`、`:57` 的服务命令路由只有登录组保护，未添加与其他管理能力相同的 `AdminPrivilege`；`http/controller/admin/rustdesk.go:104`、`service/serverCmd.go:59` 可进入本机 TCP 命令转发。静态证据提示普通已登录用户可能触达服务命令，尚未运行复现；本项目首期建议不引入该功能。
- 强制账号连接：`README.md:17`、`:279`、`:300` 把强制登录与其 fork server 的 `MUST_LOGIN` 联系起来。设备上报可由 API 自行接收，不等于官方 OSS 信令服务执行账号 ACL。
- 管理端：`../rustdesk-api-web/package.json:9` 使用 Vue 3、Element Plus、Pinia、Vue Router、Axios、Vite；`src/utils/auth.js:9` 使用 localStorage 保存 token。可以参考页面，但需要独立审查会话与权限。
- 资源边界：API顶层 MIT 不代表内嵌 webclient 的所有资源同许可；上游当前提交说明移除 webclient2，管理端 `src/utils/webclient.js:12` 仍有该入口。本项目首期不包含 Web 远控，也不复制这些资源。

对应固定源码入口：[API树](https://github.com/lejianwen/rustdesk-api/tree/c5687e150615bfb00d9d105f0883a2802750a8ad)、[管理端树](https://github.com/lejianwen/rustdesk-api-web/tree/3998c2a9213fcd047252776d0f0db33e6717026c)。最近拉取的 API 与管理端提交日期分别为2025-09-29、2025-08-31；不能用仓库可访问或功能多推断维护充分、安全或生产成熟。

## 其他候选

以下是 `external_reference`，本轮只查项目原始材料，未本地拉取、构建或审计。

| 项目 | 原始材料中的定位 | 对本项目的含义 |
|---|---|---|
| [dockers-x/rustdesk-console](https://github.com/dockers-x/rustdesk-console) | Rust、顶层MIT，自述为Go项目重写，包含管理UI、账号和通讯录等 | Rust路线的候选参考；是否复用仍需源码与兼容验证 |
| [sctg-development/sctgdesk-api-server](https://github.com/sctg-development/sctgdesk-api-server) | Rust/Rocket、AGPL-3.0，README自述仍在开发、未准备好生产使用 | 可研究接口，不能据此直接定为生产基线 |
| [kingmo888/rustdesk-api-server](https://github.com/kingmo888/rustdesk-api-server) | Python/Django，README标称支持1.3.1；本轮未确认许可证文件 | 当前客户端适配与许可仍待核实，暂不作为代码复用基线 |

## 尚未覆盖

本文静态调查不提供发布二进制行为、移动端/各桌面平台差异、LDAP/2FA、远控鉴权强制点、依赖漏洞扫描、嵌入资源完整许可核验、生产容量或备份演练证据。M0已另行执行真实Keycloak与双库HTTP验证，结果和未覆盖范围见[M0记录](m0-validation.md)；官方客户端实机结果仍单独记录。


### E-15 本轮M2/M3授权与交付边界

- 分类：`confirmed_fact`；来源：2026-10-31用户标注“同步专项验收、账号审计和生产运维能力”并要求“完成这些任务”。
- 采用：补齐RD-007/008/010/011/012/013/014/016的同步专项、账号审计与单实例部署恢复。
- `inference`：无实际生产服务器和域名，先交付Compose及本机HTTPS演练；审计默认90天、可配0永久，不采集IP；登录限流按账号与进程总量，不读取代理IP头。参数可由部署者调整。
- 运维依据：SQLite一致性备份采用[官方Backup API](https://www.sqlite.org/backup.html)；PostgreSQL自定义归档及恢复采用[PG16 pg_dump](https://www.postgresql.org/docs/16/app-pgdump.html)；本机TLS采用[Caddy internal CA](https://caddyserver.com/docs/caddyfile/directives/tls)，测试只信任该次导出的CA文件，不更改系统信任。
- 实测结果归属：`docs/m23-validation.md`及`validation/results/`，历史M0/M1证据保持独立；不扩充官方客户端实机支持矩阵。

### E-16 实际实例部署

- 分类：`confirmed_fact`；来源：2026-10-31用户确认准备实际部署，指定 `me-aliyun`、`desk.166198.xyz`、本服务器 PostgreSQL 容器，暂不接入 OIDC。
- 采用：复用已有 Traefik 与共享 PG 容器，为账号服务创建独立数据库及专用角色；上线结果独立归属 [部署记录](deployment-2026-10-31.md)，不改写历史本机验收结果。
- 代理依据：[Traefik 3.6 Docker 配置](https://doc.traefik.io/traefik/v3.6/expose/docker/advanced/)、[TLS options](https://doc.traefik.io/traefik/v3.6/reference/routing-configuration/http/tls/tls-options/)、[路由观测配置](https://doc.traefik.io/traefik/reference/routing-configuration/http/routing/observability/)。同域名既有路由的 TLS options 必须在新 router 保持一致；实际配置还须通过目标环境解析和公网请求核验。


### E-17 设备与凭据范围更新

`confirmed_fact`：用户要求个人通讯录、连接密码跨设备同步、设备归属和“我的设备”按官方方案实现；经说明远控ACL需要信令/中继执行点后，用户明确“不做 按用户、用户组限制远控 功能”。不新增用户组或远控ACL；当时未包含信令变更，后续E-18追加握手兼容范围。

`implementation_fact` C-11：官方1.5.0（fada664df7a294d1d1a9ca3e7cd3637069122f17）client.rs:4835–4848先计算SHA256(password || salt)，再结合challenge；:4761–4778读取个人簿hash；ab_model.dart:443–455收到remember事件后更新既有peer，:1309–1322将hash写入legacy快照。该hash是可用凭据，非普通元数据。本轮依据上述客户端协议独立实现AES-GCM静态加密，不推断闭源服务端加密方法。来源及精确链接见[扩展设计](device-credentials-design.md)。

`implementation_fact` C-12：官方group_model.dart依次取device-group、users、peers，按user_name过滤本人；hbbs.dart:80–104接受目录status但toPeer未用其设置在线。登录id/uuid和deviceInfo为自报材料。官方公开文档描述登录/管理员分配的归属入口，但不能据此确认闭源服务端的冲突、退出或转移策略；本服务策略单独明确，不称其内部行为与Pro完全相同。

E-17取代D-07的历史metadata-only产品目标；M0/M1/M23报告及首次生产schema3仍保留当时实现和验证范围，新目标不倒写为历史已通过。

### E-18 登录后远控兼容

`confirmed_fact`：用户退出账号后远控恢复，随后明确“使用PR #734”“不要做过度验证”。采用固定PR提交`ef2166f7760905a18b6588f0882d633d3688ad2f`与其子模块，停止独立握手补丁；只切换hbbs，首轮开关Y/N/N/N，不增加远控ACL。固定来源、维护边界及实际结果见[兼容记录](rendezvous-compatibility.md)。2026-10-31核对PR仍未合并，不能标为官方稳定版。
