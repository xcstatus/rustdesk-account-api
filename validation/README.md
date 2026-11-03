# M0 本地真实依赖验证

本节仅供本机 M0 验证。固定 Compose project `rustdesk-m0`，Keycloak `26.8.1`、PostgreSQL `16-alpine`，只绑定 `127.0.0.1:18080/15432`；API 固定 `127.0.0.1:21114`。本轮已实证 PostgreSQL **16.15**；PostgreSQL 17 镜像拉取多次重试后 EOF，未完成验证。不启动或停止已有 Linux VM，不改其他容器或已有数据库。依赖启动前确保这些端口空闲。

```sh
python3 validation/run_m0.py prepare
docker compose -p rustdesk-m0 --env-file validation/.runtime/.env -f validation/compose.yaml up -d
# 等待 Keycloak realm 的 .well-known/openid-configuration 可用；首次启动可能较慢。
cargo build --manifest-path api/Cargo.toml
python3 validation/run_m0.py run --database sqlite
python3 validation/run_m0.py run --database postgres
```

默认使用已验证的 PostgreSQL 16。若后续显式验证其他 major，启动及 run 命令需要同时设置 `M0_POSTGRES_IMAGE` 和 `M0_POSTGRES_DATA_VERSION`，以使用独立命名卷，避免跨 major 复用数据目录。报告记录 `SHOW server_version` 的实际值及 API 二进制 SHA-256。可分享结果与本地镜像 `RepoDigests` 记录于 `results/m0-2026-10-31.json`；该文件不包含 fixture 或原始日志。

`prepare` 首次生成随机 realm、confidential client secret、bootstrap 管理员和 admin/member/denied 三个测试用户。`groups` mapper 写入 ID token，admin 同时属于 `rustdesk-admins` 与 `rustdesk-users`，member 属于 `rustdesk-users`，denied 属于 unmatched。client 固定登记 API 回调，并要求 PKCE S256。该开发实例为读取宿主机 0600 导入文件使用容器内 root，不是部署配置。

所有 fixture、密码、原始 API 日志和报告位于已忽略的 `validation/.runtime/`，机密文件权限为 0600；不要提交或分享原始目录。脚本与结构化报告不输出 token、密码、授权 URL 或 code。`prepare` 复用现有 fixture，不覆盖；中途失败的 prepare 可能需要人工检查文件，脚本不会自动删除。每次 run 创建新的 SQLite 目录或专用 PostgreSQL database，不清理既有数据。单一 API 子进程由脚本启动并在 finally 停止；若端口已有监听则拒绝操作。脚本会为策略切换重启自己启动的 API。

两种数据库运行同一套用例：本地错密/好密、两个设备会话、个人簿双向元数据同步、用户隔离、非空 hash/password 排除与明确响应头、无效快照不覆盖、退出后全部受保护路由 401；真实 Keycloak 登录表单/Cookie/重定向、授权码+PKCE 参数、准确 pending 响应、id/uuid 绑定、无效 state、单次回调/领取、user.info、JIT 重登稳定 ID、admin 优先、默认 deny、可配置 member 兜底、策略变化撤销与恢复 deny。故障会如实标记 fail/error；原始 HTTP 内容不会写入报告。

Keycloak 在该 loopback HTTP 测试源上仍设置 `Secure` 登录 Cookie；stdlib 默认不回传。runner 的 Cookie policy 仅对精确的 `127.0.0.1:18080` 使用本地安全上下文例外，其余目标继续遵循默认策略。测试用户预填完整 profile，避免登录停在用户资料补全页。member 兜底模式还验证无效授权码换码失败不能被当成成功。

这是 HTTP 模拟浏览器和客户端协议的集成验证，**不等于官方 RustDesk 客户端 GUI、Windows/macOS/Linux 跨端实机验证**，也未覆盖 TLS 生产部署、生产备份、容量或所有恶意 IdP token 变体。

2026-10-31已另行完成本机官方macOS arm64 1.5.0（build 68）实机验证，由用户操作界面并结合服务端证据；本地/OIDC登录、两类个人簿保存与重登读取、退出撤销均通过。结果见[实机记录](results/client-manual-2026-10-31.json)及[M0说明](../docs/m0-validation.md)。测试账号最终有效会话为0，用户确认恢复原API地址；本次API与Compose依赖已停止，数据保留。

## M1账号与Web验证

构建当前API及`web/dist`、启动上述依赖后运行：

```sh
python3 validation/run_m1.py run --database sqlite
python3 validation/run_m1.py run --database postgres
```

每库20组用例覆盖独立Web会话、Cookie/Origin/CSRF、账号启停/本地角色与密码管理、本人/管理员会话撤销、OIDC托管限制、真实Keycloak浏览器表单、回调绑定及重放。脚本复用M0隔离数据库规则，显式设置`RD_WEB_DIR`为本项目`web/dist`；API子进程结束后自动停止，不操作其他服务。运行前需`npm ci`与`npm run build`，命令和Node要求见[Web说明](../web/README.md)。

当前构建还须单独运行原M0两库回归；新结果归档在M1记录，不覆盖M0历史GUI证据。Rust Store/管理测试支持通过`DATABASE_URL`启用专用PG验证，其中Store测试使用随机独立schema，不能把生产数据库作为测试目标。完整结果与边界见[M1说明](../docs/m1-validation.md)。

## 手动或浏览器实机入口

经明确授权进行本机官方客户端实机验证时，先启动上述依赖，再运行：

```sh
python3 validation/manual_client.py
```

此入口为手动测试新建独立 SQLite 和本地用户，复用现有 Keycloak `member` fixture，并持续监听 `127.0.0.1:21114`。端口已被占用时拒绝启动。stdout 仅报告凭据文件路径、API URL 和 ready 状态；受控测试凭据、数据库目录和启动状态只写入权限 0600 的 `.runtime/manual-client.json`，不复制到可分享结果。按 Ctrl-C 停止入口会清理它创建的 API 子进程，保留数据库、fixture 和 Keycloak/PostgreSQL。后台启动时可向该文件中 `launcher_pid` 发送 SIGINT；不要停止其他进程。手动入口本身不会执行自动化用例，也不表示 GUI 测试通过。

用户在官方客户端操作期间，可执行`python3 validation/probe_manual.py`只读检查会话数量/撤销状态、通讯录条目数和测试别名`M0-本机实测`是否落库；OIDC测试使用`--marker M0-OIDC实测`。输出的`marker_alias`标明当前匹配目标，其他账号的`marker_present: false`不代表其通讯录丢失。该命令不输出密码、token、远控凭据或其他联系人内容；服务端证据需要与用户的实际界面结果共同判断，不能仅凭数据库变化宣称GUI通过。测试结束后恢复原API地址，并确认测试账号已退出。

仅需停止本项目依赖时运行下列命令；不带 `-v`，保留 fixture 与数据库：

```sh
docker compose -p rustdesk-m0 --env-file validation/.runtime/.env -f validation/compose.yaml stop
```


## M2同步、账号审计与M3运维

当前源码要求先构建`web/dist`；M0 fixture也显式传入绝对`RD_WEB_DIR`。M1功能回归为避免快速反复使用同一身份耗尽共享限流，单独把每账号额度设100；默认额度与恢复行为由Rust operations测试及M3实机HTTP演练验证。

```sh
python3 validation/run_m2.py run --database sqlite
python3 validation/run_m2.py run --database postgres
python3 validation/run_audit.py run --database sqlite
python3 validation/run_audit.py run --database postgres
python3 validation/run_m3.py --execute --backend both
```

M2每库24项，audit每库14项；依赖已有隔离Keycloak/PG fixture，前两类runner只启动/停止自身API。M3使用部署镜像、随机project、loopback HTTPS端口和该次Caddy CA，具体命令与运维说明见[deploy](../deploy/README.md)。并行运行不同进程不得争用21114；M3使用随机端口可独立运行。报告归属[M2/M3记录](../docs/m23-validation.md)，只公开脱敏汇总。

PG审计故障回滚测试：`DATABASE_URL`必须指向专用测试库，`cargo test --manifest-path api/Cargo.toml --test audit`会额外在独立随机schema验证v2→v3保留数据、拒绝审计写时密码/停用/策略与撤销原子回滚，并清理自己的schema。测试不创建生产账号。Docker镜像、旧卷及测试备份保留，最终停止仅限本轮project。

## 已部署实例的受控检查

[run_deployed.py](run_deployed.py) 默认拒绝网络和写操作，显式 `--execute` 才运行。它只适用于已授权、OIDC关闭、选定管理员个人簿为空且没有其他写者的维护窗口；会临时改写该账号通讯录并在 finally 恢复、撤销本轮会话。没有通讯录 CAS，不能在正常使用中的账号上随意执行。不会新建或修改账号、密码；登录/退出审计会保留。

```sh
python3 validation/run_deployed.py --execute \
  --origin https://accounts.example.com \
  --username administrator \
  --password-file /private/path/administrator-password \
  --report /private/path/deployed-smoke-report.json
```

密码文件必须为非symlink普通文件、`0600`；TLS使用系统信任链，禁止跟随重定向。报告仅含固定case ID、状态和固定错误消息，不输出正文、密码、token或cookie。26项包含恢复和退出清理；登录响应丢失可能产生无法识别的会话，此时报告明确失败，需要管理员核对会话。普通SIGINT/SIGTERM会尝试清理，SIGKILL或断电仍需人工核对。实际生产执行证据见[实例记录](../docs/deployment-2026-10-31.md)；此脚本不替代官方客户端GUI验收，也不包含外部PG备份操作。

## 设备与凭据扩展

`python3 -B run_device_credentials.py --database both`复用上述私有Keycloak/PG fixture，用新数据库验证启用密钥后的官方个人簿hash、设备归属、目录、OIDC绑定和错钥拒绝。结果独立保存；测试结束停止自己的API并清理自己PG库。`run_credentials_restore.py`默认仅显示说明，显式`--execute`配合含`check-credentials`的新镜像验证密文恢复，创建唯一项目/新卷，不删除卷。精确通过记录见[扩展验收](../docs/device-credentials-validation.md)。
