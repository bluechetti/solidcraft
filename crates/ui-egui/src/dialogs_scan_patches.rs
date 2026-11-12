//! Fit Patches (`scan.fit_patches`): click the part of the scan to cover (or select it first) and
//! choose the layout: one patch in a grid, blocks of patches, or a quad whose edges follow the
//! region's edge. The patches are fitted and drawn over the scan, with how close they come, as
//! the layout, the control points and the smoothing change; OK keeps them as a surface body.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::patches::fitted;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::scan_job;
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The layouts, in the dialog's order.
const LAYOUTS: [&str; 3] = ["grid", "blocks", "quads"];

/// The Fit Patches dialog's values (the click on the scan is its selection input).
#[derive(Clone, Debug)]
pub struct FitPatches {
    pub layout: usize,
    pub controls: usize,
    pub rows: usize,
    pub columns: usize,
    pub smoothing: f64,
    /// How far round the click the region goes, mm; 0 for the whole piece (what is selected is not
    /// bounded by it).
    pub radius: f64,
    /// The scan under the patches is coloured by how far it is from them (on by default).
    pub colours: bool,
    /// A tolerance tried here instead of the scan's own, mm (0: the scan's, see `scan.tolerance`).
    pub tolerance: f64,
    /// The scan's tolerance as changed here, kept by OK.
    keep_tolerance: Option<(String, f64)>,
    /// The fit being worked out beside the window (a large scan), for the values it was begun for.
    pending: Option<Arc<scan_job::Job<String, Computed>>>,
    shown: Option<Box<Shown>>,
}

impl Default for FitPatches {
    fn default() -> Self {
        FitPatches {
            layout: 0,
            controls: 10,
            rows: 2,
            columns: 2,
            smoothing: 0.1,
            radius: 0.0,
            colours: true,
            tolerance: 0.0,
            keep_tolerance: None,
            pending: None,
            shown: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    /// How close they come, and lines across each patch to draw.
    result: Result<(f64, f64, usize, Vec<Vec<Vec3>>, [usize; 3]), String>,
    /// The scan under the patches and how far it is from them (as Free Form's colouring is made).
    under: Option<Under>,
}

/// The scan under the patches, its distances and the coloured dots (for which tolerance and
/// colours).
#[derive(Clone, Debug)]
struct Under {
    scan: String,
    shade: solidcraft_engine::cmd::scan::surface::network::Shade,
    values: Vec<f32>,
    dots: Option<(u64, [[u8; 3]; 3], u64, Arc<Vec<u8>>)>,
}

/// The colouring's id among the clouds the viewport draws.
const SHADE_CLOUD_ID: u64 = u64::MAX - 11;

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.fit_patches").then(|| (Kind::ScanFitPatches(FitPatches::default()), vec![SelInput::new("Point", FACES, false)]))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// What `scan.fit_patches` is sent.
fn params(app: &SolveApp, k: &FitPatches, inputs: &[SelInput]) -> Option<Value> {
    let mut p = match items(inputs, 0).first() {
        Some(Sel::Face { body, point, .. }) => {
            let mut p = json!({"body": body, "point": [point.x, point.y, point.z]});
            if k.radius > 0.0 {
                p["radius"] = json!(k.radius);
            }
            p
        }
        _ => {
            let sel = solidcraft_engine::cmd::scan::select::selected(&app.session).filter(|s| s.count > 0)?;
            json!({"body": sel.scan})
        }
    };
    p["layout"] = json!(LAYOUTS[k.layout.min(2)]);
    p["controls"] = json!(k.controls);
    p["smoothing"] = json!(k.smoothing);
    if k.layout == 1 {
        p["rows"] = json!(k.rows);
        p["columns"] = json!(k.columns);
    }
    Some(p)
}

/// Fit again when the click, the values or the design have changed.
fn look(app: &SolveApp, k: &mut FitPatches, inputs: &[SelInput]) {
    let Some(p) = params(app, k, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    // On a large scan the fit is worked out beside the window and what was drawn stays until it arrives.
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done((result, under)) if job.key == key => {
                    k.pending = None;
                    k.shown = Some(Box::new(Shown { key, result, under }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = p.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let (result, under) = compute(&app.session, &p);
    k.shown = Some(Box::new(Shown { key, result, under }));
}

/// What a fit of `p` comes to, and the scan under the patches: all that looks over the scan.
type Computed = (Result<(f64, f64, usize, Vec<Vec<Vec3>>, [usize; 3]), String>, Option<Under>);

fn compute(s: &solidcraft_engine::Session, p: &Value) -> Computed {
    let mut under = None;
    let result = fitted(s, p, "scan.fit_patches").map_err(|e| e.to_string()).map(|(scan, _, _, found)| {
        let surface = solidcraft_engine::cmd::scan::surface::shade(s, Some(&scan), &found.surfaces, "scan.fit_patches");
        if let Ok((_, mut shade)) = surface {
            let mut values = Vec::new();
            solidcraft_engine::cmd::scan::surface::network::shade_values(&mut shade, &found.surfaces, None, &mut values);
            // The points beside the patches are what the colouring is of: kept with their values.
            under = Some((scan, shade, values));
        }
        // Each patch's sides and three lines across it each way.
        let mut lines = Vec::new();
        for s in &found.surfaces {
            for k in 0..=4 {
                let at = f64::from(k) / 4.0;
                let along_u: Vec<Vec3> = (0..=24).filter_map(|i| s.eval(f64::from(i) / 24.0, at).ok()).collect();
                let along_v: Vec<Vec3> = (0..=24).filter_map(|i| s.eval(at, f64::from(i) / 24.0).ok()).collect();
                lines.push(along_u);
                lines.push(along_v);
            }
        }
        (found.rms, found.max, found.surfaces.len(), lines, found.trouble)
    });
    (result, under.map(|(scan, shade, values)| Under { scan, shade, values, dots: None }))
}

/// The dialog's rows (after the click).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FitPatches, inputs: &[SelInput]) -> bool {
    let layouts = [tr("ui.scan_patches_grid"), tr("ui.scan_patches_blocks"), tr("ui.scan_patches_quads")];
    choice_row(ui, tr("ui.scan_patches_layout"), "scan_patches_layout", &mut k.layout, &layouts);
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_flatten_radius"), |ui| ui_kit::length_field(ui, &mut k.radius, &units, decimals));
    k.radius = k.radius.max(0.0);
    form_row(ui, tr("ui.scan_patches_controls"), |ui| ui_kit::number_field(ui, &mut k.controls, 4..=30, 0.2, ""));
    if k.layout == 1 {
        form_row(ui, tr("ui.scan_patches_rows"), |ui| ui_kit::number_field(ui, &mut k.rows, 1..=8, 0.1, ""));
        form_row(ui, tr("ui.scan_patches_columns"), |ui| ui_kit::number_field(ui, &mut k.columns, 1..=8, 0.1, ""));
    }
    let format = |v: f64| format!("{v:.2}");
    form_row(ui, tr("ui.scan_patches_smoothing"), |ui| {
        ui_kit::slider(ui, &mut k.smoothing, 0.0..=1.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    look(app, k, inputs);
    if k.shown.as_ref().is_some_and(|s| s.under.is_some()) {
        crate::dialogs_scan_surface::colours_row(ui, &mut k.colours);
        let scan = k.shown.as_ref().and_then(|s| s.under.as_ref()).map(|u| u.scan.clone());
        k.keep_tolerance = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.tolerance).keep;
    }
    colour(app, k);
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_patches_refining");
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((rms, max, count, _, trouble))) => {
            let line = crate::i18n::format(
                "ui.scan_patches_found",
                &[("patches", &count.to_string()), ("rms", &format!("{rms:.3}")), ("max", &format!("{max:.3}"))],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_patches_found"));
            if let Some((stats, tolerance)) = fidelity(app, k) {
                crate::scan_fidelity::readout(app, ui, "scan_patches_fidelity", &stats, tolerance);
            }
            crate::dialogs_surface_check::fold_warning(ui, *trouble);
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_patches_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_patches_hint"), ui_kit::Text::Hint("scan_patches_hint"));
        }
    }
    false
}

/// The colouring of the scan under the patches made again when the tolerance or the colours have
/// changed.
fn colour(app: &SolveApp, k: &mut FitPatches) {
    let colours = crate::scan_deviation::colours();
    let Some(under) = k.shown.as_mut().and_then(|s| s.under.as_mut()) else { return };
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.tolerance);
    if under.dots.as_ref().is_some_and(|(t, c, _, _)| *t == tolerance.to_bits() && *c == colours) {
        return;
    }
    let dots = Arc::new(crate::dialogs_scan_surface::shade_dots(&under.shade, &under.values, tolerance, colours));
    under.dots = Some((tolerance.to_bits(), colours, crate::dialogs_scan_surface::next_shade_version(), dots));
}

/// What the colouring of the scan under the patches adds up to, and its tolerance, for the
/// viewport's key and the dialog's line.
pub(crate) fn fidelity(app: &SolveApp, k: &FitPatches) -> Option<(crate::scan_fidelity::Stats, f64)> {
    if !k.colours {
        return None;
    }
    let under = k.shown.as_ref()?.under.as_ref()?;
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.tolerance);
    Some((crate::scan_fidelity::stats(&under.values, tolerance)?, tolerance))
}

/// The colouring of the scan under the patches, as a cloud for the GPU viewport.
pub(crate) fn gpu_shade(app: &SolveApp) -> Option<solidcraft_gpu::view::GpuCloud> {
    let Some(Kind::ScanFitPatches(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    if !k.colours {
        return None;
    }
    let (_, _, version, dots) = k.shown.as_ref()?.under.as_ref()?.dots.as_ref()?;
    Some(crate::dialogs_scan_surface::shade_cloud(SHADE_CLOUD_ID, *version, dots.clone()))
}

/// What OK runs: the patches, and the scan's tolerance if it was changed here.
pub fn commands(app: &SolveApp, k: &FitPatches, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(app, k, inputs).ok_or_else(|| tr("ui.scan_patches_hint").to_owned())?;
    k.shown.as_ref().ok_or("still fitting")?.result.as_ref().map_err(Clone::clone)?;
    let mut all = Vec::new();
    all.extend(crate::scan_fidelity::keep(k.keep_tolerance.as_ref()));
    all.push(("scan.fit_patches".into(), p));
    Ok(all)
}

/// The patches' lines over the picture while the dialog is open.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanFitPatches(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, _, _, lines, _))) = k.shown.as_ref().map(|s| &s.result) else { return };
    let stroke = egui::Stroke::new(1.0, crate::theme::Tokens::get().accent);
    for line in lines {
        let pts: Vec<egui::Pos2> = line.iter().filter_map(|q| proj.to_screen(*q)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn bent_stl() -> String {
        let at = |i: u32, j: u32| {
            let a = f64::from(i) / 20.0 * 2.1 - 1.05;
            [20.0 * a.sin(), f64::from(j) * 1.5, 20.0 * (1.0 - a.cos())]
        };
        let mut out = String::from("solid s\n");
        for j in 0..20 {
            for i in 0..20 {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    out.push_str("facet normal 0 0 0\nouter loop\n");
                    for [x, y, z] in tri {
                        out.push_str(&format!("vertex {x} {y} {z}\n"));
                    }
                    out.push_str("endloop\nendfacet\n");
                }
            }
        }
        out.push_str("endsolid s\n");
        out
    }

    /// On a large scan the patches are fitted beside the window and arrive as they would have here.
    #[test]
    fn on_a_large_scan_the_patches_are_fitted_beside_the_window() {
        crate::scan_job::force(true);
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("bent.stl");
        std::fs::write(&path, bent_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.fit_patches");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Face { body: scan, index: 0, point: Vec3::new(0.0, 15.0, 0.0) });
        }
        h.frames(1);
        let under_way = matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFitPatches(k)) if k.pending.is_some());
        for _ in 0..500 {
            h.frames(1);
            if matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanFitPatches(k)) if k.shown.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        crate::scan_job::force(false);
        let Some(Kind::ScanFitPatches(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fit Patches dialog") };
        assert!(under_way, "begun beside the window");
        assert!(
            k.shown.as_ref().is_some_and(|s| s.result.is_ok() && s.under.is_some()),
            "fitted and measured: {:?}",
            k.shown.as_ref().map(|s| &s.result)
        );
    }

    /// A click on a curving sheet, Blocks chosen: the blocks are fitted and drawn before anything
    /// changes; OK keeps them as one surface body of four faces.
    #[test]
    fn blocks_are_fitted_on_a_click_and_ok_keeps_them() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("bent.stl");
        std::fs::write(&path, bent_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.fit_patches");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Face { body: scan, index: 0, point: Vec3::new(0.0, 15.0, 0.0) });
            if let Kind::ScanFitPatches(k) = &mut d.kind {
                k.layout = 1;
                k.controls = 8;
            }
        }
        h.frames(3);
        let Some(Kind::ScanFitPatches(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fit Patches dialog") };
        let found = k.shown.as_ref().map(|s| s.result.clone()).expect("fitted");
        assert_eq!(found.as_ref().map(|r| r.2), Ok(4), "{:?}", found.as_ref().err());
        // The scan under the patches is coloured and the one line of fidelity says how close they
        // come, against the scan's own tolerance (0.1 mm until one is set).
        let (stats, tolerance) = fidelity(&h.app, k).expect("the scan under the patches measured");
        assert!(stats.count > 100 && tolerance == 0.1 && stats.rms < 1.0, "{stats:?} {tolerance}");
        assert!(gpu_shade(&h.app).is_some_and(|c| !c.dots.is_empty()), "the scan coloured");
        assert!(crate::scan_fidelity::legend(&h.app).is_some(), "and its key");
        // A tolerance set here is the scan's: kept by OK, so the next tool opens with it.
        if let Some(Kind::ScanFitPatches(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.tolerance = 0.5;
        }
        h.frames(2);
        let Some(Kind::ScanFitPatches(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fit Patches dialog") };
        assert_eq!(fidelity(&h.app, k).map(|(_, t)| t), Some(0.5));
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.model.state().body("Patches1").map(|b| b.body.face_count()), Some(4));
        assert_eq!(h.app.session.doc.scan_tolerances.values().copied().collect::<Vec<_>>(), vec![0.5], "the scan keeps it");
    }
}
