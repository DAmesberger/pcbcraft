// launchkit: window-only screen recording and scripted input for Pcbcraft launch takes.
//
// Build: swiftc -O -o target/launch/launchkit tools/launch/launchkit.swift
//        (tools/launch/take.sh does this for you)
//
//   launchkit window PID                   print the window id, frame (pt) and backing scale as JSON
//   launchkit record PID OUT.mov [--fps 60] [--seconds N] [--started FILE]
//                                          record only that process's largest window (ScreenCaptureKit,
//                                          H.264, backing resolution); stops on SIGINT/SIGTERM or after N s
//   launchkit play PID SHOT.shot TRACK.json [--activate]
//                                          run a shot script against the window, log the pointer track
//
// Only the given pid is ever recorded or receives events (CGEventPostToPid). Other windows are
// neither captured nor sent input. Requires macOS 15 (SCRecordingOutput) and the Screen Recording
// permission for the terminal that runs it; `play` needs Accessibility for posted events.

import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit
import AVFoundation

// MARK: - Window lookup

struct WindowInfo {
    let id: CGWindowID
    let frame: CGRect  // global points, top-left origin
}

func largestWindow(pid: pid_t) -> WindowInfo? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]]
    else { return nil }
    var best: WindowInfo?
    for entry in list {
        guard let owner = entry[kCGWindowOwnerPID as String] as? Int, owner == Int(pid),
            let layer = entry[kCGWindowLayer as String] as? Int, layer == 0,
            let number = entry[kCGWindowNumber as String] as? Int,
            let boundsDict = entry[kCGWindowBounds as String] as? NSDictionary,
            let bounds = CGRect(dictionaryRepresentation: boundsDict)
        else { continue }
        if best == nil || bounds.width * bounds.height > best!.frame.width * best!.frame.height {
            best = WindowInfo(id: CGWindowID(number), frame: bounds)
        }
    }
    return best
}

func backingScale(for frame: CGRect) -> CGFloat {
    // NSScreen uses a bottom-left origin; convert the window centre.
    guard let primary = NSScreen.screens.first else { return 2 }
    let centre = CGPoint(x: frame.midX, y: primary.frame.height - frame.midY)
    for screen in NSScreen.screens where screen.frame.contains(centre) {
        return screen.backingScaleFactor
    }
    return primary.backingScaleFactor
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

// MARK: - Recording

final class Recorder: NSObject, SCStreamOutput, SCRecordingOutputDelegate, SCStreamDelegate, @unchecked Sendable {
    var stream: SCStream?
    var started: Date?
    var startedFile: String?
    let finished = DispatchSemaphore(value: 0)

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {}

    func recordingOutputDidStartRecording(_ recordingOutput: SCRecordingOutput) {
        let now = Date()
        started = now
        if let file = startedFile {
            let text = String(format: "{\"started\": %.6f}\n", now.timeIntervalSince1970)
            try? text.write(toFile: file, atomically: true, encoding: .utf8)
        }
        FileHandle.standardError.write("recording\n".data(using: .utf8)!)
    }

    func recordingOutputDidFinishRecording(_ recordingOutput: SCRecordingOutput) {
        finished.signal()
    }

    func recordingOutput(_ recordingOutput: SCRecordingOutput, didFailWithError error: Error) {
        FileHandle.standardError.write("recording failed: \(error)\n".data(using: .utf8)!)
        finished.signal()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        FileHandle.standardError.write("stream stopped: \(error)\n".data(using: .utf8)!)
        finished.signal()
    }
}

func record(pid: pid_t, out: String, fps: Int, seconds: Double?, startedFile: String?) async {
    guard let info = largestWindow(pid: pid) else { fail("no on-screen window for pid \(pid)") }
    let content: SCShareableContent
    do {
        content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
    } catch {
        fail("ScreenCaptureKit refused: \(error). Grant Screen Recording to this terminal (System Settings › Privacy & Security › Screen & System Audio Recording).")
    }
    guard let window = content.windows.first(where: { $0.windowID == info.id }) else {
        fail("window \(info.id) not shareable")
    }
    let scale = backingScale(for: info.frame)
    let config = SCStreamConfiguration()
    config.width = Int(info.frame.width * scale) & ~1
    config.height = Int(info.frame.height * scale) & ~1
    config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(fps))
    config.showsCursor = false
    config.capturesAudio = false
    config.queueDepth = 8
    config.pixelFormat = kCVPixelFormatType_32BGRA
    config.colorSpaceName = CGColorSpace.sRGB

    let filter = SCContentFilter(desktopIndependentWindow: window)
    let recorder = Recorder()
    recorder.startedFile = startedFile
    let stream = SCStream(filter: filter, configuration: config, delegate: recorder)
    recorder.stream = stream

    let recConfig = SCRecordingOutputConfiguration()
    recConfig.outputURL = URL(fileURLWithPath: out)
    recConfig.outputFileType = .mov
    recConfig.videoCodecType = .h264
    let output = SCRecordingOutput(configuration: recConfig, delegate: recorder)
    do {
        try stream.addStreamOutput(recorder, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames"))
        try stream.addRecordingOutput(output)
        try await stream.startCapture()
    } catch {
        fail("could not start capture: \(error)")
    }
    FileHandle.standardError.write(
        "capturing window \(info.id) \(config.width)x\(config.height) at \(fps) fps -> \(out)\n".data(using: .utf8)!)

    let stop = DispatchSemaphore(value: 0)
    signal(SIGINT, SIG_IGN)
    signal(SIGTERM, SIG_IGN)
    let sources = [SIGINT, SIGTERM].map { sig -> DispatchSourceSignal in
        let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
        source.setEventHandler { stop.signal() }
        source.resume()
        return source
    }
    _ = sources
    await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
        DispatchQueue.global().async {
            if let seconds { _ = stop.wait(timeout: .now() + seconds) } else { stop.wait() }
            cont.resume()
        }
    }
    do { try await stream.stopCapture() } catch {
        FileHandle.standardError.write("stop: \(error)\n".data(using: .utf8)!)
    }
    await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
        DispatchQueue.global().async {
            _ = recorder.finished.wait(timeout: .now() + 10)
            cont.resume()
        }
    }
    FileHandle.standardError.write("saved \(out)\n".data(using: .utf8)!)
}

// MARK: - Scripted input

/// Virtual key codes for the keys a shot uses (ANSI layout).
let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
    "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
    "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34,
    "p": 35, "return": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44,
    "n": 45, "m": 46, ".": 47, "tab": 48, "space": 49, "`": 50, "delete": 51, "escape": 53,
    "left": 123, "right": 124, "down": 125, "up": 126, "home": 115, "end": 119, "pageup": 116,
    "pagedown": 121, "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96,
]

func flags(_ names: [String]) -> CGEventFlags {
    var result: CGEventFlags = []
    for name in names {
        switch name {
        case "cmd": result.insert(.maskCommand)
        case "shift": result.insert(.maskShift)
        case "alt", "option": result.insert(.maskAlternate)
        case "ctrl": result.insert(.maskControl)
        default: fail("unknown modifier \(name)")
        }
    }
    return result
}

struct TrackEvent: Encodable {
    let t: Double  // wall clock, seconds since 1970
    let kind: String  // move, down, up, key, caption, zoom, mark, scroll
    var x: Double? = nil  // window points
    var y: Double? = nil
    var text: String? = nil
    var duration: Double? = nil
    var zoom: Double? = nil
}

final class Player {
    let pid: pid_t
    var origin: CGPoint
    var pointer = CGPoint(x: 0, y: 0)  // window points
    var buttons: Set<String> = []
    var mods: CGEventFlags = []
    var track: [TrackEvent] = []
    let source = CGEventSource(stateID: .privateState)

    init(pid: pid_t, window: WindowInfo) {
        self.pid = pid
        self.origin = window.frame.origin
        self.pointer = CGPoint(x: window.frame.width / 2, y: window.frame.height / 2)
    }

    func now() -> Double { Date().timeIntervalSince1970 }

    func log(_ event: TrackEvent) { track.append(event) }

    func post(_ event: CGEvent?) {
        guard let event else { return }
        event.postToPid(pid)
    }

    func global(_ p: CGPoint) -> CGPoint { CGPoint(x: origin.x + p.x, y: origin.y + p.y) }

    func mouse(_ type: CGEventType, button: CGMouseButton, at p: CGPoint, clicks: Int64 = 1) {
        let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: global(p), mouseButton: button)
        event?.flags = mods
        event?.setIntegerValueField(.mouseEventClickState, value: clicks)
        post(event)
    }

    func buttonOf(_ name: String) -> (CGMouseButton, CGEventType, CGEventType, CGEventType) {
        switch name {
        case "right": return (.right, .rightMouseDown, .rightMouseUp, .rightMouseDragged)
        case "middle": return (.center, .otherMouseDown, .otherMouseUp, .otherMouseDragged)
        default: return (.left, .leftMouseDown, .leftMouseUp, .leftMouseDragged)
        }
    }

    /// Glide the pointer with an ease-in-out curve at 120 Hz.
    func glide(to target: CGPoint, over seconds: Double) {
        let start = pointer
        let steps = max(1, Int(seconds * 120))
        for i in 1...steps {
            let u = Double(i) / Double(steps)
            let e = u < 0.5 ? 4 * u * u * u : 1 - pow(-2 * u + 2, 3) / 2
            let p = CGPoint(x: start.x + (target.x - start.x) * e, y: start.y + (target.y - start.y) * e)
            pointer = p
            if let held = buttons.first {
                let (button, _, _, dragType) = buttonOf(held)
                mouse(dragType, button: button, at: p)
            } else {
                mouse(.mouseMoved, button: .left, at: p)
            }
            log(TrackEvent(t: now(), kind: "move", x: p.x, y: p.y))
            Thread.sleep(forTimeInterval: seconds / Double(steps))
        }
    }

    func down(_ name: String, clicks: Int64 = 1) {
        let (button, downType, _, _) = buttonOf(name)
        buttons.insert(name)
        mouse(downType, button: button, at: pointer, clicks: clicks)
        log(TrackEvent(t: now(), kind: "down", x: pointer.x, y: pointer.y, text: name))
    }

    func up(_ name: String, clicks: Int64 = 1) {
        let (button, _, upType, _) = buttonOf(name)
        buttons.remove(name)
        mouse(upType, button: button, at: pointer, clicks: clicks)
        log(TrackEvent(t: now(), kind: "up", x: pointer.x, y: pointer.y, text: name))
    }

    func key(_ name: String, modifiers: [String]) {
        guard let code = keyCodes[name.lowercased()] else { fail("unknown key \(name)") }
        let f = flags(modifiers).union(mods)
        let down = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true)
        down?.flags = f
        post(down)
        Thread.sleep(forTimeInterval: 0.03)
        let up = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)
        up?.flags = f
        post(up)
        log(TrackEvent(t: now(), kind: "key", text: (modifiers + [name]).joined(separator: "+")))
    }

    func type(_ text: String, cps: Double) {
        for scalar in text.unicodeScalars {
            let units = Array(String(scalar).utf16)
            for keyDown in [true, false] {
                let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: keyDown)
                event?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
                post(event)
            }
            Thread.sleep(forTimeInterval: 1 / cps)
        }
        log(TrackEvent(t: now(), kind: "type", text: text))
    }

    func scroll(dx: Int32, dy: Int32, steps: Int, seconds: Double) {
        for _ in 0..<max(1, steps) {
            let event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0)
            event?.location = global(pointer)
            event?.flags = mods
            post(event)
            Thread.sleep(forTimeInterval: seconds / Double(max(1, steps)))
        }
        log(TrackEvent(t: now(), kind: "scroll", x: pointer.x, y: pointer.y))
    }
}

/// Split a line into words, keeping "quoted text" together.
func words(_ line: String) -> [String] {
    var result: [String] = []
    var current = ""
    var quoted = false
    for ch in line {
        if ch == "\"" { quoted.toggle(); if !quoted { result.append(current); current = "" }; continue }
        if ch == " " && !quoted { if !current.isEmpty { result.append(current); current = "" }; continue }
        current.append(ch)
    }
    if !current.isEmpty { result.append(current) }
    return result
}

func play(pid: pid_t, shot: String, trackOut: String, activate: Bool) {
    guard let window = largestWindow(pid: pid) else { fail("no on-screen window for pid \(pid)") }
    if !AXIsProcessTrusted() {
        FileHandle.standardError.write("warning: this terminal lacks Accessibility; posted events may be dropped\n".data(using: .utf8)!)
    }
    if activate, let app = NSRunningApplication(processIdentifier: pid) {
        app.activate()
        Thread.sleep(forTimeInterval: 0.4)
    }
    guard let text = try? String(contentsOfFile: shot, encoding: .utf8) else { fail("cannot read \(shot)") }
    let player = Player(pid: pid, window: window)
    player.log(TrackEvent(t: player.now(), kind: "begin", x: Double(window.frame.width), y: Double(window.frame.height)))
    for (number, raw) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
        let line = raw.split(separator: "#", maxSplits: 1, omittingEmptySubsequences: false).first.map(String.init) ?? ""
        let w = words(line.trimmingCharacters(in: .whitespaces))
        guard let op = w.first else { continue }
        let args = Array(w.dropFirst())
        func num(_ i: Int, _ fallback: Double? = nil) -> Double {
            if i < args.count, let v = Double(args[i]) { return v }
            if let fallback { return fallback }
            fail("\(shot):\(number + 1): \(op) needs a number at argument \(i + 1)")
        }
        switch op {
        case "wait": Thread.sleep(forTimeInterval: num(0))
        case "move": player.glide(to: CGPoint(x: num(0), y: num(1)), over: num(2, 0.6))
        case "click":
            let button = args.first(where: { ["left", "right", "middle"].contains($0) }) ?? "left"
            let coords = args.compactMap(Double.init)
            if coords.count >= 2 { player.glide(to: CGPoint(x: coords[0], y: coords[1]), over: coords.count > 2 ? coords[2] : 0.5) }
            player.down(button); Thread.sleep(forTimeInterval: 0.06); player.up(button)
        case "dclick":
            player.down("left", clicks: 1); player.up("left", clicks: 1)
            Thread.sleep(forTimeInterval: 0.08)
            player.down("left", clicks: 2); player.up("left", clicks: 2)
        case "down": player.down(args.first ?? "left")
        case "up": player.up(args.first ?? "left")
        case "drag":
            // drag X1 Y1 X2 Y2 SECONDS [left|middle|right]
            let button = args.count > 5 ? args[5] : "left"
            player.glide(to: CGPoint(x: num(0), y: num(1)), over: 0.4)
            player.down(button); Thread.sleep(forTimeInterval: 0.05)
            player.glide(to: CGPoint(x: num(2), y: num(3)), over: num(4, 1.0))
            Thread.sleep(forTimeInterval: 0.05); player.up(button)
        case "hold": player.mods = flags(args)  // hold cmd shift … until "release"
        case "release": player.mods = []
        case "key": player.key(args.first ?? "", modifiers: Array(args.dropFirst()))
        case "repeat":
            // repeat N INTERVAL key NAME [mods…]  — e.g. a smooth orbit from arrow keys
            let count = Int(num(0)); let interval = num(1)
            for _ in 0..<count {
                player.key(args.count > 3 ? args[3] : "", modifiers: Array(args.dropFirst(4)))
                Thread.sleep(forTimeInterval: interval)
            }
        case "type": player.type(args.first ?? "", cps: num(1, 14))
        case "scroll": player.scroll(dx: Int32(num(0)), dy: Int32(num(1)), steps: Int(num(2, 1)), seconds: num(3, 0.2))
        case "caption":
            player.log(TrackEvent(t: player.now(), kind: "caption", text: args.first ?? "", duration: num(1, 3)))
        case "zoom":
            // zoom FACTOR CX CY SECONDS — a post-production push-in centred on a window point
            player.log(TrackEvent(t: player.now(), kind: "zoom", x: num(1), y: num(2), duration: num(3, 2), zoom: num(0)))
        case "mark": player.log(TrackEvent(t: player.now(), kind: "mark", text: args.first ?? ""))
        default: fail("\(shot):\(number + 1): unknown action \(op)")
        }
    }
    player.log(TrackEvent(t: player.now(), kind: "end"))
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted]
    guard let data = try? encoder.encode(player.track) else { fail("cannot encode track") }
    FileManager.default.createFile(atPath: trackOut, contents: data)
}

// MARK: - Entry

let argv = CommandLine.arguments
func option(_ name: String) -> String? {
    guard let i = argv.firstIndex(of: name), i + 1 < argv.count else { return nil }
    return argv[i + 1]
}
guard argv.count >= 3, let pid = pid_t(argv[2]) else {
    fail("usage: launchkit window PID | record PID OUT.mov [--fps 60] [--seconds N] [--started FILE] | play PID SHOT TRACK.json [--activate]")
}
switch argv[1] {
case "window":
    guard let info = largestWindow(pid: pid) else { fail("no window for pid \(pid)") }
    print(String(format: "{\"id\": %u, \"x\": %.1f, \"y\": %.1f, \"width\": %.1f, \"height\": %.1f, \"scale\": %.1f}",
                 info.id, info.frame.minX, info.frame.minY, info.frame.width, info.frame.height, backingScale(for: info.frame)))
case "record":
    guard argv.count >= 4 else { fail("record PID OUT.mov") }
    let fps = Int(option("--fps") ?? "60") ?? 60
    let seconds = option("--seconds").flatMap(Double.init)
    let done = DispatchSemaphore(value: 0)
    Task {
        await record(pid: pid, out: argv[3], fps: fps, seconds: seconds, startedFile: option("--started"))
        done.signal()
    }
    while done.wait(timeout: .now()) == .timedOut {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    }
case "play":
    guard argv.count >= 5 else { fail("play PID SHOT TRACK.json") }
    play(pid: pid, shot: argv[3], trackOut: argv[4], activate: argv.contains("--activate"))
default:
    fail("unknown command \(argv[1])")
}
