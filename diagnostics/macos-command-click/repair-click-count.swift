import Foundation
import ApplicationServices
import Darwin

// Optional active A/B experiment. No event posting or permission requests.
let mouseUps: Set<CGEventType> = [.leftMouseUp, .rightMouseUp, .otherMouseUp]

@discardableResult
func repair(type: CGEventType, event: CGEvent) -> Bool {
    guard mouseUps.contains(type),
          event.getIntegerValueField(.eventSourceUserData) == 100,
          event.getIntegerValueField(.mouseEventClickState) == 0 else { return false }
    event.setIntegerValueField(.mouseEventClickState, value: 1)
    return true
}

struct RepairError: Error, CustomStringConvertible {
    let description: String
    init(_ text: String) { description = text }
}

func message(_ text: String) { FileHandle.standardError.write(Data((text + "\n").utf8)) }

func selfTest() throws {
    // Construct and mutate local objects only; no tap, preflight, or post.
    let cases: [(CGEventType, Int64, Int64, Bool)] = [
        (.leftMouseUp, 100, 0, true), (.rightMouseUp, 100, 0, true),
        (.otherMouseUp, 100, 0, true), (.leftMouseUp, 0, 0, false),
        (.leftMouseUp, 99, 0, false), (.leftMouseUp, 101, 0, false),
        (.leftMouseUp, 100, 1, false), (.leftMouseUp, 100, 2, false),
        (.leftMouseDown, 100, 0, false), (.leftMouseDragged, 100, 0, false),
        (.flagsChanged, 100, 0, false),
    ]
    for (type, source, count, expected) in cases {
        guard let event = CGEvent(source: nil) else { throw RepairError("自检无法构造事件。") }
        event.type = type
        event.flags = [.maskCommand, .maskControl]
        event.location = CGPoint(x: 123, y: 456)
        event.setIntegerValueField(.eventSourceUserData, value: source)
        event.setIntegerValueField(.mouseEventClickState, value: count)
        let changed = repair(type: type, event: event)
        guard changed == expected,
              event.getIntegerValueField(.mouseEventClickState) == (expected ? 1 : count),
              event.getIntegerValueField(.eventSourceUserData) == source,
              event.flags == [.maskCommand, .maskControl],
              event.type == type, event.location == CGPoint(x: 123, y: 456) else {
            throw RepairError("自检失败：type=\(type.rawValue), source=\(source), count=\(count)。")
        }
    }
    print("PASS: 11 个构造事件验证严格过滤及其他字段不变；未创建 tap、未检查/申请权限、未投递输入。")
}

final class State {
    var repaired = 0
    var disabled = false
}

func run(seconds: Double) throws {
    let accessibility = AXIsProcessTrusted()
    let listening = CGPreflightListenEventAccess()
    guard accessibility && listening else {
        throw RepairError("权限预检失败：辅助功能=\(accessibility)，输入监控=\(listening)。请自行检查系统设置 → 隐私与安全性中的终端/程序权限，完全退出并重开后再试；工具不会请求或开启权限。")
    }
    let state = State()
    let mask = mouseUps.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
    guard let tap = CGEvent.tapCreate(tap: .cgAnnotatedSessionEventTap, place: .headInsertEventTap,
                                      options: .defaultTap, eventsOfInterest: mask, callback: { _, type, event, context in
        guard let context else { return Unmanaged.passUnretained(event) }
        let state = Unmanaged<State>.fromOpaque(context).takeUnretainedValue()
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            state.disabled = true
            CFRunLoopStop(CFRunLoopGetMain())
        } else if repair(type: type, event: event) {
            state.repaired += 1
        }
        return Unmanaged.passUnretained(event)
    }, userInfo: Unmanaged.passUnretained(state).toOpaque()) else {
        throw RepairError("无法创建 active event tap；请检查辅助功能、输入监控权限和当前图形会话。工具不会请求权限。")
    }
    defer { CFMachPortInvalidate(tap) }
    guard let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0) else {
        throw RepairError("无法创建 event tap run-loop source。")
    }
    let loop = CFRunLoopGetMain()!
    CFRunLoopAddSource(loop, source, .commonModes)
    defer { CFRunLoopRemoveSource(loop, source, .commonModes) }
    let timer = CFRunLoopTimerCreateWithHandler(kCFAllocatorDefault, CFAbsoluteTimeGetCurrent() + seconds,
                                               0, 0, 0) { _ in CFRunLoopStop(loop) }
    CFRunLoopAddTimer(loop, timer, .commonModes)
    defer { CFRunLoopTimerInvalidate(timer) }
    let oldInterrupt = Darwin.signal(SIGINT, SIG_IGN)
    let oldTerminate = Darwin.signal(SIGTERM, SIG_IGN)
    defer { _ = Darwin.signal(SIGINT, oldInterrupt); _ = Darwin.signal(SIGTERM, oldTerminate) }
    let interrupt = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
    let terminate = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
    interrupt.setEventHandler { CFRunLoopStop(loop) }
    terminate.setEventHandler { CFRunLoopStop(loop) }
    interrupt.resume()
    terminate.resume()
    defer { interrupt.cancel(); terminate.cancel() }
    message("临时 ACTIVE tap 已启动 \(seconds) 秒：仅 sourceUserData=100、mouseUp、clickCount=0 改为 1。Ctrl+C 停止即恢复，不持久化。")
    CFRunLoopRun()
    CFMachPortInvalidate(tap)
    message("临时修正结束：\(state.repaired) 个事件；tap 已关闭，后续输入恢复原行为。")
    if state.disabled { throw RepairError("系统已停用 tap，本次实验提前停止。") }
}

do {
    var seconds: Double = 60
    var test = false
    var help = false
    var suppliedSeconds = false
    var args = Array(CommandLine.arguments.dropFirst()).makeIterator()
    while let argument = args.next() {
        switch argument {
        case "--help", "-h": help = true
        case "--self-test": test = true
        case "--seconds":
            guard let raw = args.next(), let value = Double(raw), value.isFinite, value > 0 else {
                throw RepairError("--seconds 需要大于 0 的有限秒数。")
            }
            seconds = value
            suppliedSeconds = true
        default: throw RepairError("未知参数：\(argument)；使用 --help 查看用法。")
        }
    }
    if help {
        print("用法：repair-click-count [--seconds 秒数] | --self-test\n默认 60 秒。这是 ACTIVE tap，会将 sourceUserData=100 的左/右/其他 mouseUp 中 clickCount=0 改为 1；其他事件原样通过。\nCtrl+C、SIGTERM、到时或系统停用即关闭，停止后恢复；不投递键鼠、不持久化、不请求权限。仅供临时计数因果 A/B 验证，长按语义未覆盖。\n--self-test 仅构造事件，不启动 tap。")
    } else if test {
        guard !suppliedSeconds else { throw RepairError("--self-test 请单独使用。") }
        try selfTest()
    } else {
        try run(seconds: seconds)
    }
} catch {
    message("错误：\(error)")
    exit(1)
}
