# 设备与凭据扩展实例升级

- `document_status: drafted`；`deployment_status: deployed_verified`；`client_manual_status: functional_passed_cleanup_pending`。
- 2026-10-31；实例：`me-aliyun` / `desk.166198.xyz`；实施与核验：Codex；负责人：项目发起人。
- 需求来源：PRD RD-020～022、E-17。本记录对应新扩展，首次schema3历史报告不被覆盖。

## 已部署结果

账号API由旧M3镜像升级为`rustdesk-account-api:0.1.0-devices-amd64`，schema3→4迁移成功，容器healthy。新增个人簿官方hash加密同步、认证登录设备关联、本人设备目录及Web管理员归属维护。生产仍不启用OIDC；真实Keycloak证据归本机双库验收。按用户/用户组限制远控已明确取消。

| 项目 | 结果与证据 |
|---|---|
| Image ID | `sha256:6e16c4ff737d4b590a6195b49705cb1dc560e3b3a2f0a748422824b797660237`，linux/amd64，无公共registry发布 |
| 镜像归档 | 41,488,418 bytes；SHA256 `2f46f50044f8c31c043e99a83cd6edbc3e280035638a73b6693f7e948d993796`；目标加载前核验 |
| [升级报告](../validation/results/device-credentials-deployment-2026-10-31.json) | 专用角色迁移schema4、UID10001正确读取密钥、只读根目录、健康检查通过；包含容器二进制和Web指纹 |
| [线上HTTPS检查](../validation/results/device-credentials-deployment-smoke-2026-10-31.json) | 16项通过，包含Web Cookie/Origin/CSRF、新普通账号双token凭据同步、非法hash不覆盖、目录隔离、清簿及退出撤销 |
| 密钥 | `.secrets/book_keys.json`，0600、10001:10001；通过`compose.book-credentials.yml`挂载，只在环境配置文件路径；key本身不进入Compose值或报告 |
| 现有服务 | Traefik、PostgreSQL、hbbs/hbbr启动时间与升级前一致，未重启或改造；现有同域名TLS options保留 |
| 账号与数据 | smoke只写自己新建普通账号的簿，结束清空并撤销该账号本轮会话。已有账号、簿和会话不被清理；为人工实测保留一个普通测试账号 |

API继续使用同宿主PG16.10、独立`rustdesk_accounts`库/owner、原内网连接配置和Traefik入口，无新增宿主API端口。限制仍为384MiB、0.75CPU、128PIDs。既有`/ws/id`、`/ws/relay`的首次上线502为独立历史问题，本次不作其修复或可用性声明。

## 升级前备份与密钥备份

已读取服务器父级AGENTS、共享PG README、backup-policy.json及数据库shared_backup.policy。当前数据库备份归`/home/migu/app/usr/postgres`统一管理；旧`aliyun-backup-check.py`已退役。本轮没有新建按服务导出或数据库定时任务。

升级后还已完成[集中备份结果](../validation/results/device-credentials-central-backup-2026-10-31.json)对应备份集`set-20261031T053144Z-9a708b`的隔离恢复，149张表核验通过、未清理任何历史集。

升级前短暂停止本API，执行集中`backup.py backup`，备份集`set-20261031T052651Z-ffa3b1`完成隔离恢复验证，再迁移新库。其他应用与PG/代理保持运行。该统一入口动态覆盖非模板数据库，不执行prune；回退应由PG维护者按统一手册在独立目标恢复并核验，再切换本API。

本服务非PG文件另行保存：

- 服务器目录：`backups/app-files-devices-20261031T052649Z/`，0700。
- `before-upgrade.tar.gz`：旧实例配置、原秘密及凭据；0600。
- `with-book-keys.tar.gz`：新实例配置、key ring及其他应用秘密；0600。文件SHA见升级报告。
- 后者有本机私有副本`validation/.runtime/production-devices-application-files-2026-10-31.tar.gz`，0600，已与服务器归档SHA一致；不进入镜像、源码清单或公开结果。

数据库快照不含应用加密key；恢复凭据必须同时保留正确key ring。此处本机副本与服务器同机归档不构成已配置的自动异地备份。共享PG每日备份、保留及实际计划触发证据由共享PG维护目录负责，本文不将人工运行当作计划触发验收。

schema4不可直接用旧schema3镜像运行。需要撤回时保留新库与新key，使用集中备份恢复到新目标，核验旧schema，再配合旧镜像和before-upgrade配置切换；禁止覆盖共享PG原数据目录。未执行生产降级演练。

## 人工验收与交接

管理端为`https://desk.166198.xyz/admin/`，官方客户端API地址为`https://desk.166198.xyz`；ID Server、Relay和Key继续按原配置。管理员凭据仍在原服务器credentials目录，未重置。

双机人工测试使用独立普通账号；凭据保存在服务器release目录及本机`validation/.runtime/production-client-devices-2026-10-31.json`，均0600，不能复制进公开报告。用户已确认有两台电脑，官方“可访问的设备”页已在macOS 27/Windows 11的1.5.0客户端经显式刷新确认均显示两台，个人通讯录已通过跨设备及重新登录验收。连接凭据的实机验收此前曾受阻：客户端登录账号后与OSS hbbs的额外安全握手超时，退出账号可恢复远控；已按用户决定采用[固定PR #734](rendezvous-compatibility.md)，16:25完成原数据备份并仅切换hbbs；hbbr/API保持运行，用户随后确认“mac连接成功， windows免密成功”，本次登录后远控和跨端凭据功能验收通过。客户端最终退出/配置恢复尚未报告，测试账号及设备归属保持现有状态，该收尾项单独记录。本次HTTP协议检查不代替官方GUI验收，完整平台矩阵仍未覆盖。

常用入口为实例`./accountctl`，现包含外部PG、Traefik、实例及book-credentials四个Compose文件。`./accountctl run --rm --no-deps -l traefik.enable=false api check-credentials`会验证全部已有密文（也会先执行前向迁移）；日常健康查看`./accountctl ps`。key轮换保留旧key，应用重启加载新配置，勿直接覆盖或删除现有key文件。
