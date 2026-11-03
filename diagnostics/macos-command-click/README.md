# macOS Command+click 事件诊断与 A/B 验证

用于对比 RustDesk Mac → Mac 1.5.0 中 IDEA 的 Command+click：悬停可高亮，但点击不跳转。请在**被控 Mac**上运行工具，分别记录本地成功操作与远程失败操作；普通、映射、转换三种键盘模式各采一份。只读采集器提供系统事件证据；目录中另有需要手动启动的临时事件修正器，用于 A/B 验证，详见下文。

本次真实失败日志 `remote-before.jsonl` 有 79 条事件、7 对点击，全部为 down `clickCount=1`、up `clickCount=0`；其中 5 对 Command 在按下和释放时都为 `true`。这将验证重点收敛到 mouse-up 的点击次数：IDEA 的 `IdeMouseEventDispatcher` 对普通左键释放时 `clickCount < 1` 的事件不执行鼠标快捷键。修复后的回归目标是单击 down/up 都为 `1`，双击对应点击都为 `2`，Command+click 的修饰键仍保持正确，并在 IDEA 中实际恢复跳转。

用户实测（2026-10-31）：临时修正器运行时恢复 IDEA 跳转，退出后再次失效，已确认本次点击计数问题的因果关系。该结果不代表包含源码补丁的 RustDesk 客户端或三种键盘模式完整回归已通过。

## 构建与自检

需要 macOS 11 或以上以及 Xcode Command Line Tools 的 Swift 编译器。不依赖 Flutter。构建产物使用当前机器的 CPU 架构，目标最低系统版本为 macOS 11。

```sh
cd /Users/xc/workspace/other/rustdesk/diagnostics/macos-command-click
bash build.sh
./build/macos-command-click --self-test
```

构建产物和 Swift 模块缓存均位于本目录 `build/`。自检只在内存中构造事件并输出两条示例 JSONL，验证字段和筛选逻辑；不创建 event tap、不检查或申请权限、不发送键鼠事件。

## 分析点击次数与离线实验

分析器仅使用 Python 标准库，按鼠标按钮配对 down/up，显示原日志行号、点击次数和 Command 是否保持，不输出鼠标坐标或完整轨迹：

```sh
python3 analyze.py remote-before.jsonl
python3 -m unittest -v test_analyze.py
```

失败日志预期返回退出码 `1`，报告 7 对 `clickCount=1/0` 异常以及 5 对 Command 保持。修复后请另存 `remote-after.jsonl`，运行 `python3 analyze.py remote-after.jsonl`；完整点击对的次数应匹配且至少为 `1`，Command 不应在 down/up 之间变化。没有完整点击对、配对缺失或字段异常不能算验证通过。退出码 `0` 表示这些日志检查通过，`1` 表示点击/配对检查失败，`2` 表示输入文件或格式错误；仍需确认 IDEA 实际跳转结果。

可选 AppKit/CGEvent A/B 实验只构造内存事件，不启动 GUI、tap，不申请权限、不发送输入：

```sh
xcrun swiftc -target "$(uname -m)-apple-macosx11.0" \
  -module-cache-path "$PWD/build/module-cache" -framework AppKit \
  click-count-check.swift -o build/click-count-check
./build/click-count-check
```

预期默认 down 为 `1`、默认 up 为 `0`，up 显式设置 `mouseEventClickState=1` 后，转换所得 `NSEvent.clickCount` 也为 `1`。这验证点击次数字段会传到 AppKit，不代表已验证 RustDesk 实际发出的事件。

## 可选：临时 A/B 修正实验

`build.sh` 还会编译 `build/repair-click-count`。它是 **active tap，会修改匹配事件**，与上面的只读采集器分开。仅对 `.cgAnnotatedSessionEventTap` 上 `sourceUserData == 100`、左/右/其他 mouse-up 且 `clickCount == 0` 的事件，将该字段改为 `1`，其他事件原样返回。不监听普通键盘，不重新投递键鼠，不持久化。`100` 是此次 RustDesk 的来源标记，不是系统保证的唯一身份；仅在本次受控复现场景短时使用。

先构建并执行无 tap 自检：

```sh
bash build.sh
./build/repair-click-count --self-test
./build/repair-click-count --help
```

手动 A/B 步骤（在被控 Mac 执行）：

1. 保持 RustDesk 连接、键盘模式、IDEA 工程及同一符号不变，确认修正器未运行时 Command+click 仍失败。
2. 自行确认“辅助功能”和“输入监控”已授权给运行程序的终端/程序，再在终端运行 `./build/repair-click-count --seconds 60`。启动只预检 `AXIsProcessTrusted()` 和 `CGPreflightListenEventAccess()`；缺少权限会报错退出，不弹出权限请求或自行开启设置。
3. 在 60 秒内通过 RustDesk 对同一符号执行 Command+单击，记下是否恢复跳转。返回终端按 Ctrl+C，或等待到时；SIGTERM、系统停用 tap 也会结束。停止会关闭 tap，后续输入立即恢复原行为。
4. 停止修正器后再对同一符号执行一次，确认行为是否再次失败。记录三轮结果和修正器打印的匹配事件数。

这只验证点击次数与故障的因果关系，不是正式 RustDesk 修复。修正器无状态地将匹配的 `0` 改为 `1`，没有恢复完整的双击计数，**长按、拖动及其他复杂点击语义未覆盖**。若同时运行只读采集器，两个 tap 的先后顺序会影响日志看到修正前还是修正后的事件，不能据此保证 IDEA 最终收到的字段。

## 分次采集

1. 在被控 Mac 的 IDEA 中打开已知可跳转的符号，使用本地鼠标键盘确认 Command+click 能跳转。保持两轮采集的 IDEA 工程、符号和跳转设置相同。
2. 在被控 Mac 的终端启动本地采集（默认 60 秒，也可用 `--seconds 30`）。返回 IDEA，将鼠标移至符号，按下 Command，稍停，单击一次，再释放 Command。重复 2–3 次；完成后等待结束或回终端按 Ctrl+C。

   ```sh
   ./build/macos-command-click --output local-success.jsonl
   ```

3. 通过 RustDesk 连接同一台 Mac。在被控 Mac 上启动另一次采集，然后通过控制端在同一符号执行上述动作，记录当前键盘模式和成功/失败结果。每次切换键盘模式后使用新的文件名。

   ```sh
   ./build/macos-command-click --seconds 60 --output remote-mode-1-failed.jsonl
   ./build/macos-command-click --seconds 60 --output remote-mode-2-failed.jsonl
   ./build/macos-command-click --seconds 60 --output remote-mode-3-failed.jsonl
   ```

   以上命令按轮次分别执行。文件名中的 `mode-1/2/3` 是标签，请对应实际界面模式记下名称，不代表工具推断的模式。

4. 对比 `flagsChanged` 和对应 `leftMouseDown` / `leftMouseUp` 的 `command`、`flags`、`clickCount`、`position` 及 `sourceUserData`。尤其观察 Command 是否在 mouseDown 和 mouseUp 上都保持为 `true`、按下释放顺序是否一致、是否出现拖动。`sourceUserData` 是事件原始数值，不保证能唯一识别 RustDesk 或事件来源。

相对日志路径以启动命令的当前目录为准；按本页 `cd` 后运行，日志位于此 README 同目录。也可传绝对路径。目录需已存在。已有文件一律报错，不覆盖也不追加；请换一个文件名。新文件权限为 `0600`。未指定 `--output` 时 JSONL 写往 stdout，状态和错误写往 stderr。每条日志使用无缓冲写入立即可读；这不等于逐条同步到物理磁盘。

## 只读采集器的权限与数据范围

采集前仅使用 `CGPreflightListenEventAccess()` 检查已有权限。未授权会报 Input Monitoring（输入监控）错误并退出；工具不会调用权限请求 API，不会打开设置或自行授权。若要采集，请自行在“系统设置 → 隐私与安全性 → 输入监控”检查运行工具的终端/程序，授权后完全退出并重新打开；不同 macOS 版本中的名称和归属可能不同。预检通过但 tap 创建失败时，也会明确报错。无需以 root 身份运行。

工具在 `.cgAnnotatedSessionEventTap` 位置使用 `.listenOnly`，仅观察鼠标按下/释放/拖动、修饰键变化，以及按住 Command 时的鼠标移动。所有移动/拖动日志合计最多约每秒 20 条；按下、释放和修饰键变化不限流。没有普通 `keyDown` / `keyUp` 监听，不读取按键文字或键码，不注入、不拦截、不更改输入。**日志仍会记录屏幕鼠标坐标和操作时间，可能暴露操作轨迹**，分享前请检查内容。采集期间尽量只操作复现所需窗口。

每行 JSON 包含：`time`（UTC ISO 8601 时间）、`type`、`flags`（十六进制）、`command`、`control`、`shift`、`option`、`button`、`clickCount`、`position`（`x` / `y`）和 `sourceUserData`。`flagsChanged` 的 `button` / `clickCount` 为 `null`。鼠标移动事件的按钮/点击次数字段直接保留系统值，不应当成实际点击。坐标采用 CGEvent 全局屏幕坐标，多屏可能出现负值。

这是系统级事件视角，不能证明 IDEA 最终接收或如何处理某个事件；移动限流也会省略部分轨迹。需要结合 IDEA 实际跳转结果判断。Ctrl+C、SIGTERM 或指定时长结束都会关闭 tap；tap 被系统停用或写日志失败时停止并以非零状态退出。
