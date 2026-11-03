# 部署操作手册契约（schema 4）

- Document type: 版本化部署、恢复与升级运行手册
- Audience: 拥有 Docker 宿主机与数据库权限的运维人员
- Purpose: 可重复部署单实例账号服务，并从停写快照恢复
- Scope: 镜像、TLS、secrets、SQLite/PostgreSQL、健康、保留、备份恢复、凭据密钥分离保管与轮换、升级回退
- Out of scope: HA、多实例、IdP运维、RustDesk中继部署、跨平台客户端验收
- Owner / Final durable owner: 本仓库 deploy/；主代理集成与最终交付
- Review owner: 主代理
- Target location: deploy/README.md
- Upstream sources: api/src/main.rs、api/src/store.rs、api/src/credentials.rs、api/migrations/*/0004_devices_credentials.sql、api/README.md、web/package*.json；主代理M3环境契约
- Downstream consumers: 部署人员、validation/run_m3.py
- Success criteria: OPS-01 配置可解析；OPS-02 非root镜像与HTTPS隔离；OPS-03 停写完整备份；OPS-04 拒绝覆盖恢复；OPS-05 回退绑定旧镜像和旧快照；OPS-06 密钥独立备份与恢复解密验证
- Document status: revised_pending_integration_review
- Deployment status: schema 4密钥本机专项已验证（SQLite/内置PG各10项，UID10001与污染环境隔离），生产仍待主代理验收；此前 M3 为 `local_verified`，另有 `me-aliyun` amd64/共享PG实例 `deployed_verified`，26项线上HTTP和8项备份恢复通过；独立证据在 `docs/deployment-2026-10-31.md`
- Evidence classes: implementation_fact（配置与脚本）；runtime_observation（仅实际执行的验证）；open_question（生产域名/IdP/镜像digest）
- Index or overview sync: 主代理更新根README与验收文档
- Open questions: 本次生产域名为 `desk.166198.xyz`，使用 `me-aliyun` 同宿主共享 PG、独立应用库及已有 Traefik；OIDC 暂不启用。实际镜像指纹与运行验收结果由实例部署记录填写。

| ID | 来源 / 归属 | 使用方 | 验证 | 未解决风险 |
|---|---|---|---|---|
| OPS-01 | 主任务 / deploy | 运维 | docker compose config -q | 本机Docker版本差异 |
| OPS-02 | 主任务 / Dockerfile + compose | 服务 | M3 HTTPS + 容器inspect；实际amd64/已有Traefik部署独立记录 | 新ACME签发及续期未覆盖 |
| OPS-03 | 主任务 / ops.py | 备份 | backup + checksum + restore | 外部写者必须由运维停掉 |
| OPS-04 | 主任务 / ops.py | 恢复 | 重复恢复必须失败 | 不可绕过脚本手工覆盖 |
| OPS-05 | schema4契约 / README | 升级 | 旧镜像+旧快照回退手册经复核；本机验证同schema快照回退 | 历史Linux旧镜像启动回退未演练；升级期间写入不能合并 |
| OPS-06 | schema4凭据契约 / credentials.rs、ops.py、compose.book-credentials.yml | 运维 | deploy/tests 11项；validation/results/device-credentials-restore-2026-10-31.json 两后端20项；生产由主代理统一验收 | 密钥丢失不可恢复；旧备份所需key不得删除；静态测试不替代真实解密恢复 |
