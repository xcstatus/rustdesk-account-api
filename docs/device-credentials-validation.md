# 设备与凭据扩展验收

- `document_status: drafted`；`device_credentials_status: verified_on_tested_clients`；2026-10-31。
- 负责人：项目发起人；实现与核验：Codex；需求来源：PRD RD-020～022、E-17。
- 范围：个人通讯录官方hash静态加密同步、认证登录关联设备、本人目录与管理员归属维护。不包含用户组或远控ACL。

## 自动化与实际页面证据

| 验证 | 结果 | 证据及边界 |
|---|---|---|
| Rust全量（实际设置DATABASE_URL） | 48通过 | 包含SQLite/PG持久化、旧库迁移、AES-GCM绑定/篡改/轮换、事务回滚、HTTP隔离/CSRF/归属撤销；PG 16.15；本机私有日志保留于validation/.runtime/device-credentials-rust-tests.log |
| Rust格式与Clippy | 通过 | `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings` |
| Web测试/类型/构建 | 14通过 | Node22；设备异步过期响应、权限失效、保存后重读、401/403清理；不把测试数量当GUI证据 |
| 真实HTTP及Keycloak | SQLite20、PG20通过 | [SQLite报告](../validation/results/device-credentials-sqlite-20261031050055_201b8139.json)、[PG报告](../validation/results/device-credentials-postgres-20261031050101_ab33767e.json)；含OIDC JIT/轮询设备绑定、密钥重启、目录、退出；API均停止，测试PG库已清理 |
| 实际浏览器 | 4项通过 | [结果](../validation/results/device-credentials-web-2026-10-31.json)；管理员全量目录、修改归属刷新、成员我的设备无管理控件、退出清除私有视图。独立fixture已停止且有效会话0 |
| 部署安全单测与Compose配置 | 11通过 | `python3 -B -m unittest discover -s deploy/tests -v`；含8种backend/OIDC/Traefik组合、密钥权限和恢复失败不报成功 |
| 双库密文恢复 | SQLite10、PG10通过 | [恢复报告](../validation/results/device-credentials-restore-2026-10-31.json)；正确/缺失/错钥、轮换、篡改、UID10001与只读运行；污染环境回归后重跑，未创建哨兵外部卷 |

本机API SHA256：`9b44d50c9f733c95942a126bb2f930b84e6f445708cdc5eae9bc8c6d9046bc32`；Web index SHA256：`32e8f3895410a53818a36f52bd643c0cfe5cc1117f0f8f73fdf275d47b3d4640`。Rust和Web锁文件随源码保存；Linux镜像另外记录，不能把本机二进制指纹套用到容器。

源码构建输入及容器镜像绑定见[扩展源码清单](../validation/results/device-credentials-source-manifest-2026-10-31.json)。独立审查发现的peer ID上限差异及恢复脚本环境污染问题均已修复，分别补边界HTTP测试和污染环境下重新演练。

## 实机和实例升级门槛

用户已在macOS 27和Windows 11（系统与客户端版本为用户报告，均RustDesk1.5.0）完成登录及双端目录验证：进入“可访问的设备”、清空搜索并手动刷新后，两端均显示两台设备。首次空视图的具体原因未隔离，不将其写成已修复的服务端缺陷。个人通讯录已确认跨设备及退出后重新登录仍可见。此前连接凭据实测曾受阻：登录账号后报`Failed to secure tcp: deadline has elapsed`，用户确认保持全部服务器/Key配置不变、仅退出Mac账号即可恢复连接；定位为客户端token触发的hbbs额外握手与OSS 1.1.16不兼容，尚未进入目标密码认证。诊断期间未修改或重启生产服务。已按用户选择上线[固定PR #734](rendezvous-compatibility.md)，18项握手测试通过；用户随后按双端步骤确认“mac连接成功， windows免密成功”，登录后的远控及RD-020跨端凭据同步通过本次实机验收。详见[本轮实机记录](../validation/results/device-credentials-client-manual-2026-10-31.json)。M0 macOS历史实机结果只证明当时的登录和元数据簿，不替代这次扩展验收。两个协议token读回同一hash也不等于两台官方客户端实际无密码重连。

实际服务器已完成新镜像、schema4、密钥私有挂载和16项HTTPS检查；集中PG备份与应用密钥单独备份见[扩展部署](device-credentials-deployment-2026-10-31.md)。2026-10-31首次schema3上线证据保持历史版本，不覆盖其manifest。

## 复现

```sh
cargo test --manifest-path api/Cargo.toml --locked
cargo clippy --manifest-path api/Cargo.toml --all-targets --locked -- -D warnings
python3 -B validation/run_device_credentials.py --database both
python3 -B -m unittest discover -s deploy/tests -v
```

Rust的可选PG分支需要`DATABASE_URL`，否则不算PG实际验收；使用隔离测试库，禁止指向生产。真实HTTP脚本复用私有M0 Keycloak/PG fixture，需先按[验证入口](../validation/README.md)启动依赖，不输出秘密。新增加密恢复脚本默认不执行容器动作，显式`--execute`才创建唯一测试项目和新卷，不覆盖已有卷；数据库快照不包含密钥，应使用单独保存的key ring。
