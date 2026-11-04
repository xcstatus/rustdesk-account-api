# 安全维护与自动化

- `document_status: drafted`；更新日期：2026-11-04；负责人：仓库维护者。
- 仓库：[xcstatus/rustdesk-account-api](https://github.com/xcstatus/rustdesk-account-api)，2026-11-04按项目负责人要求公开；项目许可证尚未确定。
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

仓库公开后具备使用免费CodeQL代码扫描的条件，但本次仅变更可见性，尚未配置CodeQL、Rulesets或AI审查App；现有依赖扫描继续运行。来源：[GitHub CodeQL说明](https://docs.github.com/en/code-security/concepts/code-scanning/codeql/codeql-code-scanning)。后续可独立配置Rust/TypeScript代码扫描和PR审查，不能把可用性变化记作已启用。

Dependabot的Cargo/npm/Actions支持情况见[官方生态支持表](https://docs.github.com/en/code-security/reference/supply-chain-security/supported-ecosystems-and-repositories)，其自动修复能力取决于漏洞库、依赖解析和可用修复版本，不能保证每条告警都会生成PR。

## 秘密与报告

不要向Issue、PR、Actions日志或文档提交密码、token、私钥、个人簿hash、生产数据库或备份。原始运行截图、`.runtime`、secrets和构建资产不入库；CI使用的测试账号和PG密码仅为隔离runner中的公开fixture。

发现疑似凭据泄露时，先在私有渠道通知仓库负责人，撤销/轮换实际凭据，再处理仓库历史；仅删除当前文件不能保证旧提交与日志中的凭据失效。公开发布前仍需单独检查报告、截图与实例画像，并确定许可证。
