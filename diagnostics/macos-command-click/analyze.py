#!/usr/bin/env python3
"""此次 Command+click 回归的 JSONL 检查；只显示点击字段和行号。"""

import argparse
from dataclasses import dataclass, field
import json
from pathlib import Path
import sys


PHASES = {
    "leftMouseDown": "down", "rightMouseDown": "down", "otherMouseDown": "down",
    "leftMouseUp": "up", "rightMouseUp": "up", "otherMouseUp": "up",
}


@dataclass
class Click:
    line: int
    button: int
    count: int
    command: bool


@dataclass
class Pair:
    down: Click
    up: Click

    @property
    def problems(self):
        problems = []
        if self.down.count != self.up.count:
            problems.append("down/up clickCount 不一致")
        if self.down.count < 1 or self.up.count < 1:
            problems.append("clickCount < 1")
        if self.down.command != self.up.command:
            problems.append("Command 在按下/释放之间变化")
        return problems


@dataclass
class Analysis:
    records: int = 0
    pairs: list = field(default_factory=list)
    issues: list = field(default_factory=list)

    @property
    def failed(self):
        return not self.pairs or bool(self.issues) or any(pair.problems for pair in self.pairs)


def analyze(lines):
    result = Analysis()
    pending = {}
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"L{number}: 无效 JSON（{error.msg}）") from error
        if not isinstance(row, dict):
            raise ValueError(f"L{number}: 事件必须是 JSON object")
        result.records += 1
        phase = PHASES.get(row.get("type")) if isinstance(row.get("type"), str) else None
        if phase is None:
            continue
        button, count, command = (row.get(key) for key in ("button", "clickCount", "command"))
        if type(button) is not int or button < 0 or type(count) is not int or count < 0 or type(command) is not bool:
            raise ValueError(f"L{number}: 点击需要非负整数 button/clickCount 和布尔值 command")
        click = Click(number, button, count, command)
        if phase == "down":
            if button in pending:
                result.issues.append(f"button={button} L{pending[button].line}: down 缺少 up")
            pending[button] = click
        elif button in pending:
            result.pairs.append(Pair(pending.pop(button), click))
        else:
            result.issues.append(f"button={button} L{number}: up 缺少 down")
    for button, click in pending.items():
        result.issues.append(f"button={button} L{click.line}: down 缺少 up（可能采集提前结束）")
    return result


def report(result, output):
    for pair in result.pairs:
        down, up = pair.down, pair.up
        command = f"{str(down.command).lower()}/{str(up.command).lower()}"
        held = "保持" if down.command and up.command else "未按住" if not down.command and not up.command else "变化"
        verdict = "FAIL: " + "；".join(pair.problems) if pair.problems else "PASS"
        print(f"button={down.button} down:L{down.line}/up:L{up.line} "
              f"clickCount={down.count}/{up.count} Command={command}（{held}） {verdict}", file=output)
    for issue in result.issues:
        print(f"FAIL: {issue}", file=output)
    held = sum(pair.down.command and pair.up.command for pair in result.pairs)
    bad = sum(bool(pair.problems) for pair in result.pairs)
    print(f"{'FAIL' if result.failed else 'PASS'}: {result.records} 条事件，{len(result.pairs)} 对点击，"
          f"{held} 对 Command 保持，{bad} 对异常，{len(result.issues)} 个配对问题。", file=output)
    if not result.pairs:
        print("FAIL: 没有完整点击对，无法判断回归结果。", file=output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path, help="采集工具输出的 JSONL 文件")
    args = parser.parse_args()
    try:
        with args.log.open(encoding="utf-8") as stream:
            result = analyze(stream)
    except (OSError, ValueError) as error:
        print(f"错误：{args.log}: {error}", file=sys.stderr)
        return 2
    report(result, sys.stdout)
    return 1 if result.failed else 0


if __name__ == "__main__":
    sys.exit(main())
