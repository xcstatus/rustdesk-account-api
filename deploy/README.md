# 单实例部署与恢复

本文由本仓库维护，适用 schema 4 的账号服务（设备目录与可选个人通讯录凭据同步）。配置与脚本已实现；运行验收以实际报告为准，历史 M3 部署结果不代表本次 schema 4 与密钥功能已经上线。文档契约与验证映射见 [document-brief](assets/document-brief.md)。不支持多副本，也不包含 RustDesk `hbbs` / `hbbr` 部署。

所有命令从仓库根目录执行。要求 Docker Engine / Docker Desktop、Compose v2、Python 3；Linux 宿主机需可设置 secret 文件 UID。Docker 管理权限等同宿主机管理权限。正式发布前在测试机完整演练备份恢复。

## 镜像与配置

```sh
cp deploy/.env.example deploy/.env
docker build --pull -t rustdesk-account-api:0.1.0-devices .
```

schema 4 与 `check-credentials` 必须使用本次新构建的 `0.1.0-devices` 或明确验证的更新镜像；历史 M3 镜像不能打开升级后的数据库。架构专用 tag 在实例 `.env` 显式设置。

镜像多阶段使用 `node:22.22.1-bookworm-slim`、`rust:1.93.1-bookworm`、`debian:bookworm-slim`；锁定 npm / Cargo 依赖。运行阶段带 CA、curl、sqlite3，API 为 UID/GID `10001:10001`，二进制为 PID 1。Web 产物位于 `/app/web/dist`。Compose 对 API 设置只读根文件系统、`/tmp` tmpfs、去除 capabilities、30 秒 SIGTERM 停机；`/app/data` 独立持久卷。

默认外部镜像为 `caddy:2-alpine`、`postgres:16-alpine`。这些 tag 会移动；发布人员应先拉取并审查镜像，再用实际 `RepoDigests` 固定 `.env` 中的 `CADDY_IMAGE`、`POSTGRES_IMAGE` 与 `RD_API_IMAGE`。构建基础镜像可用 `--build-arg NODE_IMAGE=...@sha256:...`、`RUST_IMAGE`、`RUNTIME_IMAGE` 固定。不要填写猜测的 digest。记录本次发布实际结果：

```sh
docker image inspect rustdesk-account-api:0.1.0-devices --format '{{.Id}} {{json .RepoDigests}}'
docker image inspect caddy:2-alpine postgres:16-alpine --format '{{.Id}} {{json .RepoDigests}}'
```

编辑 `deploy/.env` 的非秘密配置：

- `RD_DOMAIN=accounts.example.com`；`RD_WEB_ORIGIN=https://accounts.example.com`。origin 不包含路径、query 或末尾 `/`。
- `ACME_EMAIL` 为有效运维邮箱。公网 DNS 指向本机；TCP 80/443 可达；`CADDY_TLS_DIRECTIVE` 留空以使用 ACME。
- 各 `RD_*_VOLUME` 名称属于本部署；不能复用其他实例的卷。
- `RD_SECRETS_DIR=./.secrets` 相对 `deploy/`。该目录、`.env` 和 `backups/` 已忽略，不提交到版本控制。

选择一种后端，建立当前 shell 的简写。**以后同一部署始终使用相同 project 与文件组合。**

```sh
# SQLite
dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.sqlite.yml "$@"; }
# PostgreSQL：改为下面的函数，不同时启用两份配置
# dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.postgres.yml "$@"; }
dc config -q
```

只有 Caddy 发布端口，API / PostgreSQL 只在 Compose 网络中可见。API 显式允许 `0.0.0.0:21114`，不信任 `Forwarded` / IP 请求头；Caddy 删除客户端提供的代理头并覆盖 Host、协议。限流按账号与进程全局计数：`RD_LOGIN_WINDOW_SECONDS=60`、`RD_LOGIN_PER_ACCOUNT=10`、`RD_LOGIN_GLOBAL=300`、`RD_LOGIN_MAX_KEYS=1024`。计数在重启后清空，不作为集群限流使用。

## Secrets 与首次初始化

```sh
# PostgreSQL 时将 sqlite 改为 postgres；启用机密 OIDC 客户端时加 --oidc
python3 deploy/init-secrets.py --backend sqlite
```

初始化器只创建新目录，文件模式 `0600`，目录 `0700`，不输出密码。生成 `database_url`、`bootstrap_password`、`oidc_client_secret`；PG 另有 `postgres_password`（仅初始化/备份管理角色 `rustdesk_admin`）及独立 `app_password`（API角色 `rustdesk`），URL 使用后者。首次新卷初始化脚本创建 `NOSUPERUSER / NOCREATEDB / NOCREATEROLE / NOREPLICATION / NOBYPASSRLS` 的应用角色，并把 `rustdesk` 数据库归它所有。保存 bootstrap 密码至密码管理器，通过可信本机终端私下读取文件。秘密备份同样加密并限制访问。

角色初始化只在空 PGDATA 卷执行，不能直接用本配置接管旧的单角色集群；旧集群应先停写备份，再按本手册恢复到带新角色的新卷。不要把管理角色密码提供给 API。

Linux 下本地 Compose 的 file secrets 挂载保留宿主文件权限，不能依赖 Compose 的 `uid` / `gid` 声明改变归属。API secrets 应归 UID 10001：

```sh
sudo chown 10001:10001 deploy/.secrets/database_url deploy/.secrets/bootstrap_password deploy/.secrets/oidc_client_secret
sudo chmod 600 deploy/.secrets/database_url deploy/.secrets/bootstrap_password deploy/.secrets/oidc_client_secret
# 仅PG：先查询所用实际镜像的 postgres UID，再设置归属；不要假定是999或70
pg_uid=$(docker run --rm --network none --entrypoint id postgres:16-alpine -u postgres)
sudo chown "$pg_uid:$pg_uid" deploy/.secrets/postgres_password deploy/.secrets/app_password
sudo chmod 600 deploy/.secrets/postgres_password deploy/.secrets/app_password
```

若已固定其他 `POSTGRES_IMAGE`，上面的查询必须改用同一镜像。Docker Desktop / rootless 的 UID 映射不同，先用一次性 CLI 验证读取权限；不能用放宽为全局可读来绕过。初始化失败时保留文件，排查权限后重试操作；不要覆盖已有秘密目录。

API 支持 `RD_DATABASE_URL_FILE`、`RD_BOOTSTRAP_PASSWORD_FILE`、`RD_OIDC_CLIENT_SECRET_FILE`。同时设置同名 plain env 与 `_FILE` 会报错；读取时仅去除一个末尾换行，保留其他密码空白；空 OIDC secret 表示 public client。不要把密码、完整数据库 URL、token 或授权 URL 写入命令行、`.env` 或日志。

```sh
# 仅PG，首次CLI前先启动数据库
# dc up -d --wait postgres
dc run --rm --no-deps api migrate
dc run --rm --no-deps -v "$PWD/deploy/.secrets/bootstrap_password:/run/secrets/bootstrap_password:ro" \
  -e RD_BOOTSTRAP_PASSWORD_FILE=/run/secrets/bootstrap_password api bootstrap-admin administrator
dc up -d --no-build --wait
curl --fail --silent https://accounts.example.com/readyz
```

打开 `https://accounts.example.com/admin/` 登录。bootstrap 只能成功一次；完成并保存凭据后将 bootstrap 文件移至离线密钥库。离线恢复管理员可用相同临时 secret 挂载方式执行 `create-local recovery-admin --role admin`，处理后撤销临时访问。不要重置数据库来恢复管理员。

## 可选个人通讯录凭据密钥

不配置 `RD_BOOK_KEYS_FILE` 时，个人通讯录仅同步 metadata；设备目录不依赖此开关。启用后，服务用 AES-256-GCM 加密保存 RustDesk 可复用的密码 hash，服务端持有密钥并能解密，因此不是端到端加密。数据库中已经存在 `book_credentials` 密文时，缺少密钥、缺少历史 key ID、错误 key 值或认证失败都会拒绝 API/CLI 启动，不能通过去掉 overlay 恢复 metadata-only 模式。

先创建上文 secret 目录，再生成独立密钥文件。生成器使用系统随机源创建 32 bytes key，只接受不存在的目标，拒绝已有文件和软链，不输出 key。下面的 Linux 命令同时设置容器 UID/GID；Docker Desktop / rootless 按实际 UID 映射运行，不支持 chown 时省略 owner 参数，再通过 CLI 验证可读性，不能放宽文件权限。

```sh
sudo python3 deploy/init-book-keys.py --owner-uid 10001 --owner-gid 10001
# 非默认路径：加 --file /private/path/book_keys.json；父目录必须已存在。
```

文件必须为普通非软链 JSON 文件，权限严格 `0600`，形如 `{ "active_key": "v1", "keys": { "v1": "<标准base64编码的32随机bytes>" } }`。此处仅为格式说明，不能用该占位值。将文件备份到独立、加密的离线密钥库；**密钥丢失后数据库密文无法恢复**。密钥不能写入 `.env`、Git、公开 manifest、日志或工单。

在 `.env` 设置非秘密路径 `RD_BOOK_KEYS_HOST_FILE=./.secrets/book_keys.json`（相对 `deploy/`；自定义 `RD_SECRETS_DIR` 不会自动改变此路径），并在该部署已有的 Compose 文件组合末尾加入 [密钥 overlay](compose.book-credentials.yml)。它可用于 SQLite、内置/外部 PostgreSQL，并可与 OIDC、Traefik overlay 组合。例如：

```sh
dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.sqlite.yml -f deploy/compose.book-credentials.yml "$@"; }
# 内置PG改为compose.postgres.yml；外部PG改为compose.external-postgres.yml。
# 已有Traefik时保留 -f deploy/compose.traefik.yml，仍放在密钥overlay之前。
dc config -q
dc stop api
dc run --rm --no-deps api check-credentials
dc up -d --no-build --wait api
```

`check-credentials` 在连接新 schema 后验证库内所有凭据，不输出 key、密码 hash 或密文；连接可能执行迁移，因此只对兼容版本的目标库执行。Traefik 部署的临时 `dc run` 同样加 `-l traefik.enable=false`。此检查也验证 UID10001 能读取实际挂载文件；启动失败时保留数据与密钥，私下排查权限和版本。

轮换时在可信离线环境生成新随机 key，用新 ID 加入原有 `keys` 并切换 `active_key`，保留旧 ID 对应的原始 key；通过私有临时文件和原子替换更新整个 JSON，并恢复 `0600`/UID10001，重建 API 容器后再执行检查。只改 active key 不会重加密已有行，之后写入的凭据才使用新 key。最多保留16个 key；只要当前库或保留期内任一备份仍引用旧 ID，就必须保留其旧 key。更新独立加密密钥备份后再结束维护窗口。

## 外部 PostgreSQL（不启用 OIDC）

使用已有或公共 PostgreSQL 时，以本节替代上面的后端选择、secret 初始化和首次启动步骤。[外部 PG 配置](compose.external-postgres.yml)只运行 API 与 Caddy，不创建数据库、`postgres` 服务、PG 数据卷或本地 PG 管理密码文件；API 的只读、非 root、健康检查、日志轮转及数据卷设置与其他部署配置一致。本节不加载 `compose.oidc.yml`。Caddy 仍需独占配置的 HTTP/HTTPS 宿主端口；已有反向代理占用这些端口时，需另行配置代理接入后再启动。

先由数据库管理员准备**独立数据库和专用应用角色**，不要把本服务装进其他业务已有库。应用角色应为该库及其迁移对象的 owner，具备本库连接和目标 `public` schema 的 `USAGE` / `CREATE` 权限，以便首次建表及后续迁移；角色使用 `LOGIN / NOSUPERUSER / NOCREATEDB / NOCREATEROLE / NOREPLICATION / NOBYPASSRLS`，只授予本服务所需权限，不给 API 管理员或其他业务库凭据。网络访问规则仅放行实际 API 出口。API 连接会自动迁移，因此备份和维护时还需停止所有 CLI 写者。

数据库地址必须从 **API 容器**可达；`127.0.0.1` 在容器内指 API 自身，不能用于宿主机或另一容器的 PostgreSQL。经公网或不可信网络连接时，必须使用数据库支持的 TLS，并在 URL 中显式指定 `sslmode=verify-full`，同时验证可信 CA 和数据库 hostname；不要依赖驱动默认的 `prefer`，也不要把仅加密的 `require` 当成完整身份验证。URL 格式示意如下，凭据特殊字符需做 URL percent-encoding：

```text
postgres://APP_ROLE:ENCODED_PASSWORD@db.example.com:5432/DEDICATED_DATABASE?sslmode=verify-full
```

实际 URL 只写入 secret 文件；Compose 设置 `RD_DATABASE_URL_FILE=/run/secrets/database_url`，从 `RD_SECRETS_DIR` 下的 `database_url` 挂载读取，不要同时设置 `RD_DATABASE_URL`。若数据库证书由私有 CA 签发，需另加经过审查的 Compose overlay，将可信 CA 文件只读挂载到 API，并在 secret URL 中通过 `sslrootcert=/容器内/CA文件路径` 指定；基础配置没有该挂载。数据库 TLS 能力、证书链、hostname 和容器网络连通性必须在目标环境验收；此处配置解析检查不能证明 TLS 连接成功。

沿用上文镜像构建及域名配置，在 `deploy/.env` 设置 `RD_SECRETS_DIR=./.secrets`，为 `RD_DATA_VOLUME`、`RD_CADDY_VOLUME` 选独立名称；`POSTGRES_IMAGE`、`RD_POSTGRES_VOLUME` 在本配置中不使用。下面两种入口选择一种，并在此部署后续命令中始终使用同一函数。独立 Caddy 入口：

```sh
dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.external-postgres.yml "$@"; }
dc config -q
```

若已有 Traefik 占用 80/443，改用 [Traefik overlay](compose.traefik.yml)。先在 `deploy/.env` 设置必需的 `RD_TRAEFIK_NETWORK`（现有 Traefik 网络）、`RD_DATABASE_NETWORK`（现有 PostgreSQL 网络）、`RD_TRAEFIK_CERTRESOLVER`（现有证书 resolver）；两个网络必须已存在，API 需要能经数据库网络解析并连接 PostgreSQL。`RD_TRAEFIK_HTTP_ENTRYPOINT` / `RD_TRAEFIK_HTTPS_ENTRYPOINT` 默认 `web` / `websecure`，如现有代理不同则按实际名称设置。overlay 将 Caddy 放入 `standalone-caddy` profile，默认只启动 API，并配置 HTTP 跳 HTTPS；不要启用该 profile。使用以下组合替代上面的函数，再执行本节的初始化与启动命令：

```sh
dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.external-postgres.yml -f deploy/compose.traefik.yml "$@"; }
dc config -q
```

同一 hostname 已有路由时，还需在实例 overlay 中保持相同的 TLS options，例如 `traefik.http.routers.rdaccounts.tls.options: existing-options@file`；跨 provider 引用需要后缀，且同域名的不同 TLS options 可能产生冲突，见 [Traefik TLS options](https://doc.traefik.io/traefik/v3.6/reference/routing-configuration/http/tls/tls-options/)。使用 Traefik 时，以下临时 CLI 容器的 `dc run` 加 `-l traefik.enable=false`，避免将维护命令注册为后端；正式 API 服务保留原路由标签。

以下命令只接受新 secret 目录，通过隐藏输入接收数据库 URL，并生成本地 bootstrap 密码，不创建 PG 密码文件。不要运行 `init-secrets.py --backend postgres`，它生成的是内置数据库连接配置。

```sh
python3 - <<'PY'
import getpass
import os
from pathlib import Path
import secrets
from urllib.parse import urlsplit

os.umask(0o077)
database_url = getpass.getpass('External PostgreSQL URL (hidden): ')
try:
    parsed = urlsplit(database_url)
    valid = (parsed.scheme in ('postgres', 'postgresql') and parsed.hostname
             and parsed.username and parsed.path not in ('', '/'))
except ValueError:
    valid = False
if not valid:
    raise SystemExit('Invalid external PostgreSQL URL; no files created.')
directory = Path('deploy/.secrets')
directory.mkdir(mode=0o700, exist_ok=False)
for name, value in {'database_url': database_url,
                    'bootstrap_password': secrets.token_urlsafe(30)}.items():
    with (directory / name).open('x', encoding='utf-8') as stream:
        stream.write(value + '\n')
    (directory / name).chmod(0o600)
print('External database and bootstrap secret files created; values were not printed.')
PY

# Linux file secrets；Docker Desktop / rootless 按上文验证实际 UID 映射
sudo chown 10001:10001 deploy/.secrets/database_url deploy/.secrets/bootstrap_password
sudo chmod 600 deploy/.secrets/database_url deploy/.secrets/bootstrap_password

dc run --rm --no-deps api migrate
dc run --rm --no-deps -v "$PWD/deploy/.secrets/bootstrap_password:/run/secrets/bootstrap_password:ro" \
  -e RD_BOOTSTRAP_PASSWORD_FILE=/run/secrets/bootstrap_password api bootstrap-admin administrator
dc up -d --no-build --wait
curl --fail --silent https://accounts.example.com/readyz
```

按实际域名修改最后的健康检查地址。通过 `/admin/` 验证本地管理员登录，并按上文将 bootstrap 文件转存离线密钥库。外部数据库的凭据、备份加密、保留和轮换由数据库运维另行管理。

**外部库禁止套用下文 `ops.py --backend postgres` 的备份或恢复命令。** 当前 [ops.py](ops.py) 只接受 `sqlite` / `postgres`，其中 `postgres` 固定加载 `compose.postgres.yml`，备份时进入内置 `postgres` 服务并使用本地管理 secret；恢复时创建独立 PG 容器和新数据卷，不接受外部数据库 URL。它不能备份或恢复本节部署，还可能作用于同名 project 下的内置 PG 配置。外部库应由 DBA 使用其备份方案，在停写维护窗口验证完整性并实际演练恢复到独立目标库；核对 schema、应用角色、镜像兼容性和业务数据后才能切换 secret URL。下文关于切换 `RD_POSTGRES_VOLUME`、PG helper 和新卷回退的步骤同样仅适用于内置 PG；本文未验证外部库的备份恢复或 TLS 运行结果。schema 4 的完整数据库备份必须包含 `devices`、`book_credentials`、`device_events`；若库中存在凭据密文，DBA 恢复流程还必须从独立加密密钥备份恢复所有被引用 key，在隔离目标库用匹配镜像和挂载后的 `RD_BOOK_KEYS_FILE` 执行 `check-credentials`，缺失或验证失败不得切换流量。数据库 dump 不能替代密钥备份。

## OIDC 与客户端

IdP 创建 OIDC 客户端，启用 Authorization Code / PKCE，精确登记 `https://accounts.example.com/api/oidc/callback`。浏览器、origin 与回调必须使用相同 hostname。凭据写入 private `oidc_client_secret`，使用 `init-secrets.py --oidc` 时可交互输入；启用前已有目录则通过可信编辑器更新该文件并恢复 `0600` / UID 10001。

配置 `.env` 中 issuer、client ID、可信 claim 路径及 admin/member 精确映射；默认 `RD_OIDC_UNMATCHED=deny`。选择 `member` 代表允许其他已认证 IdP 用户以成员进入。保持本地应急管理员可用。替换 shell 函数，加入 overlay：

```sh
dc() { docker compose --env-file deploy/.env -p rdaccounts -f deploy/compose.sqlite.yml -f deploy/compose.oidc.yml "$@"; }
# PG 时把 compose.sqlite.yml 改为 compose.postgres.yml
dc config -q
dc stop api
dc up -d --no-build --wait api
```

停机重启会应用角色策略并撤销旧 OIDC 会话；JWKS 变更后重启。不把 IdP 管理口发布到此服务。不要为正式 IdP 设置 `RD_ALLOW_HTTP_LOOPBACK`。

RustDesk 客户端的账号 API 填 `https://accounts.example.com`；ID/中继服务器和公钥仍使用已有 `hbbs`/`hbbr` 配置。用当前支持版本实机检查本地/OIDC登录、个人通讯录和退出；本机 runner 不代替客户端验收。可选密码 hash 同步和设备目录还需按本次功能逐项实机验收。

## 健康、日志与审计

`/healthz` 仅检查进程存活；`/readyz` 检查 schema 4、数据库可写以及 Web 构建产物，探针写操作回滚。数据库故障时应保持 liveness 200、readiness 503。Compose 以 `/readyz` 判断 API healthy；unhealthy 本身不会自动重启，监控应报警，避免反复重启掩盖故障。离线 CLI 可执行 `dc run --rm --no-deps api ready-check`；它也会执行连接时迁移，因此不能用旧镜像检查新库。

```sh
dc ps
dc logs --tail 100 api
```

API 请求日志仅方法、匹配路由、状态、耗时；不记录 query、body、Cookie 或 Authorization。Caddy 访问日志保持禁用，并通过[全局 logger 排除](https://caddyserver.com/docs/caddyfile/options#log) `http.log.access` / `http.log.error`，防止上游故障时错误日志记录 OIDC callback query。Docker `json-file` 按每服务 `10m × 5` 轮转；Caddy 自身运行日志仍需受宿主机权限保护。收集器也不能额外启用完整 URL / 请求头记录。

审计事件在数据库中，默认 `RD_AUDIT_RETENTION_DAYS=90`；`0` 永久保留。运行期间每小时最多清理10批，每批最多1000条。手动清理：

```sh
dc run --rm --no-deps api prune-audit --retention-days 90 --batch 1000
```

重复执行直到删除数为0可清完当前过期批次。保留期变更前确认组织要求；已经删除的事件仅能从旧备份恢复。备份自身含账号/会话摘要、通讯录和审计，需独立制定加密、异地存放和删除周期；90天在线清理不自动清除历史备份。

## 停写备份与新卷恢复

先停止该实例所有人工 CLI / 外部数据库写者。备份脚本只负责停止指定 Compose project 的 API，保持其停止，不能发现项目外写者。Caddy 此时可能返回502，属于维护窗口。

```sh
# 使用唯一的新目录，不能覆盖旧备份。OIDC部署加全局 --oidc。
python3 deploy/ops.py --backend sqlite --project rdaccounts backup deploy/backups/pre-upgrade-001
# PG：--backend postgres；其余相同
# 启用凭据同步时，在 backup 前加全局参数：
# --book-keys-file deploy/.secrets/book_keys.json
```

SQLite 用 `.backup` 合并已提交 WAL 内容，并执行 `integrity_check`，不直接复制主库文件。PG 使用管理角色 `rustdesk_admin` 执行 `pg_dump -Fc --no-owner --no-acl`。目录 `0700` / 文件 `0600`；manifest 记录 schema、API image ID、RepoDigests、SHA256、时间，schema 4 另记录非秘密 `credential_key_ids`。完整库快照包含设备、个人通讯录凭据密文和设备事件。密钥文件及其路径不进入快照或 manifest；存在密文时必须提供全局 `--book-keys-file`，脚本预检文件权限、格式和所有所需 ID。执行 `ops.py` 的运维身份必须能读取该0600密钥文件；Linux 文件归UID10001时通常需以 `sudo python3 deploy/ops.py ...` 运行。预检不能证明 key 值正确，必须演练下述实际解密恢复。将当时完整 key ring 独立加密备份，并在私有运维记录中关联对应数据库快照。脚本不输出数据库 URL、密码、token 或 SQL 错误原文。失败产物保留，但没有完成的 manifest 不能视为可恢复备份。成功后核对文件存在并备份到加密异地存储，再按需 `dc up -d --no-build --wait api`。

恢复必须指定**从未存在的新卷名称**；脚本连空的已有卷也拒绝。不会删除任何源卷或目标卷。开始前校验 backup manifest 与 SHA256；schema 4 有密文时还会在创建目标卷前拒绝缺少或不完整的密钥文件。备份目录应为可信来源，校验和提供完整性检查，不能代替离线签名。

```sh
# 本机必须保留manifest记录的准确API镜像（docker image save/load或可信registry）
# 含凭据密文的备份：以下所有命令在 restore 前加全局参数
# --book-keys-file /private/offline-restored/book_keys.json
python3 deploy/ops.py --backend sqlite --project rdaccounts restore \
  deploy/backups/pre-upgrade-001 --target-volume rdaccounts-restore-001

# PG使用新空集群卷，内部新建rustdesk数据库；不接受现有数据库URL
python3 deploy/ops.py --backend postgres --project rdaccounts restore \
  deploy/backups/pre-upgrade-001 --target-volume rdaccounts-pg-restore-001 \
  --postgres-image postgres:16-alpine --pg-password-file deploy/.secrets/postgres_password \
  --app-password-file deploy/.secrets/app_password
```

PG 恢复 helper 不发布网络端口，确认新库 public schema 无表后执行事务性 `pg_restore`；结束仅停止并移除自身 helper，卷保留。恢复时同样挂载角色初始化脚本，使用 `rustdesk_admin` 登录、`pg_restore --role=rustdesk --no-owner` 创建应用拥有的表。应用密码 file 必须与应用 `database_url` 对应；管理密码与应用密码独立，PG恢复镜像应与备份时主版本一致；发布记录保留实际PG digest。SQLite 恢复文件归 UID10001，模式0600。schema 4 恢复还会用记录的 API 镜像对新数据库执行 `check-credentials`：SQLite 只打开新卷，PG 只连接新 helper，不打开源备份。实际 AES-GCM 解密失败会拒绝报告恢复成功；目标卷保留。校验 helper 为 root、只读根文件系统，以读取私有宿主 secret，不等同于验证正式 API 的 UID10001 挂载权限。切换前仍需用正式 Compose 文件组合执行 `dc run --rm --no-deps api check-credentials`。任何失败目标卷只供排查，下一次重试另选新名称。

切换前 `dc stop`，修改 `.env` 的 `RD_DATA_VOLUME`（SQLite）或 `RD_POSTGRES_VOLUME`（PG）为新卷；保持 Caddy 卷及 secrets，并对 schema 4 凭据恢复保留密钥 overlay 和全部旧 key。确认 `RD_API_IMAGE` 与目标 schema 兼容，再 `dc up -d --no-build --force-recreate --wait`。检查 readyz、Web登录、旧会话、通讯录和审计。不要执行 `docker compose down -v`；旧卷按正式保留策略单独处理。

## 升级与回退

1. 在独立测试卷演练本次镜像、schema升级与恢复。记录旧/新API、PG镜像ID/digest，备份当前非秘密配置与加密 secrets；记录`manifest.json`中的旧schema。
2. 进入维护窗口，停止外部写者，运行上述备份，确认 manifest 与离线副本。保存旧镜像为 `docker image save` 归档，保留原卷。
3. 修改 `.env` 的 `RD_API_IMAGE` 为新版本，先 `dc run --rm --no-deps api migrate`。PG先确认数据库运行。自动迁移成功后 `dc up -d --no-build --wait`，验证健康与业务。
4. 回退时先停止新 API。把**升级前快照恢复到新的卷**，配置为快照记录的旧 API 镜像，再切换卷启动。schema 4 不能交给旧 M3/schema 3 镜像打开，旧 M0/M1 也不能打开较新 schema；不支持原地降级。回退 schema 4 升级必须使用升级前的旧数据库快照与旧镜像，仅切换镜像或移除密钥 overlay 无法回退。升级后新增写入不会自动合并回旧快照，必须按维护计划处理。

## 本机 HTTPS 与可重复验证

手动本机测试可设 `RD_DOMAIN=localhost`、`RD_WEB_ORIGIN=https://localhost:8443`、`RD_HTTPS_BIND=127.0.0.1:8443`、`RD_HTTP_BIND=127.0.0.1:8080`、`CADDY_TLS_DIRECTIVE=tls internal`。Caddy 导出的本地CA可仅供测试客户端信任，不要替换系统信任库或使用跳过TLS验证的方式验收。正式ACME部署使用标准80/443，HTTP跳转按正式origin核验。

```sh
# 部署密钥/恢复安全边界与overlay组合（不启动容器）
python3 -m unittest discover -s deploy/tests -v
# 无容器启动：两后端配置静态验证
python3 validation/run_m3.py
# 密钥恢复专项：使用新镜像，实际启动隔离随机project，只绑定loopback HTTP；不代替TLS验收
python3 validation/run_credentials_restore.py --execute --api-image rustdesk-account-api:0.1.0-devices
# 镜像须已构建/拉取；实际启动隔离的随机project和loopback随机HTTPS端口
python3 validation/run_m3.py --execute --backend both
```

runner 从 Caddy 导出本地CA并验证证书，检查HTTPS/Web/Secure Cookie、非root只读API、SIGTERM与重启、会话持久性、数据库/schema故障的readiness、SQLite/PG停写备份和新卷恢复、拒绝覆盖、日志不含测试密码/token。默认最后仅停止自己的容器，`--keep-running` 可保留运行。所有数据、秘密凭据、报告与卷保留在独立project和 `validation/.runtime/rdm3-*`；不删除历史项目。端口分配存在很小竞争窗口，启动失败应查看私有环境，另起新project重试。

凭据专项 runner 在 UID10001、只读根文件系统与去除 capabilities 的 API 上生成全新密文，检查两后端的正确/缺失/错误/历史/轮换key、篡改密文、原会话与hash恢复；默认仅打印执行提示，`--execute` 后在新OS临时目录保留fixture、随机project和卷。所有子进程会清理继承的部署/Compose/Docker环境覆盖值，并要求本机Unix socket Docker context。它不删除卷、不读取已有秘密，也不覆盖正式 TLS 或外部PG运维恢复。脱敏运行结果见 `validation/results/device-credentials-restore-2026-10-31.json`。

runner不覆盖OIDC真实IdP交互、ACME公网签发、迁移前旧版本快照、备份加密/异地传输、主机宕机、负载和多平台客户端；这些必须由对应验收单独记录。生产仅限单实例；SQLite使用本地可靠磁盘；不要在共享网络文件系统上运行SQLite。
