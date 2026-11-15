//! Curve on Scan (`scan.curve_on_scan`): click points along the scan, in order, and the smooth line
//! through them is laid on the scan and drawn as the curve will follow it. Clicking the first point
//! again (or Closed) closes it; Remove Last Point takes the last back. A drag begun on the scan
//! draws a stroke instead: let go, it is followed along the scan, cut where it leaves the scan or
//! jumps to another layer, and its longest run, boiled down to the fewest points that still trace
//! it, is added as points (SurfaceCraft's stroke). OK keeps it as a 3D sketch spline within the
//! tolerance of the scan, as SurfaceCraft's Draw Curve on Scan does.

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

/// How far, in pixels, a stroke may stand off a straight line on the screen before it gets a
/// point there.
const STROKE_TOLERANCE: f32 = 2.5;

/// How long a drag has to be, in pixels, to be a stroke and not a click that moved a little.
const MINIMUM_STROKE: f32 = 8.0;

/// How far apart, in pixels, a stroke is looked at for where it is on the scan.
const SAMPLE_STEP: f32 = 4.0;

/// How many times deeper than sideways the scan may go between two neighbouring samples of a
/// stroke before they are on different layers of it.
const LAYER_JUMP: f64 = 6.0;

/// The Curve on Scan dialog's values.
#[derive(Clone, Debug)]
pub struct CurveOnScan {
    /// The scan the points are on (the first one clicked), and the points.
    scan: Option<String>,
    pub(crate) picks: Vec<Vec3>,
    pub closed: bool,
    /// How far the curve may stray from the line on the scan, in mm; 0 until the scan's own default
    /// is known (as fine as its triangles).
    pub tolerance: f64,
    shown: Option<Shown>,
    /// The stroke being drawn, on the screen, until the button is let go.
    stroke: Vec<egui::Pos2>,
}

impl Default for CurveOnScan {
    fn default() -> Self {
        CurveOnScan { scan: None, picks: Vec::new(), closed: false, tolerance: 0.0, shown: None, stroke: Vec::new() }
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
    solidcraft_ui_text::tr(id)
}

fn bits(v: Vec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// Lay the line again when the points, the closing, the tolerance or the design have changed.
fn lay(app: &SolveApp, k: &mut CurveOnScan) {
    // The scan's own tolerance, once there is one scan to take it from (the selected or only one,
    // or the one clicked).
    if !(k.tolerance > 0.0)
        && let Ok(tolerance) = engine_curve::default_tolerance(&app.session, k.scan.as_deref())
    {
        k.tolerance = tolerance;
    }
    let key = (k.picks.iter().map(|p| bits(*p)).collect(), k.closed, k.tolerance.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) || k.picks.len() < 2 || !(k.tolerance > 0.0) {
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
    // 0 (or less) goes back to the scan's own.
    k.tolerance = if k.tolerance > 0.0 { k.tolerance.clamp(1e-4, 10.0) } else { 0.0 };
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
            let fitted = solidcraft_ui_text::format(
                "ui.scan_curve_fit_value",
                &[("points", &fit.len().to_string()), ("max", &crate::prefs::show_mm(app, *max, 1))],
            );
            form_row(ui, tr("ui.scan_curve_fit"), |ui| ui_kit::text(ui, &fitted, ui_kit::Text::Cell));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_curve_error", true));
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
    let mut p = json!({"points": points, "closed": k.closed});
    if k.tolerance > 0.0 {
        p["tolerance"] = json!(format!("{} mm", k.tolerance));
    }
    if let Some(scan) = &k.scan {
        p["body"] = json!(scan);
    }
    Ok(vec![("scan.curve_on_scan".into(), p)])
}

/// Whether the dialog takes clicks on the scan as the curve's points.
pub(crate) fn drawing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanCurve(_)))
}

/// The scan under `at` on the screen, and the point of it there.
fn scan_at(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<(String, Vec3)> {
    crate::scan_snap::scan_hit(app, proj, at, None)
}

fn dialog(app: &mut SolveApp) -> Option<&mut CurveOnScan> {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(Kind::ScanCurve(k)) => Some(k),
        _ => None,
    }
}

/// A click at `at`: on the first point (with three or more) it closes the curve, else on the scan
/// it adds a point there.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = scan_at(app, proj, at);
    let Some(k) = dialog(app) else { return };
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
    k.scan = Some(body.clone());
    // Where the click lands: on a corner, edge, crease or curve point close to it.
    let point = crate::scan_snap::snapped(app, proj, &body, point);
    let Some(k) = dialog(app) else { return };
    k.picks.push(point);
}

/// A primary drag begun at `at`, now at `to`: a stroke when it began on the scan.
pub(crate) fn stroke_to(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, to: egui::Pos2) {
    if dialog(app).is_some_and(|k| k.stroke.is_empty()) {
        let on = scan_at(app, proj, at);
        let Some(k) = dialog(app) else { return };
        match on {
            Some((body, _)) if k.scan.as_ref().is_none_or(|s| *s == body) => k.stroke.push(at),
            _ => return,
        }
    }
    let Some(k) = dialog(app) else { return };
    if k.stroke.last().is_some_and(|last| last.distance(to) >= 2.0) {
        k.stroke.push(to);
    }
}

/// The indices of the points to keep: the ends, and every point standing off the straight line
/// between the ones kept either side of it by more than `tolerance` (Douglas–Peucker).
fn simplified(points: &[egui::Pos2], tolerance: f32) -> Vec<usize> {
    if points.len() <= 2 {
        return (0..points.len()).collect();
    }
    let off = |p: egui::Pos2, a: egui::Pos2, b: egui::Pos2| {
        let ab = b - a;
        let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
        p.distance(a + ab * t)
    };
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut spans = vec![(0, points.len() - 1)];
    while let Some((lo, hi)) = spans.pop() {
        let worst = (lo + 1..hi).map(|k| (k, off(points[k], points[lo], points[hi]))).max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((k, d)) = worst
            && d > tolerance
        {
            keep[k] = true;
            spans.push((lo, k));
            spans.push((k, hi));
        }
    }
    (0..points.len()).filter(|&k| keep[k]).collect()
}

/// The stroke let go: followed a few pixels at a time to see where it is on the scan, cut where it
/// leaves the scan or the scan under it jumps to another layer, and its longest run boiled down to
/// the fewest points that trace it and added to the curve. Ended near the first point (with three
/// or more), it closes the curve. A click or a stroke never on the scan adds nothing.
pub(crate) fn stroke_end(app: &mut SolveApp, proj: &crate::viewport::Proj) {
    let Some(raw) = dialog(app).map(|k| std::mem::take(&mut k.stroke)) else { return };
    let travelled: f32 = raw.windows(2).map(|w| w[0].distance(w[1])).sum();
    if raw.len() < 2 || travelled < MINIMUM_STROKE {
        return;
    }
    let mut samples = vec![raw[0]];
    for &p in &raw[1..] {
        if samples.last().is_some_and(|last| p.distance(*last) >= SAMPLE_STEP) {
            samples.push(p);
        }
    }
    if samples.last() != raw.last()
        && let Some(last) = raw.last()
    {
        samples.push(*last);
    }
    let toward = proj.cam.back();
    let mut runs: Vec<Vec<(egui::Pos2, String, Vec3)>> = Vec::new();
    let mut current: Vec<(egui::Pos2, String, Vec3)> = Vec::new();
    let mut size = 0.0f64;
    for at in samples {
        let Some((body, point)) = scan_at(app, proj, at) else {
            runs.push(std::mem::take(&mut current));
            continue;
        };
        if size == 0.0 {
            size = engine_curve::size(&app.session, &body);
        }
        if let Some((_, last_body, last)) = current.last() {
            let step = point - *last;
            let depth = step.dot(toward).abs();
            let across = (step - toward * step.dot(toward)).len();
            // Sideways it moved about a sample; far deeper than that, it is another layer.
            if *last_body != body || (depth > LAYER_JUMP * across.max(1e-9) && depth > 0.01 * size) {
                runs.push(std::mem::take(&mut current));
            }
        }
        current.push((at, body, point));
    }
    runs.push(current);
    let Some(run) = runs.into_iter().filter(|r| r.len() >= 2).max_by_key(Vec::len) else { return };
    let screen: Vec<egui::Pos2> = run.iter().map(|(p, _, _)| *p).collect();
    let Some(k) = dialog(app) else { return };
    let body = run[0].1.clone();
    if k.scan.as_ref().is_some_and(|s| *s != body) {
        return;
    }
    k.scan = Some(body);
    for i in simplified(&screen, STROKE_TOLERANCE) {
        let point = run[i].2;
        if k.picks.last().is_none_or(|p| (*p - point).len() > 1e-6) {
            k.picks.push(point);
        }
    }
    // Ended on the first point, the curve closes there.
    let first = k.picks.first().and_then(|p| proj.to_screen(*p));
    if k.picks.len() >= 4
        && let (Some(first), Some(end)) = (first, screen.last())
        && first.distance(*end) <= CLOSE
    {
        k.picks.pop();
        k.closed = true;
    }
}

/// The line on the scan and the points clicked, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanCurve(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let t = solidcraft_theme::Tokens::get();
    if let Some(Ok((path, fit, _, _))) = k.shown.as_ref().map(|s| &s.result) {
        let line: Vec<egui::Pos2> = path.iter().filter_map(|p| proj.to_screen(*p)).collect();
        painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, t.sketch_accent)));
        for p in fit.iter().filter_map(|p| proj.to_screen(*p)) {
            painter.circle_filled(p, DOT / 3.0, t.sketch_accent);
        }
    }
    if k.stroke.len() >= 2 {
        painter.add(egui::Shape::line(k.stroke.clone(), egui::Stroke::new(2.0, t.accent)));
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
        assert!(h.app.session.selection.is_empty(), "the points select nothing: {:?}", h.app.session.selection);
        assert!(k.picks.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "on the tube: {:?}", k.picks);
        let Some(Ok((line, _, max, _))) = k.shown.as_ref().map(|s| s.result.clone()) else { panic!("the line on the scan: {:?}", k.shown) };
        assert!(line.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "the line lies on the tube");
        // Within the scan's own tolerance, as fine as its triangles.
        assert!(k.tolerance > 0.0 && max <= k.tolerance, "{max} within {}", k.tolerance);

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == engine_curve::SKETCH_NAME), "kept in Scan Curves");
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(!h.app.session.doc.features.iter().any(|f| f.name == engine_curve::SKETCH_NAME), "one undo step");
    }

    /// A stroke dragged along the top of a tube is followed on the scan and boiled down to a few
    /// points on it; a straight stroke is its two ends, a bent one keeps its bend. A drag begun
    /// off the scan draws nothing.
    #[test]
    fn a_stroke_dragged_on_the_scan_adds_its_points() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, tube_stl(64)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.curve_on_scan");
        h.frames(2);
        let on = |x: f64, y: f64| json!({"world": [x, y, (400.0 - x * x).sqrt()]});
        // Off the scan: nothing.
        h.step(&json!({"drag": [{"world": [60.0, 8.0, 0.0]}, {"world": [60.0, 32.0, 0.0]}], "steps": 12})).unwrap();
        h.frames(2);
        assert!(kind(&h).picks.is_empty(), "a stroke begun off the scan adds nothing");
        // Straight along the tube: its two ends.
        h.step(&json!({"drag": [on(0.0, 4.0), on(0.0, 36.0)], "steps": 24})).unwrap();
        h.frames(2);
        let k = kind(&h);
        assert_eq!(k.picks.len(), 2, "a straight stroke is its ends: {:?}", k.picks);
        assert!(k.picks.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "on the tube: {:?}", k.picks);
        if let Some(Kind::ScanCurve(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.picks.clear();
        }
        // Bent: across, then along.
        h.step(&json!({"drag": [on(-14.0, 6.0), on(0.0, 6.0), on(0.0, 34.0)], "steps": 24})).unwrap();
        h.frames(2);
        let k = kind(&h);
        assert_eq!(k.picks.len(), 3, "the bend is kept: {:?}", k.picks);
        assert!((k.picks[1].x.abs() < 1.5) && (k.picks[1].y - 6.0).abs() < 1.5, "at the bend: {:?}", k.picks[1]);
        assert!(matches!(k.shown.as_ref().map(|s| &s.result), Some(Ok(_))), "the line is laid: {:?}", k.shown);
    }
}
