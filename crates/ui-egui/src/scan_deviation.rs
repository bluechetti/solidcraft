//! The scan deviation map (`scan.deviation`) in the viewport: in a shaded view the scan's own
//! triangles are painted, each point by its distance from the model ([`crate::scan_paint`]: 4
//! bytes a point on the GPU, so a scan of a hundred million triangles is painted whole). In
//! wireframe the scan's triangles are painted over it through the point-cloud path instead, each
//! coloured by its corners' distance; a scan of more than [`PAINT_LIMIT`] triangles is then drawn
//! as one dot per point. Either is made once per map and costs nothing to draw again when the
//! pointer moves.
//!
//! The colours are the theme's analysis colours, so they follow the colour vision setting: within
//! the tolerance the draft analysis's good colour, above the model its bad colour and below it
//! the curvature map's deep end, deepening to the limit.
//!
//! The largest distance above and the largest below are labelled on the scan, so are the places
//! the user pins (Pin Deviation: a click on the scan), and the pointer over the scan reads out the
//! distance under it.

use std::cell::RefCell;
use std::sync::Arc;

use solidcraft_engine::cmd::scan::DeviationMap;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;

/// The map's id among the clouds the viewport draws: far above any point cloud's.
const CLOUD_ID: u64 = u64::MAX - 1;

/// A dot's size on screen, in pixels: dense enough on a scan to read as a coloured surface.
const DOT_PIXELS: f32 = 3.0;

/// The most triangles the map paints.
const PAINT_LIMIT: usize = crate::scan_dots::PAINT_MOST;

thread_local! {
    /// The colouring last built: for which map and colours, its version, and the records.
    static BUILT: RefCell<Option<(Key, u64, Painted)>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
struct Key {
    map: Arc<DeviationMap>,
    colours: Colours,
}

/// The same map (by identity: comparing millions of values every frame would cost more than
/// drawing them) in the same colours.
impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        Arc::ptr_eq(&self.map, &other.map) && self.colours == other.colours
    }
}

/// A map being measured again beside the window: for which revision of the design, and what it
/// finds (`None` where the scan or the model is gone).
type Remeasure = crate::scan_job::Job<u64, Option<Measured>>;

/// A map measured beside the window, and where each of the scan's points sits on its scale
/// (worked out there too: a scale for every point of a large scan is a frame's work and more).
struct Measured {
    map: Arc<DeviationMap>,
    scale: Arc<Vec<f32>>,
}

thread_local! {
    static REMEASURING: RefCell<Option<Remeasure>> = const { RefCell::new(None) };
    /// The first measuring of a large scan's map, beside the window: its button left the window
    /// still for half a second on a 10-million-triangle scan.
    static STARTING: RefCell<Option<Remeasure>> = const { RefCell::new(None) };
    /// The scan whose map was drawn against the default while its noise was still being read: it
    /// is measured again once the noise is in.
    static AWAITING_NOISE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Scan Deviation's button: a scan with no tolerance of its own is measured against the one its
/// tools start from (three times its noise, the preference `scan_tolerance_from_noise`), so the
/// map and the tools agree on what is within. On a large scan whose noise is still being read
/// the map is drawn against the default now and measured again once it is in. A large scan is
/// measured beside the window ([`follow`] shows the map once it is in). False when the map was
/// made here or is being made.
pub(crate) fn start(app: &mut SolveApp) -> bool {
    let Ok(scan) = solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.deviation") else {
        return true;
    };
    match crate::scan_fidelity::noise_tolerance(app, Some(&scan)) {
        Some((mm, _)) => {
            let mut p = serde_json::json!({"scan": scan, "tolerance": format!("{mm} mm")});
            look(app, &mut p);
            if crate::scan_job::beside_window(app)
                && let Some(job) = measure_beside(app, p.clone())
            {
                STARTING.with(|j| *j.borrow_mut() = Some(job));
                return false;
            }
            app.run("scan.deviation", p).is_err()
        }
        None => {
            let waiting = crate::scan_fidelity::reading(Some(&scan)).then_some(scan);
            AWAITING_NOISE.with(|w| *w.borrow_mut() = waiting);
            true
        }
    }
}

/// How the map looks, from the preferences: its range read from the scan and its colours in steps
/// (both off by default, as before).
fn look(app: &SolveApp, p: &mut serde_json::Value) {
    if app.preferences.scan_deviation_auto_range {
        p["auto_limit"] = serde_json::json!(true);
    }
    let bands = app.preferences.scan_deviation_bands.round();
    if bands >= 2.0 {
        p["bands"] = serde_json::json!(bands.min(12.0) as u32);
    }
}

/// `scan.deviation` with `params` on a copy of the design as it is now, beside the window: the
/// map as measured at this revision, which it stands for.
fn measure_beside(app: &SolveApp, params: serde_json::Value) -> Option<Remeasure> {
    let revision = app.session.revision;
    crate::scan_job::spawn(app, revision, move |s| {
        let mut copy = s.scratch();
        copy.execute("scan.deviation", &params).ok()?;
        let measured = copy.scan_deviation.as_ref()?;
        let map = Arc::new(DeviationMap { revision, ..(**measured).clone() });
        let scale = Arc::new(scale_of(&map));
        Some(Measured { map, scale })
    })
}

/// A map measured beside the window shown, painted with the scale worked out there.
fn show_measured(app: &mut SolveApp, measured: Measured) {
    let key = crate::dialogs_scan_surface::next_shade_version();
    PAINTED.with(|p| *p.borrow_mut() = Some((measured.map.clone(), colours(), key, measured.scale)));
    app.session.scan_deviation = Some(measured.map);
}

/// The first map of a large scan, once it is measured: shown as measured at the revision it was
/// measured at ([`follow`] measures it again if the design has changed since).
fn take_started(app: &mut SolveApp, ctx: &egui::Context) {
    let done = STARTING.with(|j| {
        let mut j = j.borrow_mut();
        let job = j.as_ref()?;
        match job.poll() {
            crate::scan_job::Poll::Working => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                None
            }
            crate::scan_job::Poll::Done(found) => {
                *j = None;
                found
            }
            crate::scan_job::Poll::Lost => {
                *j = None;
                None
            }
        }
    });
    if let Some(measured) = done {
        show_measured(app, measured);
        ctx.request_repaint();
    }
}

/// A map drawn against the default while the scan's noise was read: measured again against the
/// tolerance the noise gives once it is in (same scan and model, the limit following).
fn once_noise_is_read(app: &mut SolveApp, ctx: &egui::Context, map: &DeviationMap) {
    let Some(scan) = AWAITING_NOISE.with(|w| w.borrow().clone()) else {
        return;
    };
    if scan != map.scan {
        AWAITING_NOISE.with(|w| *w.borrow_mut() = None);
        return;
    }
    match crate::scan_fidelity::noise_tolerance(app, Some(&scan)) {
        Some((mm, _)) => {
            AWAITING_NOISE.with(|w| *w.borrow_mut() = None);
            if (mm - map.tolerance).abs() > 1e-12 {
                // The limit follows the new tolerance, as it did when no limit was given.
                let mut p = remeasure_params(map);
                p["tolerance"] = serde_json::json!(format!("{mm} mm"));
                if let Some(o) = p.as_object_mut() {
                    o.remove("limit");
                }
                measure_here(app, &p);
                ctx.request_repaint();
            }
        }
        None if crate::scan_fidelity::reading(Some(&scan)) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        None => AWAITING_NOISE.with(|w| *w.borrow_mut() = None),
    }
}

/// A map measured at some revision of the design, measured again at this one: same scan, same
/// model, same tolerance and limit (read again from the scan when it was read from it), the same
/// steps and the same places labelled.
fn remeasure_params(map: &DeviationMap) -> serde_json::Value {
    let mut p = serde_json::json!({ "scan": map.scan, "body": map.body, "tolerance": map.tolerance, "bands": map.bands });
    if map.auto_limit {
        p["auto_limit"] = serde_json::json!(true);
    } else {
        p["limit"] = serde_json::json!(map.limit);
    }
    if !map.pins.is_empty() {
        p["pins"] = serde_json::json!(map.pins.iter().map(|(q, _)| [q.x, q.y, q.z]).collect::<Vec<_>>());
    }
    p
}

/// Keeps a shown map true to the design: when an edit has made it stale, it is measured again
/// (beside the window on a large scan, here on a small one), so the colours, the numbers and the
/// verdict follow what the user does instead of vanishing until Scan Deviation is run again. A
/// scan or model that is gone ends it.
pub(crate) fn follow(app: &mut SolveApp, ctx: &egui::Context) {
    take_started(app, ctx);
    let Some(map) = app.session.scan_deviation.clone() else {
        REMEASURING.with(|r| *r.borrow_mut() = None);
        return;
    };
    let revision = app.session.revision;
    if map.current(&app.session) {
        REMEASURING.with(|r| *r.borrow_mut() = None);
        once_noise_is_read(app, ctx, &map);
        return;
    }
    let params = remeasure_params(&map);
    let polled = REMEASURING.with(|r| {
        let mut r = r.borrow_mut();
        let job = r.as_ref().filter(|j| j.key == revision)?;
        match job.poll() {
            crate::scan_job::Poll::Working => Some(None),
            crate::scan_job::Poll::Done(found) => {
                *r = None;
                Some(Some(found))
            }
            crate::scan_job::Poll::Lost => {
                *r = None;
                Some(Some(None))
            }
        }
    });
    match polled {
        Some(None) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Some(Some(found)) => {
            // Measured on a copy of the design as it was at `revision`, which is the design now.
            match found {
                Some(measured) => show_measured(app, measured),
                None => app.session.scan_deviation = None,
            }
            ctx.request_repaint();
        }
        None if crate::scan_job::beside_window(app) => {
            match measure_beside(app, params.clone()) {
                Some(job) => {
                    REMEASURING.with(|r| *r.borrow_mut() = Some(job));
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                // No thread to work on: here, as for a small scan.
                None => measure_here(app, &params),
            }
        }
        None => measure_here(app, &params),
    }
}

/// Measures again now; a scan or model that has gone ends the map.
fn measure_here(app: &mut SolveApp, params: &serde_json::Value) {
    if app.session.execute("scan.deviation", params).is_err() {
        app.session.scan_deviation = None;
    }
}

/// The map, if one is shown and still stands for the design.
pub(crate) fn current(app: &SolveApp) -> Option<Arc<DeviationMap>> {
    app.session.scan_deviation.clone().filter(|m| m.current(&app.session))
}

/// Whether the map is shown: the Scan Deviation button shows pressed.
pub(crate) fn shown(app: &SolveApp) -> bool {
    current(app).is_some()
}

/// The colours of the scale, the theme's: far below, below, within, above, far above.
pub(crate) type Colours = [[u8; 3]; 5];

/// The scale's colours in the theme now.
pub(crate) fn colours() -> Colours {
    let a = solidcraft_theme::Tokens::get().analysis;
    a.deviation.map(|c| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8))
}

/// A point's colour at `s` on the scale (-1 to 1, 0 within the tolerance): out of the tolerance it
/// starts well along to its side's lighter colour, so it never reads as within, and reaches the
/// far colour at the end of the scale.
pub(crate) fn colour([far_below, below, within, above, far_above]: Colours, s: f64) -> [u8; 3] {
    if s == 0.0 || !s.is_finite() {
        return within;
    }
    let (near, far) = if s > 0.0 { (above, far_above) } else { (below, far_below) };
    // 0 to 2 along within → near → far; out of the tolerance at least 0.7.
    let t = 2.0 * (0.35 + 0.65 * s.abs().min(1.0));
    let (a, b, k) = if t <= 1.0 { (within, near, t) } else { (near, far, t - 1.0) };
    std::array::from_fn(|i| (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * k).round() as u8)
}

thread_local! {
    /// The map last put on the scan's own triangles: the map (held, so its address stays its
    /// own), the colours, the painting's key and where each of the scan's points sits on the scale.
    static PAINTED: RefCell<Option<(Arc<DeviationMap>, Colours, u64, Arc<Vec<f32>>)>> = const { RefCell::new(None) };
}

/// Where each of the scan's points sits on `map`'s scale (not a number for a point the model
/// leaves out).
fn scale_of(map: &DeviationMap) -> Vec<f32> {
    map.values.iter().map(|v| map.scale(*v).map_or(f32::NAN, |s| s as f32)).collect()
}

/// The map painted on the scan's own triangles ([`crate::scan_paint`]), when one is shown: each of
/// the scan's points by where its distance sits on the scale.
pub(crate) fn painting(app: &SolveApp) -> Option<crate::scan_paint::Painting> {
    let map = current(app)?;
    let colours = colours();
    let (key, scale) = PAINTED.with(|p| {
        let mut p = p.borrow_mut();
        if let Some((m, c, key, scale)) = p.as_ref()
            && Arc::ptr_eq(m, &map)
            && *c == colours
        {
            return (*key, scale.clone());
        }
        let scale: Arc<Vec<f32>> = Arc::new(scale_of(&map));
        let key = crate::dialogs_scan_surface::next_shade_version();
        *p = Some((map.clone(), colours, key, scale.clone()));
        (key, scale)
    });
    Some(crate::scan_paint::Painting { scan: map.scan.clone(), key, scale, colours })
}

/// The map as a cloud for the GPU viewport, or nothing when none is shown or it is painted on the
/// scan's own triangles ([`crate::scan_paint::on_scan`]).
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    if crate::scan_paint::on_scan(app) {
        return None;
    }
    let map = current(app)?;
    let key = Key { map: map.clone(), colours: colours() };
    let (version, built) = BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((k, version, built)) = b.as_ref()
            && *k == key
        {
            return Some((*version, built.clone()));
        }
        let built = records(app, &map, key.colours)?;
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        *b = Some((key, version, built.clone()));
        Some((version, built))
    })?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version,
        dots: built.dots,
        segments: crate::scan_dots::none(),
        triangles: built.triangles,
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// The map's records: its triangles painted, or its points as dots.
#[derive(Clone)]
struct Painted {
    dots: Arc<Vec<u8>>,
    triangles: Arc<Vec<u8>>,
}

/// The scan's triangles painted, each by the mean of its corners the model measured (one with
/// none is not drawn); past [`PAINT_LIMIT`] triangles, one dot per measured point instead.
fn records(app: &SolveApp, map: &DeviationMap, colours: Colours) -> Option<Painted> {
    let shown = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &map.scan)?;
    let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
    if triangles.len() <= PAINT_LIMIT {
        let mut out = Vec::with_capacity(triangles.len() * crate::scan_dots::DOT_SIZE);
        let lifts = crate::scan_dots::PaintLifts::of(positions, triangles);
        for tri in triangles {
            let measured: Vec<f32> = tri.iter().filter_map(|i| map.values.get(*i as usize).copied()).filter(|v| map.scale(*v).is_some()).collect();
            if measured.is_empty() {
                continue;
            }
            let v = measured.iter().sum::<f32>() / measured.len() as f32;
            let Some(s) = map.scale(v) else { continue };
            lifts.paint(&mut out, positions, *tri, v, colour(colours, s));
        }
        return Some(Painted { dots: crate::scan_dots::none(), triangles: Arc::new(out) });
    }
    let mut out = Vec::with_capacity(positions.len() * crate::scan_dots::DOT_SIZE);
    for (p, &v) in positions.iter().zip(&map.values) {
        let Some(s) = map.scale(v) else { continue };
        let dot = Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: v, rgb: colour(colours, s) };
        cloud_dot(&mut out, &dot);
    }
    Some(Painted { dots: Arc::new(out), triangles: crate::scan_dots::none() })
}

thread_local! {
    /// Whether Pin Deviation is on: a click on the scan labels the place.
    static PINNING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The distance under the pointer: where, and how far.
    static UNDER: RefCell<Option<(egui::Pos2, f32)>> = const { RefCell::new(None) };
    /// For which map and pointer [`UNDER`] was read.
    static TRACKED: RefCell<Option<(usize, Option<(u32, u32)>)>> = const { RefCell::new(None) };
}

/// Pin Deviation's button: turns pinning on (showing the map first when it is not shown) or off.
/// False: the button runs nothing more.
pub(crate) fn toggle_pinning(app: &mut SolveApp) -> bool {
    if pinning(app) {
        PINNING.with(|p| p.set(false));
        return false;
    }
    if !shown(app) {
        let _ = start(app);
    }
    PINNING.with(|p| p.set(shown(app)));
    false
}

/// Whether a click on the scan labels it now: Pin Deviation on, with the map shown.
pub(crate) fn pinning(app: &SolveApp) -> bool {
    let on = PINNING.with(std::cell::Cell::get);
    if on && !shown(app) {
        PINNING.with(|p| p.set(false));
        return false;
    }
    on
}

/// A click while pinning: the place on the scan under it is labelled with its distance.
pub(crate) fn pin_click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let Some(map) = current(app) else { return };
    let Some(q) = scan_under(app, proj, at, &map.scan) else { return };
    let _ = app.run("scan.deviation_pin", serde_json::json!({"at": [q.x, q.y, q.z]}));
}

/// The scan's point under `at` on the screen, through any model in front of it.
fn scan_under(app: &SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, scan: &str) -> Option<solidcore_record::Vec3> {
    crate::scan_snap::scan_hit(app, proj, at, Some(scan)).map(|(_, point)| point)
}

/// Follows the pointer while the map is shown: the distance at the scan point under it, read at
/// the nearest corner of the triangle hit, for [`draw`] to show.
pub(crate) fn track(app: &mut SolveApp, proj: &crate::viewport::Proj, pointer: Option<egui::Pos2>) {
    // Read again only when the pointer or the map has moved on.
    let key = current(app).map(|m| (Arc::as_ptr(&m) as usize, pointer.map(|p| (p.x.to_bits(), p.y.to_bits()))));
    if TRACKED.with(|t| *t.borrow() == key) && key.is_some() {
        return;
    }
    TRACKED.with(|t| *t.borrow_mut() = key);
    let found = (|| {
        let map = current(app)?;
        let at = pointer?;
        let (_, q, triangle) = crate::scan_snap::scan_hit_triangle(app, proj, at, Some(&map.scan))?;
        let shown = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &map.scan)?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        use solidcraft_engine::geom::Vec3Geom as _;
        // The pick's point lies on the triangle hit; its nearest measured corner gives the value
        // (not a walk over every point of the scan each time the pointer moves).
        let mut best: Option<(f64, f32)> = None;
        for &i in triangles.get(triangle)? {
            let (Some(p), Some(&v)) = (positions.get(i as usize), map.values.get(i as usize)) else { continue };
            let d = (*p - q).len2();
            if v.is_finite() && best.is_none_or(|(e, _)| d < e) {
                best = Some((d, v));
            }
        }
        best.map(|(_, v)| (at, v))
    })();
    UNDER.with(|u| *u.borrow_mut() = found);
}

/// A distance as the labels show it: signed, in the user's units.
fn signed(app: &SolveApp, v: f32) -> String {
    let v = f64::from(v);
    let mm = crate::prefs::show_mm(app, v.abs(), 1);
    if v > 0.0 {
        format!("+{mm}")
    } else if v < 0.0 {
        format!("−{mm}")
    } else {
        mm
    }
}

/// The labels over the view while the map is shown: its largest distance above and below, the
/// places pinned, and the reading under the pointer.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{Align2, FontId, Stroke, vec2};
    let Some(map) = current(app) else { return };
    let t = solidcraft_theme::Tokens::get();
    let colours = colours();
    let tint = |v: f32| {
        let rgb = map.scale(v).map_or(colours[2], |s| colour(colours, s));
        solidcraft_theme::srgb(rgb.map(|x| f32::from(x) / 255.0))
    };
    let font = FontId::proportional(t.font_small);
    let mut tags = crate::view_tags::Tags::default();
    let mut tag = |at: egui::Pos2, text: String, v: f32| {
        let galley = painter.layout_no_wrap(text, font.clone(), t.text);
        let rect = tags.place(at, galley.size() + vec2(10.0, 6.0));
        painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, tint(v)));
        painter.circle_filled(at, 3.5, tint(v));
        painter.rect_filled(rect, 3.0, t.panel);
        painter.rect_stroke(rect, 3.0, Stroke::new(1.5, tint(v)), egui::StrokeKind::Inside);
        painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
    };
    let extremes = [("ui.scan_deviation_most_above", map.max_above), ("ui.scan_deviation_most_below", map.max_below)];
    for (id, found) in extremes {
        let Some((q, v)) = found else { continue };
        let Some(at) = proj.to_screen(q) else { continue };
        tag(at, solidcraft_ui_text::format(id, &[("value", &signed(app, v))]), v);
    }
    for &(q, v) in &map.pins {
        if let Some(at) = proj.to_screen(q) {
            tag(at, signed(app, v), v);
        }
    }
    if let Some((at, v)) = UNDER.with(|u| *u.borrow()) {
        let id = match map.side(v) {
            "above" => "ui.scan_deviation_under_above",
            "below" => "ui.scan_deviation_under_below",
            _ => "ui.scan_deviation_under_within",
        };
        let text = solidcraft_ui_text::format(id, &[("value", &signed(app, v))]);
        painter.text(at + vec2(16.0, 16.0), Align2::LEFT_TOP, text, font, t.text);
    }
}

/// The legend's rows while the map is shown: above, within and below the tolerance, and a line of
/// what it adds up to.
pub(crate) fn legend(app: &SolveApp) -> Option<(String, Vec<(egui::Color32, String, bool)>)> {
    let map = current(app)?;
    let tol = crate::prefs::show_mm(app, map.tolerance, 1);
    let summary = solidcraft_ui_text::format(
        "ui.scan_deviation_summary",
        &[("rms", &crate::prefs::show_mm(app, map.rms, 1)), ("within", &format!("{:.0} %", 100.0 * map.within)), ("tolerance", &tol)],
    );
    // Passing takes both ways (the model on the scan too): a map that fails though enough of the
    // scan is within says why in the command's own words.
    let verdict = match &map.why_not {
        Some(why) if map.within >= solidcraft_engine::cmd::scan::PASS_SHARE => why.clone(),
        _ => crate::scan_fidelity::verdict(map.within),
    };
    let summary = format!("{summary} · {verdict}");
    Some((summary, crate::scan_fidelity::key_rows(app, map.tolerance, map.limit)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A scan of a 10 mm cube measured against a solid 10 mm cube: every point is on it, the map is
    /// painted on the scan's triangles, the key reads in millimetres, and the button takes it away
    /// again.
    #[test]
    fn the_map_is_drawn_keyed_and_taken_away_by_its_button() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid c\n");
        let corner = |i: usize| [if i & 1 == 0 { 0 } else { 10 }, if i & 2 == 0 { 0 } else { 10 }, if i & 4 == 0 { 0 } else { 10 }];
        for f in [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]]
        {
            stl += "facet normal 0 0 0\nouter loop\n";
            for i in f {
                let [x, y, z] = corner(i);
                stl += &format!("vertex {x} {y} {z}\n");
            }
            stl += "endloop\nendfacet\n";
        }
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid c\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 10, "width": 10, "height": 10})).unwrap();
        h.app.run("scan.deviation", json!({"body": "Model"})).unwrap();
        assert!(shown(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.deviation"));
        let cloud = gpu_cloud(&h.app).unwrap();
        assert!(cloud.dots.is_empty(), "painted, not dots");
        assert_eq!(cloud.triangles.len(), 12 * crate::scan_dots::DOT_SIZE, "each of the cube's twelve triangles painted");
        let (summary, rows) = legend(&h.app).unwrap();
        assert!(summary.contains("mm") && !summary.contains("mm²") && !summary.contains("mm³"), "{summary}");
        assert_eq!(rows.len(), 5, "far above, above, within, below, far below");
        assert!(crate::viewport::analysis_legend_data(&h.app).is_some(), "the corner key");

        h.app.start("scan.deviation");
        assert!(!shown(&h.app) && gpu_cloud(&h.app).is_none(), "pressed again, the map goes");
    }

    /// In a shaded view on the GPU the map is painted on the scan's own triangles, every point in
    /// the colour its distance has (here all within), with no cloud over it; a new map is a new
    /// painting, so the scene is built again; in wireframe the cloud is back and the scan is not
    /// painted.
    #[test]
    fn a_shaded_view_paints_the_map_on_the_scans_own_triangles() {
        let mut h = crate::scenario::Harness::new();
        h.app.viewport.gpu = Some(solidcraft_viewport_gpu::view::GpuTarget {
            format: egui_wgpu::wgpu::TextureFormat::Rgba8Unorm,
            depth: None,
            samples: 1,
            tess: false,
            oit: false,
            edges: false,
        });
        let mut stl = String::from("solid c\n");
        let corner = |i: usize| [if i & 1 == 0 { 0 } else { 10 }, if i & 2 == 0 { 0 } else { 10 }, if i & 4 == 0 { 0 } else { 10 }];
        for f in [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]]
        {
            stl += "facet normal 0 0 0\nouter loop\n";
            for i in f {
                let [x, y, z] = corner(i);
                stl += &format!("vertex {x} {y} {z}\n");
            }
            stl += "endloop\nendfacet\n";
        }
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid c\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).unwrap().name.clone();
        h.app.run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 10, "width": 10, "height": 10})).unwrap();
        let before = crate::scan_paint::scene_key(&h.app);
        h.app.run("scan.deviation", json!({"body": "Model"})).unwrap();
        assert!(gpu_cloud(&h.app).is_none(), "no cloud over a painted scan");
        assert_ne!(crate::scan_paint::scene_key(&h.app), before, "the scene is built again with the paint");
        let scene = crate::viewport::build_scene(&h.app);
        let painted: Vec<_> = scene.instanced.iter().filter_map(|m| m.paint.as_ref().map(|p| (m, p))).collect();
        assert_eq!(painted.len(), 1, "the scan, and not the model, is painted");
        let (mesh, paint) = painted[0];
        let within = solidcraft_viewport_gpu::view::paint_colour(colours()[2]);
        assert_eq!(paint.levels[0].len(), mesh.tris.vertex_count());
        assert!(paint.levels[0].iter().all(|c| *c == within), "every point within: {:?}", paint.levels[0]);
        assert_eq!(crate::scan_paint::paintings(&h.app)[0].scan, scan);

        h.app.ui.visual_style = 2;
        assert!(gpu_cloud(&h.app).is_some_and(|c| !c.triangles.is_empty()), "in wireframe, the cloud");
        assert!(crate::viewport::build_scene(&h.app).instanced.iter().all(|m| m.paint.is_none()));
    }

    /// The button on a noisy scan with no tolerance of its own measures against three times its
    /// noise, as the scan tools do, not 0.1 mm; with the preference off, 0.1 mm as before.
    #[test]
    fn a_large_scans_map_is_measured_beside_the_window() {
        crate::scan_job::force(true);
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("noisy.stl");
        std::fs::write(&path, crate::dialogs_scan_patches::tests::noisy_sheet_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app
            .run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 60, "width": 60, "height": 5, "corner": [0, 0, -5]}))
            .unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        // The scan's noise is read first (beside the window too): the button waits for it.
        let scan = h.app.session.model.state().bodies[0].name.clone();
        for _ in 0..500 {
            if crate::scan_fidelity::noise_tolerance(&h.app, Some(&scan)).is_some() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let gate = crate::scan_job::hold();
        h.app.start("scan.deviation");
        let waiting = !shown(&h.app) && STARTING.with(|j| j.borrow().is_some());
        gate.open();
        for _ in 0..500 {
            h.frames(1);
            if shown(&h.app) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        crate::scan_job::force(false);
        assert!(waiting, "measured beside the window, not on it");
        let map = current(&h.app).expect("the map arrives");
        assert_eq!(map.revision, h.app.session.revision, "as the design is");
        assert!(painting(&h.app).is_some(), "and is painted");
    }

    #[test]
    fn the_button_measures_against_the_tolerance_the_tools_start_from() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("noisy.stl");
        std::fs::write(&path, crate::dialogs_scan_patches::tests::noisy_sheet_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app
            .run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 60, "width": 60, "height": 5, "corner": [0, 0, -5]}))
            .unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        let noise = solidcraft_engine::cmd::scan::tolerance::noise(&h.app.session, &scan).expect("the noise read");
        let expected = solidcraft_engine::cmd::scan::tolerance::for_noise(noise);
        assert!(expected > 0.15, "a noisy scan: {noise} → {expected}");

        h.app.start("scan.deviation");
        let map = current(&h.app).expect("the map");
        assert!((map.tolerance - expected).abs() < 1e-9, "{} against {expected}", map.tolerance);
        assert!(!solidcraft_engine::cmd::scan::tolerance::is_set(&h.app.session, &scan), "nothing kept by a look");

        h.app.start("scan.deviation");
        assert!(!shown(&h.app));
        h.app.preferences.scan_tolerance_from_noise = false;
        h.app.start("scan.deviation");
        assert_eq!(current(&h.app).expect("the map").tolerance, 0.1, "off: as before");
    }

    #[test]
    fn the_scale_runs_from_below_through_within_to_above() {
        let c = [[0, 0, 200], [0, 200, 200], [0, 200, 0], [200, 200, 0], [200, 0, 0]];
        assert_eq!(colour(c, 0.0), [0, 200, 0]);
        assert_eq!(colour(c, 1.0), [200, 0, 0]);
        assert_eq!(colour(c, -1.0), [0, 0, 200]);
        let just_above = colour(c, 0.01);
        assert!(just_above[0] > 120 && just_above[1] > 180, "out of the tolerance it is nearly the lighter colour, never within: {just_above:?}");
        let halfway = colour(c, 0.5);
        assert!(halfway[0] == 200 && halfway[1] < 200 && halfway[1] > 0, "then on towards the far colour: {halfway:?}");
        let just_below = colour(c, -0.01);
        assert!(just_below[2] > 120 && just_below[1] > 180, "and the same way below: {just_below:?}");
    }
}

#[cfg(test)]
mod follow_tests {
    use super::*;
    use serde_json::json;

    /// A map made, then the design edited: the map is stale and hidden until `follow` measures it
    /// again for the design as it is, with the verdict the key shows.
    #[test]
    fn an_edit_makes_the_map_follow_the_design_instead_of_vanishing() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid c\n");
        let corner = |i: usize| [if i & 1 == 0 { 0 } else { 10 }, if i & 2 == 0 { 0 } else { 10 }, if i & 4 == 0 { 0 } else { 10 }];
        for f in [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]]
        {
            stl += "facet normal 0 0 0\nouter loop\n";
            for i in f {
                let [x, y, z] = corner(i);
                stl += &format!("vertex {x} {y} {z}\n");
            }
            stl += "endloop\nendfacet\n";
        }
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid c\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 10, "width": 10, "height": 10})).unwrap();
        let reply = h.app.run("scan.deviation", json!({"body": "Model"})).unwrap();
        assert_eq!(reply["passes"], json!(true), "a scan on its model passes: {reply}");
        let ctx = egui::Context::default();

        // Any edit makes the map stale; it is hidden, then measured again for the design now.
        h.app.run("solid.box", json!({"body_name": "Other", "operation": "new", "length": 3, "width": 3, "height": 3, "x": 50})).unwrap();
        assert!(!shown(&h.app), "stale after an edit");
        follow(&mut h.app, &ctx);
        assert!(shown(&h.app), "followed the design");
        assert!(legend(&h.app).unwrap().0.contains("passes"), "{}", legend(&h.app).unwrap().0);

        // The model gone, the map goes with it.
        h.app.run("edit.undo", json!({})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        follow(&mut h.app, &ctx);
        assert!(!shown(&h.app) && h.app.session.scan_deviation.is_none(), "no model, no map");
    }
}
