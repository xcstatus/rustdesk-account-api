# macOS Command + 点击修复方案简报

- Document type: 缺陷分析与实施方案。
- Audience: 项目发起人、客户端开发者、验证者。
- Purpose: 分析 RustDesk 远控 Mac 时 IDEA Command + 点击失效的原因，给出能区分原因的诊断步骤、最小修复方向和验收标准。
- Scope: 用户确认的 macOS 27 / RustDesk 1.5.0 → macOS 27 / RustDesk 1.5.0；原生桌面客户端、键盘修饰键和鼠标点击链路。
- Out of scope: 不改变账号 API 设计；不将 Windows 专属问题直接套用到用户环境；不发布或替换用户安装的正式客户端。
- Owner: Codex 负责本次分析与文档。
- Review owner: Codex 负责代码证据及独立核验；项目发起人负责实际使用环境验收。
- Final durable owner: 当前项目文档目录，作为客户端缺陷专项方案。
- Target location: `docs/macos-command-click-fix-plan.md`。
- Upstream sources: 用户反馈；官方客户端 master `33a154a5d7502419381dc601aff1e4b0c660054d`；1.5.0 `fada664df7a294d1d1a9ca3e7cd3637069122f17`；GitHub #10188/#10189/#9729；Apple 事件文档。
- Downstream consumers: 客户端修复实施、回归测试、后续向上游报告。
- Success criteria: 区分源码事实与运行时推断；标出证据位置；明确诊断分叉、修改范围、稳定需求 ID、可观察验收及风险；不宣称未经实测的根因或修复。
- Document status: `reviewed`
- Deliverable status field: `command_click_fix_status: cause_confirmed_by_runtime_ab_awaiting_client_validation`
- Knowledge base target and page type: 不适用。
- Index or overview sync: README 文档入口增加专项方案链接。
- Open questions: 用户确切 IDEA/JBR 版本、包含补丁的客户端实测与完整回归。已有原版日志确认 Command flags 正确、单击 down/up count 为 1/0；用户 A/B 确认临时修正时恢复跳转、退出后再次失效。

证据类别：`confirmed_fact` 用户确认；`implementation_fact` 固定源码事实；`external_reference` 社区/官方文档；`runtime_observation` 已执行实验；`inference` 推断；`open_question` 未验证项。需求统一使用 `MC-` 前缀。

2026-10-31：用户授权执行修改并配合验证。以 1.5.0 创建独立修复 worktree。用户采集实际日志后，原因收敛到 mouseUp.clickCount=0；撤销 flags 候选，最终仅修正 macOS mouseUp 的单击计数。提供采集、分析、A/B 实验工具；不将源码实现或离线实验等同于实机验收完成。

2026-10-31 用户实测更新：临时修正器运行时恢复跳转，退出后再次失效，确认本次故障的点击计数因果。源码补丁已完成；A/B 并非包含补丁的客户端验收，后者及完整回归仍待执行。
