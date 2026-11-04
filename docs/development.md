# 开发与维护入口

- 文档状态：`drafted`；维护者：项目发起人，源码与文档一致性由修改者核对。
- 整理日期：2026-11-03；依据当前源码、锁文件及 2026-10-31 的验收记录。
- 开发状态：账号 API、Web、双数据库、设备目录及可选凭据同步已有实现与验收；本次仅整理文档，未重跑测试或检查线上状态。
- 适用范围：单实例账号服务及其兼容集成。参数和接口以 [API README](../api/README.md) 为入口；生产操作使用 [运维入口](operations.md) 和 [部署手册](../deploy/README.md)。

## 先找到要改的部分

| 目录或文件 | 职责与阅读入口 |
|---|---|
| `api/src/main.rs` | CLI、环境配置、数据库连接、OIDC初始化、启动就绪、路由组合与优雅退出 |
| `api/src/compat.rs` | 官方客户端账号、个人通讯录、设备目录兼容协议 |
| `api/src/store.rs` | 用户、会话、个人簿、设备归属及迁移；写事务和权限复核边界 |
| `api/src/admin.rs`、`web_auth.rs` | 管理接口、Web Cookie、Origin/CSRF、账号与会话操作 |
| `api/src/oidc.rs` | Discovery、授权码/PKCE、回调与轮询绑定、身份及角色映射 |
| `api/src/credentials.rs` | 个人簿 hash 的密钥环、AES-GCM 与存量密文验证 |
| `api/src/audit.rs`、`limits.rs`、`operations.rs` | 审计类型、登录限流、就绪探针及脱敏请求日志 |
| `api/src/web_files.rs` | `/admin/` 静态资源服务 |
| `api/migrations/sqlite/`、`postgres/` | 两套前向迁移，当前版本为 schema 4 |
| `api/tests/` | 账号、会话、审计、同步、设备和凭据的 Rust/HTTP 定点测试 |
| `web/src/App.vue`、`api.ts` | 页面入口、账号流程及同源 API 调用 |
| `web/src/AuditPanel.vue`、`audit.ts` | 审计筛选、分页与异步数据状态 |
| `web/src/DevicePanel.vue`、`devices.ts` | 我的设备、管理员归属维护及过期响应处理 |
| `web/tests/`、`web/vite.config.ts` | Web测试、`/admin/` base 与本地开发代理 |
| `validation/` | 隔离依赖、HTTP/OIDC与运维 runner；使用方法见[验证说明](../validation/README.md) |
| `deploy/`、根 `Dockerfile` | 账号服务镜像、Compose、初始化密钥、备份恢复 |
| `deploy/rustdesk-pr734/` | 已采用的固定 hbbs PR 构建与实例 overlay，见下文 |
| `docs/` | 需求、设计、实现说明及有日期的验收证据 |

业务约束先读 [业务逻辑](logic.md)，跨模块关系读 [架构](architecture.md)，需求状态读 [PRD](PRD.md)。
历史验收材料保留当时的构建和范围，不能仅凭旧报告判定新修改已通过。

## 准备环境与依赖

- Rust/Cargo 工具链需支持 `api/Cargo.toml` 的 edition 2024；manifest 未声明 `rust-version`，不在本文另定最低编译器版本。
- Rust 依赖声明包括 Tokio 1、Axum 0.8.9、SQLx 0.8.6、openidconnect 4.0.1；实际解析版本以 [Cargo.lock](../api/Cargo.lock) 为准。
- [Web manifest](../web/package.json) 的 Node engines 为 `^20.19.0 || >=22.12.0`；`npm test` 使用类型擦除功能，要求 Node 22.12+，不能仅按构建下限选择测试环境。
- Web声明 Vue 3.5.44、Vite 8.3.5、TypeScript 5.9.3 和 vue-tsc 3.3.14；使用 `npm ci` 按 [package-lock.json](../web/package-lock.json) 安装。
- SQLite 开发不需独立数据库服务；PostgreSQL、真实 OIDC 或容器运维复现另需专用依赖。runner 使用 Python 3，容器场景使用 Docker/Compose v2。
- [Dockerfile](../Dockerfile) 保存容器构建基础镜像参数；这些是可审查的构建输入，不代表“最新版本”或本机 SDK 的长期要求。发布固定 digest 的方法归部署手册。

不要直接升级全部依赖来处理无关修改。依赖升级时同时审查 manifest、对应锁文件与实际受影响行为。
`cargo --locked` 和 `npm ci` 用于复用已有解析结果，锁文件不能代替运行验证。

## 从空环境启动本地服务

以下命令供开发者按需执行；本次文档整理未执行它们。所有路径默认相对项目根目录。
先安装上述工具，确认 `21114` 未被其他实例占用，并使用新建的隔离数据目录。

1. 先构建 Web，再构建 API：

```sh
(cd web && npm ci && npm run build)
cargo build --locked --manifest-path api/Cargo.toml
```

`serve` 和 `ready-check` 检查 `RD_WEB_DIR/index.html`。只启动 Vite 不能满足后端检查，首次开发也必须先有 `web/dist`。

2. 在同一终端准备本次 SQLite 环境与管理员秘密文件：

```sh
umask 077
rd_dev_dir=$(mktemp -d "${TMPDIR:-/tmp}/rustdesk-dev.XXXXXX")
export RD_DATABASE_URL="sqlite://$rd_dev_dir/accounts.sqlite"
export RD_WEB_DIR="$PWD/web/dist"
export RD_WEB_ORIGIN="http://127.0.0.1:21114"
export RD_BOOTSTRAP_PASSWORD_FILE="$rd_dev_dir/bootstrap_password"
python3 - <<'PY'
import getpass
import os
password = getpass.getpass('新管理员密码（至少12字节，不回显）: ')
if not 12 <= len(password.encode()) <= 4096:
    raise SystemExit('密码长度不符合要求，未写入文件')
fd = os.open(os.environ['RD_BOOTSTRAP_PASSWORD_FILE'],
             os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as stream:
    stream.write(password)
PY
```

密码输入失败时先修正，确认秘密文件成功生成后再继续；不要将密码作为 argv 或复制到文档、日志。
这些设置用于新开发实例，不能复用生产 URL、生产库快照或已有 fixture。

3. 应用迁移、首次初始化并启动：

```sh
cargo run --locked --manifest-path api/Cargo.toml -- migrate
cargo run --locked --manifest-path api/Cargo.toml -- bootstrap-admin dev-admin
cargo run --locked --manifest-path api/Cargo.toml -- ready-check
unset RD_BOOTSTRAP_PASSWORD_FILE
cargo run --locked --manifest-path api/Cargo.toml -- serve
```

打开 `http://127.0.0.1:21114/admin/`，使用刚输入的密码登录；另一个终端可执行：

```sh
curl --fail --silent http://127.0.0.1:21114/readyz
```

`bootstrap-admin` 对同一数据库只能成功一次，没有内置默认账号；重新启动应复用该开发库及配置，不重复 bootstrap。
按 Ctrl-C 停止本终端启动的服务。保留数据或秘密前确认用途，秘密转存密码管理器；不要误删其他实例目录。
`main.rs` 还会创建相对工作目录下的 `data/`，实际 SQLite 文件位置仍由本例的 URL 决定。

## PostgreSQL、OIDC 与可选凭据

SQLite 适合无外部依赖的快速开发；PG 用于覆盖另一套 SQL、锁和迁移行为。两者是可选存储后端，不自动复制数据。
切换 URL 等于切换数据源，不会将 SQLite 用户或会话搬进 PG；需要的数据迁移应另行设计和验证。

- PostgreSQL 开发使用专用 database 与有建表权限的应用角色，秘密 URL 写入 `0600` 文件，由 `RD_DATABASE_URL_FILE` 读取；不要与 `RD_DATABASE_URL` 同时设置。
- 首次 CLI 前先保证数据库可达，然后沿用上面的 Web构建、`migrate`、`bootstrap-admin`、`ready-check`、`serve` 顺序。
- 需要真实 Keycloak/PG 的自动化环境时，使用[验证说明](../validation/README.md)中的 `prepare` 和 Compose入口；其 fixture、端口、数据库命名与生命周期由 runner 管理。
- Rust测试的可选 PG 分支读取 `DATABASE_URL`，不同于服务配置 `RD_DATABASE_URL`；必须显式指向专用测试库，未设置时不能声称已跑 PG。
- 自动化 runner 会创建隔离 SQLite/PG 数据，不以正常开发库或生产库作为目标。共享 `21114` 的 runner 与手动服务串行运行，不停止占用端口的其他进程。

OIDC 默认关闭，首次本地启动不要求 IdP。启用参数、角色准入和 secret 文件入口见 [API配置](../api/README.md)，测试IdP准备见验证说明。
开发时只有显式配置允许的 loopback HTTP 可作为 OIDC测试源；正式配置采用 HTTPS，默认 unmatched 为 deny。

`RD_BOOK_KEYS_FILE` 是可选个人簿凭据密钥环；不设置时仅同步 metadata，设备目录仍可使用。
需要本地验证 hash 时可在新的私有目录使用现有 [init-book-keys.py](../deploy/init-book-keys.py) 生成新文件，再设置该变量；格式、轮换和恢复约束见[扩展设计](device-credentials-design.md)及部署手册。
已有密文时不能移除密钥开关绕过校验。hash 可用于远控认证，API持钥可解密，不是端到端加密；密钥备份和库备份分别保管。
管理员丢失访问时的可信 CLI `create-local ... --role admin` 与秘密文件流程见部署手册，不通过重置数据库恢复。

## Web开发代理与生产产物

首次 Web构建完成后，在 API终端改用开发浏览器的 origin：

```sh
export RD_WEB_ORIGIN="http://127.0.0.1:5173"
cargo run --locked --manifest-path api/Cargo.toml -- serve
```

另一终端运行：

```sh
cd web
npm run dev
```

访问 `http://127.0.0.1:5173/admin/`。Vite只绑定 `127.0.0.1:5173`，固定端口被占用时退出；`/admin/api` 与 `/api/oidc` 代理到 `127.0.0.1:21114`。
浏览器使用相对路径，代理保留 Origin；不要改写 Origin 绕过后端校验。OIDC的浏览器与回调须使用相同 hostname，避免混用 `localhost` 和 `127.0.0.1`。

`npm run build` 先做类型检查，再输出 base 为 `/admin/` 的 `web/dist/`；生产由 Axum提供该产物。
`npm run preview` 只有静态预览，不带开发代理。其他工作目录启动 API时显式设置 `RD_WEB_DIR`。
Cookie、CSRF、退出清理及异步过期响应约束以 [Web README](../web/README.md) 和对应源码为准。

## 按变更风险选择最小验证

2026-10-31 已有双库、真实 OIDC、Web、密文恢复及官方1.5.0 Mac/Windows闭环；证据见[扩展验收](device-credentials-validation.md)及[握手兼容记录](rendezvous-compatibility.md)。
本次没有重跑。后续先说明改动影响，再选择必要验证，不把历史完整矩阵变成每次提交的固定动作。

| 变更范围 | 最小验证与扩大条件 |
|---|---|
| 纯文档 | 自检事实来源、相对链接和命令前置条件；不因此启动服务或重做GUI验收 |
| Web业务或状态处理 | 相关 `web/tests`、类型检查与构建；涉及真实交互而自动化不足时补对应页面检查 |
| API业务或权限 | 相关 Rust/HTTP定点用例，检查拒绝路径、用户隔离与事务回滚；影响公共存储行为时覆盖两库 |
| schema、SQL或凭据 | 两库迁移/持久化及相关失败回滚；触及加密、备份或恢复时补相应专项 |
| OIDC协议或准入 | 相关真实IdP流程及绑定、拒绝、撤销用例；不以mock替代此次协议路径 |
| 官方客户端兼容或hbbs握手 | 先定点协议测试；只有影响实机兼容时，再对本次目标设备执行必要登录/同步/连接检查 |
| 镜像或部署脚本 | 配置与安全定点检查；改变运行或恢复行为时再用隔离project/新卷演练 |

常用命令如下，测试名按实际影响选择；这不是要求逐条全跑的清单：

```sh
cargo test --locked --manifest-path api/Cargo.toml --test devices_http
cargo fmt --manifest-path api/Cargo.toml --check
cargo clippy --locked --manifest-path api/Cargo.toml --all-targets -- -D warnings
(cd web && npm test && npm run typecheck && npm run build)
```

真实依赖 runner、密文恢复与部署测试命令统一维护于 [validation](../validation/README.md) 和部署手册。
其中 M0/M1等章节保存阶段复现步骤；对新修改采用本节的风险判断，不默认重放全部阶段。
测试输出记录实际数据库、构建指纹和用例结果；HTTP通过不能代替GUI通过，单机结果不能扩成全平台或容量结论。
只停止本轮创建的进程/容器，恢复测试客户端原配置；会话、测试数据和未完成收尾如实记录。

## 修改存储与兼容代码的边界

迁移必须同步维护 `api/migrations/sqlite/`、`api/migrations/postgres/` 和 `Store::migrate` 的加载与版本判断；已有版本不原地改写。
当前 schema 4 引入 `devices`、会话 `device_id`、`book_credentials` 和 `device_events`。`Store::connect` 会自动前向迁移，因此 `ready-check` 等 CLI也会触库，不能当作无副作用的离线检查。
就绪检查要求 schema 4、数据库写路径和Web产物，探针事务回滚；存量凭据在配置密钥环时一并验证。

变更新版本上限时还需同步 readiness、升级恢复检查及文档。schema 4 不能交给旧 schema 3/M3镜像打开；不支持原地降级。
回退必须使用升级前快照和匹配旧镜像，新写入不会自动合并回旧快照；详细维护窗口与恢复步骤统一归部署手册。
权限、准入和停用检查应保留在写事务内；密码修改、归属变化、会话撤销及审计的原子性不能只靠页面限制。
设备归属不授予或限制远控权限；不在兼容修复中引入用户/用户组远控ACL。

## 上游参考、私有资产与交付

[sources.lock.json](../sources.lock.json) 固定最初 `upstream/`、`references/` 参考源码提交、子模块及许可证，表示研究来源；不表示全部代码已采用或运行兼容。
账号服务与Web使用自己的 Cargo/npm锁文件。更新参考提交时记录用途与差异，不把本地快照当作始终跟随上游。

hbbs采用的 PR #734另有固定提交及 `hbb_common`：见[PR部署包](../deploy/rustdesk-pr734/README.md)和握手兼容记录。
其当前采用基线是 `ef2166f7760905a18b6588f0882d633d3688ad2f`；2026-10-31记录为未合并PR的独立构建，不称为官方稳定发行，也不自动跟随PR更新。
构建中保留该基线 Cargo.lock；Git依赖离线替换、SQLx构建输入与运行镜像边界以PR部署包为准，不能带入生产配置、数据库或私钥。
旧 `patches/rustdesk-server-account-compat/` 方案已停止采用，其测试不作为PR #734证据；`fixes/`、`diagnostics/` 属于其他调查资产，不随账号开发顺手修改。

`validation/.runtime/`、部署 `.secrets`/`.env`、数据库、原始日志和备份均为私有运行资产，不读取或复制进可分享文档、源码或镜像。
[忽略规则](../.gitignore) 还将下载源码、构建产物和本地数据库排除在项目交付之外；忽略不代表备份或删除策略。
可分享验收只使用脱敏的 `validation/results/` 与有日期记录。数据库快照不能替代密钥备份，不清理历史私有资产来制造“干净仓库”。

修改配置/CLI/接口时更新 API README；代理、构建与页面约束更新 Web README；runner前置条件更新 validation README；部署、密钥和恢复更新运维入口/部署手册。
业务规则、架构与需求状态分别更新 logic、architecture、PRD；新增验收记录保留日期与证据，不覆盖历史指纹。
待办、维护责任与尚未验收范围统一交 [维护计划](maintenance.md)，不在多个文档复制一份待办表。
项目使用GitHub公开仓库，main变更走PR，通过Rust/Web CI并解决审查讨论后由维护者决定合并；CodeRabbit接入状态和依赖安全自动化见[安全维护](../SECURITY.md)。自有内容采用[非商业许可](../LICENSE)，第三方保留原许可，范围见[NOTICE](../NOTICE)；镜像/版本发布另行安排。
