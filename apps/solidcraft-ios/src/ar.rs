//! AR: the design in the room through the iPad's camera (ARKit).
//!
//! ARKit runs in a small Swift framework (`ios-device/AR/ARBridge.swift`, loaded at run time as
//! the AI's is), which hands over each frame's camera picture and camera matrices as plain
//! floats. The model is drawn by the design view's own renderer from ARKit's camera
//! (`Renderer::draw_second_view`, from VR pull request #18712), into a picture of its own with a
//! clear background, and laid over the camera picture by one full-screen pass. The design view
//! itself is drawn as ever under it, so leaving AR puts everything back as it was.
//!
//! What the user does: tap a surface to put the model there, upright and at real size (larger
//! than a room it is shrunk to fit), pinch to scale (it snaps back to real size with a tick),
//! twist two fingers to turn it. Done leaves AR. A session that cannot start (no camera access,
//! a device without world tracking) says why and leaves the app as it was.
//!
//! Calls into the framework go through C function pointers, so this module allows `unsafe`
//! (as `ai`, `picker` and `keyboard` do).
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char};

use eframe::{egui_wgpu, wgpu};
use solidcraft_ui_egui::SolveApp;
use solidcraft_ui_kit::container::{BackingTooltipExt, Container, Surface};
use solidcraft_ui_kit::{Button, button};
use solidcraft_viewport_gpu::view::{Renderer, ViewTarget};

type Mat4 = [[f64; 4]; 4];

/// The nearest and farthest the camera sees (metres).
const NEAR: f64 = 0.03;
const FAR: f64 = 200.0;
/// Metres per millimetre: the model at real size.
const REAL_SIZE: f64 = 0.001;
/// A model larger than this across (metres) is first put down shrunk to it.
const LARGEST_FIRST: f64 = 1.5;
/// Pinching within this share of real size snaps to it.
const SNAP: f64 = 0.06;

/// The framework's entry points.
struct Bridge {
    supported: extern "C" fn() -> i32,
    start: extern "C" fn(),
    stop: extern "C" fn(),
    failure: extern "C" fn() -> *mut c_char,
    take_restarted: extern "C" fn() -> i32,
    coaching: extern "C" fn() -> i32,
    #[allow(clippy::type_complexity)]
    frame: extern "C" fn(f32, f32, *mut f32, *mut f32, *mut f32, *mut f32, *mut i32, *mut *const u8) -> i32,
    frame_done: extern "C" fn(),
    raycast: extern "C" fn(f32, f32, f32, f32, *mut f32) -> i32,
    tick: extern "C" fn(),
    thermal: extern "C" fn() -> i32,
    depth: extern "C" fn(*mut *const u8, *mut i32) -> i32,
    set_device: extern "C" fn(*mut std::ffi::c_void) -> i32,
    metal_planes: extern "C" fn(*mut *mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32,
    release: extern "C" fn(*mut std::ffi::c_void),
    save_photo: extern "C" fn(*const u8, i32, i32) -> i32,
    photo_result: extern "C" fn() -> *mut c_char,
}

// The function types come from `Bridge`'s fields, which each transmute fills.
#[allow(clippy::missing_transmute_annotations)]
fn load() -> Result<Bridge, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = exe.parent().ok_or("no app folder")?.join("Frameworks/SolidCraftAR.framework/SolidCraftAR");
    let cpath = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: loading a framework of our own, which is signed into the app.
    let handle = unsafe { libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW) };
    if handle.is_null() {
        // SAFETY: `dlerror` returns a C string or null.
        let why = unsafe { libc::dlerror() };
        let why = if why.is_null() { String::new() } else { unsafe { CStr::from_ptr(why) }.to_string_lossy().into_owned() };
        return Err(format!("cannot load {}: {why}", path.display()));
    }
    let sym = |name: &CStr| -> Result<*mut libc::c_void, String> {
        // SAFETY: `handle` is a live library handle.
        let p = unsafe { libc::dlsym(handle, name.as_ptr()) };
        if p.is_null() { Err(format!("missing {}", name.to_string_lossy())) } else { Ok(p) }
    };
    // SAFETY: the symbols are the `@_cdecl` functions of ARBridge.swift, with exactly these signatures.
    unsafe {
        Ok(Bridge {
            supported: std::mem::transmute(sym(c"sc_ar_supported")?),
            start: std::mem::transmute(sym(c"sc_ar_start")?),
            stop: std::mem::transmute(sym(c"sc_ar_stop")?),
            failure: std::mem::transmute(sym(c"sc_ar_failure")?),
            take_restarted: std::mem::transmute(sym(c"sc_ar_take_restarted")?),
            coaching: std::mem::transmute(sym(c"sc_ar_coaching")?),
            frame: std::mem::transmute(sym(c"sc_ar_frame")?),
            frame_done: std::mem::transmute(sym(c"sc_ar_frame_done")?),
            raycast: std::mem::transmute(sym(c"sc_ar_raycast")?),
            tick: std::mem::transmute(sym(c"sc_ar_tick")?),
            thermal: std::mem::transmute(sym(c"sc_ar_thermal")?),
            depth: std::mem::transmute(sym(c"sc_ar_depth")?),
            set_device: std::mem::transmute(sym(c"sc_ar_set_device")?),
            metal_planes: std::mem::transmute(sym(c"sc_ar_metal_planes")?),
            release: std::mem::transmute(sym(c"sc_ar_release")?),
            save_photo: std::mem::transmute(sym(c"sc_ar_save_photo")?),
            photo_result: std::mem::transmute(sym(c"sc_ar_photo_result")?),
        })
    }
}

/// A string the framework made with `strdup`, taken and freed.
fn take_string(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string from `strdup`, freed once here.
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { libc::free(p.cast()) };
    Some(s)
}

/// One ARKit frame as the app uses it.
struct Seen {
    /// Room (ARKit's world, metres, Y up) to clip space, the renderer's depth convention.
    view_proj: Mat4,
    /// The camera's position and its back direction (toward the user) in the room.
    eye: [f64; 3],
    back: [f64; 3],
    /// View point (0..1) to camera picture point: a, b, c, d, tx, ty.
    display: [f32; 6],
    tracking: i32,
    reason: i32,
    /// What the room's light does to the model's colours (red, green, blue gains).
    light: [f32; 3],
}

/// The model in the room: room point = translation + scale · axes · model point.
#[derive(Clone, Copy, Debug)]
struct Placed {
    axes: [[f64; 3]; 3],
    scale: f64,
    translation: [f64; 3],
    /// The model point that sits on the surface (the middle of its underside).
    foot: [f64; 3],
    /// The model's box (least and greatest corner, model units), for its shadow.
    bounds: [[f64; 3]; 2],
}

/// A face being pushed or pulled with the Pencil.
#[derive(Clone, Debug, PartialEq)]
struct Pull {
    body: String,
    /// Where the Pencil landed on the face, and its outward normal (model space).
    point: [f64; 3],
    normal: [f64; 3],
    /// Where the Pencil landed on the screen.
    start: egui::Pos2,
    /// How far the face goes (model units, positive outward), in half-millimetre steps.
    distance: f64,
    /// The distance when this stroke began (a pull waiting for Apply is adjusted from it).
    base: f64,
    /// The Pencil is on it; once lifted the pull waits for Apply or Cancel.
    held: bool,
}

/// A section cut through the model in AR: a plane in the model's own coordinates (so it moves
/// with the model), hiding what lies on its normal's side, as the design view's planes do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cut {
    /// Unit, model space.
    normal: [f64; 3],
    /// The plane is normal · p = d (model units).
    d: f64,
}

/// A soft contact shadow on the floor under the placed model: an ellipse on the room's
/// horizontal plane through the model's foot, along the model's footprint.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shadow {
    /// The ellipse's middle (room, metres); its height is the floor's.
    middle: [f64; 3],
    /// Its two horizontal half-axes (room, metres): direction times radius.
    axes: [[f64; 2]; 2],
}

impl Placed {
    /// A room direction in the model's axes (unit stays unit: the axes are orthonormal).
    fn direction_to_model(&self, v: [f64; 3]) -> [f64; 3] {
        self.axes.map(|axis| dot(axis, v))
    }

    /// A room point in the model's space.
    fn point_to_model(&self, p: [f64; 3]) -> [f64; 3] {
        self.direction_to_model(sub(p, self.translation)).map(|x| x / self.scale)
    }

    /// A cut through the model's middle, square to the floor and facing `toward` (a room
    /// direction, the camera's back): the half nearer the camera is hidden, to look inside.
    fn cut_facing(&self, toward: [f64; 3]) -> Option<Cut> {
        let n_room = unit([toward[0], 0.0, toward[2]])?;
        let normal = self.direction_to_model(n_room);
        let [lo, hi] = self.bounds;
        let middle = scale3(add(lo, hi), 0.5);
        Some(Cut { normal, d: dot(normal, middle) })
    }

    /// The contact shadow: the model's footprint (its box seen from above) a little enlarged.
    fn contact_shadow(&self) -> Option<Shadow> {
        let [lo, hi] = self.bounds;
        let corners: Vec<[f64; 3]> = (0..8)
            .map(|i| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }])
            .map(|c| add(self.translation, scale3(turn(&self.axes, c), self.scale)))
            .collect();
        // The model's most level axis gives the footprint's first direction.
        let level = self.axes.iter().copied().min_by(|a, b| a[1].abs().total_cmp(&b[1].abs()))?;
        let d1 = unit([level[0], 0.0, level[2]])?;
        let d2 = [-d1[2], 0.0, d1[0]];
        let foot = self.anchor();
        let span = |d: [f64; 3]| {
            let v: Vec<f64> = corners.iter().map(|c| dot(sub(*c, foot), d)).collect();
            let (a, b) = (v.iter().copied().fold(f64::INFINITY, f64::min), v.iter().copied().fold(f64::NEG_INFINITY, f64::max));
            ((a + b) * 0.5, (b - a) * 0.5)
        };
        let ((m1, r1), (m2, r2)) = (span(d1), span(d2));
        let middle = add(foot, add(scale3(d1, m1), scale3(d2, m2)));
        // Past the edge by a share of the model's size, as a soft shadow under a lamp.
        let grow = |r: f64| r * 1.12 + 0.08 * r1.max(r2);
        (r1 > 0.0 && r2 > 0.0).then_some(Shadow { middle, axes: [[d1[0] * grow(r1), d1[2] * grow(r1)], [d2[0] * grow(r2), d2[2] * grow(r2)]] })
    }

    fn matrix(&self) -> Mat4 {
        let [x, y, z] = self.axes.map(|a| a.map(|v| v * self.scale));
        let t = self.translation;
        [[x[0], x[1], x[2], 0.0], [y[0], y[1], y[2], 0.0], [z[0], z[1], z[2], 0.0], [t[0], t[1], t[2], 1.0]]
    }

    /// Where model point `q` is in the room.
    fn apply_room(&self, q: [f64; 3]) -> [f64; 3] {
        add(self.translation, scale3(turn(&self.axes, q), self.scale))
    }

    /// Where the foot is in the room: what stays put while the model is scaled or turned.
    fn anchor(&self) -> [f64; 3] {
        add(self.translation, scale3(turn(&self.axes, self.foot), self.scale))
    }

    /// The same model with its foot at `at`.
    fn with_anchor(mut self, at: [f64; 3]) -> Placed {
        self.translation = sub(at, scale3(turn(&self.axes, self.foot), self.scale));
        self
    }
}

/// The GPU side, kept in egui's callback resources so the paint callback can draw it.
struct Gpu {
    pipeline: wgpu::RenderPipeline,
    /// The same pass for a photo (see [`photo`]).
    capture: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    luma: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    chroma: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    blank: wgpu::TextureView,
    bind: Option<wgpu::BindGroup>,
    /// The model picture's sample count the pipeline reads its depth with.
    samples: u32,
    /// A model depth with nothing drawn (for frames without a model), at `samples`.
    blank_depth: wgpu::TextureView,
    /// The room's depth from LiDAR (metres, 32-bit float), and one far away for frames without.
    room_depth: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    far_room: wgpu::TextureView,
}

/// The AR view's state, owned by the app.
#[derive(Default)]
pub struct State {
    bridge: Option<Result<Bridge, String>>,
    /// AR is showing.
    pub on: bool,
    placed: Option<Placed>,
    /// The model's picture from ARKit's camera.
    target: Option<ViewTarget>,
    /// The last design view the window drew (kept while it draws none).
    source: Option<solidcraft_ui_egui::stereo::Source>,
    /// What the status line says, when it says something of its own.
    message: String,
    /// A pinch in progress: the scale it started from, and whether it is at real size now.
    pinch: Option<(f64, bool)>,
    /// The app's own touch sorting (long press = menu, one finger = orbit), off while AR has
    /// the screen and put back as it was.
    tablet_was: bool,
    /// A tap asked for by the test hook (0..1 across and down the screen), done on the next frame.
    pub test_tap: Option<(f32, f32)>,
    /// A Pencil tap asked for by the test hook, done on the next frame.
    pub test_pick: Option<(f32, f32)>,
    /// The test hook turns Section on (`ar-section`) or Measure on (`ar-measure`) next frame.
    pub test_section: bool,
    pub test_measure: bool,
    /// The test hook presses Explode (`ar-explode`) next frame.
    pub test_explode: bool,
    /// The test hook opens Joints (`ar-joints`) or presses Check (`ar-check`) next frame.
    pub test_joints: bool,
    pub test_check: bool,
    /// A Pencil pull asked for by the test hook: from and to (0..1 across and down the screen).
    pub test_pull: Option<[f32; 4]>,
    /// The Pencil touch in progress: its id, where and when it began.
    pencil: Option<(egui::TouchId, egui::Pos2, f64)>,
    /// When the Pencil last left the glass (a finger's tap just after is the same hand resting).
    pencil_up_at: f64,
    /// The part the Pencil selected last, named on the panel.
    selected: Option<String>,
    /// A one-finger drag in progress: from where the finger first hit a surface to the foot.
    drag: Option<[f64; 3]>,
    /// The device's thermal state as last seen (0 nominal .. 3 critical).
    thermal: i32,
    /// LiDAR depth has arrived since AR started: the model is hidden behind real things.
    has_depth: bool,
    /// The camera picture is shared with the GPU without a copy (the app's Metal device was
    /// given to ARKit's texture cache); off, its bytes are copied each frame.
    zero_copy: bool,
    /// The Metal device was offered to the texture cache (once).
    device_offered: bool,
    /// The section cut while Section is on.
    cut: Option<Cut>,
    /// The points measured while Measure is on (model units; at most two).
    measuring: Option<Vec<[f64; 3]>>,
    /// The exploded view AR shows (0: assembled, k: the design's k-th), set by `explode.show`.
    exploded: usize,
    /// Pull is on: the Pencil pushes and pulls faces (`solid.press_pull`).
    pull_mode: bool,
    /// The Joints list is open, with the joints as last listed (name; each coordinate's value
    /// and unit).
    joints: Option<Vec<(String, Vec<(f64, String)>)>>,
    /// Overlapping parts from the last Check: the two bodies and the overlap's middle (model
    /// units), marked in the room until the model changes.
    overlaps: Vec<(String, String, [f64; 3])>,
    /// Photo was asked for: taken after this frame's AR pass is set up.
    pub take_photo: bool,
    /// Where the AR panel was last drawn (screen points).
    panel: Option<egui::Rect>,
    /// The face being pulled.
    pulling: Option<Pull>,
    /// Whether the design hid its origin before AR, put back after.
    origin_was: Option<Option<bool>>,
    /// Milliseconds spent on the CPU (camera upload; camera and model), summed over the frames
    /// since the last log line.
    timing: [f64; 2],
    /// The picture's camera to clip space for the last frame, for nothing but the log.
    frames: u64,
}

impl State {
    fn bridge(&mut self) -> Result<&Bridge, String> {
        match self.bridge.get_or_insert_with(load) {
            Ok(b) => Ok(b),
            Err(e) => Err(e.clone()),
        }
    }

    /// Turn AR on (or off). Returns what went wrong when it cannot start.
    pub fn set_on(&mut self, app: &mut SolveApp, on: bool) -> Result<(), String> {
        if on == self.on {
            return Ok(());
        }
        if on {
            let b = self.bridge()?;
            if (b.supported)() == 0 {
                return Err("AR needs world tracking, which this device does not have.".into());
            }
            (b.start)();
            self.placed = None;
            self.cut = None;
            self.measuring = None;
            self.message.clear();
            self.has_depth = false;
            self.on = true;
            app.viewport.stereo.on = true;
            self.tablet_was = app.tablet.enabled;
            app.tablet.enabled = false;
            // Only the model in the room: the origin's marker stays in the design view.
            self.origin_was = Some(app.session.visibility.origin_hidden);
            app.session.visibility.origin_hidden = Some(true);
        } else {
            if let Ok(b) = self.bridge() {
                (b.stop)();
            }
            self.on = false;
            self.placed = None;
            self.target = None;
            self.source = None;
            self.pinch = None;
            app.viewport.stereo.on = false;
            app.tablet.enabled = self.tablet_was;
            if self.exploded > 0 {
                let _ = app.run("explode.show", serde_json::json!({ "view": "none" }));
                self.exploded = 0;
            }
            if let Some(origin) = self.origin_was.take() {
                app.session.visibility.origin_hidden = origin;
            }
        }
        Ok(())
    }

    /// The AR view for this frame, over everything else: the camera, the model, the controls.
    /// Call after the app's UI was drawn (the design view leaves its frame for it then).
    pub fn show(&mut self, ctx: &egui::Context, frame: &eframe::Frame, app: &mut SolveApp) {
        if !self.on {
            return;
        }
        let Some(rs) = frame.wgpu_render_state() else {
            let _ = self.set_on(app, false);
            return;
        };
        if let Some(why) = self.bridge().ok().and_then(|b| take_string((b.failure)())) {
            let _ = self.set_on(app, false);
            app.set_status(why, true);
            return;
        }
        if let Some(source) = app.viewport.stereo.take() {
            self.source = Some(source);
        }
        if self.bridge().map(|b| (b.take_restarted)() == 1).unwrap_or(false) {
            // Tracking started again from scratch: the room moved under the model.
            self.placed = None;
        }
        let screen = ctx.content_rect().union(ctx.viewport_rect());
        let screen = ctx.viewport_rect().intersect(screen);
        // A hot device draws the model at fewer pixels (the camera picture is ARKit's either way);
        // at critical iPadOS is about to slow everything down, so AR stops and says why.
        let thermal = self.bridge().map(|b| (b.thermal)()).unwrap_or(0);
        if thermal >= 3 {
            let _ = self.set_on(app, false);
            app.set_status("AR stopped: the iPad is too hot. Let it cool down and try again.", true);
            return;
        }
        if thermal != self.thermal {
            crate::note(&format!("ar: thermal state {thermal}"));
            self.thermal = thermal;
        }
        let detail = if thermal >= 2 { 0.6 } else { 1.0 };
        let ppp = ctx.pixels_per_point() * detail;
        let size_px = [(screen.width() * ppp).round().max(1.0) as u32, (screen.height() * ppp).round().max(1.0) as u32];
        ensure_gpu(rs);
        if !self.device_offered {
            self.device_offered = true;
            self.zero_copy = self.bridge().ok().is_some_and(|b| offer_device(rs, b));
            crate::note(&format!("ar: camera picture {}", if self.zero_copy { "shared with the GPU" } else { "copied" }));
        }
        let started = std::time::Instant::now();
        let seen = self.next_frame(rs, [screen.width(), screen.height()]);
        let camera_ms = started.elapsed().as_secs_f64() * 1e3;
        // The model from ARKit's camera.
        let mut model_drawn = false;
        // Not while ARKit is still finding the room it knew (after an interruption): until it
        // has, the room's origin is not where the model was put.
        let relocalizing = seen.as_ref().is_some_and(|s| s.tracking == 1 && s.reason == 4);
        if !relocalizing && let (Some(seen), Some(placed), Some(source)) = (seen.as_ref(), self.placed, self.source.as_ref()) {
            model_drawn = draw_model(rs, source, seen, &placed, self.cut, size_px, &mut self.target);
        }
        let total_ms = started.elapsed().as_secs_f64() * 1e3;
        self.timing = [self.timing[0] + camera_ms, self.timing[1] + total_ms];
        if self.frames % 300 == 299 {
            crate::note(&format!(
                "ar: per frame {:.2} ms camera, {:.2} ms camera + model ({} px), model {}, light {:?}",
                self.timing[0] / 300.0,
                self.timing[1] / 300.0,
                size_px[0] * size_px[1],
                model_drawn,
                seen.as_ref().map(|s| s.light)
            ));
            self.timing = [0.0; 2];
        }
        if let Some(seen) = seen.as_ref() {
            write_uniform(rs, seen, model_drawn, self.placed.and_then(|p| p.contact_shadow()), self.has_depth);
        }
        self.bind(rs, model_drawn);
        if std::mem::take(&mut self.take_photo) {
            self.save_photo(rs, size_px);
        }
        // The photo library's answer, when it comes.
        if let Some(said) = self.bridge().ok().and_then(|b| take_string((b.photo_result)())) {
            crate::note(&format!("ar: {said}"));
            self.message = said;
        }

        // The whole screen: the picture, and the touches (they go to AR, not the app below).
        let coaching = self.bridge().map(|b| (b.coaching)() == 1).unwrap_or(false);
        // The whole screen, a canvas of the UI kit's (the one outer container), over the app.
        Container::new(ctx, Surface::Canvas)
            .area(egui::Id::unique("ar-view"))
            .order(egui::Order::Foreground)
            .constrain(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                // Under the status bar and the home indicator too: egui clips to the safe area.
                let mut painter = ui.painter().clone();
                painter.set_clip_rect(screen);
                painter.add(egui_wgpu::Callback::new_paint_callback(rect, Paint));
                if !coaching && let Some(seen) = seen.as_ref() {
                    if std::mem::take(&mut self.test_section) {
                        self.cut = self.placed.and_then(|p| p.cut_facing(seen.back));
                    }
                    if std::mem::take(&mut self.test_joints) {
                    self.joints = Some(Self::list_joints(app));
                    crate::note(&format!("ar: joints {:?}", self.joints));
                }
                if std::mem::take(&mut self.test_check) {
                    self.check_overlaps(app);
                }
                if std::mem::take(&mut self.test_explode) {
                    self.next_exploded_view(app);
                }
                if std::mem::take(&mut self.test_measure) {
                        self.measuring = Some(Vec::new());
                    }
                    self.gestures(ui, &response, rect, seen, app);
                    self.draw_measure(ui.painter(), rect, seen);
                self.draw_pull(ui.painter(), rect, seen);
                self.draw_overlaps(ui.painter(), rect, seen);
                }
            });
        self.controls(ctx, screen, seen.as_ref(), coaching, app);
        self.frames += 1;
        // ARKit's camera gives 60 pictures a second; the screen could go to 120.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    /// Ask ARKit for its frame, upload a new camera picture, and work out the camera.
    fn next_frame(&mut self, rs: &egui_wgpu::RenderState, view_pt: [f32; 2]) -> Option<Seen> {
        let b = self.bridge().ok()?;
        let (frame_fn, done, depth_fn) = (b.frame, b.frame_done, b.depth);
        let mut view = [0f32; 16];
        let mut proj = [0f32; 16];
        let mut camera = [0f32; 16];
        let mut display = [0f32; 6];
        let mut info = [0i32; 9];
        let mut planes: [*const u8; 2] = [std::ptr::null(); 2];
        let got = frame_fn(
            view_pt[0],
            view_pt[1],
            view.as_mut_ptr(),
            proj.as_mut_ptr(),
            camera.as_mut_ptr(),
            display.as_mut_ptr(),
            info.as_mut_ptr(),
            planes.as_mut_ptr(),
        );
        if got == 0 {
            return None;
        }
        if got == 1 {
            let (w, h, luma_row, chroma_row) = (info[3].max(0) as u32, info[4].max(0) as u32, info[5].max(0) as u32, info[6].max(0) as u32);
            // The picture's own memory as textures (no copy), else a copy of its bytes.
            let shared = self.zero_copy && planes_without_copy(rs, self.bridge.as_ref().and_then(|b| b.as_ref().ok()), [w, h]);
            if !shared && !planes[0].is_null() && !planes[1].is_null() && w > 1 && h > 1 {
                // SAFETY: ARKit's locked pixel buffer: plane 0 is `h` rows of `luma_row` bytes,
                // plane 1 `h / 2` rows of `chroma_row`, readable until `frame_done` below.
                let (luma, chroma) = unsafe {
                    (
                        std::slice::from_raw_parts(planes[0], (luma_row * h) as usize),
                        std::slice::from_raw_parts(planes[1], (chroma_row * (h / 2)) as usize),
                    )
                };
                upload(rs, [w, h], luma_row, luma, chroma_row, chroma);
            }
            let mut depth_bytes: *const u8 = std::ptr::null();
            let mut depth_info = [0i32; 3];
            if (depth_fn)(&mut depth_bytes, depth_info.as_mut_ptr()) == 1 && !depth_bytes.is_null() {
                let [dw, dh, row] = depth_info.map(|v| v.max(0) as u32);
                if dw > 0 && dh > 0 && row >= dw * 4 {
                    // SAFETY: ARKit's locked depth map: `dh` rows of `row` bytes, readable until
                    // `frame_done` below.
                    let bytes = unsafe { std::slice::from_raw_parts(depth_bytes, (row * dh) as usize) };
                    upload_depth(rs, [dw, dh], row, bytes);
                    if !self.has_depth {
                        crate::note(&format!("ar: LiDAR depth {dw} x {dh}"));
                    }
                    self.has_depth = true;
                }
            }
            done();
        }
        let f = |a: &[f32; 16]| -> Mat4 { std::array::from_fn(|c| std::array::from_fn(|r| f64::from(a[c * 4 + r]))) };
        let (view, mut proj, camera) = (f(&view), f(&proj), f(&camera));
        // ARKit's own depth row out, the renderer's in: depth 0 at NEAR, 1 at FAR (it turns it
        // round itself for its reversed depth buffer).
        proj[0][2] = 0.0;
        proj[1][2] = 0.0;
        proj[2][2] = FAR / (NEAR - FAR);
        proj[3][2] = NEAR * FAR / (NEAR - FAR);
        let view_proj = mul(&proj, &view);
        let all_finite = view_proj.iter().flatten().chain(camera.iter().flatten()).all(|v| v.is_finite());
        all_finite.then(|| Seen {
            view_proj,
            eye: [camera[3][0], camera[3][1], camera[3][2]],
            back: [camera[2][0], camera[2][1], camera[2][2]],
            display,
            tracking: info[0],
            reason: info[1],
            light: light_gain(info[7], info[8]),
        })
    }
}

/// The placed model as ARKit's camera sees it, into `target`. `false` when nothing was drawn.
fn draw_model(
    rs: &egui_wgpu::RenderState,
    source: &solidcraft_ui_egui::stereo::Source,
    seen: &Seen,
    placed: &Placed,
    cut: Option<Cut>,
    size: [u32; 2],
    target: &mut Option<ViewTarget>,
) -> bool {
    {
        let m = placed.matrix();
        let view_proj = mul(&seen.view_proj, &m);
        // The camera in the model's space, and the direction toward it (lighting, glass order).
        let s2 = placed.scale * placed.scale;
        let offset = sub(seen.eye, placed.translation);
        let eye = [0, 1, 2].map(|k| dot(offset, [m[k][0], m[k][1], m[k][2]]) / s2);
        let back = unit([0, 1, 2].map(|k| dot(seen.back, placed.axes[k]))).unwrap_or([0.0, 0.0, 1.0]);
        if !view_proj.iter().flatten().all(|v| v.is_finite()) || !eye.iter().all(|v| v.is_finite()) {
            return false;
        }
        let mut frame = source.frame.clone();
        frame.view_proj = to_f32(&view_proj);
        frame.turned_depth = Some(view_proj.map(|c| (c[3] - c[2]) as f32));
        frame.eye = [eye[0] as f32, eye[1] as f32, eye[2] as f32, 1.0];
        frame.back = [back[0] as f32, back[1] as f32, back[2] as f32];
        frame.size_px = [size[0] as f32, size[1] as f32];
        // AR's own cut, for the view only (the design's section is its own, and each change of
        // it an undo step): it takes the first plane, with the inside drawn in the body's colour.
        if let Some(c) = cut {
            frame.clip = [[0.0; 4]; solidcraft_viewport_gpu::view::MAX_SECTION_PLANES];
            frame.clip[0] = [c.normal[0] as f32, c.normal[1] as f32, c.normal[2] as f32, c.d as f32];
            frame.cap = [0.0; 4];
        }
        let mut renderer = rs.renderer.write();
        let Some(view) = renderer.callback_resources.get_mut::<Renderer>() else { return false };
        // Without the design's grid and ground shadow: the room has its own floor, and the AR
        // pass puts a contact shadow on it (`Shadow`).
        view.draw_second_view(&rs.device, &rs.queue, &frame, size, [0, 0, size[0], size[1]], false, target)
    }
}

impl State {
    fn bind(&mut self, rs: &egui_wgpu::RenderState, model: bool) {
        let mut renderer = rs.renderer.write();
        let picture = self.target.as_ref().filter(|_| model).map(ViewTarget::picture).cloned();
        // The model's depth, as its own depth-only view (the renderer's buffer may carry a
        // stencil too), only when its sample count is the one the pipeline reads.
        let samples = renderer.callback_resources.get::<Gpu>().map(|g| g.samples);
        let depth = self
            .target
            .as_ref()
            .filter(|t| model && Some(t.samples()) == samples)
            .and_then(ViewTarget::depth)
            .map(|v| v.texture().create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() }));
        let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() else { return };
        let (Some((_, luma, _)), Some((_, chroma, _))) = (&gpu.luma, &gpu.chroma) else {
            gpu.bind = None;
            return;
        };
        let picture = picture.as_ref().unwrap_or(&gpu.blank);
        gpu.bind = Some(rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sc_ar"),
            layout: &gpu.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(luma) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(chroma) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(picture) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: gpu.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(depth.as_ref().unwrap_or(&gpu.blank_depth)) },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(gpu.room_depth.as_ref().map_or(&gpu.far_room, |r| &r.1)),
                },
            ],
        }));
    }

    /// Tap: put the model on the surface there. Pinch: scale about its foot, snapping to real
    /// size. Twist: turn it about the vertical.
    fn gestures(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, seen: &Seen, app: &mut SolveApp) {
        // The Pencil selects (as in the design view) and fingers place and move: a touch with a
        // force reading is the Pencil's. Its touches never count as a finger's tap or drag.
        let mut pencil_tap = self.test_pick.take();
        // With Pull on, the Pencil grabs a face where it lands, and lets it go where it lifts.
        let mut pull_start: Option<egui::Pos2> = None;
        let mut pull_at: Option<egui::Pos2> = None;
        let mut pull_end = false;
        let pull_mode = self.pull_mode;
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::Touch { id, phase, pos, force, .. } = e {
                    let now = i.time;
                    // A touch that starts on the panel belongs to its buttons.
                    if matches!(phase, egui::TouchPhase::Start) && self.panel.is_some_and(|r| r.expand(8.0).contains(*pos)) {
                        continue;
                    }
                    let mine = self.pencil.is_some_and(|p| p.0 == *id);
                    if pull_mode {
                        match phase {
                            egui::TouchPhase::Start if force.is_some() => pull_start = Some(*pos),
                            egui::TouchPhase::Move if mine => pull_at = Some(*pos),
                            egui::TouchPhase::End if mine => {
                                pull_at = Some(*pos);
                                pull_end = true;
                            }
                            egui::TouchPhase::Cancel if mine => self.pulling = None,
                            _ => {}
                        }
                    }
                    match phase {
                        egui::TouchPhase::Start if force.is_some() => self.pencil = Some((*id, *pos, now)),
                        egui::TouchPhase::End if self.pencil.is_some_and(|p| p.0 == *id) => {
                            if let Some((_, start, at)) = self.pencil.take()
                                && !pull_mode
                                && start.distance(*pos) < 12.0
                                && now - at < 0.5
                            {
                                pencil_tap = Some(((pos.x - rect.min.x) / rect.width(), (pos.y - rect.min.y) / rect.height()));
                            }
                            self.pencil_up_at = now;
                        }
                        egui::TouchPhase::Cancel if self.pencil.is_some_and(|p| p.0 == *id) => self.pencil = None,
                        _ => {}
                    }
                }
            }
        });
        if let Some(u) = pencil_tap {
            self.select_at(app, seen, u);
        }
        if let Some(at) = pull_start {
            self.start_pull(app, seen, rect, at);
        }
        if let Some(at) = pull_at {
            self.drag_pull(seen, rect, at);
        }
        if pull_end {
            self.lift_pull();
        }
        if let Some([x0, y0, x1, y1]) = self.test_pull.take() {
            let at = |x: f32, y: f32| egui::pos2(rect.min.x + x * rect.width(), rect.min.y + y * rect.height());
            self.start_pull(app, seen, rect, at(x0, y0));
            self.drag_pull(seen, rect, at(x1, y1));
            self.lift_pull();
            self.apply_pull(app);
        }
        let pencil_busy = self.pencil.is_some() || ui.input(|i| i.time) - self.pencil_up_at < 0.3;
        if pencil_busy {
            return;
        }
        let tapped = response
            .clicked()
            .then(|| response.interact_pointer_pos())
            .flatten()
            .map(|at| ((at.x - rect.min.x) / rect.width(), (at.y - rect.min.y) / rect.height()));
        if let Some(u) = tapped.or_else(|| self.test_tap.take()) {
            let mut hit = [0f32; 16];
            let found = self.bridge().map(|b| (b.raycast)(u.0, u.1, rect.width(), rect.height(), hit.as_mut_ptr()) == 1).unwrap_or(false);
            if found {
                let point = [f64::from(hit[12]), f64::from(hit[13]), f64::from(hit[14])];
                match self.placed {
                    // Already down: it moves there, as it is.
                    Some(p) => self.placed = Some(p.with_anchor(point)),
                    None => {
                        self.placed = first_placement(app, self.source.as_ref(), seen, point);
                        if self.placed.is_none() {
                            self.message = "Open a design with a model to place it.".into();
                        }
                    }
                }
                if self.placed.is_some() {
                    self.message.clear();
                    if let Ok(b) = self.bridge() {
                        (b.tick)();
                    }
                }
            } else {
                self.message = "No surface there yet: move the iPad slowly over it.".into();
            }
            crate::note(&format!("ar: tap at {u:?}: {}", if self.placed.is_some() { "placed" } else { "no surface" }));
        }
        let touch = ui.ctx().multi_touch();
        let Some(mut placed) = self.placed else { return };
        // With Section on, one finger up and down moves the cut: up takes it away from you,
        // into the model. A point of drag is about a point's width at the model's distance.
        if let Some(cut) = self.cut.as_mut()
            && touch.is_none()
            && response.dragged()
        {
            let distance = sub(placed.anchor(), seen.eye);
            let metres_per_point = dot(distance, distance).sqrt() / f64::from(rect.height().max(1.0));
            let up = -f64::from(response.drag_delta().y);
            cut.d -= up * metres_per_point / placed.scale;
            return;
        }
        // One finger dragged: the model slides along the surfaces under it, keeping how far its
        // foot was from the finger's first hit, so it does not jump to the finger.
        if touch.is_none()
            && response.dragged()
            && let Some(at) = response.interact_pointer_pos()
        {
            let u = ((at.x - rect.min.x) / rect.width(), (at.y - rect.min.y) / rect.height());
            let mut hit = [0f32; 16];
            if self.bridge().map(|b| (b.raycast)(u.0, u.1, rect.width(), rect.height(), hit.as_mut_ptr()) == 1).unwrap_or(false) {
                let point = [f64::from(hit[12]), f64::from(hit[13]), f64::from(hit[14])];
                let offset = *self.drag.get_or_insert(sub(placed.anchor(), point));
                self.placed = Some(placed.with_anchor(add(point, offset)));
            }
            return;
        }
        if !response.dragged() {
            self.drag = None;
        }
        match touch {
            Some(t) if t.num_touches >= 2 => {
                let anchor = placed.anchor();
                let (start, at_real) = *self.pinch.get_or_insert((placed.scale, (placed.scale / REAL_SIZE - 1.0).abs() < 1e-9));
                let _ = start;
                let mut s = (placed.scale * f64::from(t.zoom_delta)).clamp(REAL_SIZE * 0.01, REAL_SIZE * 50.0);
                let near_real = (s / REAL_SIZE - 1.0).abs() < SNAP;
                if near_real {
                    s = REAL_SIZE;
                }
                if near_real
                    && !at_real
                    && let Ok(b) = self.bridge()
                {
                    (b.tick)();
                }
                if let Some(p) = self.pinch.as_mut() {
                    p.1 = near_real;
                }
                placed.scale = s;
                // Twisting clockwise on the screen turns the model clockwise seen from above.
                let turn_by = -f64::from(t.rotation_delta);
                placed.axes = placed.axes.map(|a| rotate_about_y(a, turn_by));
                self.placed = Some(placed.with_anchor(anchor));
            }
            _ => self.pinch = None,
        }
    }

    /// Select the part under view point `u` (0..1 across and down), or clear the selection over
    /// nothing: the design view's own pick (`view.pick`) with the ray from ARKit's camera taken
    /// into the model's space, and the selection set by `select.set`, so the design view, the
    /// browser and the next command all see it.
    fn select_at(&mut self, app: &mut SolveApp, seen: &Seen, u: (f32, f32)) {
        if self.measuring.is_some() {
            self.measure_at(app, seen, u);
            return;
        }
        let Some(placed) = self.placed else { return };
        let hit = pick(app, seen, &placed, u).map(|h| h.0);
        let items = match &hit {
            Some(name) => serde_json::json!([{ "type": "body", "name": name }]),
            None => serde_json::json!([]),
        };
        match app.run("select.set", serde_json::json!({ "items": items })) {
            Ok(_) => {
                self.selected = hit.clone();
                if hit.is_some()
                    && let Ok(b) = self.bridge()
                {
                    (b.tick)();
                }
            }
            Err(e) => self.message = e,
        }
        crate::note(&format!("ar: pencil at {u:?}: {}", hit.as_deref().unwrap_or("nothing")));
    }

    /// Pull: grab the face under the Pencil.
    fn start_pull(&mut self, app: &mut SolveApp, seen: &Seen, rect: egui::Rect, at: egui::Pos2) {
        let Some(placed) = self.placed else { return };
        // A pull waiting for Apply: the Pencil back near its arrow takes it up again, from where
        // it was left.
        if let Some(p) = self.pulling.as_mut() {
            let ends = [p.point, add(p.point, scale3(p.normal, p.distance))].map(|q| to_screen(seen, &placed, rect, q));
            if ends.iter().flatten().any(|e| e.distance(at) < 80.0) {
                p.base = p.distance;
                p.start = at;
                p.held = true;
                return;
            }
        }
        let u = ((at.x - rect.min.x) / rect.width(), (at.y - rect.min.y) / rect.height());
        let grabbed = pick(app, seen, &placed, u)
            .and_then(|(body, point, normal)| Some(Pull { body, point, normal: unit(normal)?, start: at, distance: 0.0, base: 0.0, held: true }));
        match grabbed {
            Some(p) => {
                crate::note(&format!("ar: pull grabbed {}", p.body));
                self.pulling = Some(p);
            }
            // Off the model: a pull waiting for Apply stays as it was.
            None => self.message = "Put the Pencil on a face of the model to pull it.".into(),
        }
    }

    /// Pull: the Pencil's travel along the face's normal as the screen shows it, in model units.
    fn drag_pull(&mut self, seen: &Seen, rect: egui::Rect, at: egui::Pos2) {
        let (Some(placed), Some(pull)) = (self.placed, self.pulling.as_mut()) else { return };
        let (Some(a), Some(b)) = (to_screen(seen, &placed, rect, pull.point), to_screen(seen, &placed, rect, add(pull.point, pull.normal))) else {
            return;
        };
        // Screen points per model unit along the normal.
        let along = b - a;
        let per_unit = along.length_sq();
        if per_unit < 1e-6 {
            // The normal points at the camera: the screen cannot tell how far.
            self.message = "Look at the face from the side to pull it.".into();
            return;
        }
        let travel = f64::from((at - pull.start).dot(along) / per_unit);
        pull.distance = pull.base + (travel * 2.0).round() / 2.0;
    }

    /// Pull: the face goes where the Pencil let go, as one step in the design's undo history.
    /// Pull: the Pencil lifted. The pull waits, drawn where it would go, for Apply or Cancel
    /// (or the Pencil again near its arrow, to adjust it); one that went nowhere is dropped.
    fn lift_pull(&mut self) {
        let Some(pull) = self.pulling.as_mut() else { return };
        pull.held = false;
        crate::note(&format!("ar: pull lifted at {:+.1} mm", pull.distance));
        if pull.distance.abs() < 0.5 {
            self.pulling = None;
        } else {
            self.message.clear();
        }
    }

    /// Pull: the face goes as far as the waiting pull says, as one step in the design's undo
    /// history.
    fn apply_pull(&mut self, app: &mut SolveApp) {
        let Some(pull) = self.pulling.take() else { return };
        if pull.distance.abs() < 0.5 {
            return;
        }
        let params = serde_json::json!({ "face": pull.point, "distance": pull.distance, "body": pull.body });
        match app.run("solid.press_pull", params) {
            Ok(_) => {
                self.message = format!("{} moved {:+.1} mm.", pull.body, pull.distance);
                if let Ok(b) = self.bridge() {
                    (b.tick)();
                }
            }
            Err(e) => self.message = e,
        }
        crate::note(&format!("ar: pulled {} by {:+.1} mm", pull.body, pull.distance));
    }

    /// The face being pulled: an arrow from where the Pencil landed along the face's normal, as
    /// far as the face goes, with the distance.
    fn draw_pull(&self, painter: &egui::Painter, rect: egui::Rect, seen: &Seen) {
        let (Some(pull), Some(placed)) = (&self.pulling, self.placed) else { return };
        let Some(a) = to_screen(seen, &placed, rect, pull.point) else { return };
        let Some(b) = to_screen(seen, &placed, rect, add(pull.point, scale3(pull.normal, pull.distance))) else { return };
        let ink = egui::Color32::from_rgb(64, 200, 255);
        painter.circle(a, 7.0, ink, egui::Stroke::new(2.0, egui::Color32::BLACK));
        if a.distance(b) > 1.0 {
            painter.arrow(a, b - a, egui::Stroke::new(4.0, ink));
        }
        let text = format!("{:+.1} mm", pull.distance);
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(22.0), egui::Color32::BLACK);
        let back = egui::Rect::from_center_size(b + egui::vec2(0.0, -26.0), galley.size() + egui::vec2(16.0, 8.0));
        painter.rect_filled(back, 6.0, ink);
        painter.galley(back.center() - galley.size() * 0.5, galley, egui::Color32::BLACK);
    }

    /// Photo: the AR view without the panel, saved to Photos.
    fn save_photo(&mut self, rs: &egui_wgpu::RenderState, size: [u32; 2]) {
        // At the screen's resolution, or the camera's own when the model is drawn smaller (heat).
        let result = photo(rs, size).and_then(|rgba| {
            let b = self.bridge()?;
            match (b.save_photo)(rgba.as_ptr(), size[0] as i32, size[1] as i32) {
                1 => Ok(()),
                _ => Err("the photo could not be saved".to_string()),
            }
        });
        match result {
            Ok(()) => {
                self.message = "Saving the photo…".into();
                if let Ok(b) = self.bridge() {
                    (b.tick)();
                }
            }
            Err(e) => self.message = e,
        }
        crate::note(&format!("ar: photo {}x{}: {}", size[0], size[1], self.message));
    }

    /// The design's joints, for the Joints list (`joint.list`).
    fn list_joints(app: &mut SolveApp) -> Vec<(String, Vec<(f64, String)>)> {
        let reply = app.run("joint.list", serde_json::json!({ "exclude_rigid": true })).unwrap_or_default();
        let Some(joints) = reply.get("joints").and_then(|j| j.as_array()) else { return Vec::new() };
        joints
            .iter()
            .filter_map(|j| {
                let name = j.get("name")?.as_str()?.to_owned();
                let values = j
                    .get("values")?
                    .as_array()?
                    .iter()
                    .filter_map(|v| Some((v.get("value")?.as_f64()?, v.get("unit").and_then(|u| u.as_str()).unwrap_or("").to_owned())))
                    .collect::<Vec<_>>();
                (!values.is_empty()).then_some((name, values))
            })
            .collect()
    }

    /// Move joint `name`'s coordinate `index` to `value` (`joint.drive`, held to its limits),
    /// and list the joints again to show where it went.
    fn drive_joint(&mut self, app: &mut SolveApp, name: &str, index: usize, value: f64) {
        let params = serde_json::json!({ "joint": name, "value": value, "index": index });
        if let Err(e) = app.run("joint.drive", params) {
            self.message = e;
        }
        self.overlaps.clear();
        self.joints = Some(Self::list_joints(app));
    }

    /// Check: which parts overlap (`inspect.interference`, held to a few seconds so the view
    /// does not stall). The overlapping parts are selected and each overlap is marked in the room.
    fn check_overlaps(&mut self, app: &mut SolveApp) {
        let reply = match app.run("inspect.interference", serde_json::json!({ "budget_s": 8 })) {
            Ok(r) => r,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        self.overlaps = reply
            .get("interferences")
            .and_then(|i| i.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|o| {
                        let c = o.get("center")?;
                        Some((
                            o.get("a")?.as_str()?.to_owned(),
                            o.get("b")?.as_str()?.to_owned(),
                            [c.get(0)?.as_f64()?, c.get(1)?.as_f64()?, c.get(2)?.as_f64()?],
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.overlaps.is_empty() {
            self.message = "No parts overlap.".into();
        } else {
            let mut names: Vec<&str> = self.overlaps.iter().flat_map(|(a, b, _)| [a.as_str(), b.as_str()]).collect();
            names.sort_unstable();
            names.dedup();
            let items: Vec<serde_json::Value> = names.iter().map(|n| serde_json::json!({ "type": "body", "name": n })).collect();
            let _ = app.run("select.set", serde_json::json!({ "items": items }));
            let pairs: Vec<String> = self.overlaps.iter().map(|(a, b, _)| format!("{a} and {b}")).collect();
            self.message = format!("Overlapping: {}.", pairs.join(", "));
        }
        if let Ok(b) = self.bridge() {
            (b.tick)();
        }
        crate::note(&format!("ar: check found {} overlaps", self.overlaps.len()));
    }

    /// The overlaps from the last Check, marked where they are in the room.
    fn draw_overlaps(&self, painter: &egui::Painter, rect: egui::Rect, seen: &Seen) {
        let Some(placed) = self.placed else { return };
        let red = egui::Color32::from_rgb(235, 64, 52);
        for (_, _, c) in &self.overlaps {
            if let Some(at) = to_screen(seen, &placed, rect, *c) {
                painter.circle(at, 14.0, red.gamma_multiply(0.35), egui::Stroke::new(3.0, red));
                painter.circle_filled(at, 4.0, red);
            }
        }
    }

    /// The Joints list beside the panel: each joint's coordinates with − and + (5 a step).
    fn joints_list(&mut self, ctx: &egui::Context, at: egui::Pos2, app: &mut SolveApp) -> Option<egui::Rect> {
        let joints = self.joints.clone()?;
        let mut drive: Option<(String, usize, f64)> = None;
        let container = Container::new(ctx, Surface::Popup);
        let shown = container.area(egui::Id::unique("ar-joints")).order(egui::Order::Tooltip).fixed_pos(at).show(ctx, |ui| {
            let frame = Container::popup_frame(ui.style());
            container.show_frame(ui, frame, ctx.content_rect(), |ui| {
                if joints.is_empty() {
                    ui.label("This design has no joints to move.");
                }
                for (name, values) in &joints {
                    for (k, (value, unit)) in values.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let axis = if values.len() > 1 { format!(" {}", k + 1) } else { String::new() };
                            ui.label(format!("{name}{axis}"));
                            if button(ui, "−", Button::Small).clicked() {
                                drive = Some((name.clone(), k, value - 5.0));
                            }
                            ui.label(format!("{value:.1} {unit}"));
                            if button(ui, "+", Button::Small).clicked() {
                                drive = Some((name.clone(), k, value + 5.0));
                            }
                        });
                    }
                }
            });
        });
        if let Some((name, k, v)) = drive {
            self.drive_joint(app, &name, k, v);
        }
        Some(shown.response.rect)
    }

    /// The design's next exploded view (`explode.show`: shown only, the design keeps its
    /// placements), and after the last, the assembly as it is.
    fn next_exploded_view(&mut self, app: &mut SolveApp) {
        let names: Vec<String> = app
            .run("explode.list", serde_json::json!({}))
            .ok()
            .and_then(|r| r.get("views").and_then(|v| v.as_array()).cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned))
            .collect();
        if names.is_empty() {
            self.exploded = 0;
            self.message = "This design has no exploded views. Make one with Exploded View (an assembly's parts pulled apart).".into();
            return;
        }
        let next = (self.exploded + 1) % (names.len() + 1);
        let view = if next == 0 { serde_json::json!("none") } else { serde_json::json!(names[next - 1]) };
        match app.run("explode.show", serde_json::json!({ "view": view })) {
            Ok(_) => {
                self.exploded = next;
                self.message = if next == 0 { "Assembled.".into() } else { format!("Exploded view: {}", names[next - 1]) };
            }
            Err(e) => self.message = e,
        }
        crate::note(&format!("ar: exploded view {next} of {}", names.len()));
    }

    /// Measure: a Pencil tap on the model marks a point (where the design view's pick meets the
    /// face); the second gives the distance; a third starts again.
    fn measure_at(&mut self, app: &mut SolveApp, seen: &Seen, u: (f32, f32)) {
        let Some(placed) = self.placed else { return };
        let Some((_, point, _)) = pick(app, seen, &placed, u) else {
            self.message = "Tap the model to measure.".into();
            return;
        };
        let points = self.measuring.get_or_insert_with(Vec::new);
        if points.len() >= 2 {
            points.clear();
        }
        points.push(point);
        self.message.clear();
        if let Ok(b) = self.bridge() {
            (b.tick)();
        }
        if let Some(points) = &self.measuring
            && let [a, b] = points.as_slice()
        {
            crate::note(&format!("ar: measured {:.2} mm", dot(sub(*a, *b), sub(*a, *b)).sqrt()));
        }
    }

    /// The measured points and the line between them, drawn over the AR view where they are in
    /// the room, with the distance in the model's units (millimetres) at the middle.
    fn draw_measure(&self, painter: &egui::Painter, rect: egui::Rect, seen: &Seen) {
        let (Some(points), Some(placed)) = (&self.measuring, self.placed) else { return };
        let to_screen = |q: [f64; 3]| to_screen(seen, &placed, rect, q);
        let ink = egui::Color32::from_rgb(255, 214, 10);
        let dots: Vec<egui::Pos2> = points.iter().filter_map(|q| to_screen(*q)).collect();
        if let [a, b] = dots.as_slice() {
            painter.line_segment([*a, *b], egui::Stroke::new(3.0, ink));
        }
        for d in &dots {
            painter.circle(*d, 7.0, ink, egui::Stroke::new(2.0, egui::Color32::BLACK));
        }
        if let ([a, b], [pa, pb]) = (points.as_slice(), dots.as_slice()) {
            let mm = dot(sub(*a, *b), sub(*a, *b)).sqrt();
            let at = pa.lerp(*pb, 0.5) + egui::vec2(0.0, -22.0);
            let text = format!("{mm:.1} mm");
            let galley = painter.layout_no_wrap(text, egui::FontId::proportional(22.0), egui::Color32::BLACK);
            let back = egui::Rect::from_center_size(at, galley.size() + egui::vec2(16.0, 8.0));
            painter.rect_filled(back, 6.0, ink);
            painter.galley(back.center() - galley.size() * 0.5, galley, egui::Color32::BLACK);
        }
    }

    fn controls(&mut self, ctx: &egui::Context, screen: egui::Rect, seen: Option<&Seen>, coaching: bool, app: &mut SolveApp) {
        let safe = ctx.content_rect();
        let mut leave = false;
        let container = Container::new(ctx, Surface::Popup);
        let shown = container.area(egui::Id::unique("ar-controls")).order(egui::Order::Tooltip).fixed_pos(safe.min + egui::vec2(16.0, 16.0)).show(ctx, |ui| {
            let frame = Container::popup_frame(ui.style());
            container.show_frame(ui, frame, safe, |ui| {
                ui.horizontal(|ui| {
                    if button(ui, "Done", Button::Small).clicked() {
                        leave = true;
                    }
                    if button(ui, "Photo", Button::Small).backing_on_hover_text("Save the room and the model to Photos").clicked() {
                        self.take_photo = true;
                    }
                    // The design's own undo history: a pull, a section applied, anything.
                    for (label, command) in [("Undo", "edit.undo"), ("Redo", "edit.redo")] {
                        if button(ui, label, Button::Small).clicked() {
                            self.pulling = None;
                            self.overlaps.clear();
                            if let Err(e) = app.run(command, serde_json::json!({})) {
                                self.message = e;
                            }
                        }
                    }
                    if let Some(mut p) = self.placed {
                        let real = (p.scale / REAL_SIZE - 1.0).abs() < 1e-9;
                        if solidcraft_ui_kit::text_button(ui, "Real size", !real).clicked() {
                            let anchor = p.anchor();
                            p.scale = REAL_SIZE;
                            self.placed = Some(p.with_anchor(anchor));
                        }
                        ui.label(format!("{:.0}%", p.scale / REAL_SIZE * 100.0));
                        if let Some(name) = &self.selected {
                            ui.separator();
                            ui.label(name);
                        }
                        ui.separator();
                        let on = self.cut.is_some();
                        if button(ui, "Section", if on { Button::SmallSelected } else { Button::Small }).clicked() {
                            self.cut = if on { None } else { seen.and_then(|s| p.cut_facing(s.back)) };
                        }
                        if let Some(c) = self.cut
                            && button(ui, "Apply", Button::Small).backing_on_hover_text("Keep this cut in the design (Section Analysis)").clicked()
                        {
                            // One step in the design's undo history, as the design view's own
                            // Section Analysis makes; the plane through the cut's point nearest
                            // the origin.
                            let origin = scale3(c.normal, c.d);
                            let plane = serde_json::json!({ "origin": origin, "normal": c.normal });
                            match app.run("inspect.section", serde_json::json!({ "plane": plane })) {
                                Ok(_) => {
                                    self.cut = None;
                                    self.message = "The cut is in the design now (Section Analysis).".into();
                                }
                                Err(e) => self.message = e,
                            }
                        }
                        let explode_label = if self.exploded > 0 { "Exploded" } else { "Explode" };
                        if button(ui, explode_label, if self.exploded > 0 { Button::SmallSelected } else { Button::Small }).clicked() {
                            self.next_exploded_view(app);
                        }
                        if button(ui, "Pull", if self.pull_mode { Button::SmallSelected } else { Button::Small }).clicked() {
                            self.pull_mode = !self.pull_mode;
                            self.pulling = None;
                            if self.pull_mode {
                                self.measuring = None;
                            }
                        }
                        if self.pulling.as_ref().is_some_and(|p| !p.held) {
                            if button(ui, "Apply", Button::SmallSelected).clicked() {
                                self.apply_pull(app);
                            }
                            if button(ui, "Cancel", Button::Small).clicked() {
                                self.pulling = None;
                            }
                        }
                        let joints_open = self.joints.is_some();
                        if button(ui, "Joints", if joints_open { Button::SmallSelected } else { Button::Small }).clicked() {
                            self.joints = if joints_open { None } else { Some(Self::list_joints(app)) };
                        }
                        if button(ui, "Check", Button::Small).backing_on_hover_text("Find parts that overlap").clicked() {
                            self.check_overlaps(app);
                        }
                        let measuring = self.measuring.is_some();
                        if button(ui, "Measure", if measuring { Button::SmallSelected } else { Button::Small }).clicked() {
                            self.measuring = if measuring { None } else { Some(Vec::new()) };
                            self.pull_mode = false;
                        }
                        ui.separator();
                        if button(ui, "Pick up", Button::Small).clicked() {
                            self.measuring = None;
                            self.placed = None;
                            self.cut = None;
                        }
                    }
                });
                let hint = if !self.message.is_empty() {
                    self.message.clone()
                } else if coaching {
                    String::new()
                } else {
                    match seen {
                        None => "Starting the camera…".into(),
                        Some(s) if s.tracking == 0 => "Tracking is not available.".into(),
                        Some(s) if s.tracking == 1 => match s.reason {
                            2 => "Move the iPad more slowly.".into(),
                            3 => "Point at a surface with more detail or light.".into(),
                            4 => "Point the iPad at where it was before, to find the model again.".into(),
                            _ => "Finding the room…".into(),
                        },
                        Some(_) if self.placed.is_none() => "Tap a surface to put the model there.".into(),
                        Some(_) if self.measuring.as_ref().is_some_and(|m| m.len() < 2) => "Tap two points on the model with the Pencil.".into(),
                        Some(_) if self.pulling.as_ref().is_some_and(|p| !p.held) => {
                            "Apply moves the face; drag its arrow again to change how far.".into()
                        }
                        Some(_) if self.pull_mode => "Press the Pencil on a face and drag to push or pull it.".into(),
                        Some(_) if self.cut.is_some() => "Drag up and down to move the cut. Apply keeps it in the design.".into(),
                        Some(_) => "Drag to move, pinch to scale, twist to turn. Tap a part with the Pencil to select it.".into(),
                    }
                };
                if !hint.is_empty() {
                    ui.label(hint);
                }
            });
        });
        // A Pencil touch that starts on the panel is the panel's (its buttons), not the model's.
        let list = self.joints_list(ctx, shown.response.rect.left_bottom() + egui::vec2(0.0, 8.0), app);
        self.panel = Some(list.map_or(shown.response.rect, |r| r.union(shown.response.rect)));
        let _ = screen;
        if leave {
            let _ = self.set_on(app, false);
        }
    }
}

/// Where model point `q` shows on the AR view `rect`, or `None` behind the camera.
fn to_screen(seen: &Seen, placed: &Placed, rect: egui::Rect, q: [f64; 3]) -> Option<egui::Pos2> {
    let r = placed.apply_room(q);
    let c: [f64; 4] = std::array::from_fn(|k| (0..3).map(|j| seen.view_proj[j][k] * r[j]).sum::<f64>() + seen.view_proj[3][k]);
    (c[3] > 1e-6).then(|| egui::pos2(rect.min.x + ((c[0] / c[3] + 1.0) * 0.5) as f32 * rect.width(), rect.min.y + ((1.0 - c[1] / c[3]) * 0.5) as f32 * rect.height()))
}

/// The part under view point `u` (0..1 across and down) and where its face is hit (model
/// units): the design view's own pick (`view.pick`) with the ray from ARKit's camera taken into
/// the model's space.
fn pick(app: &mut SolveApp, seen: &Seen, placed: &Placed, u: (f32, f32)) -> Option<(String, [f64; 3], [f64; 3])> {
    let inv = invert(&seen.view_proj)?;
    let ndc = [f64::from(u.0) * 2.0 - 1.0, 1.0 - f64::from(u.1) * 2.0];
    let at = |depth: f64| {
        let v = [ndc[0], ndc[1], depth, 1.0];
        let p: [f64; 4] = std::array::from_fn(|r| (0..4).map(|c| inv[c][r] * v[c]).sum());
        [p[0] / p[3], p[1] / p[3], p[2] / p[3]]
    };
    let (near, far) = (at(0.0), at(1.0));
    let origin = placed.point_to_model(near);
    let dir = unit(placed.direction_to_model(sub(far, near)))?;
    let request = serde_json::json!({ "frame": "model", "edges": false, "rays": [{ "origin": origin, "dir": dir }] });
    let reply = solidcraft_ui_egui::view_pick::pick_command(app, &request).ok()?;
    let hit = reply.get("hits")?.get(0)?;
    let body = hit.get("body")?.as_str()?.to_owned();
    let v = |k: &str| -> Option<[f64; 3]> {
        let p = hit.get(k)?;
        Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?, p.get(2)?.as_f64()?])
    };
    Some((body, v("point")?, v("normal")?))
}

/// Where the model goes on first tap: its foot (the middle of its underside) on `point`,
/// upright (the model's up along the room's), the side the window's camera showed toward the
/// iPad, at real size unless that is larger than [`LARGEST_FIRST`].
fn first_placement(app: &SolveApp, source: Option<&solidcraft_ui_egui::stereo::Source>, seen: &Seen, point: [f64; 3]) -> Option<Placed> {
    let bounds = solidcraft_ui_egui::viewport::model_bounds(app);
    if bounds.is_empty() {
        return None;
    }
    let (lo, hi) = ([bounds.min.x, bounds.min.y, bounds.min.z], [bounds.max.x, bounds.max.y, bounds.max.z]);
    let source = source?;
    let up = source.camera.up_dir();
    let back = source.camera.back();
    let model_up = unit([up.x, up.y, up.z])?;
    let level = |v: [f64; 3]| unit(sub(v, scale3(model_up, dot(v, model_up))));
    let model_front = level([back.x, back.y, back.z]).or_else(|| level([0.0, -1.0, 0.0])).or_else(|| level([0.0, 0.0, 1.0]))?;
    let model_side = cross(model_up, model_front);
    let room_up = [0.0, 1.0, 0.0];
    let room_front = unit([seen.eye[0] - point[0], 0.0, seen.eye[2] - point[2]]).unwrap_or([0.0, 0.0, 1.0]);
    let room_side = cross(room_up, room_front);
    // Model axis k in the room: its side, up and front parts carried over.
    let axes = [0, 1, 2].map(|k| {
        let (s, u, f) = (model_side[k], model_up[k], model_front[k]);
        add(add(scale3(room_side, s), scale3(room_up, u)), scale3(room_front, f))
    });
    let middle = scale3(add(lo, hi), 0.5);
    let corners =
        (0..8).map(|i| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }]);
    let lowest = corners.map(|c| dot(c, model_up)).fold(f64::INFINITY, f64::min);
    let foot = sub(middle, scale3(model_up, dot(middle, model_up) - lowest));
    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    let scale = if extent * REAL_SIZE > LARGEST_FIRST { LARGEST_FIRST / extent } else { REAL_SIZE };
    Some(Placed { axes, scale, translation: [0.0; 3], foot, bounds: [lo, hi] }.with_anchor(point))
}

fn ensure_gpu(rs: &egui_wgpu::RenderState) {
    let mut renderer = rs.renderer.write();
    // The model's depth is read with the renderer's sample count, which the shader and the
    // layout are made for: made again if that changes (the renderer drops to one sample when
    // multisampling fails on a device).
    let samples = renderer.callback_resources.get::<Renderer>().map_or(1, |r| r.target().samples.max(1));
    if renderer.callback_resources.get::<Gpu>().is_some_and(|g| g.samples == samples) {
        return;
    }
    let device = &rs.device;
    let multisampled = samples > 1;
    let source = SHADER.replace("MODEL_DEPTH", if multisampled { "texture_depth_multisampled_2d" } else { "texture_depth_2d" });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sc_ar"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sc_ar"),
        entries: &[
            tex(0),
            tex(1),
            tex(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sc_ar"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sc_ar"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: rs.target_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: egui_wgpu::depth_format_from_bits(crate::DEPTH_BITS, 0).map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState { count: rs.msaa_samples.max(1), ..Default::default() },
        multiview_mask: None,
        cache: None,
    });
    // The same pass into a picture of its own, for a photo: no depth, one sample, in a format
    // that keeps the colours as the screen shows them.
    let capture = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sc_ar_photo"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: photo_format(rs), blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sc_ar"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sc_ar"),
        size: 176,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let blank = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_ar_blank"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    rs.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &blank, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &[0, 0, 0, 0],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let blank = blank.create_view(&Default::default());
    // Nothing drawn: depth 0 (the far plane, reversed), which the first use clears it to.
    let blank_depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_ar_blank_depth"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let blank_depth = blank_depth.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sc_ar_blank_depth") });
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("sc_ar_blank_depth"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &blank_depth,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    rs.queue.submit([encoder.finish()]);
    let far_room = r32_texture(rs, [1, 1]);
    rs.queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &far_room, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &1.0e6f32.to_le_bytes(),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let far_room = far_room.create_view(&Default::default());
    // A new pipeline keeps the camera textures already made.
    let (luma, chroma, room_depth) = match renderer.callback_resources.get_mut::<Gpu>() {
        Some(old) => (old.luma.take(), old.chroma.take(), old.room_depth.take()),
        None => (None, None, None),
    };
    renderer.callback_resources.insert(Gpu {
        pipeline,
        capture,
        layout,
        sampler,
        uniform,
        luma,
        chroma,
        blank,
        bind: None,
        samples,
        blank_depth,
        room_depth,
        far_room,
    });
}

/// The photo's format: the AR pass writes linear light into an sRGB target and the screen's
/// colours into another, so the photo is sRGB-encoded either way.
fn photo_format(rs: &egui_wgpu::RenderState) -> wgpu::TextureFormat {
    if rs.target_format.is_srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm }
}

/// The AR view as it is now, without the panel (the camera picture, the model, its shadow), as
/// `size` RGBA rows: the AR pass drawn again into a picture of its own and read back.
fn photo(rs: &egui_wgpu::RenderState, size: [u32; 2]) -> Result<Vec<u8>, String> {
    let renderer = rs.renderer.read();
    let gpu = renderer.callback_resources.get::<Gpu>().ok_or("AR is not drawing yet")?;
    let bind = gpu.bind.as_ref().ok_or("no camera picture yet")?;
    let [width, height] = size;
    let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let target = rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_ar_photo"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: photo_format(rs),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sc_ar_photo") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sc_ar_photo"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&gpu.capture);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
    let row = width * 4;
    let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let read = rs.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sc_ar_photo_read"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) } },
        extent,
    );
    let submission = rs.queue.submit([encoder.finish()]);
    let slice = read.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    rs.device
        .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(std::time::Duration::from_secs(5)) })
        .map_err(|e| format!("waiting for the photo: {e}"))?;
    rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range().map_err(|e| format!("reading the photo: {e}"))?;
    let mut rgba = Vec::with_capacity((row * height) as usize);
    for r in data.chunks(padded as usize).take(height as usize) {
        rgba.extend_from_slice(r.get(..row as usize).unwrap_or_default());
    }
    if rgba.len() != (row * height) as usize {
        return Err("the photo read back short".into());
    }
    Ok(rgba)
}

/// A 32-bit float texture of `size` to write into and read in a shader.
fn r32_texture(rs: &egui_wgpu::RenderState, size: [u32; 2]) -> wgpu::Texture {
    rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sc_ar_room_depth"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Copy LiDAR's depth map (`size`, rows of `row` bytes of 32-bit floats) into its texture.
fn upload_depth(rs: &egui_wgpu::RenderState, size: [u32; 2], row: u32, bytes: &[u8]) {
    let mut renderer = rs.renderer.write();
    let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() else { return };
    if gpu.room_depth.as_ref().is_none_or(|d| d.2 != size) {
        let t = r32_texture(rs, size);
        let v = t.create_view(&Default::default());
        gpu.room_depth = Some((t, v, size));
    }
    if let Some((t, _, _)) = &gpu.room_depth {
        rs.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size[1]) },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
    }
}

/// Give the app's Metal device to the bridge's texture cache. `false` when there is none.
#[cfg(target_os = "ios")]
fn offer_device(rs: &egui_wgpu::RenderState, b: &Bridge) -> bool {
    // SAFETY: the device is wgpu's own Metal device, only read here for its raw handle.
    let raw = unsafe { rs.device.as_hal::<wgpu::hal::api::Metal>() }.map(|d| objc2::rc::Retained::as_ptr(d.raw_device()) as *mut std::ffi::c_void);
    match raw {
        Some(p) if !p.is_null() => (b.set_device)(p) == 1,
        _ => false,
    }
}

/// The current camera picture's planes as wgpu textures sharing its memory. Each keeps the
/// picture alive until wgpu is done with it (its drop callback lets ARKit have it back).
/// `false` (nothing changed) when they could not be made; the caller copies the bytes then.
#[cfg(target_os = "ios")]
fn planes_without_copy(rs: &egui_wgpu::RenderState, b: Option<&Bridge>, size: [u32; 2]) -> bool {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::{MTLTexture, MTLTextureType};
    let Some(b) = b else { return false };
    let mut textures: [*mut std::ffi::c_void; 2] = [std::ptr::null_mut(); 2];
    let mut keeps: [*mut std::ffi::c_void; 2] = [std::ptr::null_mut(); 2];
    if (b.metal_planes)(textures.as_mut_ptr(), keeps.as_mut_ptr()) != 1 {
        return false;
    }
    let release = b.release;
    let half = [size[0] / 2, size[1] / 2];
    let mut made = Vec::new();
    for (k, (format, size)) in [(wgpu::TextureFormat::R8Unorm, size), (wgpu::TextureFormat::Rg8Unorm, half)].into_iter().enumerate() {
        let keep = keeps[k] as usize;
        let drop_keep: wgpu::hal::DropCallback = Box::new(move || release(keep as *mut std::ffi::c_void));
        // SAFETY: an `id<MTLTexture>` the bridge just made, alive while `keep` is; retained here
        // for wgpu's texture.
        let Some(raw) = (unsafe { Retained::retain(textures[k].cast::<ProtocolObject<dyn MTLTexture>>()) }) else {
            drop_keep();
            continue;
        };
        let extent = wgpu::hal::CopyExtent { width: size[0], height: size[1], depth: 1 };
        // SAFETY: the texture is 2D, one layer and one mip level, in `format`, of `size`, as made.
        let hal = unsafe { wgpu::hal::metal::Device::texture_from_raw(raw, format, MTLTextureType::Type2D, 1, 1, extent, Some(drop_keep)) };
        let desc = wgpu::TextureDescriptor {
            label: Some("sc_ar_camera_shared"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: `hal` was made from a texture of exactly this descriptor's shape and format,
        // which the camera has finished writing: it is ready to be read by a shader.
        let texture = unsafe { rs.device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal, &desc, wgpu::TextureUses::RESOURCE) };
        let view = texture.create_view(&Default::default());
        made.push((texture, view, size));
    }
    if made.len() != 2 {
        // What was made is dropped, and with it the picture it held.
        return false;
    }
    let mut renderer = rs.renderer.write();
    let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() else { return false };
    let mut made = made.into_iter();
    gpu.luma = made.next();
    gpu.chroma = made.next();
    true
}

/// Off the iPad (the Mac's tests) there is no camera to share.
#[cfg(not(target_os = "ios"))]
fn offer_device(_rs: &egui_wgpu::RenderState, _b: &Bridge) -> bool {
    false
}

#[cfg(not(target_os = "ios"))]
fn planes_without_copy(_rs: &egui_wgpu::RenderState, _b: Option<&Bridge>, _size: [u32; 2]) -> bool {
    false
}

/// Copy the camera picture's two planes (luma at `size`, chroma at half) into their textures.
fn upload(rs: &egui_wgpu::RenderState, size: [u32; 2], luma_row: u32, luma: &[u8], chroma_row: u32, chroma: &[u8]) {
    let mut renderer = rs.renderer.write();
    let Some(gpu) = renderer.callback_resources.get_mut::<Gpu>() else { return };
    let half = [size[0] / 2, size[1] / 2];
    let make = |format, size: [u32; 2]| {
        let t = rs.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sc_ar_camera"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v, size)
    };
    if gpu.luma.as_ref().is_none_or(|l| l.2 != size) {
        gpu.luma = Some(make(wgpu::TextureFormat::R8Unorm, size));
        gpu.chroma = Some(make(wgpu::TextureFormat::Rg8Unorm, half));
    }
    let write = |t: &wgpu::Texture, size: [u32; 2], row: u32, bytes: &[u8]| {
        rs.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size[1]) },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
    };
    if let (Some(l), Some(c)) = (&gpu.luma, &gpu.chroma) {
        write(&l.0, size, luma_row, luma);
        write(&c.0, half, chroma_row, chroma);
    }
}

fn write_uniform(rs: &egui_wgpu::RenderState, seen: &Seen, model: bool, shadow: Option<Shadow>, occlude: bool) {
    let renderer = rs.renderer.read();
    let Some(gpu) = renderer.callback_resources.get::<Gpu>() else { return };
    let d = seen.display;
    let linear = if rs.target_format.is_srgb() { 1.0 } else { 0.0 };
    let inverse = invert(&seen.view_proj);
    let shadow = shadow.filter(|_| model && inverse.is_some());
    let mut values = [0f32; 44];
    values[..8].copy_from_slice(&[d[0], d[1], d[2], d[3], d[4], d[5], if model { 1.0 } else { 0.0 }, linear]);
    if let Some(inv) = inverse {
        for (c, column) in inv.iter().enumerate() {
            for (r, v) in column.iter().enumerate() {
                values[8 + c * 4 + r] = *v as f32;
            }
        }
        if let Some(sh) = shadow {
            values[24..28].copy_from_slice(&[sh.middle[0] as f32, sh.middle[1] as f32, sh.middle[2] as f32, 1.0]);
            values[28..32].copy_from_slice(&[sh.axes[0][0] as f32, sh.axes[0][1] as f32, sh.axes[1][0] as f32, sh.axes[1][1] as f32]);
        }
        let occlude = occlude && model;
        let [e, b] = [seen.eye, seen.back].map(|v| v.map(|x| x as f32));
        values[32..36].copy_from_slice(&[e[0], e[1], e[2], if occlude { 1.0 } else { 0.0 }]);
        values[36..40].copy_from_slice(&[-b[0], -b[1], -b[2], 0.0]);
    }
    values[40..44].copy_from_slice(&[seen.light[0], seen.light[1], seen.light[2], 1.0]);
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    rs.queue.write_buffer(&gpu.uniform, 0, &bytes);
}

/// The paint callback: one full-screen triangle.
struct Paint;

impl egui_wgpu::CallbackTrait for Paint {
    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(gpu) = resources.get::<Gpu>() else { return };
        let Some(bind) = &gpu.bind else { return };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The camera picture (ARKit's full-range BT.601 Y′CbCr, turned to the screen by the display
/// transform) with the model's picture (colours times alpha) over it.
const SHADER: &str = r#"
struct U {
    // View point to picture point: x' = a x + c y + tx, y' = b x + d y + ty.
    abcd: vec4<f32>,
    // tx, ty, a model picture (1), write linear light (1: an sRGB target).
    rest: vec4<f32>,
    // Clip space to the room, for the contact shadow's rays.
    inv: mat4x4<f32>,
    // The shadow's middle on the floor (xyz) and whether there is one (w).
    shadow_mid: vec4<f32>,
    // Its two half-axes on the floor (x, z of each).
    shadow_axes: vec4<f32>,
    // The camera in the room (xyz), and whether the room's depth hides the model (w).
    eye: vec4<f32>,
    // The way the camera looks (xyz).
    forward: vec4<f32>,
    // The room's light on the model: red, green and blue gains (linear light).
    light: vec4<f32>,
};
@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var model: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var<uniform> u: U;
// The model's depth (reversed: 1 near, 0 far and where nothing was drawn).
@group(0) @binding(5) var model_depth: MODEL_DEPTH;
// The room's depth from LiDAR: metres along the camera's axis, in the camera picture's layout.
@group(0) @binding(6) var room_depth: texture_2d<f32>;

// The room's depth at camera picture point `p` (0..1), between its four nearest samples.
fn room_at(p: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(room_depth));
    let x = clamp(p * size - 0.5, vec2<f32>(0.0), size - 1.0);
    let i = vec2<i32>(floor(x));
    let f = x - floor(x);
    let top = vec2<i32>(size) - 1;
    let a = textureLoad(room_depth, i, 0).r;
    let b = textureLoad(room_depth, min(i + vec2<i32>(1, 0), top), 0).r;
    let c = textureLoad(room_depth, min(i + vec2<i32>(0, 1), top), 0).r;
    let d = textureLoad(room_depth, min(i + vec2<i32>(1, 1), top), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

struct O { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> O {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: O;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(i: O) -> @location(0) vec4<f32> {
    let p = vec2<f32>(
        u.abcd.x * i.uv.x + u.abcd.z * i.uv.y + u.rest.x,
        u.abcd.y * i.uv.x + u.abcd.w * i.uv.y + u.rest.y,
    );
    let y = textureSampleLevel(luma, smp, p, 0.0).r;
    let cbcr = textureSampleLevel(chroma, smp, p, 0.0).rg - vec2<f32>(0.5, 0.5);
    var camera = clamp(vec3<f32>(
        y + 1.402 * cbcr.y,
        y - 0.344136 * cbcr.x - 0.714136 * cbcr.y,
        y + 1.772 * cbcr.x,
    ), vec3<f32>(0.0), vec3<f32>(1.0));
    if (u.rest.w > 0.5) {
        camera = to_linear(camera);
    }
    // The contact shadow: where this pixel's ray meets the floor, darker toward the middle of
    // the footprint. The model is drawn over it, so it shows only around the model.
    if (u.shadow_mid.w > 0.5) {
        let ndc = vec2<f32>(i.uv.x * 2.0 - 1.0, 1.0 - i.uv.y * 2.0);
        let a = u.inv * vec4<f32>(ndc, 0.0, 1.0);
        let b = u.inv * vec4<f32>(ndc, 1.0, 1.0);
        let p0 = a.xyz / a.w;
        let dir = b.xyz / b.w - p0;
        if (abs(dir.y) > 1e-6) {
            let t = (u.shadow_mid.y - p0.y) / dir.y;
            if (t > 0.0) {
                let local = (p0 + dir * t - u.shadow_mid.xyz).xz;
                let a1 = u.shadow_axes.xy;
                let a2 = u.shadow_axes.zw;
                let q = vec2<f32>(dot(local, a1) / dot(a1, a1), dot(local, a2) / dot(a2, a2));
                let r = length(q);
                // The footprint's edge is at about 0.85 of the ellipse. A dark contact band hugs
                // it (what makes a thing look stood on the floor), and a faint soft shadow spreads
                // beyond, as under a room's light from above.
                let contact = 0.55 * (1.0 - smoothstep(0.82, 1.0, r));
                let soft = 0.2 * (1.0 - smoothstep(0.85, 1.7, r));
                let dark = max(contact, soft);
                camera = camera * (1.0 - dark);
            }
        }
    }
    var m = vec4<f32>(0.0);
    if (u.rest.z > 0.5) {
        m = textureSampleLevel(model, smp, i.uv, 0.0);
        // Occlusion: where the room is nearer than the model, a real thing stands in front of
        // it. The model's distance along the camera's axis comes from its depth, turned back
        // into a room point; a centimetre's grace and a soft two keep LiDAR's noise from
        // nibbling at the model where it stands on the table.
        if (u.eye.w > 0.5 && m.a > 0.0) {
            let dims = vec2<f32>(textureDimensions(model_depth));
            let texel = vec2<i32>(clamp(i.uv * dims, vec2<f32>(0.0), dims - 1.0));
            let d = textureLoad(model_depth, texel, 0);
            if (d > 0.0) {
                let ndc = vec2<f32>(i.uv.x * 2.0 - 1.0, 1.0 - i.uv.y * 2.0);
                let w = u.inv * vec4<f32>(ndc, 1.0 - d, 1.0);
                let model_far = dot(w.xyz / w.w - u.eye.xyz, u.forward.xyz);
                let room = room_at(p);
                let hidden = clamp((model_far - room - 0.01) / 0.02, 0.0, 1.0);
                m = m * (1.0 - hidden);
            }
        }
        // The room's light, in linear light: on an sRGB target the picture is linear already;
        // on another its colours are as the screen shows them.
        if (u.rest.w > 0.5) {
            m = vec4<f32>(m.rgb * u.light.rgb, m.a);
        } else {
            m = vec4<f32>(m.rgb * pow(u.light.rgb, vec3<f32>(1.0 / 2.2)), m.a);
        }
    }
    return vec4<f32>(m.rgb + camera * (1.0 - m.a), 1.0);
}
"#;

/// The model's colours under the room's light: ARKit's ambient intensity (lumens, 1000 for a
/// well-lit room) as brightness, its colour temperature (kelvin) as a tint, both held back
/// (a CAD model should stay readable in a dim room, and a white part should stay near white).
fn light_gain(lumens: i32, kelvin: i32) -> [f32; 3] {
    let lumens = if lumens > 0 { lumens as f32 } else { 1000.0 };
    let bright = (lumens / 1000.0).clamp(0.55, 1.15).powf(0.6);
    let tint = kelvin_rgb(if kelvin > 0 { kelvin as f32 } else { 6500.0 });
    let white = kelvin_rgb(6500.0);
    // Half way between neutral and the room's colour.
    std::array::from_fn(|k| bright * (0.5 + 0.5 * tint[k] / white[k].max(1e-3)))
}

/// The colour of a black body at `kelvin` (1000..40000), each channel 0..1: the usual curve
/// fit (Tanner Helland's), close enough for a tint.
fn kelvin_rgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.698_73 * (t - 60.0).powf(-0.133_204_76) };
    let g = if t <= 66.0 { 99.470_8 * t.ln() - 161.119_57 } else { 288.122_16 * (t - 60.0).powf(-0.075_514_85) };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|c| (c / 255.0).clamp(0.0, 1.0))
}

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}
/// The inverse of `m` (Gauss-Jordan with partial pivoting), or `None` when it has none.
fn invert(m: &Mat4) -> Option<Mat4> {
    // Rows of [m | I], m read by rows.
    let mut a: [[f64; 8]; 4] = std::array::from_fn(|r| {
        std::array::from_fn(|c| {
            if c < 4 {
                m[c][r]
            } else if c - 4 == r {
                1.0
            } else {
                0.0
            }
        })
    });
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in a[col].iter_mut() {
            *v /= p;
        }
        for r in 0..4 {
            if r != col {
                let f = a[r][col];
                let row = a[col];
                for (v, w) in a[r].iter_mut().zip(row) {
                    *v -= f * w;
                }
            }
        }
    }
    let out: Mat4 = std::array::from_fn(|c| std::array::from_fn(|r| a[r][4 + c]));
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

fn to_f32(m: &Mat4) -> [[f32; 4]; 4] {
    m.map(|c| c.map(|v| v as f32))
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l.is_finite() && l > 1e-9).then(|| scale3(a, 1.0 / l))
}
/// `p` in the frame whose X, Y and Z are `axes`.
fn turn(axes: &[[f64; 3]; 3], p: [f64; 3]) -> [f64; 3] {
    add(add(scale3(axes[0], p[0]), scale3(axes[1], p[1])), scale3(axes[2], p[2]))
}
/// `v` turned by `angle` (radians, counter-clockwise seen from above) about the room's vertical.
fn rotate_about_y(v: [f64; 3], angle: f64) -> [f64; 3] {
    let (s, c) = angle.sin_cos();
    [c * v[0] + s * v[2], v[1], -s * v[0] + c * v[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9)
    }

    /// Scaling and turning keep the foot where it was put.
    #[test]
    fn the_foot_stays_put() {
        let p = Placed {
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
            scale: REAL_SIZE,
            translation: [0.0; 3],
            foot: [5.0, 5.0, 0.0],
            bounds: [[0.0; 3], [10.0, 10.0, 4.0]],
        }
        .with_anchor([0.2, 0.7, -0.4]);
        assert!(close(p.anchor(), [0.2, 0.7, -0.4]));
        let mut q = p;
        q.scale *= 3.0;
        q.axes = q.axes.map(|a| rotate_about_y(a, 0.7));
        let q = q.with_anchor(p.anchor());
        assert!(close(q.anchor(), [0.2, 0.7, -0.4]));
        // A Z-up model stands up along the room's Y.
        assert!(close(p.axes[2], [0.0, 1.0, 0.0]));
    }

    /// The inverse undoes the matrix.
    #[test]
    fn the_inverse_undoes() {
        let m: Mat4 = [[2.0, 0.1, 0.0, 0.0], [0.0, 1.5, 0.3, 0.0], [0.2, 0.0, -1.0, -1.0], [0.5, 0.2, -0.1, 0.0]];
        let Some(inv) = invert(&m) else { panic!("invertible") };
        let id = mul(&m, &inv);
        for (c, column) in id.iter().enumerate() {
            for (r, v) in column.iter().enumerate() {
                assert!((v - if c == r { 1.0 } else { 0.0 }).abs() < 1e-9, "{id:?}");
            }
        }
    }

    /// The shadow lies under the model's footprint, on the floor at its foot.
    #[test]
    fn the_shadow_is_under_the_footprint() {
        let p = Placed {
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
            scale: REAL_SIZE,
            translation: [0.0; 3],
            foot: [100.0, 60.0, 0.0],
            bounds: [[0.0; 3], [200.0, 120.0, 80.0]],
        }
        .with_anchor([0.3, 0.75, -0.5]);
        let Some(sh) = p.contact_shadow() else { panic!("a shadow") };
        assert!(close(sh.middle, [0.3, 0.75, -0.5]), "{sh:?}");
        let r: Vec<f64> = sh.axes.iter().map(|a| (a[0] * a[0] + a[1] * a[1]).sqrt()).collect();
        assert!(r.iter().any(|r| *r > 0.1 && *r < 0.13) && r.iter().any(|r| *r > 0.06 && *r < 0.09), "{r:?}");
    }

    /// A well-lit neutral room leaves the model's colours as they are; a dim warm one darkens
    /// them and warms them, within bounds.
    #[test]
    fn the_rooms_light() {
        let neutral = light_gain(1000, 6500);
        assert!(neutral.iter().all(|g| (g - 1.0).abs() < 0.02), "{neutral:?}");
        let warm = light_gain(300, 3000);
        assert!(warm[0] > warm[2], "{warm:?}");
        assert!(warm.iter().all(|g| *g > 0.3 && *g < 1.0), "{warm:?}");
        let dark = light_gain(0, 0);
        assert!(dark.iter().all(|g| (g - 1.0).abs() < 0.02), "no estimate is neutral: {dark:?}");
    }

    /// A cut facing the camera goes through the model's middle and hides the near half.
    #[test]
    fn a_cut_faces_the_camera() {
        let p = Placed {
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]],
            scale: REAL_SIZE,
            translation: [0.0; 3],
            foot: [100.0, 60.0, 0.0],
            bounds: [[0.0; 3], [200.0, 120.0, 80.0]],
        }
        .with_anchor([0.0, 0.7, -0.5]);
        // The camera along the room's +Z from the model: its back points +Z.
        let Some(c) = p.cut_facing([0.0, 0.3, 1.0]) else { panic!("a cut") };
        let middle = [100.0, 60.0, 40.0];
        assert!((dot(c.normal, middle) - c.d).abs() < 1e-9, "through the middle");
        // A model point nearer the camera is on the hidden side (normal · p > d).
        let toward_camera = p.point_to_model(add(p.apply_room(middle), [0.0, 0.0, 0.05]));
        assert!(dot(c.normal, toward_camera) > c.d);
    }

    /// The placement matrix moves model points as `Placed` says.
    #[test]
    fn the_matrix_agrees() {
        let p = Placed {
            axes: [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            scale: 0.002,
            translation: [1.0, 2.0, 3.0],
            foot: [0.0; 3],
            bounds: [[0.0; 3]; 2],
        };
        let m = p.matrix();
        let q = [10.0, 20.0, 30.0];
        let by_matrix = [0, 1, 2].map(|r| (0..3).map(|c| m[c][r] * q[c]).sum::<f64>() + m[3][r]);
        assert!(close(by_matrix, add(p.translation, scale3(turn(&p.axes, q), p.scale))));
    }
}
