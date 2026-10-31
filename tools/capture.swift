// Copyright 2026 Raph Levien
// SPDX-License-Identifier: Apache-2.0 OR MIT

// Grab frames from the HDMI capture device, save the last as a PNG, and
// count rows of red err_line pixels in each frame.
//
// Usage: capture OUT.png [FRAMES [WIDTHxHEIGHT:CODEC]]
// The format is one of the device's, for example 1280x960:420v, at its
// highest frame rate. If OUT ends in .mov, the frames are recorded instead,
// enlarged by $CAPTURE_SCALE (default 3) with nearest neighbour, as ProRes.
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
    FileHandle.standardError.write("usage: capture OUT.png [FRAMES [WIDTHxHEIGHT:CODEC]]\n".data(using: .utf8)!)
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

/// Writes frames to a movie, enlarged by a whole number. The device's own
/// scaler blurs, so it's better to capture at the source's size.
final class Recorder {
    let scale: Int
    let writer: AVAssetWriter
    var input: AVAssetWriterInput?
    var adaptor: AVAssetWriterInputPixelBufferAdaptor?
    var started = false
    var dropped = 0

    init(path: String, scale: Int) throws {
        self.scale = scale
        try? FileManager.default.removeItem(atPath: path)
        writer = try AVAssetWriter(url: URL(fileURLWithPath: path), fileType: .mov)
    }

    /// Starting takes a couple of hundred ms, so do it during warmup.
    func start(width: Int, height: Int) {
        let w = width * scale
        let h = height * scale
        let input = AVAssetWriterInput(
            mediaType: .video,
            outputSettings: [
                AVVideoCodecKey: AVVideoCodecType.proRes422HQ,
                AVVideoWidthKey: w,
                AVVideoHeightKey: h,
            ])
        input.expectsMediaDataInRealTime = true
        adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: w,
                kCVPixelBufferHeightKey as String: h,
            ])
        writer.add(input)
        writer.startWriting()
        self.input = input
    }

    /// Add a locked frame.
    func append(_ pb: CVPixelBuffer, at time: CMTime) {
        let width = CVPixelBufferGetWidth(pb)
        let height = CVPixelBufferGetHeight(pb)
        if !started {
            writer.startSession(atSourceTime: time)
            started = true
        }
        var out: CVPixelBuffer?
        guard input!.isReadyForMoreMediaData, let pool = adaptor!.pixelBufferPool,
            CVPixelBufferPoolCreatePixelBuffer(nil, pool, &out) == kCVReturnSuccess,
            let out
        else {
            dropped += 1
            return
        }
        CVPixelBufferLockBaseAddress(out, [])
        let srcStride = CVPixelBufferGetBytesPerRow(pb)
        let dstStride = CVPixelBufferGetBytesPerRow(out)
        let src = CVPixelBufferGetBaseAddress(pb)!
        let dst = CVPixelBufferGetBaseAddress(out)!
        for y in 0..<height {
            let row = (src + y * srcStride).assumingMemoryBound(to: UInt32.self)
            let first = dst + y * scale * dstStride
            let wide = first.assumingMemoryBound(to: UInt32.self)
            for x in 0..<width {
                for i in 0..<scale { wide[x * scale + i] = row[x] }
            }
            for i in 1..<max(scale, 1) {
                memcpy(first + i * dstStride, first, width * scale * 4)
            }
        }
        CVPixelBufferUnlockBaseAddress(out, [])
        if !adaptor!.append(out, withPresentationTime: time) { dropped += 1 }
    }

    func finish() {
        input?.markAsFinished()
        let done = DispatchSemaphore(value: 0)
        writer.finishWriting { done.signal() }
        done.wait()
    }
}

final class Grabber: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    var seen = 0
    var redRows: [Int] = []
    var last: CVPixelBuffer?
    var width = 0
    var height = 0
    // For pacing: frames that changed, and the times they arrived
    var changed = 0
    var lastHash: UInt64 = 0
    var times: [Double] = []

    func captureOutput(
        _ output: AVCaptureOutput, didOutput sample: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        seen += 1
        guard let pb = CMSampleBufferGetImageBuffer(sample) else { return }
        if seen == 1 {
            let (w, h) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb))
            recorder?.start(width: w, height: h)
        }
        guard seen > warmup else { return }
        if redRows.count >= wantFrames { return }
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        recorder?.append(pb, at: CMSampleBufferGetPresentationTimeStamp(sample))
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
        // Hash a sparse grid of pixels, to spot repeated frames.
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for y in Swift.stride(from: 0, to: height, by: 8) {
            let row = base + y * stride
            for x in Swift.stride(from: 0, to: width * 4, by: 28) {
                hash = (hash ^ UInt64(row[x])) &* 0x100_0000_01b3
            }
        }
        if hash != lastHash { changed += 1 }
        lastHash = hash
        times.append(CMSampleBufferGetPresentationTimeStamp(sample).seconds)
        last = pb
        if redRows.count == wantFrames {
            CFRunLoopStop(CFRunLoopGetMain())
        }
    }
}

/// The device format named by a string like 1280x960:420v.
func findFormat(_ spec: String) -> AVCaptureDevice.Format? {
    device.formats.first { f in
        let dims = CMVideoFormatDescriptionGetDimensions(f.formatDescription)
        let sub = CMFormatDescriptionGetMediaSubType(f.formatDescription)
        let codec = String(decoding: [24, 16, 8, 0].map { UInt8((sub >> $0) & 255) }, as: UTF8.self)
        return "\(dims.width)x\(dims.height):\(codec)" == spec
    }
}

let session = AVCaptureSession()
if args.count <= 3, session.canSetSessionPreset(.vga640x480) {
    session.sessionPreset = .vga640x480
}
guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) else {
    fail("can't open \(device.localizedName) (camera permission?)")
}
session.addInput(input)
let output = AVCaptureVideoDataOutput()
output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
var recorder: Recorder?
if outPath.hasSuffix(".mov") {
    let scale = Int(ProcessInfo.processInfo.environment["CAPTURE_SCALE"] ?? "") ?? 3
    guard let r = try? Recorder(path: outPath, scale: scale) else { fail("can't write \(outPath)") }
    recorder = r
}
let grabber = Grabber()
output.setSampleBufferDelegate(grabber, queue: DispatchQueue(label: "capture"))
guard session.canAddOutput(output) else { fail("can't add video output") }
session.addOutput(output)
if args.count > 3 {
    guard let format = findFormat(args[3]) else { fail("no format \(args[3])") }
    guard let fastest = format.videoSupportedFrameRateRanges.max(by: {
        $0.maxFrameRate < $1.maxFrameRate
    }) else { fail("no frame rates for \(args[3])") }
    let dims = CMVideoFormatDescriptionGetDimensions(format.formatDescription)
    guard (try? device.lockForConfiguration()) != nil else { fail("can't configure device") }
    device.activeFormat = format
    device.activeVideoMinFrameDuration = fastest.minFrameDuration
    device.activeVideoMaxFrameDuration = fastest.minFrameDuration
    device.unlockForConfiguration()
    // Otherwise the output can scale to a size of its own choosing.
    output.videoSettings = [
        kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
        kCVPixelBufferWidthKey as String: Int(dims.width),
        kCVPixelBufferHeightKey as String: Int(dims.height),
    ]
}
session.startRunning()

// Give up after a while if no frames arrive.
DispatchQueue.main.asyncAfter(deadline: .now() + 10 + Double(wantFrames) / 30) {
    CFRunLoopStop(CFRunLoopGetMain())
}
CFRunLoopRun()
session.stopRunning()

guard let pb = grabber.last else { fail("no frames from \(device.localizedName)") }
if let recorder {
    recorder.finish()
    if recorder.writer.status != .completed {
        fail("recording failed: \(String(describing: recorder.writer.error))")
    }
    print("recorded \(outPath), dropped \(recorder.dropped)")
} else {
    let ci = CIImage(cvPixelBuffer: pb)
    guard let cg = CIContext().createCGImage(ci, from: ci.extent),
        let dest = CGImageDestinationCreateWithURL(
            URL(fileURLWithPath: outPath) as CFURL, UTType.png.identifier as CFString, 1, nil)
    else { fail("can't write \(outPath)") }
    CGImageDestinationAddImage(dest, cg, nil)
    CGImageDestinationFinalize(dest)
}

let rows = grabber.redRows
print("\(device.localizedName): \(grabber.width)x\(grabber.height), \(rows.count) frames")
let gaps = zip(grabber.times.dropFirst(), grabber.times).map { ($0 - $1) * 1000 }
if let shortest = gaps.min(), let longest = gaps.max() {
    let mean = gaps.reduce(0, +) / Double(gaps.count)
    print(String(
        format: "frames changed %d of %d, ms between frames avg %.2f min %.2f max %.2f",
        grabber.changed, rows.count, mean, shortest, longest))
}
print("red rows per frame: max \(rows.max() ?? 0), frames with red \(rows.filter { $0 > 0 }.count)")
finish(rows.contains { $0 > 0 } ? 3 : 0)
