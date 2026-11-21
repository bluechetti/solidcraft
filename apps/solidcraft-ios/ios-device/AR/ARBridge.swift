// ARKit for SolidCraft's AR view: the world-tracking session, each frame's camera picture and
// camera matrices as plain floats, raycasts onto surfaces, and Apple's coaching overlay.
// Loaded by the Rust app at run time (dlopen) and reached through the C functions at the bottom
// (`src/ar.rs`). Swift because ARKit's matrices are SIMD types that Rust bindings cannot pass.
// Everything here runs on the main thread (the app's UI thread), where ARKit's delegate is called.
import ARKit
import CoreVideo
import Metal
import Photos
import ReplayKit
import UIKit

private final class Bridge: NSObject, ARSessionDelegate, ARCoachingOverlayViewDelegate {
    let session = ARSession()
    var overlay: ARCoachingOverlayView?
    var running = false
    /// What the app shows when the session cannot run (empty while it can).
    var failure = ""
    /// Set when tracking started again from scratch (after an interruption or a reset): the
    /// world's origin moved, so a placed model is no longer where it was put.
    var restarted = false
    var interrupted = false
    var lastTimestamp: TimeInterval = -1
    /// The pixel buffer locked for the app to read, unlocked by `sc_ar_frame_done`.
    var locked: CVPixelBuffer?
    /// The depth map locked with it.
    var lockedDepth: CVPixelBuffer?

    override init() {
        super.init()
        session.delegate = self
        session.delegateQueue = .main
    }

    func configuration() -> ARWorldTrackingConfiguration {
        let c = ARWorldTrackingConfiguration()
        c.planeDetection = [.horizontal, .vertical]
        c.environmentTexturing = .none
        c.isLightEstimationEnabled = true
        // LiDAR's depth of the room, for occlusion: real things in front of the model hide it.
        // Smoothed over a few frames, so edges do not flicker.
        if ARWorldTrackingConfiguration.supportsFrameSemantics(.smoothedSceneDepth) {
            c.frameSemantics.insert(.smoothedSceneDepth)
        }
        return c
    }

    func start() {
        failure = ""
        session.run(configuration(), options: [.resetTracking, .removeExistingAnchors])
        running = true
        restarted = true
        showOverlay()
    }

    func stop() {
        unlock()
        session.pause()
        running = false
        overlay?.setActive(false, animated: false)
        overlay?.removeFromSuperview()
        overlay = nil
    }

    func unlock() {
        if let b = locked {
            CVPixelBufferUnlockBaseAddress(b, .readOnly)
            locked = nil
        }
        if let d = lockedDepth {
            CVPixelBufferUnlockBaseAddress(d, .readOnly)
            lockedDepth = nil
        }
    }

    func window() -> UIWindow? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let scene = scenes.first { $0.activationState == .foregroundActive } ?? scenes.first
        return scene?.windows.first { $0.isKeyWindow } ?? scene?.windows.first
    }

    func orientation() -> UIInterfaceOrientation {
        window()?.windowScene?.effectiveGeometry.interfaceOrientation ?? .landscapeRight
    }

    func showOverlay() {
        guard overlay == nil, let w = window() else { return }
        let o = ARCoachingOverlayView(frame: w.bounds)
        o.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        o.session = session
        o.goal = .horizontalPlane
        o.activatesAutomatically = true
        o.delegate = self
        w.addSubview(o)
        overlay = o
    }

    // MARK: ARSessionDelegate

    func session(_ session: ARSession, didFailWithError error: Error) {
        let e = error as NSError
        if e.domain == ARError.errorDomain, e.code == ARError.Code.cameraUnauthorized.rawValue {
            failure = "Camera access is off for SolidCraft. Turn it on in Settings › Privacy › Camera."
        } else {
            failure = e.localizedDescription
        }
        stop()
    }

    func sessionWasInterrupted(_ session: ARSession) { interrupted = true }

    func sessionInterruptionEnded(_ session: ARSession) {
        // ARKit carries on with the room it knew: it relocalizes (tracking is limited with the
        // reason .relocalizing until it recognizes where it is), and the world's origin is where
        // it was, so the model stays where it was put. The coaching overlay asks the user to
        // point at where they were, and offers a reset if that does not work.
        interrupted = false
    }

    func sessionShouldAttemptRelocalization(_ session: ARSession) -> Bool { true }

    // MARK: ARCoachingOverlayViewDelegate

    func coachingOverlayViewDidRequestSessionReset(_ coachingOverlayView: ARCoachingOverlayView) {
        start()
    }
}

nonisolated(unsafe) private var bridge: Bridge?

private func shared() -> Bridge {
    if let b = bridge { return b }
    let b = Bridge()
    bridge = b
    return b
}

private func write(_ m: simd_float4x4, _ out: UnsafeMutablePointer<Float>) {
    for c in 0..<4 { for r in 0..<4 { out[c * 4 + r] = m[c][r] } }
}

/// 1 when this device can run world tracking.
@_cdecl("sc_ar_supported")
public func sc_ar_supported() -> Int32 { ARWorldTrackingConfiguration.isSupported ? 1 : 0 }

/// 1 when the device has LiDAR (scene depth), for later occlusion.
@_cdecl("sc_ar_has_depth")
public func sc_ar_has_depth() -> Int32 {
    ARWorldTrackingConfiguration.supportsFrameSemantics(.sceneDepth) ? 1 : 0
}

@_cdecl("sc_ar_start")
public func sc_ar_start() { shared().start() }

@_cdecl("sc_ar_stop")
public func sc_ar_stop() { bridge?.stop() }

/// Why the session is not running (a string to free), or null while it is or has not failed.
@_cdecl("sc_ar_failure")
public func sc_ar_failure() -> UnsafeMutablePointer<CChar>? {
    guard let b = bridge, !b.failure.isEmpty else { return nil }
    return strdup(b.failure)
}

/// 1 once after tracking started again from scratch (the placed model must be put down again).
@_cdecl("sc_ar_take_restarted")
public func sc_ar_take_restarted() -> Int32 {
    guard let b = bridge, b.restarted else { return 0 }
    b.restarted = false
    return 1
}

/// 1 while Apple's coaching overlay is up (the app shows no hints of its own then).
@_cdecl("sc_ar_coaching")
public func sc_ar_coaching() -> Int32 { (bridge?.overlay?.isActive ?? false) ? 1 : 0 }

/// The current frame for a view of `width` x `height` points:
/// - `view`, `proj` (16 floats each, column-major): world to camera, and camera to clip with
///   ARKit's depth (the app puts its own depth row in), for the screen's orientation;
/// - `camera` (16): the camera's pose in the world;
/// - `display` (6): view point (0..1) to camera picture point (0..1), as a, b, c, d, tx, ty;
/// - `info` (9 ints): tracking state (0 not available, 1 limited, 2 normal), the limited reason
///   (0 none, 1 initializing, 2 excessive motion, 3 insufficient features, 4 relocalizing),
///   interrupted, picture width, height, luma row bytes, chroma row bytes, light (lumens, 1000
///   for a well-lit room), its colour temperature (kelvin, 6500 neutral);
/// - `planes` (2 pointers): the picture's luma (R8) and chroma (RG8, half size) bytes, filled
///   only for a new picture and readable until `sc_ar_frame_done`.
/// Returns 0 with no frame, 1 with a new picture, 2 for the same picture as before.
@_cdecl("sc_ar_frame")
public func sc_ar_frame(
    _ width: Float, _ height: Float,
    _ view: UnsafeMutablePointer<Float>, _ proj: UnsafeMutablePointer<Float>,
    _ camera: UnsafeMutablePointer<Float>, _ display: UnsafeMutablePointer<Float>,
    _ info: UnsafeMutablePointer<Int32>, _ planes: UnsafeMutablePointer<UnsafeRawPointer?>
) -> Int32 {
    guard let b = bridge, b.running, let frame = b.session.currentFrame, width > 0, height > 0 else { return 0 }
    b.unlock()
    let o = b.orientation()
    let size = CGSize(width: CGFloat(width), height: CGFloat(height))
    let cam = frame.camera
    write(cam.viewMatrix(for: o), view)
    write(cam.projectionMatrix(for: o, viewportSize: size, zNear: 0.03, zFar: 200), proj)
    write(cam.transform, camera)
    let t = frame.displayTransform(for: o, viewportSize: size).inverted()
    for (i, v) in [t.a, t.b, t.c, t.d, t.tx, t.ty].enumerated() { display[i] = Float(v) }
    switch cam.trackingState {
    case .notAvailable: info[0] = 0; info[1] = 0
    case .normal: info[0] = 2; info[1] = 0
    case .limited(let why):
        info[0] = 1
        switch why {
        case .initializing: info[1] = 1
        case .excessiveMotion: info[1] = 2
        case .insufficientFeatures: info[1] = 3
        case .relocalizing: info[1] = 4
        @unknown default: info[1] = 0
        }
    }
    info[2] = b.interrupted ? 1 : 0
    info[7] = Int32(frame.lightEstimate?.ambientIntensity ?? 1000)
    info[8] = Int32(frame.lightEstimate?.ambientColorTemperature ?? 6500)
    let pixels = frame.capturedImage
    info[3] = Int32(CVPixelBufferGetWidthOfPlane(pixels, 0))
    info[4] = Int32(CVPixelBufferGetHeightOfPlane(pixels, 0))
    if frame.timestamp == b.lastTimestamp { return 2 }
    guard CVPixelBufferGetPlaneCount(pixels) >= 2,
          CVPixelBufferLockBaseAddress(pixels, .readOnly) == kCVReturnSuccess else { return 2 }
    b.locked = pixels
    b.lastTimestamp = frame.timestamp
    info[5] = Int32(CVPixelBufferGetBytesPerRowOfPlane(pixels, 0))
    info[6] = Int32(CVPixelBufferGetBytesPerRowOfPlane(pixels, 1))
    planes[0] = UnsafeRawPointer(CVPixelBufferGetBaseAddressOfPlane(pixels, 0))
    planes[1] = UnsafeRawPointer(CVPixelBufferGetBaseAddressOfPlane(pixels, 1))
    return 1
}

/// The app has copied the picture: its bytes go back to ARKit.
@_cdecl("sc_ar_frame_done")
public func sc_ar_frame_done() { bridge?.unlock() }

/// A ray from view point (`x`, `y`, 0..1 across and down the view of `width` x `height` points)
/// onto a surface ARKit found (a detected plane first, else an estimated one). Writes the hit's
/// pose (16 floats, column-major; its Y is the surface's normal) and returns 1 on a hit.
@_cdecl("sc_ar_raycast")
public func sc_ar_raycast(_ x: Float, _ y: Float, _ width: Float, _ height: Float, _ out: UnsafeMutablePointer<Float>) -> Int32 {
    guard let b = bridge, b.running, let frame = b.session.currentFrame else { return 0 }
    let size = CGSize(width: CGFloat(width), height: CGFloat(height))
    let toImage = frame.displayTransform(for: b.orientation(), viewportSize: size).inverted()
    let point = CGPoint(x: CGFloat(x), y: CGFloat(y)).applying(toImage)
    for target in [ARRaycastQuery.Target.existingPlaneGeometry, .estimatedPlane] {
        let query = frame.raycastQuery(from: point, allowing: target, alignment: .any)
        if let hit = b.session.raycast(query).first {
            write(hit.worldTransform, out)
            return 1
        }
    }
    return 0
}

/// A light tap from the system's haptics (the snap to real size, a placement).
@_cdecl("sc_ar_tick")
public func sc_ar_tick() {
    // An iPad has no haptic engine of its own; Apple Pencil Pro does, and a canvas feedback
    // generator plays there while the Pencil is held (and nowhere otherwise).
    if let view = bridge?.window() {
        let g = UICanvasFeedbackGenerator(view: view)
        g.alignmentOccurred(at: CGPoint(x: view.bounds.midX, y: view.bounds.midY))
    }
    UIImpactFeedbackGenerator(style: .light).impactOccurred()
}

/// How hot the device runs: 0 nominal, 1 fair, 2 serious, 3 critical (iPadOS slows it down).
@_cdecl("sc_ar_thermal")
public func sc_ar_thermal() -> Int32 {
    switch ProcessInfo.processInfo.thermalState {
    case .nominal: return 0
    case .fair: return 1
    case .serious: return 2
    case .critical: return 3
    @unknown default: return 1
    }
}

/// The room's depth for the picture `sc_ar_frame` just gave (call it only after a 1 from there,
/// before `sc_ar_frame_done`): metres from the camera along its axis, 32-bit floats, in the
/// camera picture's own layout (the same display transform maps to it). Writes the bytes'
/// address and `info` (width, height, row bytes), readable until `sc_ar_frame_done`; returns 1
/// with a depth map, 0 without (no LiDAR, or none yet).
@_cdecl("sc_ar_depth")
public func sc_ar_depth(_ bytes: UnsafeMutablePointer<UnsafeRawPointer?>, _ info: UnsafeMutablePointer<Int32>) -> Int32 {
    guard let b = bridge, b.running, b.lockedDepth == nil, let frame = b.session.currentFrame,
          let map = (frame.smoothedSceneDepth ?? frame.sceneDepth)?.depthMap,
          CVPixelBufferGetPixelFormatType(map) == kCVPixelFormatType_DepthFloat32,
          CVPixelBufferLockBaseAddress(map, .readOnly) == kCVReturnSuccess else { return 0 }
    b.lockedDepth = map
    info[0] = Int32(CVPixelBufferGetWidth(map))
    info[1] = Int32(CVPixelBufferGetHeight(map))
    info[2] = Int32(CVPixelBufferGetBytesPerRow(map))
    bytes.pointee = UnsafeRawPointer(CVPixelBufferGetBaseAddress(map))
    return 1
}

// MARK: The camera picture as Metal textures, without a copy

nonisolated(unsafe) private var textureCache: CVMetalTextureCache?

/// The app's Metal device (an `id<MTLDevice>`, not retained by the caller): camera pictures are
/// made into textures on it. Returns 1 when the texture cache was made.
@_cdecl("sc_ar_set_device")
public func sc_ar_set_device(_ device: UnsafeMutableRawPointer) -> Int32 {
    let mtl = Unmanaged<AnyObject>.fromOpaque(device).takeUnretainedValue()
    guard let mtl = mtl as? MTLDevice else { return 0 }
    var cache: CVMetalTextureCache?
    guard CVMetalTextureCacheCreate(nil, nil, mtl, nil, &cache) == kCVReturnSuccess, let cache else { return 0 }
    textureCache = cache
    return 1
}

/// The picture `sc_ar_frame` just gave as two Metal textures sharing its memory: luma (R8) and
/// chroma (RG8, half size). Writes each `id<MTLTexture>` (not retained: the caller retains it)
/// and a retained handle that keeps the picture's memory alive until `sc_ar_release` is called
/// with it, after the GPU is done. Returns 1 with both, 0 without (then nothing to release).
@_cdecl("sc_ar_metal_planes")
public func sc_ar_metal_planes(_ textures: UnsafeMutablePointer<UnsafeMutableRawPointer?>, _ keeps: UnsafeMutablePointer<UnsafeMutableRawPointer?>) -> Int32 {
    guard let cache = textureCache, let b = bridge, let pixels = b.locked ?? b.session.currentFrame?.capturedImage,
          CVPixelBufferGetPlaneCount(pixels) >= 2 else { return 0 }
    var made: [(MTLTexture, CVMetalTexture)] = []
    for (plane, format) in [(0, MTLPixelFormat.r8Unorm), (1, MTLPixelFormat.rg8Unorm)] {
        var cv: CVMetalTexture?
        let w = CVPixelBufferGetWidthOfPlane(pixels, plane)
        let h = CVPixelBufferGetHeightOfPlane(pixels, plane)
        guard CVMetalTextureCacheCreateTextureFromImage(nil, cache, pixels, nil, format, w, h, plane, &cv) == kCVReturnSuccess,
              let cv, let mtl = CVMetalTextureGetTexture(cv) else { return 0 }
        made.append((mtl, cv))
    }
    for (i, (mtl, cv)) in made.enumerated() {
        textures[i] = Unmanaged.passUnretained(mtl as AnyObject).toOpaque()
        keeps[i] = Unmanaged.passRetained(cv).toOpaque()
    }
    return 1
}

/// Lets go of a handle `sc_ar_metal_planes` gave (any thread: the GPU's completion may call it).
@_cdecl("sc_ar_release")
public func sc_ar_release(_ keep: UnsafeMutableRawPointer) {
    Unmanaged<CVMetalTexture>.fromOpaque(keep).release()
}

// MARK: Photos

/// Save a picture (`width` x `height`, 8-bit RGBA rows, opaque) to the photo library. iPadOS asks
/// once for permission to add photos. Returns 1 when it was handed over.
@_cdecl("sc_ar_save_photo")
public func sc_ar_save_photo(_ bytes: UnsafePointer<UInt8>, _ width: Int32, _ height: Int32) -> Int32 {
    let w = Int(width), h = Int(height)
    guard w > 0, h > 0 else { return 0 }
    let data = Data(bytes: bytes, count: w * h * 4)
    guard let provider = CGDataProvider(data: data as CFData),
          let image = CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                              space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
                              bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue),
                              provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent) else { return 0 }
    let photo = UIImage(cgImage: image)
    photoResult = nil
    PHPhotoLibrary.shared().performChanges({
        PHAssetChangeRequest.creationRequestForAsset(from: photo)
    }) { ok, error in
        DispatchQueue.main.async {
            photoResult = ok ? "Photo saved to Photos." : (error.map { "The photo was not saved: \($0.localizedDescription)" }
                ?? "The photo was not saved: SolidCraft may not add photos (Settings › Privacy › Photos).")
        }
    }
    return 1
}

nonisolated(unsafe) private var photoResult: String?

/// What became of the last photo (a string to free), once: null until the library has answered.
@_cdecl("sc_ar_photo_result")
public func sc_ar_photo_result() -> UnsafeMutablePointer<CChar>? {
    guard let r = photoResult else { return nil }
    photoResult = nil
    return strdup(r)
}

// MARK: Video

private final class PreviewDelegate: NSObject, RPPreviewViewControllerDelegate {
    func previewControllerDidFinish(_ previewController: RPPreviewViewController) {
        previewController.dismiss(animated: true)
    }
}

nonisolated(unsafe) private var previewDelegate = PreviewDelegate()
/// 0 not recording, 1 starting, 2 recording, 3 stopping.
nonisolated(unsafe) private var recordState: Int32 = 0
nonisolated(unsafe) private var recordMessage: String?

/// Start a screen video of the app (iPadOS asks once). The state and a message follow through
/// `sc_ar_record_state` and `sc_ar_record_message`.
@_cdecl("sc_ar_record_start")
public func sc_ar_record_start() {
    let r = RPScreenRecorder.shared()
    guard r.isAvailable, recordState == 0 else {
        recordMessage = "Screen recording is not available now."
        return
    }
    r.isMicrophoneEnabled = false
    recordState = 1
    r.startRecording { error in
        DispatchQueue.main.async {
            if let error {
                recordState = 0
                recordMessage = "Recording did not start: \(error.localizedDescription)"
            } else {
                recordState = 2
                recordMessage = "Recording."
            }
        }
    }
}

/// Stop the video and show Apple's preview, where it is trimmed, saved or shared.
@_cdecl("sc_ar_record_stop")
public func sc_ar_record_stop() {
    guard recordState == 2 else { return }
    recordState = 3
    RPScreenRecorder.shared().stopRecording { preview, error in
        DispatchQueue.main.async {
            recordState = 0
            if let preview, let root = bridge?.window()?.rootViewController {
                preview.previewControllerDelegate = previewDelegate
                preview.modalPresentationStyle = .fullScreen
                root.present(preview, animated: true)
                recordMessage = "Recording stopped: save or share it in the preview."
            } else {
                recordMessage = "The recording was lost: \(error?.localizedDescription ?? "no video")."
            }
        }
    }
}

@_cdecl("sc_ar_record_state")
public func sc_ar_record_state() -> Int32 { recordState }

/// A message about the recording (a string to free), once, or null.
@_cdecl("sc_ar_record_message")
public func sc_ar_record_message() -> UnsafeMutablePointer<CChar>? {
    guard let m = recordMessage else { return nil }
    recordMessage = nil
    return strdup(m)
}
