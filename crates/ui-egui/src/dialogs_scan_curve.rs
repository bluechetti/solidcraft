//! Curve on Scan (`scan.curve_on_scan`): click points along the scan, in order, and the smooth line
//! through them is laid on the scan and drawn as the curve will follow it. Clicking the first point
//! again (or Closed) closes it; Remove Last Point takes the last back. OK keeps it as a 3D sketch
//! spline within the tolerance of the scan, as SurfaceCraft's Draw Curve on Scan does.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::curve as engine_curve;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit;

/// How near the first point a click must be to close the curve, in pixels.
const CLOSE: f32 = 10.0;

/// A clicked point's dot, in pixels across.
const DOT: f32 = 6.0;

/// The Curve on Scan dialog's values.
#[derive(Clone, Debug)]
pub struct CurveOnScan {
    /// The scan the points are on (the first one clicked), and the points.
    scan: Option<String>,
    picks: Vec<Vec3>,
    pub closed: bool,
    /// How far the curve may stray from the line on the scan, in mm.
    pub tolerance: f64,
    shown: Option<Shown>,
}

impl Default for CurveOnScan {
    fn default() -> Self {
        CurveOnScan { scan: None, picks: Vec::new(), closed: false, tolerance: engine_curve::TOLERANCE, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (Vec<[u64; 3]>, bool, u64, u64),
    /// The line on the scan, the fit points, how far the curve strays and its length; or why not.
    result: Result<(Vec<Vec3>, Vec<Vec3>, f64, f64), String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.curve_on_scan").then(|| (Kind::ScanCurve(CurveOnScan::default()), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

fn bits(v: Vec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// Lay the line again when the points, the closing, the tolerance or the design have changed.
fn lay(app: &SolveApp, k: &mut CurveOnScan) {
    let key = (k.picks.iter().map(|p| bits(*p)).collect(), k.closed, k.tolerance.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) || k.picks.len() < 2 {
        if k.picks.len() < 2 {
            k.shown = None;
        }
        return;
    }
    let result = engine_curve::preview(&app.session, k.scan.as_deref(), &k.picks, k.closed, k.tolerance).map_err(|e| e.to_string()).map(
        |(_, laid, fitted)| {
            let length = laid.path.windows(2).map(|w| (w[1] - w[0]).len()).sum();
            (laid.path, fitted.fit, fitted.max, length)
        },
    );
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut CurveOnScan) -> bool {
    lay(app, k);
    let count = k.picks.len().to_string();
    form_row(ui, tr("ui.scan_curve_points"), |ui| ui_kit::text(ui, &count, ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_curve_closed"), |ui| check(ui, &mut k.closed));
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_curve_within"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals.max(3)));
    k.tolerance = k.tolerance.clamp(1e-4, 10.0);
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_curve_undo_point"), ui_kit::Button::Enabled(!k.picks.is_empty()))).clicked() {
        k.picks.pop();
        if k.picks.len() < 3 {
            k.closed = false;
        }
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((_, fit, max, length))) => {
            let length = crate::prefs::show_mm(app, *length, 1);
            form_row(ui, tr("ui.scan_curve_length"), |ui| ui_kit::text(ui, &length, ui_kit::Text::Cell));
            let fitted =
                crate::i18n::format("ui.scan_curve_fit_value", &[("points", &fit.len().to_string()), ("max", &crate::prefs::show_mm(app, *max, 3))]);
            form_row(ui, tr("ui.scan_curve_fit"), |ui| ui_kit::text(ui, &fitted, ui_kit::Text::Cell));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_curve_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_curve_hint"), ui_kit::Text::Hint("scan_curve_hint"));
    false
}

/// What OK runs: the curve through the points clicked.
pub fn commands(k: &CurveOnScan) -> Result<Vec<(String, Value)>, String> {
    if k.picks.len() < 2 {
        return Err(tr("ui.scan_curve_need").into());
    }
    if let Some(Err(why)) = k.shown.as_ref().map(|s| &s.result) {
        return Err(why.clone());
    }
    let points: Vec<[f64; 3]> = k.picks.iter().map(|p| [p.x, p.y, p.z]).collect();
    let mut p = json!({"points": points, "closed": k.closed, "tolerance": format!("{} mm", k.tolerance)});
    if let Some(scan) = &k.scan {
        p["body"] = json!(scan);
    }
    Ok(vec![("scan.curve_on_scan".into(), p)])
}

/// Whether the dialog takes clicks on the scan as the curve's points.
pub(crate) fn drawing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanCurve(_)))
}

/// A click at `at`: on the first point (with three or more) it closes the curve, else on the scan
/// it adds a point there.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::Face { body, point, .. } => Some((body, point)),
        _ => None,
    });
    let state = app.session.model.state();
    let hit = hit.filter(|(body, _)| state.body(body).is_some_and(|b| b.body.is_mesh()));
    drop(state);
    let Some(Kind::ScanCurve(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else { return };
    let on_first = k.picks.first().and_then(|p| proj.to_screen(*p)).is_some_and(|s| s.distance(at) <= CLOSE);
    if on_first && k.picks.len() >= 3 {
        k.closed = true;
        return;
    }
    let Some((body, point)) = hit else { return };
    // The points are on one scan: the first one clicked.
    if k.scan.as_ref().is_some_and(|s| *s != body) {
        return;
    }
    k.scan = Some(body);
    k.picks.push(point);
}

/// The line on the scan and the points clicked, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanCurve(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let t = crate::theme::Tokens::get();
    if let Some(Ok((path, fit, _, _))) = k.shown.as_ref().map(|s| &s.result) {
        let line: Vec<egui::Pos2> = path.iter().filter_map(|p| proj.to_screen(*p)).collect();
        painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, t.sketch_accent)));
        for p in fit.iter().filter_map(|p| proj.to_screen(*p)) {
            painter.circle_filled(p, DOT / 3.0, t.sketch_accent);
        }
    }
    for (i, p) in k.picks.iter().enumerate() {
        let Some(s) = proj.to_screen(*p) else { continue };
        let fill = if i == 0 { t.accent } else { t.text };
        painter.circle_filled(s, DOT / 2.0, fill);
        painter.circle_stroke(s, DOT / 2.0, egui::Stroke::new(1.0, t.panel));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A tube of radius 20 along Y, open at its ends, as an STL.
    fn tube_stl(n: u32) -> String {
        let at = |i: u32, j: u32| {
            let a = std::f64::consts::TAU * f64::from(i % n) / f64::from(n);
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

    fn kind(h: &crate::scenario::Harness) -> &CurveOnScan {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanCurve(k)) => k,
            _ => panic!("the Curve on Scan dialog"),
        }
    }

    /// Three points clicked over the top of a tube through the viewport's own input lay a line on
    /// it; OK keeps a 3D spline in `Scan Curves`, one undo step.
    #[test]
    fn points_clicked_on_the_scan_make_a_curve_on_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, tube_stl(64)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.curve_on_scan");
        h.frames(2);
        assert!(drawing(&h.app));
        // From above, across the top of the tube and along it.
        for [x, y] in [[-12.0, 8.0], [0.0, 20.0], [12.0, 32.0]] {
            let z = (400.0f64 - x * x).sqrt();
            h.step(&json!({"click": {"world": [x, y, z]}})).unwrap();
            h.frames(2);
        }
        let k = kind(&h);
        assert_eq!(k.picks.len(), 3, "three points on the scan");
        assert!(k.picks.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "on the tube: {:?}", k.picks);
        let Some(Ok((line, _, max, _))) = k.shown.as_ref().map(|s| s.result.clone()) else { panic!("the line on the scan: {:?}", k.shown) };
        assert!(line.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "the line lies on the tube");
        assert!(max <= engine_curve::TOLERANCE);

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == engine_curve::SKETCH_NAME), "kept in Scan Curves");
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(!h.app.session.doc.features.iter().any(|f| f.name == engine_curve::SKETCH_NAME), "one undo step");
    }
}
