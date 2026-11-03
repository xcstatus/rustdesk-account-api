# 登录后的远控握手兼容：PR #734

- `document_status: drafted`；`compatibility_status: pr734_deployed_client_verified`；2026-10-31。
- 负责人：项目发起人；调查、集成与核验：Codex。
- 来源：官方客户端人工验收、固定源码及用户“使用PR #734”“不要做过度验证”的决定（E-18）。

## 已确认的问题

官方 RustDesk 1.5.0 的 macOS、Windows 两端已通过账号登录、个人通讯录跨端及重新登录验证。连接第三台设备时报 `Failed to secure tcp: deadline has elapsed`，三台均显示就绪。用户确认仅改过 API 地址；Mac 保持 API、ID Server、Relay、Key 不变，仅退出测试账号即可恢复连接。

客户端在 API token 非空且配置了服务器 Key 时，先等待服务器 KeyExchange，再发送打洞请求。现有 OSS hbbs 1.1.16 则先等待客户端消息，形成双方互等。这发生在目标密码认证之前；不通过修改 API token 绕过。[客户端调用](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/src/client.rs#L850)、[握手入口](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/src/common.rs#L2157)、[原 OSS TCP 接入](https://github.com/rustdesk/rustdesk-server/blob/1.1.16/src/rendezvous_server.rs#L1151)。

## 已选方案

使用[上游 PR #734](https://github.com/rustdesk/rustdesk-server/pull/734)完整固定源码，不叠加先前独立握手补丁。2026-10-31 查询时 PR 尚未合并，属于独立构建，不能标为官方稳定发行。

| 对象 | 固定值 |
|---|---|
| PR head | `ef2166f7760905a18b6588f0882d633d3688ad2f` |
| PR base | `873230e35a8805de7fd057e7eb763c46f7a174ef` |
| hbb_common | `229b904508364c8997aad0fb5af57effac859f60` |
| Cargo.lock SHA256 | `bd6aa4e15ca07598c17bead7c80a0c1a1fb50a943dc617d7315e55b743378afe` |
| 首轮开关 | `SECURE_TCP=Y`、`IPV6_PUNCH=N`、`WEBRTC=N`、`UDP_PUNCH=N` |

PR 支持签名密钥交换及官方客户端的方向派生密钥；同时保留其上游兼容行为，不额外施加旧方案的 v1-only 策略。PR 基于较新的 master，包含握手以外的基线变化，按固定提交维护，不自动跟随 PR 更新。[固定源码](https://github.com/NotOfficer/rustdesk-server/tree/ef2166f7760905a18b6588f0882d633d3688ad2f)。

仅切换现有 hbbs 镜像，保留 hbbr、端口、原数据挂载和服务器身份。使用 `-k _` 从原私钥读取身份，入口脚本在缺少密钥文件时拒绝启动，避免挂载异常生成新钥。生产公私钥及原配置匹配已核验；备份仅保存在服务器，不将私钥加入构建上下文。

这次变更增加 hbbs 的兼容维护，不增加账号 token 校验、用户组或远控 ACL。被控端密码/确认要求仍由既有机制执行；账号退出不等于断开已有远控。

## 精简验证与状态

按用户要求，仅做一次 Linux/amd64 构建、PR 现有 `key_exchange` 与 `compat` 默认测试，以及实际官方客户端的远控和跨端密码同步。需要额外旧版二进制的 ignored 比较项不运行，不追加独立测试矩阵。保留原镜像、配置与数据备份以供回滚。

构建与部署材料见[PR 部署包](../deploy/rustdesk-pr734/README.md)，本 PR 结果见[独立验证记录](../validation/results/rendezvous-pr734-validation-2026-10-31.json)。实际双端结果见[人工验收记录](../validation/results/device-credentials-client-manual-2026-10-31.json)。Linux/amd64 镜像已构建，18 项 key_exchange 测试通过，compat 的 3 项旧版对照按约定 ignored。生产已于 2026-10-31 16:25（Asia/Shanghai）切换 hbbs，四个开关启动日志符合配置，未出现缺少签名私钥警告，hbbr/API 容器未重建，API 健康与就绪均为 200。用户随后按双端步骤确认“mac连接成功， windows免密成功”：Mac重新登录测试账号后远控成功，Windows刷新同步后对同一目标免输密码连接成功。该结果来自用户操作的官方1.5.0客户端，适用于本次测试设备，不扩展为完整平台矩阵；不再追加测试。

先前独立补丁已停止采用，其 17 项本机测试只保留为历史记录，不作为 PR #734 的通过证据。

## 实例备份与回退

原 Compose、容器配置及停止 hbbs 后的一致数据归档保存在服务器 `/home/migu/app/usr/rustdesk/backups/pre-pr734-20261031T082523Z/`。密钥未导出到本机。原镜像仍保留；同目录 `rollback.sh` 使用原 Compose 仅重建 hbbs，原挂载继续保留。仅在需要恢复数据时另行处理归档，不自动覆盖后续数据。

当前镜像 `rustdesk-hbbs-pr734:ef2166f7-amd64`，ID `sha256:f9d74b60499cd4f84c3a5ae8edef62aa15178c1140a85e4d38d2f68542a4bd25`；服务器实际 overlay 为 `releases/pr734-ef2166f7/compose.override.yml`，基于原 Compose，当前部署命令需同时指定两者。
