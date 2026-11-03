# 设备归属与个人凭据同步

- `document_status: drafted`；`device_credentials_status: verified_on_tested_clients`；2026-10-31。
- 负责人：项目发起人；实现与核验：Codex；需求唯一来源：[PRD RD-020～022](PRD.md)。
- 来源：E-17用户扩展范围及取消远控ACL的决定；固定官方客户端1.5.0（`fada664df7a294d1d1a9ca3e7cd3637069122f17`）。本设计描述独立实现，不声称复刻闭源Pro服务端内部逻辑。

## 官方协议与实现边界

个人簿继续使用`POST /api/ab/personal`的认证后404回退，以及Legacy `GET/POST /api/ab`的`data` JSON字符串。保留ID、别名、标签、平台等字段，同时在启用密钥时往返官方`hash`。客户端把目标密码和目标salt做SHA256，记住成功连接密码时上传base64摘要；另一端拿到摘要后结合新的challenge响应认证。摘要本身可用于认证，不能再次单向散列。见官方[摘要生成](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/src/client.rs#L4835)、[读取个人簿摘要](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/src/client.rs#L4761)、[记住密码后更新](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/flutter/lib/models/ab_model.dart#L443)。

只有已经存在于个人簿中的条目才按此路径更新；“同步最近连接到通讯录”是客户端独立设置。更改被控密码或salt会使旧摘要失效。服务不获取远端原文密码，不承诺所有平台和客户端版本已实测。官方[个人通讯录说明](https://rustdesk.com/docs/en/self-host/rustdesk-server-pro/address-book/)同样描述个人簿记住密码的同步使用方式。

“我的设备”沿用官方group目录中的当前用户过滤：`/api/device-group/accessible`为空组，`/api/users`只返回当前用户，`/api/peers`分页返回当前归属设备。无需创建客户端不调用的`/api/my-devices`。响应包括`id/user/user_name/info/note/status`；`status=1`代表有效目录项，不能用来推断在线。在线由客户端会合协议维护，页面仅标注最近登录时间。见[目录拉取](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/flutter/lib/models/group_model.dart#L103)和[PeerPayload转换](https://github.com/rustdesk/rustdesk/blob/fada664df7a294d1d1a9ca3e7cd3637069122f17/flutter/lib/models/hbbs.dart#L80)。

## 密文存储与运维

- 配置`RD_BOOK_KEYS_FILE`才启用凭据同步；没有密钥且数据库无密文时，保留原metadata-only模式。响应头分别为`enabled; personal-salted-hash`和`disabled; metadata-only`。
- 文件必须是0600常规文件且非软链，内容为`{"active_key":"v1","keys":{"v1":"<32随机字节的标准base64>"}}`。秘密不写入环境变量值、数据库、日志或公开结果。
- 使用AES-256-GCM、随机96位nonce；AAD按长度编码版本、user ID、peer ID和key ID。`book_credentials`仅存key ID、nonce与密文；`books.snapshot`剥离所有hash/password。
- 此为服务端静态加密，API持有解密钥匙，经HTTPS向同一账号客户端返回摘要；不是端到端加密。数据库和密钥应分离备份、限制读取。
- 启动验证全部已有密文；读写再次验证相关密文。缺钥、错钥或篡改拒绝，不退回空簿、不悄悄删除密文。`check-credentials`可在恢复后的独立数据库上核验；与其他CLI一样会先执行前向迁移。
- 新写入使用active_key，保留旧key可读取旧密文。修改配置需重启；没有后台全库重加密。删除旧key前须确认所有在线数据及保留备份已不再依赖它。丢失密钥无法恢复对应凭据。

最多1000联系人，ID最长256字节，请求体最大2MiB；只接受标准base64的32字节hash，兼容有/无padding。非空非法hash或重复ID拒绝整次上传。整本快照最后成功提交生效；遗漏/清空hash、删除peer或清空簿会删除对应密文。旧离线快照也可能覆盖新凭据，不提供冲突合并。元数据和密文的替换在同一事务中完成，失败保持原簿。

## 设备归属策略

官方本地登录含`id/uuid/deviceInfo`；OIDC轮询可提供id/uuid。本实现只把已认证登录用于关联，不采信无Bearer的sysinfo/heartbeat。客户端材料为自报信息，不构成密码学设备证明。

1. 新id和uuid首次登录后关联当前账号；不完整、超长、控制字符或冲突材料跳过关联，账号仍可登录。
2. 已归他人的设备不能通过登录夺取。已解除归属的设备不会被自动认领，需管理员明确分配。同一owner的稳定uuid可随未占用的RustDesk ID变更；OIDC缺失名称不会擦空已有信息。
3. 退出只撤销会话，保留归属。旧schema3会话不猜测回填设备，需要重新登录。
4. Web成员只看本人设备；管理员可看全部并分配/解除。后端验证Web用途、管理员身份、Origin和CSRF。
5. 归属变更、撤销所有绑定该设备的client sessions、device_events记录同事务提交；撤销不影响Web管理会话。旧未绑定会话仍需按用户或公共会话ID撤销。

设备归属是目录与管理关系。用户明确取消按用户、用户组限制远控；不实现用户组/远控ACL，也不把通讯录可见性解释为远控授权。E-18另行采用固定PR #734解决hbbs握手兼容，保留hbbr；见[兼容记录](rendezvous-compatibility.md)。账号/设备会话撤销不能断开已经建立的远控。

## 数据升级与验证

schema4新增devices、book_credentials、device_events和nullable sessions.device_id，保留旧账号、个人簿及会话。旧schema3二进制会拒绝新库，回退需升级前数据库备份和旧镜像。首次生产上线报告保留schema3历史事实；新功能测试和后续部署须另行记录。

验证分为两库持久化/迁移、安全HTTP协议、真实Keycloak回归、管理Web实际交互、官方客户端实机及生产部署。各类证据独立：协议中两个token读回同一hash不等于两台官方设备成功远控。当前执行结果见后续扩展验收记录；未覆盖项不得沿用M0历史通过结论。
