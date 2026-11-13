//! Curve from Selection Edge (`scan.curve_from_selection`): the outline of what is selected on the
//! scan, a staircase of triangle sides, smoothed over Smooth Over (the shrinking taken back), laid
//! on the scan and kept as closed 3D sketch splines within Within: the outer outline alone, or
//! with Every Edge each hole's too. The smoothed outlines are drawn over the picture with their
//! fit points; OK keeps them in one undo step.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::curve as engine_curve;
use solidcraft_engine::cmd::scan::select as engine_select;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit;

/// The Curve from Selection Edge dialog's values.
#[derive(Clone, Debug, Default)]
pub struct EdgeCurve {
    pub every: bool,
    /// How far it is smoothed, in mm: none until the scan's typical triangle side gives it one.
    pub smooth: Option<f64>,
    /// How far each curve may stray, in mm: none until the scan gives its own.
    pub tolerance: Option<f64>,
    shown: Option<Shown>,
}

/// Each outline laid on the scan with its curve's fit points and how far it strays; how many were
/// left out.
type Lines = (String, Vec<(Vec<Vec3>, Vec<Vec3>, f64)>, usize);

#[derive(Clone, Debug)]
struct Shown {
    key: (bool, u64, u64, u64, usize),
    result: Result<Lines, String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.curve_from_selection").then(|| (Kind::ScanEdgeCurve(EdgeCurve::default()), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// Lay the outlines again when a value, the design or the selection has changed.
fn lay(app: &SolveApp, k: &mut EdgeCurve) {
    let selection = engine_select::selected(&app.session);
    if k.smooth.is_none()
        && let Some(s) = &selection
        && let Ok(t) = engine_select::topology(&app.session, &s.scan, "scan.curve_from_selection")
    {
        k.smooth = Some(2.0 * t.edge);
    }
    if k.tolerance.is_none()
        && let Some(s) = &selection
    {
        k.tolerance = engine_curve::default_tolerance(&app.session, Some(&s.scan)).ok();
    }
    let Some(tolerance) = k.tolerance else { return };
    let at = selection.as_ref().map_or(0, |s| std::sync::Arc::as_ptr(s) as usize);
    let key = (k.every, k.smooth.unwrap_or(0.0).to_bits(), tolerance.to_bits(), app.session.revision, at);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = engine_select::edge_preview(&app.session, k.every, k.smooth, tolerance)
        .map(|(scan, lines, skipped)| (scan, lines.into_iter().map(|(path, f)| (path, f.fit, f.max)).collect(), skipped))
        .map_err(|e| e.to_string());
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EdgeCurve) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    lay(app, k);
    form_row(ui, tr("ui.scan_edge_every"), |ui| check(ui, &mut k.every));
    if let Some(smooth) = &mut k.smooth {
        form_row(ui, tr("ui.scan_edge_smooth"), |ui| ui_kit::length_field(ui, smooth, &units, decimals.max(2)));
        *smooth = smooth.clamp(1e-3, 1e3);
    }
    if let Some(tolerance) = &mut k.tolerance {
        form_row(ui, tr("ui.scan_curve_within"), |ui| ui_kit::length_field(ui, tolerance, &units, decimals.max(3)));
        *tolerance = tolerance.clamp(1e-4, 10.0);
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((_, lines, skipped))) => {
            let max = lines.iter().map(|l| l.2).fold(0.0, f64::max);
            let note = solidcraft_ui_text::format(
                "ui.scan_edge_found",
                &[("curves", &lines.len().to_string()), ("max", &crate::prefs::show_mm(app, max, 1))],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_edge_found"));
            if *skipped > 0 {
                let note = solidcraft_ui_text::format("ui.scan_edge_skipped", &[("skipped", &skipped.to_string())]);
                ui_kit::text(ui, &note, ui_kit::Text::Note("scan_edge_skipped"));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_edge_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_edge_need"), ui_kit::Text::Report("scan_edge_error", true));
        }
    }
    ui_kit::text(ui, tr("ui.scan_edge_hint"), ui_kit::Text::Hint("scan_edge_hint"));
    false
}

/// What OK runs: the curves kept.
pub fn commands(k: &EdgeCurve) -> Result<Vec<(String, Value)>, String> {
    let shown = k.shown.as_ref().ok_or(tr("ui.scan_edge_need"))?;
    shown.result.as_ref().map_err(Clone::clone)?;
    let mut p = json!({"every": k.every});
    if let Some(tolerance) = k.tolerance {
        p["tolerance"] = json!(format!("{tolerance} mm"));
    }
    if let Some(smooth) = k.smooth {
        p["smooth"] = json!(format!("{smooth} mm"));
    }
    Ok(vec![("scan.curve_from_selection".into(), p)])
}

/// The smoothed outlines and their fit points over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanEdgeCurve(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, lines, _))) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    for (path, fit, _) in lines {
        let line: Vec<egui::Pos2> = path.iter().filter_map(|p| proj.to_screen(*p)).collect();
        painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, t.sketch_accent)));
        for p in fit.iter().filter_map(|p| proj.to_screen(*p)) {
            painter.circle_filled(p, 2.0, t.sketch_accent);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// With the top of a box selected, the dialog lays one closed curve round it; OK keeps it in
    /// the 3D sketch Scan Curves, one undo step.
    #[test]
    fn the_edge_of_a_selected_top_is_previewed_and_kept() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid b\n");
        let n = 20;
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let at = |di: u32, dj: u32| {
                        let mut p = [0.0; 3];
                        p[axis] = if high { 20.0 } else { 0.0 };
                        p[u] = 20.0 * f64::from(i + di) / f64::from(n);
                        p[v] = 20.0 * f64::from(j + dj) / f64::from(n);
                        p
                    };
                    let quad = [at(0, 0), at(1, 0), at(1, 1), at(0, 1)];
                    for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                        let tri = if high { tri } else { [tri[0], tri[2], tri[1]] };
                        stl += "facet normal 0 0 0\nouter loop\n";
                        for [x, y, z] in tri {
                            stl += &format!("vertex {x} {y} {z}\n");
                        }
                        stl += "endloop\nendfacet\n";
                    }
                }
            }
        }
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid b\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.select", json!({"region": {"point": [10, 10, 20]}})).unwrap();
        h.app.start("scan.curve_from_selection");
        h.frames(2);
        let Some(Kind::ScanEdgeCurve(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        let Some(Ok((_, lines, _))) = k.shown.as_ref().map(|s| &s.result) else { panic!("the preview: {:?}", k.shown) };
        assert_eq!(lines.len(), 1);
        assert!(lines[0].0.iter().all(|p| (p.z - 20.0).abs() < 1e-6), "round the top, on it");
        let before = h.app.session.doc.features.len();
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == engine_curve::SKETCH_NAME), "in Scan Curves");
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.features.len(), before, "one undo step");
    }
}
