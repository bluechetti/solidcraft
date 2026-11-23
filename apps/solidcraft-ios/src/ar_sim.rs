//! A simulated ARKit for the iOS Simulator (docs/ar.md, "Testing without an iPad"): the same entry
//! points as `ARBridge.swift`, answered from a made-up room instead of the camera, so the AR view
//! runs, and its test steps (`ar-tap`, `ar-pick`, `ar-measure` …) can be checked, on the Mac with
//! no iPad. ARKit itself does not run in the Simulator.
//!
//! The room: a floor (ARKit's Y up, metres) with 0.5 m tiles and a back wall 3 m away, lit evenly.
//! The camera stands 1.3 m up, 0.8 m in front of the room's origin, looking 35° down, until the
//! test steps `ar-sim-move:right,up,back` (metres, along the camera's own axes) and
//! `ar-sim-look:yaw,pitch` (degrees) move it. The picture is drawn only when the camera moved or
//! the view changed size. A tap's ray finds the floor or the wall, as ARKit's planes would. The
//! depth map is exact. Photo writes the AR picture into Documents as `ar-photo-N.ppm`; screen
//! recording and saved room maps are not simulated and say so.

use std::ffi::{CString, c_char};
use std::sync::Mutex;

/// The camera's vertical field of view (radians), about an iPad's.
const FOV_Y: f64 = 0.95;
/// The back wall's place along Z (metres), facing +Z.
const WALL_Z: f64 = -3.0;
/// How many pixels across the picture and the depth map are.
const PICTURE_W: usize = 480;
const DEPTH_W: usize = 256;

struct Sim {
    running: bool,
    /// The camera: where it is, and its yaw (about Y, 0 looks along −Z) and pitch (radians).
    at: [f64; 3],
    yaw: f64,
    pitch: f64,
    /// Bumped when the camera moves; the picture last handed out was drawn at `drawn`.
    stamp: u64,
    drawn: Option<(u64, [usize; 2])>,
    luma: Vec<u8>,
    chroma: Vec<u8>,
    depth: Vec<f32>,
    depth_size: [usize; 2],
    anchor: Option<[f32; 16]>,
    /// What the next poll of each message says, once.
    photo_said: Option<String>,
    record_said: Option<String>,
    photos: u32,
}

impl Sim {
    fn new() -> Sim {
        Sim {
            running: false,
            at: [0.0, 1.3, 0.8],
            yaw: 0.0,
            pitch: (-35f64).to_radians(),
            stamp: 1,
            drawn: None,
            luma: Vec::new(),
            chroma: Vec::new(),
            depth: Vec::new(),
            depth_size: [0, 0],
            anchor: None,
            photo_said: None,
            record_said: None,
            photos: 0,
        }
    }

    /// The camera's axes in the room: right, up and back (ARKit's camera looks along −Z).
    fn axes(&self) -> [[f64; 3]; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let back = [sy * cp, -sp, cy * cp];
        let right = [cy, 0.0, -sy];
        let up = [back[1] * right[2] - back[2] * right[1], back[2] * right[0] - back[0] * right[2], back[0] * right[1] - back[1] * right[0]];
        [right, up, back]
    }

    /// The ray through view point (`x`, `y`, 0..1 across and down) of a view `aspect` wide per high.
    fn ray(&self, x: f64, y: f64, aspect: f64) -> [f64; 3] {
        let t = (FOV_Y / 2.0).tan();
        let (cx, cy) = ((2.0 * x - 1.0) * t * aspect, (1.0 - 2.0 * y) * t);
        let [r, u, b] = self.axes();
        let d = [0, 1, 2].map(|k| r[k] * cx + u[k] * cy - b[k]);
        let n = d.iter().map(|v| v * v).sum::<f64>().sqrt();
        d.map(|v| v / n)
    }
}

/// What a ray from `from` along unit `dir` meets first: how far, and whether it is the wall.
pub(crate) fn meet(from: [f64; 3], dir: [f64; 3]) -> Option<(f64, bool)> {
    let floor = (dir[1] < -1e-9).then(|| -from[1] / dir[1]).filter(|t| *t > 0.0);
    let wall = (dir[2] < -1e-9).then(|| (WALL_Z - from[2]) / dir[2]).filter(|t| *t > 0.0);
    match (floor, wall) {
        (Some(f), Some(w)) if w < f => Some((w, true)),
        (Some(f), _) => Some((f, false)),
        (None, Some(w)) => Some((w, true)),
        (None, None) => None,
    }
}

/// The room's brightness (luma, 0..255) where a ray meets it.
pub(crate) fn shade(p: [f64; 3], wall: bool) -> u8 {
    let line = |v: f64, step: f64| {
        let f = (v / step).rem_euclid(1.0);
        f < 0.012 || f > 0.988
    };
    if wall {
        if p[1] < 0.1 { 120 } else { 196 }
    } else if line(p[0], 0.5) || line(p[2], 0.5) {
        70
    } else if ((p[0] / 0.5).floor() + (p[2] / 0.5).floor()).rem_euclid(2.0) < 1.0 {
        150
    } else {
        128
    }
}

static SIM: Mutex<Option<Sim>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Sim) -> T) -> Option<T> {
    let mut guard = SIM.lock().ok()?;
    Some(f(guard.get_or_insert_with(Sim::new)))
}

/// A message for the app to take with `take_string` (it frees with `free`).
fn said(text: Option<String>) -> *mut c_char {
    match text.and_then(|t| CString::new(t).ok()) {
        // SAFETY: strdup copies the NUL-terminated string into memory the app frees with free().
        Some(c) => unsafe { libc::strdup(c.as_ptr()) },
        None => std::ptr::null_mut(),
    }
}

/// Write a 4 × 4 matrix, column-major, as ARKit's are.
fn write16(m: [[f64; 4]; 4], out: *mut f32) {
    if out.is_null() {
        return;
    }
    for c in 0..4 {
        for r in 0..4 {
            // SAFETY: the caller hands 16 floats.
            unsafe { *out.add(c * 4 + r) = m[c][r] as f32 };
        }
    }
}

extern "C" fn supported() -> i32 {
    1
}
extern "C" fn start() {
    with(|s| {
        s.running = true;
        s.stamp += 1;
    });
}
extern "C" fn stop() {
    with(|s| s.running = false);
}
extern "C" fn failure() -> *mut c_char {
    std::ptr::null_mut()
}
extern "C" fn take_restarted() -> i32 {
    0
}
extern "C" fn coaching() -> i32 {
    0
}

#[allow(clippy::too_many_arguments)]
extern "C" fn frame(
    width: f32,
    height: f32,
    view: *mut f32,
    proj: *mut f32,
    camera: *mut f32,
    display: *mut f32,
    info: *mut i32,
    planes: *mut *const u8,
) -> i32 {
    if !(width > 0.0 && height > 0.0) || info.is_null() || display.is_null() {
        return 0;
    }
    let mut guard = match SIM.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let s = guard.get_or_insert_with(Sim::new);
    if !s.running {
        return 0;
    }
    let aspect = f64::from(width) / f64::from(height);
    let [r, u, b] = s.axes();
    let pose = [[r[0], r[1], r[2], 0.0], [u[0], u[1], u[2], 0.0], [b[0], b[1], b[2], 0.0], [s.at[0], s.at[1], s.at[2], 1.0]];
    let dot = |a: [f64; 3], c: [f64; 3]| (0..3).map(|k| a[k] * c[k]).sum::<f64>();
    let inv = [[r[0], u[0], b[0], 0.0], [r[1], u[1], b[1], 0.0], [r[2], u[2], b[2], 0.0], [-dot(r, s.at), -dot(u, s.at), -dot(b, s.at), 1.0]];
    let f = 1.0 / (FOV_Y / 2.0).tan();
    let (near, far) = (0.03, 200.0);
    let p = [[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, far / (near - far), -1.0], [0.0, 0.0, far * near / (near - far), 0.0]];
    write16(inv, view);
    write16(p, proj);
    write16(pose, camera);
    for (i, v) in [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0].into_iter().enumerate() {
        // SAFETY: the caller hands 6 floats.
        unsafe { *display.add(i) = v };
    }
    // The picture has the view's shape, so the display transform is the identity.
    let w = PICTURE_W;
    let h = (((PICTURE_W as f64 / aspect) / 2.0).round() as usize * 2).max(2);
    let values = [2, 0, 0, w as i32, h as i32, w as i32, w as i32, 1000, 6500];
    for (i, v) in values.into_iter().enumerate() {
        // SAFETY: the caller hands 9 ints.
        unsafe { *info.add(i) = v };
    }
    if s.drawn == Some((s.stamp, [w, h])) {
        return 2;
    }
    s.luma.resize(w * h, 0);
    s.chroma.resize(w * h / 2, 128);
    for y in 0..h {
        for x in 0..w {
            let d = s.ray((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64, aspect);
            s.luma[y * w + x] = match meet(s.at, d) {
                Some((t, wall)) => shade([0, 1, 2].map(|k| s.at[k] + d[k] * t), wall),
                None => 220,
            };
        }
    }
    s.chroma.fill(128);
    let dw = DEPTH_W;
    let dh = (((DEPTH_W as f64 / aspect).round() as usize).max(1)).max(1);
    s.depth.resize(dw * dh, 0.0);
    for y in 0..dh {
        for x in 0..dw {
            let d = s.ray((x as f64 + 0.5) / dw as f64, (y as f64 + 0.5) / dh as f64, aspect);
            // Metres along the camera's axis, as ARKit's depth is.
            s.depth[y * dw + x] = meet(s.at, d).map_or(20.0, |(t, _)| (t * -dot(d, b)) as f32);
        }
    }
    s.depth_size = [dw, dh];
    s.drawn = Some((s.stamp, [w, h]));
    if !planes.is_null() {
        // SAFETY: the caller hands 2 pointers; the bytes stay until the next frame redraws them.
        unsafe {
            *planes = s.luma.as_ptr();
            *planes.add(1) = s.chroma.as_ptr();
        }
    }
    1
}
extern "C" fn frame_done() {}

extern "C" fn raycast(x: f32, y: f32, width: f32, height: f32, out: *mut f32) -> i32 {
    if !(width > 0.0 && height > 0.0) {
        return 0;
    }
    let hit = with(|s| {
        if !s.running {
            return None;
        }
        let d = s.ray(f64::from(x), f64::from(y), f64::from(width) / f64::from(height));
        meet(s.at, d).map(|(t, wall)| ([0, 1, 2].map(|k| s.at[k] + d[k] * t), wall))
    })
    .flatten();
    let Some((p, wall)) = hit else { return 0 };
    // The hit's pose: its Y is the surface's normal.
    let m = if wall {
        [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, -1.0, 0.0, 0.0], [p[0], p[1], p[2], 1.0]]
    } else {
        [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [p[0], p[1], p[2], 1.0]]
    };
    write16(m, out);
    1
}
extern "C" fn tick() {}
extern "C" fn thermal() -> i32 {
    0
}
extern "C" fn depth(bytes: *mut *const u8, info: *mut i32) -> i32 {
    if bytes.is_null() || info.is_null() {
        return 0;
    }
    let Ok(guard) = SIM.lock() else { return 0 };
    let Some(s) = guard.as_ref().filter(|s| s.running && !s.depth.is_empty()) else { return 0 };
    let [w, h] = s.depth_size;
    // SAFETY: the caller hands a pointer and 3 ints; the map stays until the next redraw.
    unsafe {
        *bytes = s.depth.as_ptr().cast();
        *info = w as i32;
        *info.add(1) = h as i32;
        *info.add(2) = (w * 4) as i32;
    }
    1
}
extern "C" fn set_device(_device: *mut std::ffi::c_void) -> i32 {
    0
}
extern "C" fn metal_planes(_textures: *mut *mut std::ffi::c_void, _keeps: *mut *mut std::ffi::c_void) -> i32 {
    0
}
extern "C" fn release(_p: *mut std::ffi::c_void) {}

/// The AR picture, RGBA, saved as `Documents/ar-photo-N.ppm` (the Simulator has no Photos to test).
extern "C" fn save_photo(rgba: *const u8, width: i32, height: i32) -> i32 {
    let (Ok(w), Ok(h)) = (usize::try_from(width), usize::try_from(height)) else { return 0 };
    if rgba.is_null() || w == 0 || h == 0 {
        return 0;
    }
    // SAFETY: the caller hands width × height RGBA pixels.
    let px = unsafe { std::slice::from_raw_parts(rgba, w * h * 4) };
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    ppm.extend(px.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]));
    let n = with(|s| {
        s.photos += 1;
        s.photos
    })
    .unwrap_or(0);
    let path = crate::files::documents().join(format!("ar-photo-{n}.ppm"));
    let result = std::fs::write(&path, ppm).map(|_| format!("Saved {}", path.display())).unwrap_or_else(|e| e.to_string());
    with(|s| s.photo_said = Some(result));
    1
}
extern "C" fn photo_result() -> *mut c_char {
    said(with(|s| s.photo_said.take()).flatten())
}
extern "C" fn record_start() {
    with(|s| s.record_said = Some("Screen recording is not simulated".into()));
}
extern "C" fn record_stop() {}
extern "C" fn record_state() -> i32 {
    0
}
extern "C" fn record_message() -> *mut c_char {
    said(with(|s| s.record_said.take()).flatten())
}
extern "C" fn save_map_and_stop(_path: *const c_char) {
    stop();
}
extern "C" fn start_with_map(_path: *const c_char) -> i32 {
    0
}
extern "C" fn map_message() -> *mut c_char {
    std::ptr::null_mut()
}
extern "C" fn set_anchor(m: *const f32) {
    if m.is_null() {
        return;
    }
    // SAFETY: the caller hands 16 floats.
    let a: [f32; 16] = std::array::from_fn(|i| unsafe { *m.add(i) });
    with(|s| s.anchor = Some(a));
}
extern "C" fn anchor(out: *mut f32) -> i32 {
    let Some(a) = with(|s| s.anchor).flatten() else { return 0 };
    if out.is_null() {
        return 0;
    }
    for (i, v) in a.into_iter().enumerate() {
        // SAFETY: the caller hands 16 floats.
        unsafe { *out.add(i) = v };
    }
    1
}

/// The simulated bridge, in place of `SolidCraftAR.framework`.
pub(super) fn bridge() -> super::Bridge {
    super::Bridge {
        supported,
        start,
        stop,
        failure,
        take_restarted,
        coaching,
        frame,
        frame_done,
        raycast,
        tick,
        thermal,
        depth,
        set_device,
        metal_planes,
        release,
        save_photo,
        photo_result,
        record_start,
        record_stop,
        record_state,
        record_message,
        save_map_and_stop,
        start_with_map,
        map_message,
        set_anchor,
        anchor,
    }
}

/// The test step `ar-sim-move:right,up,back`: the camera moves along its own axes (metres).
pub(crate) fn move_camera(by: [f64; 3]) {
    with(|s| {
        let [r, u, b] = s.axes();
        s.at = [0, 1, 2].map(|k| s.at[k] + r[k] * by[0] + u[k] * by[1] + b[k] * by[2]);
        s.stamp += 1;
    });
}

/// The test step `ar-sim-look:yaw,pitch`: the camera turns (degrees; pitch kept within ±89°).
pub(crate) fn turn_camera(yaw: f64, pitch: f64) {
    with(|s| {
        s.yaw += yaw.to_radians();
        s.pitch = (s.pitch + pitch.to_radians()).clamp(-89f64.to_radians(), 89f64.to_radians());
        s.stamp += 1;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_down_meets_the_floor_and_one_ahead_meets_the_wall() {
        let (t, wall) = meet([0.0, 1.0, 0.0], [0.0, -1.0, 0.0]).expect("floor");
        assert!((t - 1.0).abs() < 1e-12 && !wall);
        let (t, wall) = meet([0.0, 1.0, 0.0], [0.0, 0.0, -1.0]).expect("wall");
        assert!((t - 3.0).abs() < 1e-12 && wall);
        assert!(meet([0.0, 1.0, 0.0], [0.0, 1.0, 0.0]).is_none(), "up: nothing");
    }

    #[test]
    fn the_floor_has_tiles_and_lines() {
        assert_ne!(shade([0.25, 0.0, 0.25], false), shade([0.75, 0.0, 0.25], false));
        assert_eq!(shade([0.5, 0.0, 0.25], false), 70);
        assert_eq!(shade([0.0, 0.05, WALL_Z], true), 120);
    }

    #[test]
    fn the_cameras_axes_are_right_handed_and_it_looks_down_at_first() {
        let s = Sim::new();
        let [r, u, b] = s.axes();
        let cross = [r[1] * u[2] - r[2] * u[1], r[2] * u[0] - r[0] * u[2], r[0] * u[1] - r[1] * u[0]];
        assert!((0..3).all(|k| (cross[k] - b[k]).abs() < 1e-12), "right × up = back");
        let ahead = s.ray(0.5, 0.5, 1.0);
        assert!(ahead[1] < 0.0 && ahead[2] < 0.0, "looks down and ahead: {ahead:?}");
        assert!(meet(s.at, ahead).is_some_and(|(_, wall)| !wall), "the middle of the view is floor");
    }
}
