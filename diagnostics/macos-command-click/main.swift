import Foundation
import CoreGraphics
import Darwin

// Passive event diagnostics. Never call event-posting or permission-request APIs.
struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}

func diagnostic(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}

final class JSONLWriter {
    private let descriptor: Int32
    private let ownsDescriptor: Bool

    init(path: String?) throws {
        if let path {
            // O_EXCL refuses existing files, including symlinks; new logs are private.
            descriptor = Darwin.open(path, O_WRONLY | O_CREAT | O_EXCL, mode_t(0o600))
            guard descriptor >= 0 else {
                throw ProbeError("无法创建日志 \(path)：\(String(cString: strerror(errno)))；不会覆盖已有文件。")
            }
            ownsDescriptor = true
        } else {
            descriptor = STDOUT_FILENO
            ownsDescriptor = false
        }
    }

    deinit { if ownsDescriptor { Darwin.close(descriptor) } }

    func write(_ object: [String: Any]) throws {
        var data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        data.append(0x0a)
        // Unbuffered writes: each completed JSONL record is immediately visible.
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.write(descriptor, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                if count < 0 {
                    if errno == EINTR { continue }
                    throw ProbeError("写入日志失败：\(String(cString: strerror(errno)))")
                }
                guard count > 0 else { throw ProbeError("写入日志失败：write 返回 0。") }
                offset += count
            }
        }
    }
}

let eventNames: [CGEventType: String] = [
    .leftMouseDown: "leftMouseDown", .leftMouseUp: "leftMouseUp",
    .rightMouseDown: "rightMouseDown", .rightMouseUp: "rightMouseUp",
    .otherMouseDown: "otherMouseDown", .otherMouseUp: "otherMouseUp",
    .leftMouseDragged: "leftMouseDragged", .rightMouseDragged: "rightMouseDragged",
    .otherMouseDragged: "otherMouseDragged", .mouseMoved: "mouseMoved",
    .flagsChanged: "flagsChanged",
]

let movementTypes: Set<CGEventType> = [.mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged]

final class MovementFilter {
    private var lastMovement: TimeInterval = -.infinity

    func accepts(type: CGEventType, flags: CGEventFlags, now: TimeInterval) -> Bool {
        guard eventNames[type] != nil else { return false }
        guard movementTypes.contains(type) else { return true }
        guard type != .mouseMoved || flags.contains(.maskCommand) else { return false }
        guard now - lastMovement >= 0.05 else { return false } // At most 20 movement records/s.
        lastMovement = now
        return true
    }
}

func record(type: CGEventType, event: CGEvent, time: String) -> [String: Any] {
    let flags = event.flags
    let hasMouseFields = type != .flagsChanged
    return [
        "time": time,
        "type": eventNames[type] ?? "unknown",
        "flags": String(format: "0x%016llx", flags.rawValue),
        "command": flags.contains(.maskCommand),
        "control": flags.contains(.maskControl),
        "shift": flags.contains(.maskShift),
        "option": flags.contains(.maskAlternate),
        "button": hasMouseFields ? event.getIntegerValueField(.mouseEventButtonNumber) as Any : NSNull(),
        "clickCount": hasMouseFields ? event.getIntegerValueField(.mouseEventClickState) as Any : NSNull(),
        "position": ["x": event.location.x, "y": event.location.y],
        "sourceUserData": event.getIntegerValueField(.eventSourceUserData),
    ]
}

final class Probe {
    let writer: JSONLWriter
    let filter = MovementFilter()
    let clock = ISO8601DateFormatter()
    var failure: String?
    var count = 0

    init(writer: JSONLWriter) {
        self.writer = writer
        clock.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    }

    func receive(type: CGEventType, event: CGEvent) {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            failure = "系统已停用 event tap（\(type == .tapDisabledByTimeout ? "timeout" : "userInput")）；采集已停止，请检查权限或会话状态后重新运行。"
            CFRunLoopStop(CFRunLoopGetMain())
            return
        }
        guard filter.accepts(type: type, flags: event.flags, now: ProcessInfo.processInfo.systemUptime) else { return }
        do {
            try writer.write(record(type: type, event: event, time: clock.string(from: Date())))
            count += 1
        } catch {
            failure = String(describing: error)
            CFRunLoopStop(CFRunLoopGetMain())
        }
    }
}

struct Options {
    var seconds: Double = 60
    var output: String?
    var selfTest = false
    var help = false

    init(arguments: [String]) throws {
        var index = 0
        var setSeconds = false
        while index < arguments.count {
            let argument = arguments[index]
            switch argument {
            case "--help", "-h": help = true
            case "--self-test": selfTest = true
            case "--seconds", "--output":
                index += 1
                guard index < arguments.count else { throw ProbeError("\(argument) 缺少参数。") }
                if argument == "--seconds" {
                    guard let value = Double(arguments[index]), value.isFinite, value > 0 else {
                        throw ProbeError("--seconds 必须是大于 0 的有限秒数。")
                    }
                    seconds = value
                    setSeconds = true
                } else {
                    guard output == nil, !arguments[index].isEmpty else { throw ProbeError("--output 必须指定一次非空路径。") }
                    output = arguments[index]
                }
            default: throw ProbeError("未知参数：\(argument)。使用 --help 查看用法。")
            }
            index += 1
        }
        if selfTest && (setSeconds || output != nil) {
            throw ProbeError("--self-test 请单独使用；自检只将构造事件写到 stdout。")
        }
    }
}

func selfTest() throws {
    // Construct local objects only: no tap, preflight, permission request, or post.
    guard let click = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown,
                              mouseCursorPosition: CGPoint(x: 123, y: 456), mouseButton: .left),
          let modifiers = CGEvent(source: nil) else {
        throw ProbeError("自检无法构造 CGEvent。")
    }
    click.flags = [.maskCommand, .maskShift]
    click.setIntegerValueField(.mouseEventClickState, value: 1)
    click.setIntegerValueField(.eventSourceUserData, value: 4242)
    modifiers.type = .flagsChanged
    modifiers.flags = [.maskControl, .maskAlternate]
    let sample = record(type: .leftMouseDown, event: click, time: "2000-01-01T00:00:00.000Z")
    guard sample["command"] as? Bool == true, sample["control"] as? Bool == false,
          sample["shift"] as? Bool == true, sample["clickCount"] as? Int64 == 1,
          sample["sourceUserData"] as? Int64 == 4242,
          sample["flags"] as? String == "0x0000000000120000" else {
        throw ProbeError("自检失败：事件字段序列化不匹配。")
    }
    let modifierSample = record(type: .flagsChanged, event: modifiers, time: "2000-01-01T00:00:00.001Z")
    guard modifierSample["button"] is NSNull, modifierSample["clickCount"] is NSNull,
          modifierSample["control"] as? Bool == true, modifierSample["option"] as? Bool == true else {
        throw ProbeError("自检失败：修饰键字段不匹配。")
    }
    let filter = MovementFilter()
    guard !filter.accepts(type: .keyDown, flags: [], now: 0),
          !filter.accepts(type: .mouseMoved, flags: [], now: 0),
          filter.accepts(type: .mouseMoved, flags: .maskCommand, now: 1),
          !filter.accepts(type: .mouseMoved, flags: .maskCommand, now: 1.01),
          filter.accepts(type: .leftMouseDragged, flags: [], now: 1.1),
          filter.accepts(type: .leftMouseDown, flags: .maskCommand, now: 1.11) else {
        throw ProbeError("自检失败：事件筛选或移动限流不匹配。")
    }
    let writer = try JSONLWriter(path: nil)
    try writer.write(sample)
    try writer.write(modifierSample)
    diagnostic("自检通过：构造事件、字段序列化、普通按键排除、移动限流；未创建 tap、未申请权限、未发送事件。")
}

func capture(options: Options) throws {
    guard CGPreflightListenEventAccess() else {
        throw ProbeError("缺少现有输入监听权限（Input Monitoring）。请自行在系统设置 → 隐私与安全性 → 输入监控中检查运行本工具的终端/程序，并完全退出后重开。工具不会申请或修改权限。")
    }
    // Check permissions before creating a log, then create the tap before opening it.
    // This holder is populated before the run-loop source is scheduled.
    let holder = UnsafeMutablePointer<Probe?>.allocate(capacity: 1)
    holder.initialize(to: nil)
    defer { holder.deinitialize(count: 1); holder.deallocate() }
    let mask = eventNames.keys.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
    guard let tap = CGEvent.tapCreate(tap: .cgAnnotatedSessionEventTap, place: .headInsertEventTap,
                                      options: .listenOnly, eventsOfInterest: mask, callback: { _, type, event, context in
        context?.assumingMemoryBound(to: Probe?.self).pointee?.receive(type: type, event: event)
        return Unmanaged.passUnretained(event)
    }, userInfo: UnsafeMutableRawPointer(holder)) else {
        throw ProbeError("无法创建只读 annotatedSession event tap。请检查输入监控权限、当前图形登录会话及系统限制；权限预检通过并不保证 tap 可用。工具未申请权限。")
    }
    defer { CFMachPortInvalidate(tap) }
    guard let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0) else {
        throw ProbeError("无法创建 event tap run-loop source。")
    }
    let probe = Probe(writer: try JSONLWriter(path: options.output))
    holder.pointee = probe
    let runLoop = CFRunLoopGetMain()!
    CFRunLoopAddSource(runLoop, source, .commonModes)
    defer { CFRunLoopRemoveSource(runLoop, source, .commonModes) }

    let timer = CFRunLoopTimerCreateWithHandler(kCFAllocatorDefault, CFAbsoluteTimeGetCurrent() + options.seconds,
                                               0, 0, 0) { _ in CFRunLoopStop(runLoop) }
    CFRunLoopAddTimer(runLoop, timer, .commonModes)
    defer { CFRunLoopTimerInvalidate(timer) }
    // Dispatch signal handling keeps non-signal-safe code out of a POSIX handler.
    let oldInterrupt = Darwin.signal(SIGINT, SIG_IGN)
    let oldTerminate = Darwin.signal(SIGTERM, SIG_IGN)
    defer { _ = Darwin.signal(SIGINT, oldInterrupt); _ = Darwin.signal(SIGTERM, oldTerminate) }
    let interrupt = DispatchSource.makeSignalSource(signal: SIGINT, queue: .main)
    let terminate = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
    interrupt.setEventHandler { CFRunLoopStop(runLoop) }
    terminate.setEventHandler { CFRunLoopStop(runLoop) }
    interrupt.resume()
    terminate.resume()
    defer { interrupt.cancel(); terminate.cancel() }
    diagnostic("开始只读采集 \(options.seconds) 秒，Ctrl+C 提前结束。日志：\(options.output ?? "stdout")；记录鼠标坐标，不记录文字。")
    CFRunLoopRun()
    diagnostic("采集结束，共 \(probe.count) 条事件。")
    if let failure = probe.failure { throw ProbeError(failure) }
}

do {
    // Convert a closed stdout pipe into a reported write error, not signal death.
    _ = Darwin.signal(SIGPIPE, SIG_IGN)
    let options = try Options(arguments: Array(CommandLine.arguments.dropFirst()))
    if options.help {
        print("用法：macos-command-click [--seconds 秒数] [--output 新日志路径]\n      macos-command-click --self-test\n默认采集 60 秒；未指定 --output 时向 stdout 输出 JSONL。已有文件永不覆盖。")
    } else if options.selfTest {
        try selfTest()
    } else {
        try capture(options: options)
    }
} catch {
    diagnostic("错误：\(error)")
    exit(1)
}
