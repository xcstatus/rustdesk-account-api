import AppKit
import CoreGraphics
import Darwin

// In-memory A/B check only: no NSApplication, tap, permission API, or event post.
func check(_ label: String, type: CGEventType, explicitCount: Int64? = nil) -> Int {
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type,
                              mouseCursorPosition: .zero, mouseButton: .left) else {
        fputs("FAIL: CGEvent creation failed\n", stderr)
        exit(1)
    }
    if let explicitCount { event.setIntegerValueField(.mouseEventClickState, value: explicitCount) }
    guard let cocoaEvent = NSEvent(cgEvent: event) else {
        fputs("FAIL: NSEvent conversion failed\n", stderr)
        exit(1)
    }
    let raw = event.getIntegerValueField(.mouseEventClickState)
    print("\(label): CGEvent.clickCount=\(raw) NSEvent.clickCount=\(cocoaEvent.clickCount)")
    guard raw == Int64(cocoaEvent.clickCount) else {
        fputs("FAIL: CGEvent / NSEvent count mismatch\n", stderr)
        exit(1)
    }
    return cocoaEvent.clickCount
}

let down = check("down(default)", type: .leftMouseDown)
let before = check("up(default)", type: .leftMouseUp)
let after = check("up(explicit 1)", type: .leftMouseUp, explicitCount: 1)
guard down == 1, before == 0, after == 1 else {
    fputs("FAIL: unexpected platform defaults; inspect counts above\n", stderr)
    exit(1)
}
print("PASS: explicit mouse-up count survives NSEvent conversion (0 → 1); no input posted.")
