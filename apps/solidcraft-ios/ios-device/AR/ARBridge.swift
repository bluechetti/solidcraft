// ARKit for SolidCraft's AR view: the world-tracking session, each frame's camera picture and
// camera matrices as plain floats, raycasts onto surfaces, and Apple's coaching overlay.
// Loaded by the Rust app at run time (dlopen) and reached through the C functions at the bottom
// (`src/ar.rs`). Swift because ARKit's matrices are SIMD types that Rust bindings cannot pass.
// Everything here runs on the main thread (the app's UI thread), where ARKit's delegate is called.
import ARKit
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
/// - `info` (8 ints): tracking state (0 not available, 1 limited, 2 normal), the limited reason
///   (0 none, 1 initializing, 2 excessive motion, 3 insufficient features, 4 relocalizing),
///   interrupted, picture width, height, luma row bytes, chroma row bytes, light (lumens);
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
    let g = UIImpactFeedbackGenerator(style: .light)
    g.impactOccurred()
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
