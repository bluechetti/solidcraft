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
}

/// The model in the room: room point = translation + scale · axes · model point.
#[derive(Clone, Copy, Debug)]
struct Placed {
    axes: [[f64; 3]; 3],
    scale: f64,
    translation: [f64; 3],
    /// The model point that sits on the surface (the middle of its underside).
    foot: [f64; 3],
}

impl Placed {
    fn matrix(&self) -> Mat4 {
        let [x, y, z] = self.axes.map(|a| a.map(|v| v * self.scale));
        let t = self.translation;
        [[x[0], x[1], x[2], 0.0], [y[0], y[1], y[2], 0.0], [z[0], z[1], z[2], 0.0], [t[0], t[1], t[2], 1.0]]
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
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    luma: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    chroma: Option<(wgpu::Texture, wgpu::TextureView, [u32; 2])>,
    blank: wgpu::TextureView,
    bind: Option<wgpu::BindGroup>,
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
            self.message.clear();
            self.on = true;
            app.viewport.stereo.on = true;
            self.tablet_was = app.tablet.enabled;
            app.tablet.enabled = false;
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
        let ppp = ctx.pixels_per_point();
        let size_px = [(screen.width() * ppp).round().max(1.0) as u32, (screen.height() * ppp).round().max(1.0) as u32];
        ensure_gpu(rs);
        let seen = self.next_frame(rs, [screen.width(), screen.height()]);
        // The model from ARKit's camera.
        let mut model_drawn = false;
        if let (Some(seen), Some(placed), Some(source)) = (seen.as_ref(), self.placed, self.source.as_ref()) {
            model_drawn = draw_model(rs, source, seen, &placed, size_px, &mut self.target);
        }
        if let Some(seen) = seen.as_ref() {
            write_uniform(rs, seen, model_drawn);
        }
        self.bind(rs, model_drawn);

        // The whole screen: the picture, and the touches (they go to AR, not the app below).
        let coaching = self.bridge().map(|b| (b.coaching)() == 1).unwrap_or(false);
        egui::Area::new(egui::Id::unique("ar-view")).order(egui::Order::Foreground).constrain(false).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
            // Under the status bar and the home indicator too: egui clips to the safe area.
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(screen);
            painter.add(egui_wgpu::Callback::new_paint_callback(rect, Paint));
            if !coaching && let Some(seen) = seen.as_ref() {
                self.gestures(ui, &response, rect, seen, app);
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
        let (frame_fn, done) = (b.frame, b.frame_done);
        let mut view = [0f32; 16];
        let mut proj = [0f32; 16];
        let mut camera = [0f32; 16];
        let mut display = [0f32; 6];
        let mut info = [0i32; 8];
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
            if !planes[0].is_null() && !planes[1].is_null() && w > 1 && h > 1 {
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
        })
    }
}

/// The placed model as ARKit's camera sees it, into `target`. `false` when nothing was drawn.
fn draw_model(
    rs: &egui_wgpu::RenderState,
    source: &solidcraft_ui_egui::stereo::Source,
    seen: &Seen,
    placed: &Placed,
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
        let mut renderer = rs.renderer.write();
        let Some(view) = renderer.callback_resources.get_mut::<Renderer>() else { return false };
        // Without the design's grid and ground shadow: the room has a floor of its own.
        view.draw_second_view(&rs.device, &rs.queue, &frame, size, [0, 0, size[0], size[1]], false, target)
    }
}

impl State {
    fn bind(&mut self, rs: &egui_wgpu::RenderState, model: bool) {
        let mut renderer = rs.renderer.write();
        let picture = self.target.as_ref().filter(|_| model).map(ViewTarget::picture).cloned();
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
            ],
        }));
    }

    /// Tap: put the model on the surface there. Pinch: scale about its foot, snapping to real
    /// size. Twist: turn it about the vertical.
    fn gestures(&mut self, ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, seen: &Seen, app: &mut SolveApp) {
        let tapped = response.clicked().then(|| response.interact_pointer_pos()).flatten().map(|at| ((at.x - rect.min.x) / rect.width(), (at.y - rect.min.y) / rect.height()));
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
                if near_real && !at_real && let Ok(b) = self.bridge() {
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

    fn controls(&mut self, ctx: &egui::Context, screen: egui::Rect, seen: Option<&Seen>, coaching: bool, app: &mut SolveApp) {
        let safe = ctx.content_rect();
        let mut leave = false;
        egui::Area::new(egui::Id::unique("ar-controls")).order(egui::Order::Tooltip).fixed_pos(safe.min + egui::vec2(16.0, 16.0)).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        leave = true;
                    }
                    if let Some(mut p) = self.placed {
                        let real = (p.scale / REAL_SIZE - 1.0).abs() < 1e-9;
                        if ui.add_enabled(!real, egui::Button::new("Real size")).clicked() {
                            let anchor = p.anchor();
                            p.scale = REAL_SIZE;
                            self.placed = Some(p.with_anchor(anchor));
                        }
                        ui.label(format!("{:.0}%", p.scale / REAL_SIZE * 100.0));
                        if ui.button("Pick up").clicked() {
                            self.placed = None;
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
                            _ => "Finding the room…".into(),
                        },
                        Some(_) if self.placed.is_none() => "Tap a surface to put the model there.".into(),
                        Some(_) => "Pinch to scale, twist to turn, tap to move it.".into(),
                    }
                };
                if !hint.is_empty() {
                    ui.label(hint);
                }
            });
        });
        let _ = screen;
        if leave {
            let _ = self.set_on(app, false);
        }
    }
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
    let corners = (0..8).map(|i| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }]);
    let lowest = corners.map(|c| dot(c, model_up)).fold(f64::INFINITY, f64::min);
    let foot = sub(middle, scale3(model_up, dot(middle, model_up) - lowest));
    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    let scale = if extent * REAL_SIZE > LARGEST_FIRST { LARGEST_FIRST / extent } else { REAL_SIZE };
    Some(Placed { axes, scale, translation: [0.0; 3], foot }.with_anchor(point))
}

fn ensure_gpu(rs: &egui_wgpu::RenderState) {
    let mut renderer = rs.renderer.write();
    if renderer.callback_resources.get::<Gpu>().is_some() {
        return;
    }
    let device = &rs.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sc_ar"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sc_ar"),
        entries: &[
            tex(0),
            tex(1),
            tex(2),
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sc_ar"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
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
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("sc_ar"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("sc_ar"),
        size: 32,
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
    renderer.callback_resources.insert(Gpu { pipeline, layout, sampler, uniform, luma: None, chroma: None, blank, bind: None });
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

fn write_uniform(rs: &egui_wgpu::RenderState, seen: &Seen, model: bool) {
    let renderer = rs.renderer.read();
    let Some(gpu) = renderer.callback_resources.get::<Gpu>() else { return };
    let d = seen.display;
    let linear = if rs.target_format.is_srgb() { 1.0 } else { 0.0 };
    let values: [f32; 8] = [d[0], d[1], d[2], d[3], d[4], d[5], if model { 1.0 } else { 0.0 }, linear];
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
};
@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var model: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
@group(0) @binding(4) var<uniform> u: U;

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
    var m = vec4<f32>(0.0);
    if (u.rest.z > 0.5) {
        m = textureSampleLevel(model, smp, i.uv, 0.0);
    }
    return vec4<f32>(m.rgb + camera * (1.0 - m.a), 1.0);
}
"#;

fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
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
        let p = Placed { axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], scale: REAL_SIZE, translation: [0.0; 3], foot: [5.0, 5.0, 0.0] }
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

    /// The placement matrix moves model points as `Placed` says.
    #[test]
    fn the_matrix_agrees() {
        let p = Placed { axes: [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], scale: 0.002, translation: [1.0, 2.0, 3.0], foot: [0.0; 3] };
        let m = p.matrix();
        let q = [10.0, 20.0, 30.0];
        let by_matrix = [0, 1, 2].map(|r| (0..3).map(|c| m[c][r] * q[c]).sum::<f64>() + m[3][r]);
        assert!(close(by_matrix, add(p.translation, scale3(turn(&p.axes, q), p.scale))));
    }
}
