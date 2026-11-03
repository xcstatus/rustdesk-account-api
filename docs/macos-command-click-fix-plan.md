# macOS 远控 IDEA：Command + 点击失效修复

日期：2026-10-31。`document_status: reviewed`；`command_click_fix_status: cause_confirmed_by_runtime_ab_awaiting_client_validation`。

## 结论

实际故障日志显示，RustDesk 注入的单击事件 **mouseDown.clickCount=1、mouseUp.clickCount=0**，而 Command flags 在按下和抬起时均正确。IDEA 的普通鼠标快捷键在释放时派发，会忽略 `clickCount < 1` 的事件；悬停高亮由另一条逻辑处理。用户随后确认：临时修正器运行时恢复跳转，退出后再次失效。结合原版事件日志，已确认本次环境的故障由释放事件点击计数为 0 引起。

最终补丁只改 macOS Enigo 的 `mouse_up`：包括单击在内，显式写入与按下阶段一致的 `self.multiple_click`。此前的 flags 候选修改已全部撤除。

源码已修改，临时事件修正的真实 IDEA A/B 已通过；尚未构建、安装或发布包含补丁的 RustDesk App。正式客户端补丁仍需独立实测和完整回归，不能把临时工具的成功等同于该验收完成。

## 环境与证据

`confirmed_fact`：用户两端均为 macOS 27 / RustDesk 1.5.0；Map、Translate、Legacy 都失败；Command 悬停代码有高亮。

`runtime_observation`：用户运行只读采集工具后生成 `diagnostics/macos-command-click/remote-before.jsonl`，共 79 条事件：

- 7 对左键单击，全部 down=1、up=0，`sourceUserData=100`，与 RustDesk Enigo 输入标记相符。
- 其中 5 对 Command 点击，按下和释放 flags 均为 `0x100008`；无 Control/Option/Shift 混入。对应日志行 `8/9`、`24/25`、`38/39`、`56/58`、`72/73`。
- Command 点击按住约 121–176ms。第 57 行有一次 dragged，该次不能单独用于排除拖动影响；其余 Command 点击也有相同的 1/0 问题。
- 本机不投递的 Swift/CGEvent 实验确认：默认 leftMouseDown 的 click count 为 1，默认 leftMouseUp 为 0。AppKit A/B 实验确认显式设置 up=1 后，`NSEvent.clickCount` 也为 1。

`confirmed_fact` / 用户实测（2026-10-31）：运行 `repair-click-count --seconds 60` 时 Command + 点击恢复跳转，工具退出后再次失效。工具只对匹配的 mouse-up 将 count=0 改为 1，因此支持该字段在实际故障环境中的因果结论。用户未报告本次测试的键盘模式、次数或工具匹配数，不能扩大为三模式全部验收通过。

原始日志不写入补丁包或版本控制，其中含时间、坐标和操作轨迹。

`implementation_fact`：修复基线为官方 1.5.0 `fada664df7a294d1d1a9ca3e7cd3637069122f17`。

| 环节 | 源码与行为 |
|---|---|
| RustDesk 生成 mouseUp | `libs/enigo/src/macos/macos_impl.rs:334`：原实现仅 `multiple_click > 1` 才设置 CLICK_STATE，单击遗漏，保留 Quartz 默认值 0。 |
| JBR 将计数交给 Java | `AWTView.m` 的 `deliverJavaMouseEvent` 从 NSEvent 读取 clickCount；Java responder 保留该值。实际用户 JBR 版本尚未逐版本核对。 |
| IDEA 过滤释放事件 | 固定 IntelliJ commit `0eaf790339db6e987aa873c3fce75610cbd8ba0a`，`IdeMouseEventDispatcher.java:202` 起：普通左键使用 MOUSE_RELEASED；clickCount<1 被忽略，不能匹配导航快捷键。 |
| 悬停高亮独立 | 同一 IntelliJ commit 的 `CtrlMouseHandler.kt:143` 起由 mouseMoved/modifiers 计算高亮；`:448` 用人为构造的 count=1 快捷键查询动作，不依赖实际 mouseUp 的计数。 |

不要将结论表述为“AWT 必然不产生 MOUSE_CLICKED”：它也可能产生 clickCount=0 的事件。直接相关的是 IDEA 对 release 的过滤。

## 修改与交付位置

- worktree：`fixes/macos-command-click/`，分支 `fix/macos-command-click`。
- 运行代码只改 `libs/enigo/src/macos/macos_impl.rs` 的 `mouse_up`；原始 `upstream/rustdesk/` 快照未改。
- 可移植补丁：`patches/macos-command-click-1.5.0.patch`。
- 采集、分析与不投递的 AppKit 实验：`diagnostics/macos-command-click/`，说明见该目录 README。
- 便携工具包：`diagnostics/macos-command-click.zip`；不包含用户采集日志或 Swift 模块缓存。

回归面只有 macOS Enigo 支持按钮的 mouse-up 事件：单击计数从默认 0 改为与 down 一致的 1；原本大于 1 的计数保持。键盘、flags、协议、发送端、滚轮、Windows/Linux 和鼠标按下计数逻辑均未更改。双击位置判定等已有问题不在本次修复范围。

## 验证与用户配合

责任：开发者完成代码与定点验证，主代理审查最终差异，用户在实际双 Mac 和 IDEA 环境验收。

1. 原版证据已取得，无需重复采集相同失败过程。
2. 已由用户使用工具目录中的短时 A/B 修正器验证点击计数因果：运行时成功、退出后失效。它由用户手动启动，只把标记为 RustDesk 的 mouse-up 中 count=0 改为 1；停止即恢复，不能替代正式客户端修复。权限和用法见 README；主代理不自动运行 active tap。
3. 正式修复版本在被控 Mac 验证。只需先更新被控端，控制端可保持 1.5.0；补丁位于接收端输入注入路径。
4. 静止短按 Command + 点击已知符号，确认跳转；再测试移动后点击。采集时确认 down/up 均为 1、Command flags 保持。

| ID | 验收要求 | 证据/风险 |
|---|---|---|
| MC-01 | Map、Translate、Legacy 下 IDEA Command + 单击均恢复导航，各连续 20 次 | 临时修正器 A/B 恢复跳转；正式补丁、三模式与各 20 次尚待验证 |
| MC-02 | 短单击 down/up 均为 1；Command flags 保持 | 修复后 JSONL + `analyze.py`；原版 7 对已确认失败 |
| MC-03 | 普通单击、双击分别保持 1/1、2/2，不引入额外点击 | 原生窗口及 IDEA 回归；双击既有跨位置问题不扩修 |
| MC-04 | 释放 Command 后普通点击、Command+V 与拖动不退化 | 键盘和 flags 未改；实际操作验证 |
| MC-05 | 最终 diff 只改变 macOS mouseUp 的计数设置 | `git diff`、独立审查、补丁可应用检查 |

验证状态：

- `git diff --check` 通过；补丁反向校验可应用。
- 只读采集器 Swift 编译与自检通过；未由代理启动真实监听。真实采集由用户执行。
- 日志分析器对原版日志返回失败（7 对 1/0），属于预期的修复前回归结果；分析器两项单元测试通过。
- 不投递的 AppKit A/B 实验通过：默认 down=1/up=0；显式修正后 up=1。
- Enigo crate 编译通过，4 项非交互单元测试通过；原有 `test_get_key_state` 会实际发送键盘事件，首次运行等待超过 60 秒后已终止，随后明确跳过该项。已有 objc 宏产生 3 条 cfg 警告。首次 libsodium 编译的 SDK 混用已通过命令级 SDKROOT 解决，未更改系统配置。
- 独立审查确认计数初始化为 1、连续点击递增/超时重置，现有双击及后续连击路径不变；未发现阻断问题。
- 临时 A/B 修正器 Swift 编译和 11 项无 tap 自检通过；用户真实 A/B 确认运行时恢复跳转、退出后再次失效。
- 未生成完整 RustDesk App：当前环境缺少 Flutter 和完整客户端原生依赖；不得把诊断工具包描述为修复客户端安装包。

未覆盖：用户确切 IDEA/JBR 版本；包含源码补丁的正式客户端实测及完整回归；长按释放计数的系统语义。此次补丁沿用现有 `multiple_click`，不新增计时或状态机；长按如有实际回归再单独处理。

## 外部源码

- [RustDesk 原始报告 #10188](https://github.com/rustdesk/rustdesk/issues/10188) 与 [Discussion #10189](https://github.com/rustdesk/rustdesk/discussions/10189)：同类症状，原报告为 Windows→Mac。
- [IntelliJ 鼠标快捷键过滤](https://github.com/JetBrains/intellij-community/blob/0eaf790339db6e987aa873c3fce75610cbd8ba0a/platform/platform-impl/src/com/intellij/openapi/keymap/impl/IdeMouseEventDispatcher.java#L202)。
- [IntelliJ 悬停高亮](https://github.com/JetBrains/intellij-community/blob/0eaf790339db6e987aa873c3fce75610cbd8ba0a/platform/lang-impl/src/com/intellij/codeInsight/navigation/CtrlMouseHandler.kt#L143)。
- [JBR 的 NSEvent→Java 转换](https://github.com/JetBrains/JetBrainsRuntime/blob/jbr21/src/java.desktop/macosx/native/libawt_lwawt/awt/AWTView.m#L534)。
- [Apple NSEvent.clickCount](https://developer.apple.com/documentation/appkit/nsevent/clickcount)。
