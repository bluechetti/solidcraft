//! Fit Patches (`scan.fit_patches`): click the part of the scan to cover (or select it first) and
//! choose the layout: one patch in a grid, blocks of patches, or a quad whose edges follow the
//! region's edge. The patches are fitted and drawn over the scan, with how close they come, as
//! the layout, the control points and the smoothing change; OK keeps them as a surface body.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::patches::fitted;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
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
    shown: Option<Box<Shown>>,
}

impl Default for FitPatches {
    fn default() -> Self {
        FitPatches { layout: 0, controls: 10, rows: 2, columns: 2, smoothing: 0.1, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    /// How close they come, and lines across each patch to draw.
    result: Result<(f64, f64, usize, Vec<Vec<Vec3>>), String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.fit_patches").then(|| (Kind::ScanFitPatches(FitPatches::default()), vec![SelInput::new("Point", FACES, false)]))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// What `scan.fit_patches` is sent.
fn params(app: &SolveApp, k: &FitPatches, inputs: &[SelInput]) -> Option<Value> {
    let mut p = match items(inputs, 0).first() {
        Some(Sel::Face { body, point, .. }) => json!({"body": body, "point": [point.x, point.y, point.z]}),
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
        return;
    };
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = fitted(&app.session, &p, "scan.fit_patches").map_err(|e| e.to_string()).map(|(_, _, _, found)| {
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
        (found.rms, found.max, found.surfaces.len(), lines)
    });
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The dialog's rows (after the click).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FitPatches, inputs: &[SelInput]) -> bool {
    let layouts = [tr("ui.scan_patches_grid"), tr("ui.scan_patches_blocks"), tr("ui.scan_patches_quads")];
    choice_row(ui, tr("ui.scan_patches_layout"), "scan_patches_layout", &mut k.layout, &layouts);
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
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((rms, max, count, _))) => {
            let line = crate::i18n::format(
                "ui.scan_patches_found",
                &[("patches", &count.to_string()), ("rms", &format!("{rms:.3}")), ("max", &format!("{max:.3}"))],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_patches_found"));
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

/// What OK runs.
pub fn commands(app: &SolveApp, k: &FitPatches, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(app, k, inputs).ok_or_else(|| tr("ui.scan_patches_hint").to_owned())?;
    k.shown.as_ref().ok_or("still fitting")?.result.as_ref().map_err(Clone::clone)?;
    Ok(vec![("scan.fit_patches".into(), p)])
}

/// The patches' lines over the picture while the dialog is open.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanFitPatches(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, _, _, lines))) = k.shown.as_ref().map(|s| &s.result) else { return };
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
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.model.state().body("Patches1").map(|b| b.body.face_count()), Some(4));
    }
}
