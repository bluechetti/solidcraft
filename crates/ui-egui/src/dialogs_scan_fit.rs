//! Fit Shape (`scan.fit_plane`, `_cylinder`, `_sphere`, `_cone`, `_torus`): one dialog for all five.
//! Click the scan where the shape is; the region the shape was fitted to is painted on the scan,
//! and the dialog says the shape's size and how closely it fits. The region grows as far as the
//! scan lies within the Tolerance (the scan's own, shared with the other scan tools; a change is
//! kept for the scan by OK). The Area can instead be the scan round the click within a radius, or
//! what is selected on the scan (Select on Scan: brush, lasso, region), fitted as it is with no
//! click; the dialog opens on that when part of a scan is selected first. OK keeps the shape as a face trimmed to the painted region (the
//! default), as a construction plane, axis or point, or both. The shape can be changed without
//! clicking again. On a large scan the fit is worked out beside the window, the last one staying
//! painted until the new one arrives.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::fitting::{Kind as Shape, Shape as Fitted};
use solidcraft_engine::cmd::scan::{FitPreview, fit_preview_of};
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::scan_job;
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The painted region's id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 21;

/// The commands, in the dialog's order of shapes.
pub const COMMANDS: [&str; 5] = ["scan.fit_plane", "scan.fit_cylinder", "scan.fit_sphere", "scan.fit_cone", "scan.fit_torus"];
const SHAPES: [Shape; 5] = [Shape::Plane, Shape::Cylinder, Shape::Sphere, Shape::Cone, Shape::Torus];

/// Where the shape is fitted: the region grown from the click as far as the scan is that shape,
/// the scan round the click within a radius, or what is selected on the scan.
pub const AREA_GROW: usize = 0;
pub const AREA_ROUND: usize = 1;
pub const AREA_SELECTED: usize = 2;

/// A round area whose fit lies off the shape by more than this many tolerances on average is said
/// to hold more than the shape.
const POOR_ROUND: f64 = 3.0;

/// What OK keeps.
pub const KEEP_FACE: usize = 0;
pub const KEEP_CONSTRUCTION: usize = 1;
pub const KEEP_BOTH: usize = 2;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Fit Shape dialog's values (the click on the scan is its selection input).
#[derive(Clone, Debug, Default)]
pub struct FitShape {
    /// The shape fitted (index into [`COMMANDS`]).
    pub shape: usize,
    /// What OK keeps: [`KEEP_FACE`], [`KEEP_CONSTRUCTION`] or [`KEEP_BOTH`].
    pub keep: usize,
    /// [`AREA_GROW`], [`AREA_ROUND`] or [`AREA_SELECTED`].
    pub area: usize,
    /// With [`AREA_ROUND`], how far round the click, mm.
    pub radius: f64,
    /// The tolerance changed here, mm (0: the scan's).
    pub tolerance: f64,
    /// A changed tolerance OK keeps for the scan.
    keep_tolerance: Option<(String, f64)>,
    /// The tolerance in force, mm.
    within: f64,
    shown: Option<Box<Shown>>,
    pending: Option<Arc<scan_job::Job<String, Result<Painted, String>>>>,
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    /// What the fit was asked (as `scan.fit_*` takes it), for OK to ask the same.
    params: Value,
    result: Result<Painted, String>,
}

#[derive(Clone, Debug)]
struct Painted {
    found: FitPreview,
    version: u64,
    triangles: Arc<Vec<u8>>,
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let shape = COMMANDS.iter().position(|c| *c == id)?;
    let mut input = SelInput::new("Point", FACES, false);
    // A point picked on a scan before the button keeps working, as the command takes it.
    if let Some(pick) = app.session.selection.iter().rev().find(|s| matches!(s, Sel::Face { body, .. } if is_scan(app, body))) {
        input.items.push(pick.clone());
    }
    // Part of a scan selected first, and no point: that part is fitted.
    let selected = solidcraft_engine::cmd::scan::select::selected(&app.session).is_some_and(|k| k.count > 0);
    let area = if selected && input.items.is_empty() { AREA_SELECTED } else { AREA_GROW };
    Some((Kind::ScanFit(FitShape { shape, area, radius: 10.0, ..FitShape::default() }), vec![input]))
}

fn is_scan(app: &SolveApp, body: &str) -> bool {
    app.session.model.state().body(body).is_some_and(|b| b.body.is_mesh())
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The click on the scan: its scan and the point.
fn picked(app: &SolveApp, inputs: &[SelInput]) -> Option<(String, Vec3)> {
    items(inputs, 0).iter().find_map(|s| match s {
        Sel::Face { body, point, .. } if is_scan(app, body) => Some((body.clone(), *point)),
        _ => None,
    })
}

/// What the fit is asked, as `scan.fit_*` takes it: the click (and the radius round it), or what is
/// selected on the scan. `None` until there is a click, or with nothing selected.
fn params(app: &SolveApp, k: &FitShape, inputs: &[SelInput]) -> Option<Value> {
    let mut p = if k.area == AREA_SELECTED {
        let sel = solidcraft_engine::cmd::scan::select::selected(&app.session).filter(|s| s.count > 0)?;
        json!({"body": sel.scan})
    } else {
        let (body, point) = picked(app, inputs)?;
        let mut p = json!({"body": body, "point": [point.x, point.y, point.z]});
        if k.area == AREA_ROUND {
            p["radius"] = json!(k.radius.max(1e-3));
        }
        p
    };
    p["tolerance"] = json!(k.within);
    Some(p)
}

fn compute(s: &solidcraft_engine::Session, p: &Value, shape: Shape, rgb: [u8; 3]) -> Result<Painted, String> {
    let found = fit_preview_of(s, p, shape).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(found.region.len() * crate::scan_dots::DOT_SIZE);
    for &corners in &found.region {
        crate::scan_dots::cloud_painted(&mut out, corners, 0.0, rgb);
    }
    let version = VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    });
    Ok(Painted { found, version, triangles: Arc::new(out) })
}

/// Fit again when the click, the shape or the design has changed.
fn look(app: &SolveApp, k: &mut FitShape, inputs: &[SelInput]) {
    let Some(p) = params(app, k, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    let shape = SHAPES[k.shape.min(SHAPES.len() - 1)];
    // The selection is not in the design's revision: its count and first triangle stand for it.
    let selection = solidcraft_engine::cmd::scan::select::selected(&app.session).map(|s| (s.count, s.selected.iter().position(|on| *on)));
    let key = format!("{p}|{shape:?}|{}|{selection:?}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    // The colour is read here: the theme is the window thread's.
    let rgb = solidcraft_theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done(result) if job.key == key => {
                    k.pending = None;
                    k.shown = Some(Box::new(Shown { key, params: p, result }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = p.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given, shape, rgb)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let result = compute(&app.session, &p, shape, rgb);
    k.shown = Some(Box::new(Shown { key, params: p, result }));
}

/// A direction as three short numbers.
fn direction(v: Vec3) -> String {
    format!("{:.3}, {:.3}, {:.3}", v.x, v.y, v.z)
}

/// The dialog's rows (after the click).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FitShape, inputs: &[SelInput]) -> bool {
    let shapes =
        [tr("ui.scan_split_plane"), tr("ui.scan_split_cylinder"), tr("ui.scan_split_sphere"), tr("ui.scan_split_cone"), tr("ui.scan_split_torus")];
    choice_row(ui, solidcraft_ui_text::tr_label("Shape"), "scan_fit_shape", &mut k.shape, &shapes);
    let areas = [tr("ui.scan_area_grow"), tr("ui.scan_fit_area_round"), tr("ui.scan_area_selected")];
    choice_row(ui, tr("ui.scan_fit_area"), "scan_fit_shape_area", &mut k.area, &areas);
    if k.area == AREA_ROUND {
        let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
        form_row(ui, tr("ui.scan_fit_radius"), |ui| ui_kit::length_field(ui, &mut k.radius, &units, decimals));
        k.radius = k.radius.max(0.0);
    }
    let selected = solidcraft_engine::cmd::scan::select::selected(&app.session).filter(|s| s.count > 0).map(|s| s.scan.clone());
    let scan = if k.area == AREA_SELECTED { selected } else { picked(app, inputs).map(|(body, _)| body) };
    let t = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.tolerance);
    (k.within, k.keep_tolerance) = (t.mm, t.keep);
    let keeps = [tr("ui.scan_fit_keep_face"), tr("ui.scan_fit_keep_construction"), tr("ui.scan_fit_keep_both")];
    choice_row(ui, tr("ui.scan_fit_keep"), "scan_fit_keep", &mut k.keep, &keeps);
    look(app, k, inputs);
    let len = |v: f64| crate::prefs::show_mm(app, v, 1);
    let row = |ui: &mut egui::Ui, label: &str, text: &str| form_row(ui, label, |ui| ui_kit::text(ui, text, ui_kit::Text::Cell));
    let label = solidcraft_ui_text::tr_label;
    match k.shown.as_ref().map(|s| &s.result) {
        None if k.area == AREA_SELECTED => {
            ui_kit::text(ui, tr("ui.scan_area_select_first"), ui_kit::Text::Hint("scan_fit_click"));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_fit_click"), ui_kit::Text::Hint("scan_fit_click"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_fit_error", true));
        }
        Some(Ok(p)) => {
            let f = &p.found.fitted;
            let makes = match f.shape {
                Fitted::Plane { normal, .. } => {
                    row(ui, label("Direction"), &direction(normal));
                    "ui.scan_fit_makes_plane"
                }
                Fitted::Cylinder { axis, radius, .. } => {
                    row(ui, label("Diameter"), &len(2.0 * radius));
                    row(ui, label("Length"), &len(2.0 * f.half_length));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
                Fitted::Sphere { radius, .. } => {
                    row(ui, label("Diameter"), &len(2.0 * radius));
                    "ui.scan_fit_makes_point"
                }
                Fitted::Cone { axis, half_angle, .. } => {
                    row(ui, tr("ui.scan_fit_half_angle"), &format!("{:.2}°", half_angle.to_degrees()));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
                Fitted::Torus { axis, major, minor, .. } => {
                    row(ui, label("Radius"), &len(major));
                    row(ui, tr("ui.scan_fit_tube"), &len(minor));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
            };
            row(ui, label("Area"), &crate::prefs::show_mm(app, f.area, 2));
            let fit = solidcraft_ui_text::format("ui.scan_fit_off", &[("rms", &len(f.rms)), ("max", &len(f.max))]);
            row(ui, tr("ui.scan_fit_result"), &fit);
            // A round taken in as it is, lying well off the shape on average: it crossed onto the
            // faces beside the one meant (a radius wider than a hole), and the shape is a guess.
            if k.area == AREA_ROUND && f.rms > POOR_ROUND * k.within {
                ui_kit::text(ui, tr("ui.scan_fit_round_poor"), ui_kit::Text::Report("scan_fit_round_poor", true));
            }
            let makes = if k.keep == KEEP_CONSTRUCTION { tr(makes) } else { tr("ui.scan_fit_makes_face") };
            ui_kit::text(ui, makes, ui_kit::Text::Hint("scan_fit_makes"));
        }
    }
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_fit_refining");
    }
    false
}

/// What OK runs: the shape kept where it was fitted.
pub fn commands(k: &FitShape, _inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let shown = k.shown.as_ref().ok_or_else(|| tr("ui.scan_fit_click").to_owned())?;
    shown.result.as_ref().map_err(Clone::clone)?;
    let make = match k.keep {
        KEEP_FACE => "face",
        KEEP_BOTH => "both",
        _ => "construction",
    };
    // As the fit shown was asked: the click (and radius), or what is selected on the scan now.
    let mut fit = shown.params.clone();
    fit["tolerance"] = json!(format!("{} mm", k.within));
    fit["make"] = json!(make);
    let mut out: Vec<(String, Value)> = crate::scan_fidelity::keep(k.keep_tolerance.as_ref()).into_iter().collect();
    out.push((COMMANDS[k.shape.min(COMMANDS.len() - 1)].into(), fit));
    Ok(out)
}

/// The fitted region painted on the scan while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFit(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let p = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: p.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: p.triangles.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: 3.0 },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcore_record::Vec3;
    use solidcraft_engine::Sel;

    use crate::dialogs::Kind;

    /// A tube of radius `r` along Z, 24 mm long, open at its ends, inserted as a scan: its name.
    fn tube(h: &mut crate::scenario::Harness, r: f64) -> String {
        let path = h.dir.join("tube.obj");
        let (n, rings) = (48, 12);
        let mut obj = String::new();
        for j in 0..=rings {
            for i in 0..n {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                obj.push_str(&format!("v {} {} {}\n", r * a.cos(), r * a.sin(), 2.0 * f64::from(j)));
            }
        }
        for j in 0..rings {
            for i in 0..n {
                let (a, b) = (j * n + i + 1, j * n + (i + 1) % n + 1);
                let (c, d) = (a + n, b + n);
                obj.push_str(&format!("f {a} {b} {d}\nf {a} {d} {c}\n"));
            }
        }
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap()
    }

    /// Part of the tube brushed first: the dialog opens on What is selected, fits that part as it
    /// is with no click, and OK keeps it the same way.
    #[test]
    fn what_is_selected_is_fitted_without_a_click() {
        let mut h = crate::scenario::Harness::new();
        let body = tube(&mut h, 5.0);
        h.app.run("scan.select", json!({"body": body, "brush": {"points": [[5, 0, 12]], "radius": 3}})).unwrap();
        h.app.start("scan.fit_cylinder");
        h.frames(3);
        let Some(Kind::ScanFit(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fit dialog") };
        assert_eq!(k.area, super::AREA_SELECTED, "a selection first: the dialog fits it");
        let Some(Ok(p)) = k.shown.as_ref().map(|s| &s.result) else { panic!("fitted: {:?}", k.shown.as_ref().map(|s| &s.result)) };
        let solidcraft_engine::cmd::scan::fitting::Shape::Cylinder { radius, .. } = p.found.fitted.shape else { panic!("a cylinder") };
        assert!((radius - 5.0).abs() < 0.1, "radius {radius}");
        let whole = 2.0 * std::f64::consts::PI * 5.0 * 24.0;
        assert!(p.found.fitted.area < 0.3 * whole, "only the part brushed: {}", p.found.fitted.area);
        let inputs = h.app.dialog.as_ref().map(|d| d.inputs.clone()).unwrap();
        let cmds = super::commands(k, &inputs).unwrap();
        assert!(cmds[0].1.get("point").is_none(), "{}", cmds[0].1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.model.state().bodies.iter().any(|b| b.body.is_surface()), "the face is kept");
    }

    /// A tube: the Fit dialog opened from Fit Plane, its shape turned to Cylinder, a click on the
    /// wall paints the region and reads the diameter; OK keeps a face trimmed to the region, at
    /// the tolerance shown.
    #[test]
    fn a_click_on_a_tube_fits_a_cylinder_and_ok_keeps_its_axis() {
        let mut h = crate::scenario::Harness::new();
        let r = 5.0;
        let body = tube(&mut h, r);
        h.app.start("scan.fit_plane");
        let Some(d) = h.app.dialog.as_mut() else { panic!("the Fit dialog opens") };
        let Kind::ScanFit(k) = &mut d.kind else { panic!("the Fit dialog") };
        k.shape = 1;
        d.inputs[0].items.push(Sel::Face { body: body.clone(), index: 0, point: Vec3::new(r, 0.0, 12.0) });
        h.frames(3);
        let Some(Kind::ScanFit(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("still open") };
        let Some(Ok(p)) = k.shown.as_ref().map(|s| &s.result) else { panic!("fitted: {:?}", k.shown.as_ref().map(|s| &s.result)) };
        let solidcraft_engine::cmd::scan::fitting::Shape::Cylinder { radius, .. } = p.found.fitted.shape else { panic!("a cylinder") };
        assert!((radius - r).abs() < 0.1, "radius {radius}");
        assert!(!p.triangles.is_empty(), "the region is painted");
        assert!(super::gpu_marks(&h.app).is_some());
        let inputs = h.app.dialog.as_ref().map(|d| d.inputs.clone()).unwrap();
        let cmds = super::commands(k, &inputs).unwrap();
        assert_eq!(cmds[0].0, "scan.fit_cylinder");
        assert_eq!(cmds[0].1["make"], json!("face"), "a face by default");
        assert!(cmds[0].1["tolerance"].as_str().is_some_and(|t| t.ends_with(" mm")), "{}", cmds[0].1);
        let reply = h.app.run(&cmds[0].0, cmds[0].1.clone()).unwrap();
        let surface = reply["face"]["surface"].as_str().unwrap_or_default().to_owned();
        assert!(h.app.session.model.state().body(&surface).is_some_and(|b| b.body.is_surface()), "the face is kept: {reply}");
    }
}
