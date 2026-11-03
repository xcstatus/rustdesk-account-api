# M0 账号与个人同步验证

- 日期：2026-10-31；`document_status: drafted`。
- `m0_validation_status: passed`；本记录不代表完整首版已交付。
- 负责人及最终确认：项目发起人；实现、验证与证据整理：Codex。
- 上游：[PRD](PRD.md) RD-001、RD-004、RD-005、RD-007、RD-008、RD-016～RD-019；范围依据调研 E-13。
- 代码归属：独立实现的 `api/`；验证资产归属：`validation/`；具体依赖以 `api/Cargo.lock` 为准。

## 本轮边界

用户已要求全部按推荐方案推进并执行M0。采用Rust、Tokio、Axum、SQLx；SQLite与PostgreSQL均验证；本地账号与OIDC并存、OIDC账号不自动生成本地密码。实机基线为当前机器安装的RustDesk macOS arm64版1.5.0（build 68）；应用完整签名校验通过，签名者与可执行文件SHA256记录于实机JSON；原始安装包下载来源未另行采集。其他平台尚无实机结果。用独立Keycloak作为测试IdP，不代表用户生产IdP已指定。

本轮实现仅包含验证所需的兼容端点、持久化、CLI测试账号创建与停用、OIDC和个人簿。完整Web管理、账号审计查询、生产TLS与运维验收不属于这次M0完成条件。服务强制监听loopback；开发环境的HTTP仅用于显式允许的loopback地址。Compose的Keycloak与PG端口也仅绑定本机。

个人簿采用已实测的Legacy回退：有权限的`POST /api/ab/personal`返回404触发回退，未认证返回401；`GET/POST /api/ab`沿用外层data字符串。首期只同步元数据。客户端即使提交空或非空hash/password，也不将这些字段落库或跨端返回；成功写入用`X-RustDesk-Credential-Sync: disabled; metadata-only`明确范围。本服务不承诺跨端记住远控密码。无效快照不覆盖旧簿，成功整本提交采用最后提交生效。

角色映射验证只做一个可信ID Token声明、字符串/字符串数组精确匹配admin/member，固定admin优先；无匹配默认拒绝，可显式member兜底。配置变更采用单实例停机重启，先撤销存量OIDC会话，再接受新登录；不支持不同策略版本多实例并行服务。当前JWKS在初始化获取，轮换需重启刷新。这些都是M0实现边界，后续生产设计需再评审。

## 验证层次与用例

| 用例 | 可观察通过条件 | 证据类型 |
|---|---|---|
| M0-01 本地登录 | 错误密码失败；正确密码取得本服务token及直接user结构；currentUser取得本人；不能用自报ID切换归属 | Rust测试＋HTTP端到端 |
| M0-02 个人簿 | A端写入、B端读取中文/标签一致；用户之间隔离；清空可恢复为空；错误快照不覆盖；hash/password不落库 | Rust测试＋HTTP端到端 |
| M0-03 退出撤销 | 正常退出后旧token请求本人、个人簿和目录均401；重启/重新连接后仍失效；其他独立会话按策略保持有效 | Rust测试＋HTTP端到端 |
| M0-04 OIDC | 发现入口→发起→真实Keycloak登录表单和浏览器重定向→服务回调→轮询领取本服务token；user.info存在；复登issuer+sub对应相同用户 | HTTP端到端；浏览器GUI单独记录 |
| M0-05 OIDC拒绝与重放 | 未匹配拒绝；显式member兜底生效；错误state、错误轮询绑定和重放失败；角色降权/失准入/停用不保留旧权限 | Rust测试＋HTTP端到端 |
| M0-06 双库与持久化 | 两库运行相同业务用例；新建库和并发初始化可运行；重连保持用户、个人簿与撤销状态 | Rust测试＋HTTP端到端 |
| M0-07 官方客户端实机 | 未修改的官方1.5.0接入独立测试API与数据，完成本地登录、OIDC、个人簿写读及退出，记录平台和版本 | 官方客户端GUI与服务证据，不能以HTTP脚本替代 |

## 环境与执行记录

自动化验证已完成，脱敏逐项证据、构建SHA256及镜像digest见[结果JSON](../validation/results/m0-2026-10-31.json)，复现方法见[验证脚本说明](../validation/README.md)。所有自动化用例使用生成的账号、通讯录和凭据；运行时密钥仅存放于被忽略的`validation/.runtime/`，报告不包含token、密码或授权码。

| 环境/检查 | 结果 |
|---|---|
| macOS 27.0.1 / arm64；Rust/Cargo 1.93.1 | M0服务运行环境 |
| `cargo test --locked --offline` | 8项模块测试＋2项HTTP路由集成测试通过 |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | 通过 |
| SQLite＋Keycloak 26.8.1 | 同一最终构建，15/15端到端用例通过 |
| PostgreSQL 16.15＋Keycloak 26.8.1 | 同一最终构建，15/15端到端用例通过 |
| 专用PG数据库上的Store合同测试 | 并发初始化/JIT、撤销、降权、失准入、停用与持久化通过；同一测试也执行SQLite合同 |
| 官方RustDesk macOS arm64 1.5.0（build 68） | 用户操作客户端；本地/OIDC登录、两类账号个人簿保存与重登读回、自动建档与用户隔离、退出撤销全部通过 |

PostgreSQL17镜像拉取重试后因EOF失败，未纳入通过矩阵；Compose默认和复现步骤已统一为实测PG16。没有用PG16结果宣称PG17兼容。

官方客户端手动实测的脱敏过程证据单独保存在[实机记录](../validation/results/client-manual-2026-10-31.json)，7项检查均通过：用户确认本地登录，以及保存测试设备后退出再登录仍可见；随后完成company/Keycloak浏览器登录，新用户个人簿为空，再保存OIDC测试条目、退出重登并确认仍可见。服务端确认本地/OIDC个人簿各有1条独立测试条目，未保存凭据字段；OIDC复登复用同一内部用户。本地账号与OIDC账号各签发2个会话且均已撤销，最终所有测试账号有效会话为0。原始GUI token未被采集，旧token重放拒绝与跨重启撤销持久性由前述自动化用例补证。

官方客户端实机初期存在环境障碍：同一macOS用户的第二个RustDesk实例无法确认配置与IPC隔离，客户端界面控制读取超时，刷新后曾明确返回锁屏。用户确认解锁并允许使用现有客户端；解锁后读取仍超时，因此采用用户操作官方客户端、Codex只读核对独立测试服务端证据的方式。测试API为`http://127.0.0.1:21114`，由`validation/manual_client.py`单独启动；账号凭据只在被忽略的私有manifest中，`validation/probe_manual.py`可读取脱敏证据。M0-07已按上述方式通过，未宣称自动操作或读取了客户端界面。

用户已确认最后退出测试账号并恢复原API Server地址；该项依据用户确认，未采集原地址或以界面自动化再次读回。已停止本次API进程及`rustdesk-m0` Compose项目的Keycloak/PostgreSQL，确认三个测试端口均已关闭；数据库、fixture与数据卷保留，未停止其他容器或虚拟机。M0验证完成；生产IdP、其他客户端平台、完整Web管理以及生产部署/容量/备份恢复属于后续验收范围。
