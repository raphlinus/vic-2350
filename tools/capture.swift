// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Grab frames from the HDMI capture device, save the last as a PNG, and
// count rows of red err_line pixels in each frame.
//
// Usage: capture OUT.png [FRAMES]
// Built into build/Capture.app and run by tools/capture.sh, so macOS
// asks for camera access for the app itself. The exit code also goes to
// OUT.png.rc, since `open` doesn't pass it on. The device is the first
// video device whose name contains $CAPTURE_DEVICE (default "Capture").

import AVFoundation
import CoreImage
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count >= 2 else {
    FileHandle.standardError.write("usage: capture OUT.png [FRAMES]\n".data(using: .utf8)!)
    exit(2)
}
let outPath = args[1]
let wantFrames = args.count > 2 ? Int(args[2]) ?? 30 : 30
// The capture device needs some frames to settle after starting.
let warmup = 15
let needle = ProcessInfo.processInfo.environment["CAPTURE_DEVICE"] ?? "Capture"

func finish(_ code: Int32) -> Never {
    try? "\(code)\n".write(toFile: outPath + ".rc", atomically: true, encoding: .utf8)
    exit(code)
}

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write("capture: \(msg)\n".data(using: .utf8)!)
    finish(1)
}

if AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined {
    let done = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .video) { _ in done.signal() }
    done.wait()
}
if AVCaptureDevice.authorizationStatus(for: .video) != .authorized {
    fail("camera access not allowed; see System Settings > Privacy & Security > Camera")
}

let discovery = AVCaptureDevice.DiscoverySession(
    deviceTypes: [.external], mediaType: .video, position: .unspecified)
guard let device = discovery.devices.first(where: { $0.localizedName.contains(needle) }) else {
    fail("no video device matching \"\(needle)\": \(discovery.devices.map { $0.localizedName })")
}

final class Grabber: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    var seen = 0
    var redRows: [Int] = []
    var last: CVPixelBuffer?
    var width = 0
    var height = 0

    func captureOutput(
        _ output: AVCaptureOutput, didOutput sample: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        seen += 1
        guard seen > warmup, let pb = CMSampleBufferGetImageBuffer(sample) else { return }
        if redRows.count >= wantFrames { return }
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        width = CVPixelBufferGetWidth(pb)
        height = CVPixelBufferGetHeight(pb)
        let stride = CVPixelBufferGetBytesPerRow(pb)
        let base = CVPixelBufferGetBaseAddress(pb)!.assumingMemoryBound(to: UInt8.self)
        // A row counts as red if most of its pixels are saturated red (BGRA).
        var rows = 0
        for y in 0..<height {
            var red = 0
            let row = base + y * stride
            for x in Swift.stride(from: 0, to: width, by: 4) {
                let p = row + x * 4
                if p[2] > 180 && p[1] < 80 && p[0] < 80 { red += 1 }
            }
            if red * 4 * 2 > width { rows += 1 }
        }
        redRows.append(rows)
        last = pb
        if redRows.count == wantFrames {
            CFRunLoopStop(CFRunLoopGetMain())
        }
    }
}

let session = AVCaptureSession()
if session.canSetSessionPreset(.vga640x480) {
    session.sessionPreset = .vga640x480
}
guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) else {
    fail("can't open \(device.localizedName) (camera permission?)")
}
session.addInput(input)
let output = AVCaptureVideoDataOutput()
output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
let grabber = Grabber()
output.setSampleBufferDelegate(grabber, queue: DispatchQueue(label: "capture"))
guard session.canAddOutput(output) else { fail("can't add video output") }
session.addOutput(output)
session.startRunning()

// Give up after a while if no frames arrive.
DispatchQueue.main.asyncAfter(deadline: .now() + 10 + Double(wantFrames) / 30) {
    CFRunLoopStop(CFRunLoopGetMain())
}
CFRunLoopRun()
session.stopRunning()

guard let pb = grabber.last else { fail("no frames from \(device.localizedName)") }
let ci = CIImage(cvPixelBuffer: pb)
guard let cg = CIContext().createCGImage(ci, from: ci.extent),
    let dest = CGImageDestinationCreateWithURL(
        URL(fileURLWithPath: outPath) as CFURL, UTType.png.identifier as CFString, 1, nil)
else { fail("can't write \(outPath)") }
CGImageDestinationAddImage(dest, cg, nil)
CGImageDestinationFinalize(dest)

let rows = grabber.redRows
print("\(device.localizedName): \(grabber.width)x\(grabber.height), \(rows.count) frames")
print("red rows per frame: max \(rows.max() ?? 0), frames with red \(rows.filter { $0 > 0 }.count)")
finish(rows.contains { $0 > 0 } ? 3 : 0)
