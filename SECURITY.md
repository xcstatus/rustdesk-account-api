# 安全维护与自动化

- `document_status: drafted`；整理日期：2026-11-03；负责人：仓库维护者。
- 仓库：[xcstatus/rustdesk-account-api](https://github.com/xcstatus/rustdesk-account-api)，当前为私有项目，尚未决定公开发布或项目许可证。
- 自动化配置在[Dependabot](.github/dependabot.yml)、[依赖扫描](.github/workflows/security.yml)与[CI](.github/workflows/ci.yml)。实际执行结果以GitHub Actions与Security页面为准。

## 自动发现与修复流程

| 机制 | 范围与时机 | 发现后的处理 |
|---|---|---|
| Dependabot alerts | GitHub已知漏洞库与仓库依赖图；新漏洞/依赖变化触发 | 在Security页面创建告警 |
| Dependabot security updates | Cargo/npm等受支持依赖存在可用修复时 | 自动提交安全修复PR，不等待每周普通版本更新 |
| Dependabot version updates | Rust、npm、GitHub Actions和配置中的Docker基础镜像；每周 | 自动提交更新PR，控制常规更新数量 |
| RustSec `cargo-audit` | `api/Cargo.lock`；每日及受影响push/PR，也可手动运行 | 已知Rust依赖漏洞使检查失败；信息类告警另按报告判断 |
| `npm audit` | `web/package-lock.json`，包含开发依赖；同上 | high/critical阻断检查；其他级别仍在报告中可见 |
| CI | main与PR | Rust格式/测试（隔离PG）及Web测试/构建，为修复审核提供依据 |
| 扫描失败Issue | 默认分支/定时/手动扫描失败 | 维护一个`security-automation`问题入口，链接具体Actions；干净扫描后关闭。失败也可能是工具或网络错误，不等同于已确认漏洞 |

每日扫描计划为北京时间09:17（UTC01:17），GitHub调度与排队可能延迟；不承诺实时检测。依赖扫描不执行应用部署，也不连接现有服务器。PR工作流使用受限凭据；需要写Issue的步骤与执行仓库代码的扫描任务分开。

自动扫描、告警和修复PR已纳入维护流程；修复是否完整、是否可合并由维护者审核。不配置自动合并或自动部署。没有可用修复、需要跨大版本迁移、或仅由RustSec发现而GitHub尚未给出修复PR时，需按具体告警人工处理。

## 及时处理的约定

1. 查看告警或扫描失败Issue，先区分实际漏洞、未维护提示与扫描基础设施错误。
2. 对实际漏洞确认受影响版本、调用范围和修复版本；优先处理可被当前服务触达的高危问题。
3. 使用Dependabot PR或创建最小修复分支，保留锁文件更新；仅执行与修改相关的检查，不重新跑历史GUI、恢复和生产矩阵。
4. 审核CI、迁移/兼容影响后合并。生产升级按[运维手册](docs/operations.md)另行执行，保留旧镜像与恢复材料。
5. 无可用修复时记录影响、临时缓解与后续跟进；不通过删除告警、无说明忽略或降低检查标准制造通过结果。

仓库通知通过GitHub处理。维护者应在个人通知设置中启用Dependabot安全提醒、PR及Actions失败提醒；是否发送邮件或移动推送取决于个人设置，本项目不代替其配置，也不保证有人全天值守。

## 当前覆盖边界

依赖漏洞扫描依据已公开的公告，不是完整代码安全审计，也不能证明不存在漏洞。业务授权、OIDC使用方式、数据隔离、部署secret和服务器本身仍需在变更中审查。Docker版本更新不等同于已扫描生产镜像或宿主机漏洞。

GitHub目前支持Rust的CodeQL，但Code scanning对私有仓库的可用性受所有权及GitHub Code Security授权限制。本项目是个人私有仓库，未假设具有该付费能力，未为启用它改成公开仓库或购买服务。来源：[GitHub CodeQL说明](https://docs.github.com/en/code-security/concepts/code-scanning/codeql/codeql-code-scanning)。后续授权满足时可单独增加Rust/TypeScript代码扫描。

Dependabot的Cargo/npm/Actions支持情况见[官方生态支持表](https://docs.github.com/en/code-security/reference/supply-chain-security/supported-ecosystems-and-repositories)，其自动修复能力取决于漏洞库、依赖解析和可用修复版本，不能保证每条告警都会生成PR。

## 首次执行与已知依赖问题（2026-11-03）

初始提交`d402e755`的[CI已通过](https://github.com/xcstatus/rustdesk-account-api/actions/runs/41637776050)，包括Rust双库测试和Web检查。[首次依赖扫描](https://github.com/xcstatus/rustdesk-account-api/actions/runs/41637776113)中，npm high/critical检查通过，RustSec发现以下两项，自动创建了[跟踪Issue #1](https://github.com/xcstatus/rustdesk-account-api/issues/1)。这不是无漏洞基线。

| 公告 | 初始锁文件及来源 | 当前处理 |
|---|---|---|
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | `crypto-bigint 0.5.5`，由`openidconnect → p256/p384 → elliptic-curve`引入 | 修复分支升级`openidconnect 4.1.0`，锁定`crypto-bigint 0.7.10`，满足公告修补范围`>=0.7.6`；合并状态和检查结果以对应PR为准，未自动部署 |
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | `rsa 0.9.10`，由`openidconnect`引入；升级后仍存在 | 官方公告暂无修补版本；保留扫描失败和跟踪，不加入ignore。下述可达性评估不等于修复 |

RSA公告涉及私钥操作的时序泄露。已检查的生产OIDC路径是[初始化Relying Party](api/src/oidc.rs#L308)及[验证ID token](api/src/oidc.rs#L358)：使用IdP的JWKS验证签名，没有加载RSA私钥、签名或解密操作。`openidconnect 4.1.0`的`core/crypto.rs::verify_rsa_signature`从公开的`n/e`创建`RsaPublicKey`并调用`verify`；与库中独立的`CoreRsaPrivateSigningKey`路径不同。该调用范围中未发现公告所需的RSA私钥操作，这是可达性评估，不能宣称整个依赖已安全。来源：[4.1.0发布源码](https://docs.rs/crate/openidconnect/4.1.0/source/src/core/crypto.rs)。生产实例按上次部署记录尚未启用OIDC，本轮未重新巡检或变更生产。

维护者继续跟进RSA上游修复。新增RSA签名、解密、OIDC客户端私钥认证，或更换相关库时，必须重新评估；不能沿用本结论。依赖告警未全部解决时，不将安全扫描描述为通过，也不以常规Dependabot大版本升级替代有针对性的修复评审。

## 秘密与报告

不要向Issue、PR、Actions日志或文档提交密码、token、私钥、个人簿hash、生产数据库或备份。原始运行截图、`.runtime`、secrets和构建资产不入库；CI使用的测试账号和PG密码仅为隔离runner中的公开fixture。

发现疑似凭据泄露时，先在私有渠道通知仓库负责人，撤销/轮换实际凭据，再处理仓库历史；仅删除当前文件不能保证旧提交与日志中的凭据失效。公开发布前仍需单独检查报告、截图与实例画像，并确定许可证。
