# 官方 1.5.0 账号客户端的 hbbs 握手兼容补丁

> 历史备选，已停止采用：用户选择完整固定 PR #734。不得将本目录补丁叠加到 PR；当前部署入口见 [PR 部署包](../../deploy/rustdesk-pr734/README.md)。下列内容仅保存历史实现。

固定 RustDesk OSS server `1.1.16` / `73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`，以及其 `hbb_common` 子模块 `83419b6549636ee39dacef7776c473f5802e08d6`。问题、边界及上线门槛见[兼容说明](../../docs/rendezvous-compatibility.md)。这是独立维护的兼容补丁，不能标称为官方服务端发布。

补丁默认关闭。启用 `HBBS_ACCOUNT_SECURE_TCP=1` 后，主 TCP 端口支持官方 1.5.0 的 version 1 方向隔离握手。`HBBS_ACCOUNT_SIGNING_KEY_FILE` 指向原有 Ed25519 私钥文件，默认 `id_ed25519`。文件必须已存在，且密钥须与原客户端 Key 匹配；缺失、损坏或不匹配会拒绝启动，不生成新钥。显式握手拒绝 version 0 降级；未握手的明文客户端保留原行为。

补丁不校验账号 token，不增加用户或用户组 ACL，不改变被控端密码或确认要求。只修改 hbbs，hbbr 沿用原镜像。私钥、数据库和生产配置不得复制进构建上下文或镜像。

## 应用和验证

先在独立目录获取固定版本及对应子模块，再应用本目录的 `0001-account-secure-tcp.patch`：

```sh
git clone --branch 1.1.16 --recurse-submodules https://github.com/rustdesk/rustdesk-server.git rustdesk-server-compat
cd rustdesk-server-compat
patch -p1 < /absolute/path/to/0001-account-secure-tcp.patch
cargo test --locked --lib account_secure_tcp
cargo test --locked --test account_secure_tcp -- --test-threads=1
cargo build --release --locked --bin hbbs
cargo build --release --locked --example account_secure_tcp_probe
```

测试只使用自动创建的私有临时目录、随机密钥和本机端口，不连接生产或测试电脑。集成测试覆盖真实 hbbs 进程、方向密钥官方向量、未登录明文回归、加密空心跳、降级拒绝、缓存 Sink 的异步打洞响应及缺钥/错钥启动失败。

## 构建容器

本目录 Dockerfile 使用离线 vendor 构建。先将 `cargo vendor --locked --versioned-dirs vendor-all` 输出的 source 配置保存到 `.cargo/config.offline.toml`，保留 `directory = "vendor-all"`，然后以已应用补丁的源码目录作为构建上下文：

```sh
cargo vendor --locked --versioned-dirs vendor-all > .cargo/config.offline.toml
cp /absolute/path/to/dockerignore .dockerignore
docker build --platform linux/amd64 -f /absolute/path/to/Dockerfile -t rustdesk-hbbs-account-compat:1.1.16-v1-amd64 .
```

原始及新增源码继续遵循上游 LICENSE。版本指纹和执行结果以验收报告为准；构建成功不能代替官方客户端实机验收。

镜像附带只读入口探测器 `hbbs-account-probe HOST:PORT PUBLIC_KEY_FILE`，仅使用服务器公钥验证签名、加密/明文 TestNat 响应和空心跳，不需要账号 token，也不登记设备或发起远控。切换后可通过它核验真实端口；其成功结果仍不替代官方客户端验收。

## 实例切换

生产切换需先确认额外的 hbbs 维护范围。保存原 Compose、原镜像和密钥备份，仅替换 hbbs 镜像并增加两个启用配置；原端口、数据卷、服务器 Key、hbbr 和账号 API 保留。启动前核对私钥匹配，切换后分别测试登录和未登录连接，再继续双端凭据同步。

本目录提供 `compose.hbbs-account-compat.yml`，只与现有 OSS 服务端的 Compose 基础文件共同使用；不能把它当作账号 API 的部署 overlay。先检查合并配置仅改变 hbbs 镜像、两个启用变量和本地镜像拉取策略，再在确认的维护窗口切换 hbbs。

回退使用原镜像和原启用配置，保留原服务器密钥与数据卷。本补丁没有新增数据库 schema；不能删除数据卷或重新生成服务器 Key 作为回退手段。
