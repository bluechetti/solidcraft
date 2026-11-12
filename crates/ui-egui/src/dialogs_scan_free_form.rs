//! New Free Form (`scan.free_form`): click the four corners of a first face in order round it, or
//! with Corners Two the ends of a diagonal (a rectangle square to the view). A click lands on the
//! scan or a body's face under it, or, with nothing there, in the view's plane through the corner
//! before. As soon as the corners are in, the net fitted to the scan is drawn over the picture,
//! with how close it comes; Fit to Scan, Snap Distance, Points and Stiffness change it. OK keeps
//! it as a surface of its own and opens Edit Surface on it.
//!
//! Wrap Surface (`scan.wrap_surface`): click the middle of a tube-like part of the scan; the net
//! wrapped round it is drawn as it will be kept, with the cylinder it was laid on. Reach, Details,
//! Axis and Arc change it; OK keeps it and opens Edit Surface on it, as New Free Form does.
//!
//! Merge Surfaces (`scan.merge_surfaces`): choose the surfaces (or none, for every one); the sides
//! that will be made one are drawn over the picture. OK makes them meet and sews them together.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::free_form::{self, Fit, Fitting, Made, Merged, Wrap, Wrapped};
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit::{self, choice_row};

/// A corner's dot, in pixels across.
const DOT: f32 = 7.0;

/// The New Free Form dialog's values.
#[derive(Clone, Debug)]
pub struct NewFreeForm {
    /// 0: four corners clicked round the face; 1: two, a rectangle's diagonal.
    pub corners_mode: usize,
    /// 0 off, 1 follow the scan, 2 overall shape, 3 flat (one plane fitted to the scan).
    pub fit: usize,
    /// The snap distance (mm; 0, no limit).
    pub reach: f64,
    pub count: usize,
    pub stiffness: f64,
    /// The deviation colouring's tolerance, mm.
    pub tolerance: f64,
    /// The scan under the face is coloured by how far it is from it (on by default).
    pub colours: bool,
    clicks: Vec<Vec3>,
    /// With Four Curves: the 3D curves clicked (their sketch and id), up to four.
    curves: Vec<(u64, String)>,
    /// Their points, as OK sends them, when the face over them was made.
    shown_curves: Option<Vec<Vec<[f64; 3]>>>,
    /// The view's right and up when the rectangle's second corner was clicked.
    view: Option<(Vec3, Vec3)>,
    shown: Option<Box<Shown>>,
}

impl Default for NewFreeForm {
    fn default() -> Self {
        NewFreeForm {
            corners_mode: 0,
            fit: 1,
            reach: 0.0,
            count: 4,
            stiffness: 0.0,
            tolerance: 0.1,
            colours: true,
            clicks: Vec::new(),
            curves: Vec::new(),
            shown_curves: None,
            view: None,
            shown: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (Vec<[u64; 3]>, usize, usize, u64, usize, u64, u64),
    result: Result<Made, String>,
    /// The scan under the face, coloured by how far it is from it: for which tolerance and colours,
    /// the dots' version, and the dots; and how much of it is within the tolerance.
    shade: Option<(u64, [[u8; 3]; 3], u64, Arc<Vec<u8>>)>,
    within: Option<f64>,
    /// Surface Check on the face: where it folds, pinches or passes through itself.
    check: Option<solidcraft_engine::cmd::scan::surface::SurfaceCheck>,
}

/// The colouring's id among the clouds the viewport draws.
const SHADE_CLOUD_ID: u64 = u64::MAX - 8;

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    match id {
        "scan.free_form" => Some((Kind::ScanFreeForm(NewFreeForm::default()), Vec::new())),
        "scan.merge_surfaces" => Some((
            Kind::ScanMergeSurfaces(MergeSurfaces::default()),
            vec![crate::selection::SelInput::new("Surfaces", crate::selection::BODIES, true)],
        )),
        "scan.wrap_surface" => {
            Some((Kind::ScanWrapSurface(WrapSurface::default()), vec![crate::selection::SelInput::new("Point", crate::selection::FACES, false)]))
        }
        _ => None,
    }
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

fn needed(k: &NewFreeForm) -> usize {
    if k.corners_mode == 1 { 2 } else { 4 }
}

/// The corners mode that takes four curves.
const CURVES_MODE: usize = 2;

/// The points of the curves clicked, once there are four.
fn curve_points(app: &SolveApp, k: &NewFreeForm) -> Option<Vec<Vec<Vec3>>> {
    if k.corners_mode != CURVES_MODE || k.curves.len() != 4 {
        return None;
    }
    let state = app.session.model.state();
    k.curves
        .iter()
        .map(|(sketch, id)| {
            let wire = state.sketch(*sketch)?.sketch.wires.iter().find(|w| w.id == *id)?;
            Some(wire.pts.clone())
        })
        .collect()
}

/// The four corners, once they are all in: as clicked, or the rectangle on the diagonal clicked,
/// its sides along the view's right and up.
fn corners(k: &NewFreeForm) -> Option<[Vec3; 4]> {
    match (k.corners_mode, k.clicks.as_slice()) {
        (0, [a, b, c, d]) => Some([*a, *b, *c, *d]),
        (1, [a, c]) => {
            let (right, up) = k.view?;
            let d = *c - *a;
            let (x, y) = (right * d.dot(right), up * d.dot(up));
            Some([*a, *a + x, *a + x + y, *a + y])
        }
        _ => None,
    }
}

fn fit_mode(k: &NewFreeForm) -> Fit {
    match k.fit {
        0 => Fit::Off,
        2 => Fit::Overall,
        3 => Fit::Flat,
        _ => Fit::Follow,
    }
}

/// The fit's name as `scan.free_form` takes it.
const FIT_NAMES: [&str; 4] = ["off", "follow", "overall", "flat"];

/// Fit again when the corners, the settings or the design have changed.
fn fit(app: &SolveApp, k: &mut NewFreeForm) {
    if k.corners_mode == CURVES_MODE {
        fit_curves(app, k);
        return;
    }
    let Some(c) = corners(k) else {
        k.shown = None;
        return;
    };
    let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    let key = (c.iter().map(|p| bits(*p)).collect(), k.fit, k.count, k.reach.to_bits(), k.corners_mode, k.stiffness.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let reach = (k.reach > 0.0).then_some(k.reach);
    let fitting = Fitting { count: k.count, stiffness: k.stiffness, overall: false };
    let result = free_form::make(&app.session, None, c, fit_mode(k), fitting, reach).map_err(|e| e.to_string());
    let check = result
        .as_ref()
        .ok()
        .and_then(|m| solidcraft_engine::cmd::scan::surface::check_surface(&m.surface, solidcraft_engine::cmd::scan::surface::SURFACE_CHECK_GRID));
    k.shown = Some(Box::new(Shown { key, result, shade: None, within: None, check }));
}

/// The face whose sides follow the four curves clicked, again when they, the settings or the design change.
fn fit_curves(app: &SolveApp, k: &mut NewFreeForm) {
    let Some(curves) = curve_points(app, k) else {
        k.shown = None;
        k.shown_curves = None;
        return;
    };
    k.shown_curves = Some(curves.iter().map(|c| c.iter().map(|p| [p.x, p.y, p.z]).collect()).collect());
    let bits = |v: &Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    let key =
        (curves.iter().flatten().map(bits).collect(), k.fit, k.count, k.reach.to_bits(), k.corners_mode, k.stiffness.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let reach = (k.reach > 0.0).then_some(k.reach);
    let result = free_form::made_from_curves(&app.session, None, &curves, k.count, reach).map_err(|e| e.to_string());
    let check = result
        .as_ref()
        .ok()
        .and_then(|m| solidcraft_engine::cmd::scan::surface::check_surface(&m.surface, solidcraft_engine::cmd::scan::surface::SURFACE_CHECK_GRID));
    k.shown = Some(Box::new(Shown { key, result, shade: None, within: None, check }));
}

/// The scan under the face coloured by how far it is from it, again when the face, the tolerance
/// or the colours have changed.
fn shade(app: &SolveApp, k: &mut NewFreeForm) {
    let tolerance = k.tolerance;
    let Some(shown) = k.shown.as_mut() else { return };
    let colours = crate::scan_deviation::colours();
    if shown.shade.as_ref().is_some_and(|(t, c, _, _)| *t == tolerance.to_bits() && *c == colours) {
        return;
    }
    let Ok(made) = &shown.result else { return };
    let Some(scan) = made.scan.as_deref() else { return };
    let faces = [made.surface.clone()];
    let Ok((_, mut found)) = solidcraft_engine::cmd::scan::surface::shade(&app.session, Some(scan), &faces, "scan.free_form") else { return };
    let mut values = Vec::new();
    solidcraft_engine::cmd::scan::surface::network::shade_values(&mut found, &faces, None, &mut values);
    let dots = Arc::new(crate::dialogs_scan_surface::shade_dots(&found, &values, tolerance, colours));
    shown.shade = Some((tolerance.to_bits(), colours, crate::dialogs_scan_surface::next_shade_version(), dots));
    let counted: Vec<f32> = values.into_iter().filter(|v| v.is_finite()).collect();
    shown.within = (!counted.is_empty()).then(|| counted.iter().filter(|v| f64::from(v.abs()) <= tolerance).count() as f64 / counted.len() as f64);
}

/// The colouring of the scan under the face, as a cloud for the GPU viewport.
pub(crate) fn gpu_shade(app: &SolveApp) -> Option<solidcraft_gpu::view::GpuCloud> {
    let Some(Kind::ScanFreeForm(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    if !k.colours {
        return None;
    }
    let (_, _, version, dots) = k.shown.as_ref()?.shade.as_ref()?;
    Some(crate::dialogs_scan_surface::shade_cloud(SHADE_CLOUD_ID, *version, dots.clone()))
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut NewFreeForm) -> bool {
    let modes = [tr("ui.scan_free_form_four"), tr("ui.scan_free_form_two"), tr("ui.scan_free_form_curves")];
    let before = k.corners_mode;
    choice_row(ui, tr("ui.scan_free_form_corners"), "scan_free_form_corners", &mut k.corners_mode, &modes);
    if k.corners_mode != before {
        k.clicks.clear();
        k.curves.clear();
        k.view = None;
        k.shown = None;
    }
    let curves_mode = k.corners_mode == CURVES_MODE;
    let fits =
        [tr("ui.scan_free_form_fit_off"), tr("ui.scan_free_form_fit_follow"), tr("ui.scan_free_form_fit_overall"), tr("ui.scan_free_form_fit_flat")];
    choice_row(ui, tr("ui.scan_free_form_fit"), "scan_free_form_fit", &mut k.fit, &fits);
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    if k.fit != 0 {
        form_row(ui, tr("ui.scan_edit_snap_reach"), |ui| ui_kit::length_field(ui, &mut k.reach, &units, decimals));
        k.reach = k.reach.max(0.0);
        // A flat face is one plane: how much it bends to the scan has no meaning.
        if k.fit != 3 {
            stiffness_row(ui, &mut k.stiffness);
        }
    }
    form_row(ui, tr("ui.scan_free_form_points"), |ui| ui_kit::number_field(ui, &mut k.count, 4..=12, 0.2, ""));
    if k.fit != 0 {
        crate::dialogs_scan_surface::colours_row(ui, &mut k.colours);
        form_row(ui, tr("ui.scan_edit_tolerance"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals));
        k.tolerance = if k.tolerance.is_finite() && k.tolerance > 0.0 { k.tolerance } else { 0.1 };
    }
    fit(app, k);
    shade(app, k);
    let (have, want) = if curves_mode { (k.curves.len(), 4) } else { (k.clicks.len(), needed(k)) };
    let count = format!("{have} / {want}");
    form_row(ui, tr(if curves_mode { "ui.scan_free_form_curves_clicked" } else { "ui.scan_free_form_clicked" }), |ui| {
        ui_kit::text(ui, &count, ui_kit::Text::Cell)
    });
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_curve_undo_point"), ui_kit::Button::Enabled(have > 0))).clicked() {
        if curves_mode {
            k.curves.pop();
        } else {
            k.clicks.pop();
        }
        k.view = None;
    }
    match k.shown.as_ref().map(|s| (&s.result, s.within, s.check.as_ref())) {
        Some((Ok(made), within, check)) if made.scan.is_some() => {
            let note = crate::i18n::format(
                "ui.scan_free_form_found",
                &[("met", &made.met.to_string()), ("places", &made.places.to_string()), ("rms", &crate::prefs::show_mm(app, made.rms, 1))],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_free_form_found"));
            let fit = crate::i18n::format(
                "ui.scan_edit_distance_value",
                &[("rms", &crate::prefs::show_mm(app, made.rms, 1)), ("max", &crate::prefs::show_mm(app, made.max, 1))],
            );
            ui_kit::text(ui, &format!("{}: {fit}", tr("ui.scan_edit_distance")), ui_kit::Text::Note("scan_free_form_distance"));
            if let Some(within) = within {
                let line = crate::i18n::format(
                    "ui.scan_edit_within",
                    &[("within", &format!("{:.0} %", 100.0 * within)), ("tolerance", &crate::prefs::show_mm(app, k.tolerance, 1))],
                );
                ui_kit::text(ui, &line, ui_kit::Text::Note("scan_free_form_within"));
            }
            // Surface Check on the face as it will be kept: a fold is seen before OK, not after.
            if let Some(c) = check {
                if c.folds + c.pinches + c.crossings == 0 {
                    form_row(ui, tr("ui.scan_edit_check"), |ui| ui_kit::text(ui, tr("ui.scan_edit_check_clean"), ui_kit::Text::Cell));
                } else {
                    let line = crate::dialogs_surface_check::trouble(c.folds, c.pinches, c.crossings);
                    form_row(ui, tr("ui.scan_edit_check"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Report("scan_free_form_check", true)));
                    ui_kit::text(ui, tr("ui.scan_free_form_folds_hint"), ui_kit::Text::Hint("scan_free_form_folds_hint"));
                }
            }
        }
        Some((Err(why), _, _)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_free_form_error", true));
        }
        _ => {}
    }
    let hint = if curves_mode { tr("ui.scan_free_form_curves_hint") } else { tr("ui.scan_free_form_hint") };
    ui_kit::text(ui, hint, ui_kit::Text::Hint("scan_free_form_hint"));
    false
}

/// Stiffness, 0 to 1, shown and typed as a percentage (10 is 10 %, not ten times the most).
fn stiffness_row(ui: &mut egui::Ui, stiffness: &mut f64) {
    let mut percent = 100.0 * *stiffness;
    let format = |v: f64| format!("{v:.0} %");
    let changed = form_row(ui, tr("ui.scan_free_form_stiffness"), |ui| {
        ui_kit::slider(ui, &mut percent, 0.0..=100.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    })
    .changed();
    if changed && percent.is_finite() {
        *stiffness = (percent / 100.0).clamp(0.0, 1.0);
    }
}

/// What OK runs: the face made over the corners.
pub fn commands(k: &NewFreeForm) -> Result<Vec<(String, Value)>, String> {
    if k.corners_mode == CURVES_MODE {
        let made = k.shown.as_ref().ok_or_else(|| tr("ui.scan_free_form_curves_hint").to_owned())?.result.as_ref().map_err(Clone::clone)?;
        let curves = k.shown_curves.as_ref().ok_or("still fitting")?;
        let mut p = json!({"curves": curves, "count": k.count});
        if let Some(scan) = &made.scan {
            p["body"] = json!(scan);
        }
        if k.reach > 0.0 {
            p["reach"] = json!(format!("{} mm", k.reach));
        }
        return Ok(vec![("scan.free_form".into(), p)]);
    }
    let c = corners(k).ok_or_else(|| tr("ui.scan_free_form_hint").to_owned())?;
    let made = k.shown.as_ref().ok_or("still fitting")?.result.as_ref().map_err(Clone::clone)?;
    let fit = FIT_NAMES[k.fit.min(FIT_NAMES.len() - 1)];
    let mut p = json!({
        "corners": c.iter().map(|p| [p.x, p.y, p.z]).collect::<Vec<_>>(),
        "fit": fit,
        "count": k.count,
        "stiffness": k.stiffness,
    });
    if let Some(scan) = &made.scan {
        p["body"] = json!(scan);
    }
    if k.reach > 0.0 && k.fit != 0 {
        p["reach"] = json!(format!("{} mm", k.reach));
    }
    Ok(vec![("scan.free_form".into(), p)])
}

/// Whether New Free Form is open: it colours the scan under the face, which the pointer's
/// highlight of the scan would hide.
pub(crate) fn colours_the_scan(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFreeForm(_)))
}

/// Whether a Free Form dialog is open (New Free Form, Wrap Surface, Fit Surface, Edit Surface) and
/// the scan is drawn plain under it (Preferences › Tools, on by default): the scan clicked is not
/// painted in the selection colour, which would hide the surface's lines and points and the
/// scan's colouring by distance.
pub(crate) fn plain_scan(app: &SolveApp) -> bool {
    app.preferences.free_form_plain_scan
        && matches!(
            app.dialog.as_ref().map(|d| &d.kind),
            Some(Kind::ScanFreeForm(_) | Kind::ScanWrapSurface(_) | Kind::ScanFitSurface(_) | Kind::ScanEditSurface(_))
        )
}

/// Whether the dialog takes clicks in the view as corners.
pub(crate) fn drawing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFreeForm(k)) if k.corners_mode != CURVES_MODE && k.clicks.len() < needed(k))
}

/// Whether the dialog takes clicks on 3D curves as the face's sides (Four Curves).
pub(crate) fn choosing_curves(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFreeForm(k)) if k.corners_mode == CURVES_MODE)
}

/// A click at `at` on a 3D curve: it becomes a side of the face (up to four); clicked again, it is let go.
pub(crate) fn click_curve(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::SketchCurve { sketch, id, .. } => Some((sketch, id)),
        _ => None,
    });
    let state = app.session.model.state();
    let wire = hit.filter(|(sketch, id)| state.sketch(*sketch).is_some_and(|ss| ss.sketch.wire_index(id).is_some()));
    drop(state);
    let (Some(wire), Some(Kind::ScanFreeForm(k))) = (wire, app.dialog.as_mut().map(|d| &mut d.kind)) else { return };
    if let Some(at) = k.curves.iter().position(|c| *c == wire) {
        k.curves.remove(at);
    } else if k.curves.len() < 4 {
        k.curves.push(wire);
    }
}

/// A click at `at`: a corner on what is under it, or in the view's plane through the corner before.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::Face { point, .. } => Some(point),
        _ => None,
    });
    let (right, up, back) = proj.cam.basis();
    let Some(Kind::ScanFreeForm(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else { return };
    if k.clicks.len() >= needed(k) {
        return;
    }
    let place = hit.or_else(|| {
        let last = *k.clicks.last()?;
        let (o, d) = proj.ray(at);
        let den = d.dot(back);
        (den.abs() > 1e-12).then(|| o + d * ((last - o).dot(back) / den))
    });
    let Some(place) = place else { return };
    k.clicks.push(place);
    if k.corners_mode == 1 && k.clicks.len() == 2 {
        k.view = Some((right, up));
    }
}

/// The corners and the face over them, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    draw_wrap(app, painter, proj);
    draw_merge(app, painter, proj);
    let Some(Kind::ScanFreeForm(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let t = crate::theme::Tokens::get();
    if let Some(Ok(made)) = k.shown.as_ref().map(|s| &s.result) {
        crate::dialogs_scan_surface::draw_grid(painter, proj, &made.surface, egui::Stroke::new(1.5, t.surface_accent), true);
    }
    // The curves clicked, in the accent.
    if k.corners_mode == CURVES_MODE {
        let state = app.session.model.state();
        for (sketch, id) in &k.curves {
            if let Some(wire) = state.sketch(*sketch).and_then(|ss| ss.sketch.wires.iter().find(|w| w.id == *id)) {
                let line: Vec<egui::Pos2> = wire.pts.iter().filter_map(|p| proj.to_screen(*p)).collect();
                crate::dialogs_scan_surface::cased_line(painter, line, egui::Stroke::new(3.0, t.accent));
            }
        }
        return;
    }
    let outline: Vec<Vec3> = corners(k).map_or_else(|| k.clicks.clone(), |c| c.to_vec());
    let on: Vec<egui::Pos2> = outline.iter().filter_map(|p| proj.to_screen(*p)).collect();
    if on.len() > 1 {
        let mut closed = on.clone();
        if corners(k).is_some() {
            closed.push(on[0]);
        }
        crate::dialogs_scan_surface::cased_line(painter, closed, egui::Stroke::new(1.0, t.text_dim));
    }
    for (i, p) in on.iter().enumerate() {
        crate::dialogs_scan_surface::cased_dot(painter, *p, DOT / 2.0, if i == 0 { t.accent } else { t.text });
    }
}

/// The Wrap Surface dialog's values (the point clicked is its selection input).
#[derive(Clone, Debug, Default)]
pub struct WrapSurface {
    /// How far round the point the part reaches, in mm; 0 until the scan's own default is known.
    pub reach: f64,
    /// 0 Low, 1 Medium, 2 Fine.
    pub details: usize,
    /// 0 fitted, 1 X, 2 Y, 3 Z.
    pub axis: usize,
    /// How far round the net goes, in degrees; 0 for the part's own reach.
    pub arc: f64,
    /// 0 (the net follows the scan) to 1 (it keeps nearly to its cylinder).
    pub stiffness: f64,
    shown: Option<Box<WrapShown>>,
}

#[derive(Clone, Debug)]
struct WrapShown {
    key: ([u64; 3], u64, usize, usize, u64, u64, u64),
    result: Result<Wrapped, String>,
    /// Where the net made folds, pinches and crosses itself (Surface Check on it).
    trouble: [usize; 3],
}

/// The point clicked on a scan: its scan and the place.
fn wrap_point(inputs: &[crate::selection::SelInput]) -> Option<(String, Vec3)> {
    crate::selection::items(inputs, 0).iter().find_map(|s| match s {
        solidcraft_engine::Sel::Face { body, point, .. } => Some((body.clone(), *point)),
        _ => None,
    })
}

const WRAP_AXES: [&str; 4] = ["fitted", "x", "y", "z"];
const WRAP_DETAILS: [&str; 3] = ["low", "medium", "fine"];

/// Wrap again when the point, the settings or the design have changed.
fn wrap(app: &SolveApp, k: &mut WrapSurface, inputs: &[crate::selection::SelInput]) {
    let Some((body, point)) = wrap_point(inputs) else {
        k.shown = None;
        return;
    };
    if !(k.reach > 0.0) {
        k.reach = free_form::default_wrap_reach(&app.session, Some(&body)).unwrap_or(20.0);
    }
    let key = (
        [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()],
        k.reach.to_bits(),
        k.details,
        k.axis,
        k.arc.to_bits(),
        k.stiffness.to_bits(),
        app.session.revision,
    );
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let axis = [None, Some(Vec3::X), Some(Vec3::Y), Some(Vec3::Z)][k.axis.min(3)];
    let arc = (k.arc > 0.0).then(|| k.arc.to_radians());
    let wrap = Wrap { axis, details: k.details.min(2), arc, stiffness: k.stiffness };
    let result = free_form::wrapped(&app.session, Some(&body), point, k.reach, wrap).map_err(|e| e.to_string());
    let trouble = result.as_ref().map_or([0; 3], |w| w.trouble);
    k.shown = Some(Box::new(WrapShown { key, result, trouble }));
}

/// The Wrap Surface dialog's rows (after the point input).
pub fn wrap_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut WrapSurface, inputs: &[crate::selection::SelInput]) -> bool {
    wrap(app, k, inputs);
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_wrap_reach"), |ui| ui_kit::length_field(ui, &mut k.reach, &units, decimals));
    k.reach = k.reach.max(0.0);
    let details = [tr("ui.scan_wrap_low"), tr("ui.scan_wrap_medium"), tr("ui.scan_wrap_fine")];
    choice_row(ui, tr("ui.scan_wrap_details"), "scan_wrap_details", &mut k.details, &details);
    let axes = [tr("ui.scan_wrap_fitted"), "X", "Y", "Z"];
    choice_row(ui, tr("ui.scan_wrap_axis"), "scan_wrap_axis", &mut k.axis, &axes);
    form_row(ui, tr("ui.scan_wrap_arc"), |ui| ui_kit::angle_field(ui, &mut k.arc));
    k.arc = k.arc.clamp(0.0, 360.0);
    stiffness_row(ui, &mut k.stiffness);
    match k.shown.as_ref().map(|s| (&s.result, s.trouble)) {
        Some((Ok(w), [folds, pinches, crossings])) => {
            let [nu, nv] = w.made.surface.count();
            let note = crate::i18n::format(
                "ui.scan_wrap_found",
                &[
                    ("net", &format!("{nu} × {nv}")),
                    ("radius", &crate::prefs::show_mm(app, w.cylinder.radius, 1)),
                    ("off", &crate::prefs::show_mm(app, w.off, 1)),
                    ("rms", &crate::prefs::show_mm(app, w.made.rms, 1)),
                ],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_wrap_found"));
            if w.off > 0.1 * w.cylinder.radius {
                ui_kit::text(ui, tr("ui.scan_wrap_poor"), ui_kit::Text::Report("scan_wrap_poor", false));
            }
            // The whole way round: open where its ends meet, and whether Merge can close it.
            if let Some(gap) = w.seam {
                let closes = gap <= free_form::default_gap(std::slice::from_ref(&w.made.surface));
                let id = if closes { "ui.scan_wrap_round_closes" } else { "ui.scan_wrap_round_open" };
                let line = crate::i18n::format(id, &[("gap", &crate::prefs::show_mm(app, gap, 1))]);
                ui_kit::text(ui, &line, ui_kit::Text::Report("scan_wrap_round", !closes));
            }
            // The net follows rims and ribs it is too coarse for: Stiffness smooths them out.
            if folds + pinches + crossings > 0 {
                let trouble = crate::dialogs_surface_check::trouble(folds, pinches, crossings);
                let line = crate::i18n::format("ui.scan_wrap_folds", &[("trouble", &trouble)]);
                ui_kit::text(ui, &line, ui_kit::Text::Report("scan_wrap_folds", true));
            }
        }
        Some((Err(why), _)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_wrap_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_wrap_hint"), ui_kit::Text::Hint("scan_wrap_hint"));
    false
}

/// What OK runs: the net wrapped round the point clicked.
pub fn wrap_commands(k: &WrapSurface, inputs: &[crate::selection::SelInput]) -> Result<Vec<(String, Value)>, String> {
    let (body, point) = wrap_point(inputs).ok_or_else(|| tr("ui.scan_wrap_hint").to_owned())?;
    k.shown.as_ref().ok_or("still wrapping")?.result.as_ref().map_err(Clone::clone)?;
    let mut p = json!({
        "body": body,
        "point": [point.x, point.y, point.z],
        "reach": format!("{} mm", k.reach),
        "axis": WRAP_AXES[k.axis.min(3)],
        "details": WRAP_DETAILS[k.details.min(2)],
    });
    if k.arc > 0.0 {
        p["arc"] = json!(format!("{} deg", k.arc));
    }
    if k.stiffness > 0.0 {
        p["stiffness"] = json!(k.stiffness);
    }
    Ok(vec![("scan.wrap_surface".into(), p)])
}

/// The net wrapped, over the picture, and the cylinder's axis.
pub(crate) fn draw_wrap(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanWrapSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok(w)) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = crate::theme::Tokens::get();
    crate::dialogs_scan_surface::draw_grid(painter, proj, &w.made.surface, egui::Stroke::new(1.5, t.surface_accent), true);
    let c = &w.cylinder;
    let middle = w.made.surface.eval(0.5, 0.5).unwrap_or(c.point);
    let foot = c.point + c.axis * (middle - c.point).dot(c.axis);
    let half = c.axis * (2.0 * c.radius);
    if let (Some(a), Some(b)) = (proj.to_screen(foot - half), proj.to_screen(foot + half)) {
        crate::dialogs_scan_surface::cased_segment(painter, [a, b], egui::Stroke::new(1.0, t.text_dim));
    }
}

/// The Merge Surfaces dialog's values (the surfaces chosen are its selection input).
#[derive(Clone, Debug)]
pub struct MergeSurfaces {
    /// How far apart two sides may be, in mm; 0 until the surfaces' own default is known.
    pub gap: f64,
    pub stitch: bool,
    shown: Option<Box<MergeShown>>,
}

impl Default for MergeSurfaces {
    fn default() -> Self {
        MergeSurfaces { gap: 0.0, stitch: true, shown: None }
    }
}

#[derive(Clone, Debug)]
struct MergeShown {
    key: (Vec<String>, u64, u64),
    /// The surfaces, as they will meet, and their names.
    result: Result<(Merged, Vec<String>), String>,
}

/// The bodies chosen, each once.
fn chosen_bodies(inputs: &[crate::selection::SelInput]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for s in crate::selection::items(inputs, 0) {
        if let solidcraft_engine::Sel::Body { name }
        | solidcraft_engine::Sel::Face { body: name, .. }
        | solidcraft_engine::Sel::Edge { body: name, .. } = s
            && !names.contains(name)
        {
            names.push(name.clone());
        }
    }
    names
}

/// Merge again when the choice, the gap or the design have changed.
fn merge(app: &SolveApp, k: &mut MergeSurfaces, inputs: &[crate::selection::SelInput]) {
    let names = chosen_bodies(inputs);
    let key = (names.clone(), k.gap.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let named = (!names.is_empty()).then_some(names.as_slice());
    let result = free_form::merged(&app.session, named, (k.gap > 0.0).then_some(k.gap)).map_err(|e| e.to_string()).map(|(targets, made, gap)| {
        if !(k.gap > 0.0) {
            k.gap = gap;
        }
        (made, targets.into_iter().map(|t| t.body).collect())
    });
    let key = (names, k.gap.to_bits(), app.session.revision);
    k.shown = Some(Box::new(MergeShown { key, result }));
}

/// The Merge Surfaces dialog's rows (after the surfaces input).
pub fn merge_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut MergeSurfaces, inputs: &[crate::selection::SelInput]) -> bool {
    merge(app, k, inputs);
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_merge_gap"), |ui| ui_kit::length_field(ui, &mut k.gap, &units, decimals));
    k.gap = k.gap.max(0.0);
    form_row(ui, tr("ui.scan_merge_stitch"), |ui| crate::dialogs::check(ui, &mut k.stitch));
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((made, names))) => {
            let widest = made.seams.iter().map(|m| m.gap).fold(0.0, f64::max);
            let note = crate::i18n::format(
                "ui.scan_merge_found",
                &[("seams", &made.seams.len().to_string()), ("surfaces", &names.len().to_string()), ("gap", &crate::prefs::show_mm(app, widest, 1))],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_merge_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_merge_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_merge_hint"), ui_kit::Text::Hint("scan_merge_hint"));
    false
}

/// What OK runs: the surfaces made to meet, and sewn when asked.
pub fn merge_commands(k: &MergeSurfaces) -> Result<Vec<(String, Value)>, String> {
    let (_, names) = k.shown.as_ref().ok_or("still merging")?.result.as_ref().map_err(Clone::clone)?;
    Ok(vec![("scan.merge_surfaces".into(), json!({"surfaces": names, "gap": format!("{} mm", k.gap), "stitch": k.stitch}))])
}

/// The sides that will be made one, over the picture.
fn draw_merge(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanMergeSurfaces(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((made, _))) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = crate::theme::Tokens::get();
    for surface in &made.surfaces {
        crate::dialogs_scan_surface::draw_grid(painter, proj, surface, egui::Stroke::new(1.0, t.surface_accent.gamma_multiply(0.6)), true);
    }
    for seam in &made.seams {
        let Some(surface) = made.surfaces.get(seam.a) else { continue };
        let line: Vec<egui::Pos2> = (0..=40).filter_map(|i| seam.a_side.at(surface, f64::from(i) / 40.0)).filter_map(|p| proj.to_screen(p)).collect();
        painter.add(egui::Shape::line(line, egui::Stroke::new(3.0, t.accent)));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The text painted in a frame of the whole window, the dialog's notes among it.
    fn painted_text(h: &mut crate::scenario::Harness) -> Vec<String> {
        // The harness clock goes on (a hair) so egui's input history never sees time go backwards.
        let time = h.ctx.input(|i| i.time) + 1e-4;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut output = h.ctx.run_ui(raw, |ui| h.app.ui(ui));
        output.textures_delta.clear();
        output.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect()
    }

    /// A dome scanned as a grid, `n` squares a side over 60 mm, as an STL.
    fn dome_stl(n: u32) -> String {
        let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        let at = |i: u32, j: u32| {
            let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
            [x, y, z(x, y)]
        };
        let mut stl = String::from("solid d\n");
        for i in 0..n {
            for j in 0..n {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for [x, y, z] in tri {
                        stl += &format!("vertex {x} {y} {z}\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        stl + "endsolid d\n"
    }

    /// Four corners clicked on a dome through the viewport's own input: the face fitted to it is
    /// previewed close to the scan, OK keeps it as FreeForm1 and opens Edit Surface on it.
    #[test]
    fn four_corners_clicked_on_the_scan_make_a_free_form() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.free_form");
        h.frames(2);
        assert!(
            matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFreeForm(_))),
            "opened: {:?}",
            h.app.dialog.as_ref().map(|d| d.error.clone())
        );
        let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        // Near the middle, clear of the panels round the view.
        for [x, y] in [[22.0, 22.0], [38.0, 22.0], [38.0, 38.0], [22.0, 38.0]] {
            h.step(&json!({"click": {"world": [x, y, z(x, y)]}})).unwrap();
            h.frames(2);
            assert!(h.app.dialog.is_some(), "open after the click at {x}, {y}");
        }
        let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else {
            panic!("the dialog: {:?}", h.app.dialog.as_ref().map(|d| format!("{:?}", d.kind).chars().take(80).collect::<String>()))
        };
        assert_eq!(k.clicks.len(), 4, "four corners on the scan");
        // The corners select nothing: a selected scan turns blue over the net's preview.
        assert!(h.app.session.selection.is_empty(), "{:?}", h.app.session.selection);
        let Some(Ok(made)) = k.shown.as_ref().map(|s| s.result.clone()) else { panic!("fitted: {:?}", k.shown) };
        assert!(made.rms < 0.05, "close to the dome: {}", made.rms);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.session.model.state().body("FreeForm1").is_some(), "kept");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(_))), "Edit Surface opens on it");
    }

    /// Flat in New Free Form: the same four corners clicked on the dome make a face in one plane
    /// (the dome's top is where it leans away, so it is farther from the scan than Follow the Scan
    /// is), and OK keeps it as such, sending `fit: flat`. Flat has no Stiffness row.
    #[test]
    fn a_flat_face_is_one_plane_on_the_scan() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.free_form");
        h.frames(2);
        let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        for [x, y] in [[16.0, 16.0], [44.0, 16.0], [44.0, 44.0], [16.0, 44.0]] {
            h.step(&json!({"click": {"world": [x, y, z(x, y)]}})).unwrap();
            h.frames(2);
        }
        let rms = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanFreeForm(k)) => k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).map(|m| m.rms),
            _ => None,
        };
        let follow = rms(&h).expect("fitted to follow the scan");
        if let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.fit = 3;
        }
        h.frames(3);
        let flat = rms(&h).expect("fitted flat");
        assert!(flat > follow, "flat is farther from the dome than following it: {flat} {follow}");
        let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        assert_eq!(commands(k).unwrap()[0].1["fit"], "flat");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.as_ref().is_some_and(|d| matches!(d.kind, Kind::ScanEditSurface(_))), "Edit Surface opens on it");
        let kept = solidcraft_engine::cmd::scan::surface::editable(&h.app.session, Some("FreeForm1"), "test").unwrap().surface;
        let net = solidcraft_engine::cmd::scan::surface::control_points(&kept);
        let (point, normal, _) = free_form::flat_plane(&net).unwrap();
        let off = net.iter().map(|p| (*p - point).dot(normal).abs()).fold(0.0, f64::max);
        assert!(off < 1e-9, "kept in one plane: {off}");
    }

    /// Half a tube of radius 20 along Y, 40 long, open at its ends, as an STL.
    fn half_tube_stl(n: u32) -> String {
        let at = |i: u32, j: u32| {
            let a = std::f64::consts::PI * f64::from(i) / f64::from(n);
            [20.0 * a.cos(), 40.0 * f64::from(j) / f64::from(n), 20.0 * a.sin()]
        };
        let mut stl = String::from("solid t\n");
        for i in 0..n {
            for j in 0..n {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for [x, y, z] in tri {
                        stl += &format!("vertex {x} {y} {z}\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        stl + "endsolid t\n"
    }

    /// The top of a half tube clicked through the viewport's own input: the net wrapped round it is
    /// previewed on a cylinder of its radius; OK keeps it and opens Edit Surface on it.
    #[test]
    fn a_tube_clicked_is_wrapped() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, half_tube_stl(48)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.wrap_surface");
        h.frames(2);
        h.step(&json!({"click": {"world": [0.0, 20.0, 20.0]}})).unwrap();
        h.frames(3);
        let Some(Kind::ScanWrapSurface(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Wrap Surface dialog") };
        let Some(Ok(w)) = k.shown.as_ref().map(|s| s.result.clone()) else {
            panic!("wrapped: {:?}", k.shown.as_ref().map(|s| s.result.as_ref().err()))
        };
        assert!((w.cylinder.radius - 20.0).abs() < 0.2 && w.made.rms < 0.1, "{:?} {}", w.cylinder, w.made.rms);
        // The radius, how far off the part is and the rms are lengths, not areas or volumes.
        let found = painted_text(&mut h).into_iter().find(|t| t.contains("cylinder of radius")).expect("the note on the net found");
        assert!(found.contains(" mm") && !found.contains('²') && !found.contains('³'), "{found}");
        // Stiffness is the dialog's too, and OK passes it.
        if let Some(Kind::ScanWrapSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.stiffness = 0.1;
        }
        h.frames(2);
        let d = h.app.dialog.as_ref().unwrap();
        let Kind::ScanWrapSurface(k) = &d.kind else { panic!("the dialog") };
        assert_eq!(wrap_commands(k, &d.inputs).unwrap()[0].1["stiffness"], 0.1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.session.model.state().body("FreeForm1").is_some(), "kept");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(_))), "Edit Surface opens on it");
    }

    /// Two Free Forms side by side: with none chosen the dialog finds the sides to make one; OK
    /// sews them into the first.
    #[test]
    fn merge_finds_the_sides_and_sews_them() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 50, 0], [10, 50, 0]]})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[30.5, 10, 0], [50, 10, 0], [50, 50, 0], [30.5, 50, 0]], "count": 5})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.merge_surfaces");
        h.frames(3);
        let Some(Kind::ScanMergeSurfaces(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Merge Surfaces dialog") };
        let Some(Ok((made, names))) = k.shown.as_ref().map(|s| s.result.clone()) else {
            panic!("merged: {:?}", k.shown.as_ref().map(|s| s.result.as_ref().err()))
        };
        assert_eq!((made.seams.len(), names.len()), (1, 2));
        assert!(k.gap > 0.0, "the default gap is shown");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.model.state().body("FreeForm2").is_none(), "sewn into the first");
    }

    /// New Free Form with Four Curves: four curves drawn on the dome with Curve on Scan are the
    /// face's sides; the face is made over them and lies on the scan, and OK keeps it as a Free
    /// Form. Three curves make no face yet.
    #[test]
    fn four_curves_drawn_on_the_scan_make_a_free_form() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let side = |a: [f64; 2], b: [f64; 2]| -> Vec<[f64; 3]> {
            (0..=6)
                .map(|i| {
                    let t = f64::from(i) / 6.0;
                    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 4.0]
                })
                .collect()
        };
        for (a, b) in [([10.0, 10.0], [50.0, 10.0]), ([50.0, 10.0], [50.0, 50.0]), ([50.0, 50.0], [10.0, 50.0]), ([10.0, 50.0], [10.0, 10.0])] {
            h.app.run("scan.curve_on_scan", json!({"points": side(a, b)})).unwrap();
        }
        let wires: Vec<(u64, String)> = {
            let state = h.app.session.model.state();
            h.app
                .session
                .doc
                .features
                .iter()
                .filter(|f| f.name == "Scan Curves")
                .flat_map(|f| {
                    state
                        .sketch(f.id)
                        .map(|s| s.sketch.wires.iter().filter(|w| w.pts.len() >= 2).map(|w| (f.id, w.id.clone())).collect::<Vec<_>>())
                        .unwrap_or_default()
                })
                .collect()
        };
        assert!(wires.len() >= 4, "four curves drawn: {wires:?}");
        h.app.start("scan.free_form");
        h.frames(2);
        if let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.corners_mode = CURVES_MODE;
            k.count = 6;
            k.curves = wires[..3].to_vec();
        }
        h.frames(2);
        assert!(commands_of(&h).is_err(), "three curves make no face yet");
        if let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.curves = wires[wires.len() - 4..].to_vec();
        }
        h.frames(3);
        let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        let made = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).expect("the face over the curves");
        assert!(made.rms < 0.3, "it lies on the scan: {}", made.rms);
        assert!(commands_of(&h).is_ok_and(|c| c[0].1.get("curves").is_some()), "OK sends the curves");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(
            h.app.dialog.as_ref().is_none_or(|d| matches!(d.kind, Kind::ScanEditSurface(_))),
            "{:?}",
            h.app.dialog.as_ref().map(|d| d.error.clone())
        );
        assert!(solidcraft_engine::cmd::scan::surface::editable(&h.app.session, Some("FreeForm1"), "test").is_ok(), "kept as a Free Form");
    }

    fn commands_of(h: &crate::scenario::Harness) -> Result<Vec<(String, Value)>, String> {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanFreeForm(k)) => commands(k),
            _ => Err("no dialog".into()),
        }
    }
}
