//! SolidCraft on iPad (and iPhone).
//!
//! Runs the same [`solidcraft_ui_egui::SolveApp`] as the desktop app through eframe's native
//! runner; winit's iOS backend supplies the UIKit window and touch/Pencil events, wgpu draws with
//! Metal.
// Built for the Mac (tests, workspace checks) the UIKit parts are left out, and what only they
// use would read as unused.
#![cfg_attr(not(target_os = "ios"), allow(dead_code))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "ios")]
mod ai;
#[cfg(any(target_os = "ios", test))]
mod ai_data;
#[cfg(any(target_os = "ios", test))]
mod ar;
mod files;
#[cfg(target_os = "ios")]
mod keyboard;
mod numpad;
#[cfg(target_os = "ios")]
mod pencil;
#[cfg(target_os = "ios")]
mod picker;
#[cfg(target_os = "ios")]
mod scribble;

use solidcraft_engine::Session;
use solidcraft_ui_egui::SolveApp;

/// A 32-bit float depth buffer, as on the other apps (reversed-depth viewport).
const DEPTH_BITS: u8 = 32;

struct App {
    solve: SolveApp,
    files: files::Files,
    #[cfg(target_os = "ios")]
    ai: ai::State,
    /// The design in the room through the camera.
    #[cfg(target_os = "ios")]
    ar: ar::State,
    /// The view is moved left to clear the AI window while it is open.
    ai_shifted: bool,
    ai_view: Option<(solidcraft_viewport::Camera, solidcraft_viewport::Camera)>,
    frames: u32,
    synced_at: f64,
    numpad: numpad::Numpad,
    /// Test hook steps still to run (last first) and when the last one ran.
    test_steps: Vec<String>,
    /// Pencil hovers written to the log (the first few, to show hover arrives).
    hover_noted: u8,
    test_at: f64,
    /// The test hook's way into the control channel, and the replies it has not read.
    control_tx: Option<std::sync::mpsc::Sender<solidcraft_control::ControlRequest>>,
    control_replies: Vec<std::sync::mpsc::Receiver<solidcraft_control::ControlResponse>>,
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.frames < 4 {
            self.frames += 1;
            let (insets, screen, content) = ctx.input(|i| (i.safe_area_insets(), i.viewport_rect(), i.content_rect()));
            note(&format!("frame {}: insets {insets:?} screen {screen:?} content {content:?}", self.frames));
        }
        // Test hook: `SIMCTL_CHILD_SOLIDCRAFT_IOS_TEST=doc.open` starts that command after start-up.
        if self.frames == 3 {
            self.frames += 1;
            if let Ok(steps) = std::env::var("SOLIDCRAFT_IOS_TEST") {
                self.test_steps = steps.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
                self.test_steps.reverse();
            }
        }
        // Test hook steps, one a second: a UI command id (`solid.extrude` opens its dialog),
        // `run:<command> <json params>` to run a command, or `menu:<x>,<y>` for the context menu.
        let now = ctx.input(|i| i.time);
        if !self.test_steps.is_empty() && now - self.test_at >= 1.0 {
            self.test_at = now;
            if let Some(step) = self.test_steps.pop() {
                // `{docs}` is the app's Documents folder (where Files puts what is copied in).
                let step = step.replace("{docs}", &files::documents().to_string_lossy());
                note(&format!("test step: {step}"));
                if let Some(rest) = step.strip_prefix("run:") {
                    let (command, params) = rest.split_once(' ').unwrap_or((rest, "{}"));
                    let params = serde_json::from_str(params).unwrap_or_else(|_| serde_json::json!({}));
                    if let Err(e) = self.solve.run(command, params) {
                        note(&format!("test step failed: {e}"));
                    }
                } else if let Some(rest) = step.strip_prefix("ui:") {
                    // A control-channel call (`ui.menu`, `ui.browser`, `ui.file`), as MCP sends it.
                    let (method, params) = rest.split_once(' ').unwrap_or((rest, "{}"));
                    let params = serde_json::from_str(params).unwrap_or_else(|_| serde_json::json!({}));
                    let (request, reply) = solidcraft_control::ControlRequest::new(method, params);
                    if let Some(tx) = &self.control_tx {
                        let _ = tx.send(request);
                    }
                    self.control_replies.push(reply);
                } else if let Some(request) = step.strip_prefix("ai:") {
                    #[cfg(target_os = "ios")]
                    self.ai.test_ask(request, false);
                    let _ = request;
                } else if let Some(request) = step.strip_prefix("claude:") {
                    #[cfg(target_os = "ios")]
                    self.ai.test_ask(request, true);
                    let _ = request;
                } else if step == "ar" || step == "ar-off" {
                    #[cfg(target_os = "ios")]
                    if let Err(e) = self.ar.set_on(&mut self.solve, step == "ar") {
                        note(&format!("ar: {e}"));
                    }
                } else if let Some(at) = step.strip_prefix("ar-tap:") {
                    let mut xy = at.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
                    #[cfg(target_os = "ios")]
                    if let (Some(x), Some(y)) = (xy.next(), xy.next()) {
                        self.ar.test_tap = Some((x, y));
                    }
                    let _ = &mut xy;
                } else if step == "ar-section" || step == "ar-measure" || step == "ar-explode" || step == "ar-photo" || step == "ar-joints" || step == "ar-check" {
                    #[cfg(target_os = "ios")]
                    match step.as_str() {
                        "ar-section" => self.ar.test_section = true,
                        "ar-measure" => self.ar.test_measure = true,
                        "ar-photo" => self.ar.take_photo = true,
                        "ar-joints" => self.ar.test_joints = true,
                        "ar-check" => self.ar.test_check = true,
                        _ => self.ar.test_explode = true,
                    }
                } else if step == "ar-record" || step == "ar-record-stop" {
                    #[cfg(target_os = "ios")]
                    {
                        self.ar.test_record = Some(step == "ar-record");
                    }
                } else if step == "ar-scales" {
                    #[cfg(target_os = "ios")]
                    {
                        self.ar.scales_open = !self.ar.scales_open;
                    }
                } else if step == "ar-sizes" {
                    #[cfg(target_os = "ios")]
                    {
                        self.ar.sizes = !self.ar.sizes;
                    }
                } else if step == "ar-doubletap" {
                    #[cfg(target_os = "ios")]
                    {
                        self.ar.double_taps += 1;
                    }
                } else if let Some(at) = step.strip_prefix("ar-squeeze:") {
                    let v: Vec<f32> = at.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect();
                    #[cfg(target_os = "ios")]
                    if let [x, y] = v[..] {
                        self.ar.squeeze = Some(egui::pos2(x, y));
                    }
                    let _ = v;
                } else if let Some(at) = step.strip_prefix("ar-pull:").or_else(|| step.strip_prefix("ar-pullhold:")) {
                    #[cfg(target_os = "ios")]
                    {
                        self.ar.test_pull_hold = step.starts_with("ar-pullhold:");
                    }
                    let v: Vec<f32> = at.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).collect();
                    #[cfg(target_os = "ios")]
                    if let [x0, y0, x1, y1] = v[..] {
                        self.ar.test_pull = Some([x0, y0, x1, y1]);
                    }
                    let _ = v;
                } else if let Some(at) = step.strip_prefix("ar-pick:") {
                    let mut xy = at.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
                    #[cfg(target_os = "ios")]
                    if let (Some(x), Some(y)) = (xy.next(), xy.next()) {
                        self.ar.test_pick = Some((x, y));
                    }
                    let _ = &mut xy;
                } else if step == "ai-report" {
                    #[cfg(target_os = "ios")]
                    note(&format!("ai report: {}", self.ai.test_report()));
                } else if let Some(at) = step.strip_prefix("menu:") {
                    let mut xy = at.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
                    if let (Some(x), Some(y)) = (xy.next(), xy.next()) {
                        self.solve.tablet.open_menu(egui::pos2(x, y));
                    }
                } else {
                    self.solve.start(&step);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.control_replies.retain(|reply| match reply.try_recv() {
            Ok(response) => {
                note(&format!("test reply: {}", serde_json::to_string(&response).unwrap_or_default().chars().take(20000).collect::<String>()));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        // Saves of a file opened from iCloud Drive (or another folder) are written back to it.
        #[cfg(target_os = "ios")]
        if picker::linked() {
            let now = ctx.input(|i| i.time);
            if now - self.synced_at >= 1.0 {
                self.synced_at = now;
                for (text, error) in picker::sync() {
                    self.solve.set_status(text, error);
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        #[cfg(target_os = "ios")]
        self.ai.tick(&mut self.solve);
        self.solve.logic(ctx);
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The software keyboard's Return, as an Enter key press and release.
        #[cfg(target_os = "ios")]
        for _ in 0..keyboard::take_returns() {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: Some(egui::Key::Enter),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
        // The pencil's double tap ends the tool in hand, as Escape does; its squeeze opens the menu.
        #[cfg(target_os = "ios")]
        {
            for _ in 0..pencil::take_taps() {
                // In AR the double tap undoes, as drawing apps let it.
                if self.ar.on {
                    self.ar.double_taps += 1;
                    continue;
                }
                for pressed in [true, false] {
                    raw.events.push(egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: Some(egui::Key::Escape),
                        pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            // The squeeze opens the context menu at the tip, as a right click there does.
            // In AR it opens AR's tool palette there instead.
            if let Some(at) = pencil::take_squeeze() {
                if self.ar.on {
                    self.ar.squeeze = Some(at);
                } else {
                    self.solve.tablet.open_menu(at);
                }
            }
        }
        // Values written with the pencil (Scribble) into a number field.
        #[cfg(target_os = "ios")]
        scribble::take(_ctx, raw);
        self.solve.raw_input_hook(raw);
        // The pencil hovering just above the glass moves the pointer (tooltips, highlights). After
        // the touch sorting, which holds a pencil's pointer moves back between its strokes.
        #[cfg(target_os = "ios")]
        if pencil::take_hover(raw) && self.hover_noted < 3 {
            self.hover_noted += 1;
            note("pencil hover");
        }
        // Taps on the number pad become the text and keys they stand for.
        self.numpad.take_presses(raw);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Inside the status bar, the home indicator and any rounded corners.
        #[allow(unused_mut)]
        let mut safe = ui.ctx().content_rect();
        // A docked keyboard covers the bottom: the UI sits above it, so the field being typed in
        // is never hidden (a floating or split keyboard leaves the layout alone).
        #[cfg(target_os = "ios")]
        {
            keyboard::install(ui.ctx());
            if let Some(view) = objc2::MainThreadMarker::new().and_then(keyboard::view) {
                scribble::install(&view);
            }
            pencil::install(ui.ctx());
            let covered = (keyboard::covered() - ui.ctx().input(|i| i.safe_area_insets().0.bottom)).max(0.0);
            safe.max.y = (safe.max.y - covered).max(safe.min.y + 200.0);
        }
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(safe));
        inner.set_clip_rect(safe);
        self.solve.ui(&mut inner);
        #[cfg(target_os = "ios")]
        scribble::set_fields(ui.ctx());
        // Number fields get the app's number pad beside them (ABC on it gives the field the
        // system keyboard in its numbers layout).
        let ctx = ui.ctx().clone();
        let (_pad, _numeric) = self.numpad.show(&ctx, safe, solidcraft_ui_egui::numeric_focus(&ctx));
        // No keyboard while a pencil or finger is down: a drag in the view (an extrude's arrow)
        // focuses its value box, and a keyboard coming and going would resize the layout under
        // the drag. It comes up once the touch lifts.
        if ctx.input(|i| i.pointer.any_down()) {
            ctx.output_mut(|o| o.ime = None);
        }
        #[cfg(target_os = "ios")]
        keyboard::set_numpad(_pad);
        #[cfg(target_os = "ios")]
        keyboard::set_numeric(_numeric);
        self.files.show(ui.ctx());
        #[cfg(target_os = "ios")]
        // The AI window rides above a docked keyboard.
        #[cfg(target_os = "ios")]
        {
            let lift = (keyboard::covered() - ui.ctx().input(|i| i.safe_area_insets().0.bottom)).max(0.0);
            self.ai.show(ui.ctx(), lift);
            // While the window is up the model is framed in the space left of it; closing puts the
            // view back (unless it was moved meanwhile).
            if self.ai.is_open() != self.ai_shifted {
                self.ai_shifted = self.ai.is_open();
                if self.ai_shifted {
                    let before = self.solve.cam;
                    let mut cam = self.solve.fitted(before);
                    cam.distance *= 1.6;
                    cam.pan(-90.0, 0.0, f64::from(safe.height()));
                    self.solve.cam = cam;
                    self.ai_view = Some((before, cam));
                } else if let Some((before, after)) = self.ai_view.take() {
                    let now = self.solve.cam;
                    if now == after {
                        self.solve.cam = before;
                    }
                }
            }
        }
        // AR: a button at the bottom right of the view while it is off; the AR view over
        // everything while it is on.
        #[cfg(target_os = "ios")]
        {
            let ctx = ui.ctx().clone();
            if !self.ar.on {
                let at = safe.right_bottom() - egui::vec2(72.0, 64.0);
                solidcraft_ui_kit::container::Container::new(&ctx, solidcraft_ui_kit::container::Surface::Popup)
                    .area(egui::Id::unique("ar-button"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(at)
                    .show(&ctx, |ui| {
                        use solidcraft_ui_kit::container::BackingTooltipExt as _;
                        if solidcraft_ui_kit::button(ui, egui::RichText::new("AR").size(18.0), solidcraft_ui_kit::Button::Normal)
                            .backing_on_hover_text("See the design in the room")
                            .clicked()
                            && let Err(e) = self.ar.set_on(&mut self.solve, true)
                        {
                            self.solve.set_status(e, true);
                        }
                    });
            }
            self.ar.pencil_hover = pencil::hovering();
            self.ar.show(&ctx, frame, &mut self.solve);
        }
        let _ = frame;
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(solidcraft_ui_egui::prefs::STORAGE_KEY, self.solve.prefs());
        // eframe saves when the app goes to the background (and on its auto-save timer): the
        // design is kept for recovery too, as iPadOS may end a background app without warning.
        if self.solve.session.is_dirty()
            && let Some(autosave) = self.solve.autosave.as_mut()
            && let Err(e) = autosave.save(&self.solve.session)
        {
            note(&format!("recovery save failed: {e}"));
        }
    }
}

/// Where a failure is written: the simulator and device show no console, so a start that ends
/// at once leaves its reason here (the app's tmp folder).
fn note(text: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("solidcraft-last-error.txt")) {
        let _ = writeln!(f, "{text}");
    }
}

/// Sends `log` records (eframe, winit and wgpu report start-up failures there) to the same file.
struct FileLog;

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            note(&format!("{} {}: {}", r.level(), r.target(), r.args()));
        }
    }
    fn flush(&self) {}
}

fn main() -> eframe::Result {
    if log::set_logger(&FileLog).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|info| note(&format!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture()))));
    note("start");
    let result = run();
    match &result {
        Ok(()) => note("run_native returned Ok"),
        Err(e) => note(&format!("exit: {e}")),
    }
    result
}

fn run() -> eframe::Result {
    // The simulator's GPU (and some devices) offers less than wgpu's default limits (15 stage
    // variables, not 16): ask for the lesser of the two.
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut d = base(adapter);
            d.required_limits = d.required_limits.or_worse_values_from(&adapter.limits());
            d
        });
    }
    // Settings live in the app's Library folder (the platform has no data directory std can find).
    let library = std::env::temp_dir().parent().map(|p| p.join("Library"));
    let recovery_dir = library.as_ref().map(|l| l.join("Application Support/SolidCraft/recovery"));
    if let Some(dir) = &library {
        let _ = std::fs::create_dir_all(dir);
    }
    let options =
        eframe::NativeOptions { depth_buffer: DEPTH_BITS, wgpu_options, persistence_path: library.map(|d| d.join("app.ron")), ..Default::default() };
    eframe::run_native(
        "SolidCraft",
        options,
        Box::new(|cc| {
            note("creating app");
            solidcraft_ui_egui::fonts::setup_fonts(&cc.egui_ctx);
            // A touch screen: drop-down rows show their pins (no pointer hovers them).
            solidcraft_ui_egui::ribbon_touch(&cc.egui_ctx, true);
            let files = files::Files::default();
            let mut app = SolveApp::new(Session::default(), files.services());
            // The test hook's control channel, only when a test asks for one.
            let mut control_tx = None;
            if std::env::var_os("SOLIDCRAFT_IOS_TEST").is_some() {
                let (tx, rx) = std::sync::mpsc::channel();
                app = app.with_control(rx);
                control_tx = Some(tx);
            }
            if let Some(prefs) = cc.storage.and_then(|s| solidcraft_ui_egui::prefs::stored(|k| s.get_string(k))) {
                app.load_prefs(&prefs);
            }
            // Apple Pencil draws, fingers navigate (from the pencil's first touch on): the iPad's
            // own touch handling (`solidcraft_ui_egui::tablet`). The web app's Pencil mode and
            // touchscreen navigation stay off so the two never handle the same touch.
            app.tablet.enabled = true;
            app.preferences.pencil_mode = false;
            app.preferences.touchscreen_navigation = false;
            if let Some(rs) = &cc.wgpu_render_state {
                app.set_wgpu(rs, DEPTH_BITS, 0, rs.msaa_samples);
                solidcraft_ui_egui::gpu::install_picture_renderer(rs);
            }
            // The app opens on an empty design (no sample part).
            note("app created");
            // Autosave and crash recovery, as on the desktop: a design is kept every few minutes
            // (and whenever the app goes to the background, `save`), and one an iPadOS shutdown or
            // a crash left behind is offered on the next start. In Library, which iPadOS keeps.
            if let Some(dir) = recovery_dir.as_deref() {
                let every = if app.autosave_minutes > 0.0 { app.autosave_minutes * 60.0 } else { f64::MAX / 4.0 };
                solidcraft_ui_egui::recovery_ui::start(&mut app, dir, std::time::Duration::from_secs_f64(every.min(1e9)));
            }
            Ok(Box::new(App {
                solve: app,
                files,
                #[cfg(target_os = "ios")]
                ai: ai::State::default(),
                #[cfg(target_os = "ios")]
                ar: ar::State::default(),
                ai_shifted: false,
                ai_view: None,
                frames: 0,
                synced_at: 0.0,
                numpad: numpad::Numpad::default(),
                test_steps: Vec::new(),
                hover_noted: 0,
                test_at: 0.0,
                control_tx,
                control_replies: Vec::new(),
            }))
        }),
    )
}
