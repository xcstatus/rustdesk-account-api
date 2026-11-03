# PR #734 实例部署

用户已选择使用[上游 PR #734](https://github.com/rustdesk/rustdesk-server/pull/734)，固定提交 `ef2166f7760905a18b6588f0882d633d3688ad2f`，`hbb_common` 固定 `229b904508364c8997aad0fb5af57effac859f60`。这是未合并 PR 的独立构建，不能标为官方稳定发行；不叠加之前的独立握手补丁。

首轮只开启 `SECURE_TCP=Y`，明确设置 `IPV6_PUNCH=N WEBRTC=N UDP_PUNCH=N`。PR 源码保持原样。入口脚本只检查原密钥文件存在，避免数据挂载丢失时生成新身份。

`compose.override.yml` 专用于当前 `me-aliyun` 的既有 OSS Compose 项目，沿用 31116、原 `/root` 数据挂载及现有 hbbr；不能作为账号 API 的 overlay 单独使用。启动参数改为 `-k _`，从原 `id_ed25519` 读取私钥并导出同一公钥。不得把私钥放进环境变量或镜像。切换前必须核对原公私钥匹配并备份数据及配置。

构建上下文为固定提交及正确子模块的源码目录；`vendor-git` 为锁定的八个 Git 源的目录替换，Cargo.lock 不变；`.cargo/config.offline.toml` 保持 crates.io 为原 registry 源，仅将 Git 源替换到 `vendor-git`。上游 `.env` 与 `db_v2.sqlite3` 是 SQLx 编译输入，仅进入构建阶段，不进入运行镜像；必须使用固定源码附带文件，不得使用生产配置或数据库备份。准备 Linux 目标所需 registry 缓存后，复制本目录 `entrypoint.sh` 到上下文 `deploy-entrypoint.sh`、`dockerignore` 到上下文 `.dockerignore`，使用 Docker named context `cargo_registry` 指向 registry 目录：

```sh
docker build --platform linux/amd64 \
  --build-context cargo_registry=/absolute/path/to/cargo/registry \
  -f /absolute/path/to/rustdesk-pr734/Dockerfile \
  -t rustdesk-hbbs-pr734:ef2166f7-amd64 .
```

验证按用户要求保持精简：一次 Linux/amd64 构建，运行 PR 现有 `key_exchange` 和 `compat` 测试（不运行需要额外旧版服务端的 ignored 比较项），随后继续实际官方客户端远控与密码同步。之前独立补丁的 17 项结果不作为本 PR 证据。结果见[握手兼容记录](../../docs/rendezvous-compatibility.md)。

切换仅重建 hbbs。原 hbbr、账号 API、数据库、代理和客户端 Key 均保留；回退使用原镜像及原 Compose 配置和原数据卷，不删除密钥或重建身份。
