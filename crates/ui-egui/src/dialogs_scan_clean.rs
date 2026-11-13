//! The scan-cleaning dialogs. Remove Floating Pieces (`scan.remove_floating`): the scan's pieces are
//! found once, and the slider says how small a piece must be, against the largest, to float; the
//! pieces that would go are marked on the scan (through the point-cloud path, in the analysis
//! colours' bad colour) and counted. OK takes them off, one undo step.
//!
//! Fill Holes (`scan.fill_holes`): the scan's holes are found once and outlined over the picture,
//! those it would close in the accent colour and those larger than the limit dimmed; OK closes
//! them, one undo step.
//!
//! Reduce Mesh (`scan.reduce`): how much of the scan to keep (and the count it means), the most a
//! collapse may move the surface, and whether the rim stays put; OK reduces it, one undo step.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::cmd::scan::clean;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit;

/// The marks' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 4;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Remove Floating Pieces dialog's values.
#[derive(Clone, Debug)]
pub struct Floating {
    /// The share of the largest piece below which a piece floats, as log₁₀ of a percentage.
    pub log_percent: f64,
    shown: Option<Shown>,
}

impl Default for Floating {
    fn default() -> Self {
        Floating { log_percent: (100.0 * clean::FLOATING_SHARE).log10(), shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (u64, u64),
    result: Result<Marked, String>,
}

#[derive(Clone, Debug)]
struct Marked {
    body: String,
    pieces: usize,
    floating: usize,
    triangles: usize,
    version: u64,
    dots: Arc<Vec<u8>>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    match id {
        "scan.remove_floating" => Some((Kind::ScanFloating(Floating::default()), Vec::new())),
        "scan.fill_holes" => Some((Kind::ScanHoles(Holes::default()), Vec::new())),
        "scan.reduce" => Some((Kind::ScanReduce(Reduce::default()), Vec::new())),
        _ => None,
    }
}

impl Floating {
    /// The share of the largest piece, 0 to 0.5.
    pub fn share(&self) -> f64 {
        (10f64.powf(self.log_percent) / 100.0).clamp(0.0, 0.5)
    }
}

/// Mark the floating pieces again when the share or the design has changed.
fn mark(app: &SolveApp, k: &mut Floating) {
    let key = (k.log_percent.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let share = k.share();
    let result = clean::pieces_of(&app.session, None, "scan.remove_floating").map_err(|e| e.to_string()).and_then(|(body, found)| {
        let floating = found.floating(share);
        let state = app.session.model.state();
        let (positions, triangles) = state.body(&body).and_then(|b| b.body.triangle_mesh()).ok_or("the scan is gone")?;
        let rgb = solidcraft_theme::Tokens::get().analysis.draft[2].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut dots = Vec::new();
        let mut count = 0;
        for (t, tri) in triangles.iter().enumerate() {
            if !found.of.get(t).is_some_and(|&p| p != u32::MAX && floating[p as usize]) {
                continue;
            }
            count += 1;
            for &i in tri {
                if let Some(p) = positions.get(i as usize) {
                    cloud_dot(&mut dots, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], rgb, ..Dot::default() });
                }
            }
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Ok(Marked {
            body,
            pieces: found.sizes.len(),
            floating: floating.iter().filter(|f| **f).count(),
            triangles: count,
            version,
            dots: Arc::new(dots),
        })
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Floating) -> bool {
    mark(app, k);
    // 0.05 % to 50 % of the largest piece, on a log scale.
    let format = |v: f64| format!("{:.2} %", 10f64.powf(v));
    form_row(ui, solidcraft_ui_text::tr("ui.scan_floating_below"), |ui| {
        ui_kit::slider(ui, &mut k.log_percent, -1.3..=1.7, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let found = solidcraft_ui_text::format(
                "ui.scan_floating_found",
                &[("floating", &m.floating.to_string()), ("pieces", &m.pieces.to_string()), ("triangles", &m.triangles.to_string())],
            );
            ui_kit::text(ui, &found, ui_kit::Text::Note("scan_floating_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_floating_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, solidcraft_ui_text::tr("ui.scan_floating_hint"), ui_kit::Text::Hint("scan_floating_hint"));
    false
}

/// What OK runs: the floating pieces taken off.
pub fn commands(k: &Floating) -> Result<Vec<(String, Value)>, String> {
    let m = k.shown.as_ref().ok_or("still finding the pieces")?.result.as_ref().map_err(Clone::clone)?;
    if m.floating == 0 {
        return Err(solidcraft_ui_text::tr("ui.scan_floating_none").into());
    }
    Ok(vec![("scan.remove_floating".into(), json!({"body": m.body, "below": k.share()}))])
}

/// The floating pieces marked for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFloating(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: m.version,
        dots: m.dots.clone(),
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// The Fill Holes dialog's values.
#[derive(Clone, Debug)]
pub struct Holes {
    /// The largest hole to close, in edges round it.
    pub max_edges: usize,
    /// Each patch shaped to carry the surface across.
    pub fair: bool,
    found: Option<FoundHoles>,
}

impl Default for Holes {
    fn default() -> Self {
        Holes { max_edges: clean::MAX_EDGES, fair: true, found: None }
    }
}

#[derive(Clone, Debug)]
struct FoundHoles {
    revision: u64,
    /// The scan, and each hole's outline (closed) with how many edges it has; or why not.
    result: Result<(String, Vec<(Vec<solidcore_record::Vec3>, usize)>, usize), String>,
}

/// Find the holes again when the design has changed.
fn find_holes(app: &SolveApp, k: &mut Holes) {
    if k.found.as_ref().is_some_and(|f| f.revision == app.session.revision) {
        return;
    }
    let result = clean::holes_of(&app.session, None, "scan.fill_holes").map_err(|e| e.to_string()).and_then(|(body, found)| {
        let state = app.session.model.state();
        let (positions, _) = state.body(&body).and_then(|b| b.body.triangle_mesh()).ok_or("the scan is gone")?;
        let outlines = found
            .rings
            .iter()
            .filter(|r| !r.rim)
            .map(|r| (r.points.iter().filter_map(|&i| positions.get(i as usize).copied()).collect(), r.points.len()))
            .collect();
        Ok((body, outlines, found.unfillable_edges))
    });
    k.found = Some(FoundHoles { revision: app.session.revision, result });
}

/// The Fill Holes dialog's rows.
pub fn holes_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Holes) -> bool {
    find_holes(app, k);
    form_row(ui, solidcraft_ui_text::tr("ui.scan_holes_max"), |ui| ui_kit::number_field(ui, &mut k.max_edges, 3..=5000, 1.0, ""));
    form_row(ui, solidcraft_ui_text::tr("ui.scan_holes_fair"), |ui| check(ui, &mut k.fair));
    match k.found.as_ref().map(|f| &f.result) {
        Some(Ok((_, outlines, unfillable))) => {
            let within = outlines.iter().filter(|(_, n)| *n <= k.max_edges).count();
            let found = solidcraft_ui_text::format(
                "ui.scan_holes_found",
                &[("within", &within.to_string()), ("holes", &outlines.len().to_string()), ("unfillable", &unfillable.to_string())],
            );
            ui_kit::text(ui, &found, ui_kit::Text::Note("scan_holes_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_holes_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, solidcraft_ui_text::tr("ui.scan_holes_hint"), ui_kit::Text::Hint("scan_holes_hint"));
    false
}

/// What OK runs: the holes within the limit closed.
pub fn holes_commands(k: &Holes) -> Result<Vec<(String, Value)>, String> {
    let (body, outlines, _) = k.found.as_ref().ok_or("still finding the holes")?.result.as_ref().map_err(Clone::clone)?;
    if !outlines.iter().any(|(_, n)| *n <= k.max_edges) {
        return Err(solidcraft_ui_text::tr("ui.scan_holes_none").into());
    }
    Ok(vec![("scan.fill_holes".into(), json!({"body": body, "max_edges": k.max_edges, "fair": k.fair}))])
}

/// The holes outlined over the picture while the dialog is open.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanHoles(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, outlines, _))) = k.found.as_ref().map(|f| &f.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    for (outline, edges) in outlines {
        let stroke = if *edges <= k.max_edges { egui::Stroke::new(2.0, t.accent) } else { egui::Stroke::new(1.0, t.text_dim) };
        let mut line: Vec<egui::Pos2> = outline.iter().filter_map(|p| proj.to_screen(*p)).collect();
        if line.len() == outline.len() && line.len() > 1 {
            line.push(line[0]);
            painter.add(egui::Shape::line(line, stroke));
        }
    }
}

/// The Reduce Mesh dialog's values.
#[derive(Clone, Debug)]
pub struct Reduce {
    /// The share of the triangles to keep, in percent.
    pub percent: f64,
    /// The most a collapse may move the surface, in mm; 0 for no such limit.
    pub max_error: f64,
    pub keep_rim: bool,
    /// The scan and its triangles, for the count the share means.
    scan: Option<(u64, Result<(String, usize), String>)>,
}

impl Default for Reduce {
    fn default() -> Self {
        Reduce { percent: 50.0, max_error: 0.0, keep_rim: true, scan: None }
    }
}

/// The Reduce Mesh dialog's rows.
pub fn reduce_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Reduce) -> bool {
    if k.scan.as_ref().is_none_or(|(revision, _)| *revision != app.session.revision) {
        let found = solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.reduce").map_err(|e| e.to_string()).and_then(|name| {
            let count = app.session.model.state().body(&name).and_then(|b| b.body.triangle_mesh()).map(|(_, t)| t.len()).ok_or("the scan is gone")?;
            Ok((name, count))
        });
        k.scan = Some((app.session.revision, found));
    }
    let tr = solidcraft_ui_text::tr;
    let format = |v: f64| format!("{v:.0} %");
    form_row(ui, tr("ui.scan_reduce_keep"), |ui| {
        ui_kit::slider(ui, &mut k.percent, 1.0..=99.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_reduce_error"), |ui| ui_kit::length_field(ui, &mut k.max_error, &units, decimals.max(3)));
    k.max_error = k.max_error.clamp(0.0, 100.0);
    form_row(ui, tr("ui.scan_reduce_rim"), |ui| check(ui, &mut k.keep_rim));
    match k.scan.as_ref().map(|(_, r)| r) {
        Some(Ok((_, count))) => {
            let kept = ((k.percent / 100.0) * *count as f64).round() as usize;
            let note = solidcraft_ui_text::format("ui.scan_reduce_count", &[("now", &count.to_string()), ("kept", &kept.to_string())]);
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_reduce_count"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_reduce_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_reduce_hint"), ui_kit::Text::Hint("scan_reduce_hint"));
    false
}

/// What OK runs: the scan reduced.
pub fn reduce_commands(k: &Reduce) -> Result<Vec<(String, Value)>, String> {
    let (body, _) = k.scan.as_ref().ok_or("still reading the scan")?.1.as_ref().map_err(Clone::clone)?;
    let mut p = json!({"body": body, "ratio": (k.percent / 100.0).clamp(0.01, 0.99), "keep_rim": k.keep_rim});
    if k.max_error > 0.0 {
        p["max_error"] = json!(format!("{} mm", k.max_error));
    }
    Ok(vec![("scan.reduce".into(), p)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A box of `n` × `n` squares a side from `lo` to `hi`, as STL facets.
    fn box_facets(lo: [f64; 3], hi: [f64; 3], n: u32) -> String {
        let mut stl = String::new();
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let at = |i: u32, j: u32| {
                    let mut c = lo;
                    if high {
                        c[axis] = hi[axis];
                    }
                    c[u] = lo[u] + (hi[u] - lo[u]) * f64::from(i) / f64::from(n);
                    c[v] = lo[v] + (hi[v] - lo[v]) * f64::from(j) / f64::from(n);
                    c
                };
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
            }
        }
        stl
    }

    /// A block with a speck beside it: the dialog marks the speck's 12 triangles and OK takes
    /// them off.
    #[test]
    fn the_speck_is_marked_and_taken_off() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.remove_floating");
        h.frames(2);
        let marks = gpu_marks(&h.app).expect("the speck marked");
        assert_eq!(marks.dots.len(), 12 * 3 * crate::scan_dots::DOT_SIZE, "its 12 triangles' corners");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x <= 30.0), "the speck is gone");
    }

    /// Taking the speck off is computed beside the window, as a feature's OK is: a real scan's
    /// millions of triangles took a minute on the window thread. The dialog stays, saying it is
    /// computing, nothing changes until the result is in, and then it is applied.
    #[test]
    fn taking_the_speck_off_is_computed_beside_the_window() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.apply_off_thread = Some(std::time::Duration::ZERO);
        let (finish, gate) = std::sync::mpsc::channel();
        h.app.apply_gate = Some(std::sync::Arc::new(std::sync::Mutex::new(gate)));
        h.app.start("scan.remove_floating");
        h.frames(2);
        let before = h.app.session.doc.clone();
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        assert!(h.app.applying.is_some(), "computing beside the window");
        h.frames(2);
        assert!(std::sync::Arc::ptr_eq(&before, &h.app.session.doc) && h.app.dialog.is_some(), "nothing applied while it computes");
        finish.send(()).unwrap();
        for _ in 0..500 {
            if h.app.applying.is_none() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h.frames(1);
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x <= 30.0), "the speck is gone");
        // One undo step, as on the window thread.
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &h.app.session.doc));
    }

    /// A plate with a square hole: the dialog finds the hole, and OK closes it.
    #[test]
    fn the_hole_is_found_and_closed() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid p\n");
        for y in 0..10 {
            for x in 0..10 {
                if (4..=5).contains(&x) && (4..=5).contains(&y) {
                    continue;
                }
                let p = |i: i32, j: i32| [x + i, y + j, 0];
                for tri in [[p(0, 0), p(1, 0), p(1, 1)], [p(0, 0), p(1, 1), p(0, 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for [a, b, c] in tri {
                        stl += &format!("vertex {a} {b} {c}\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        let path = h.dir.join("plate.stl");
        std::fs::write(&path, stl + "endsolid p\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.fill_holes");
        h.frames(2);
        let Some(Kind::ScanHoles(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fill Holes dialog") };
        let Some(Ok((_, outlines, _))) = k.found.as_ref().map(|f| &f.result) else { panic!("holes found") };
        assert_eq!(outlines.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![8], "the hole, not the rim");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (_, triangles) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(triangles.len() > 192, "patched: {}", triangles.len());
    }

    /// A finely cut block reduced to a tenth through the dialog keeps its shape.
    #[test]
    fn the_dialog_reduces_a_block() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("block.stl");
        std::fs::write(&path, format!("solid b\n{}endsolid b\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12))).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.reduce");
        h.frames(2);
        if let Some(Kind::ScanReduce(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.percent = 10.0;
        }
        h.frames(1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, triangles) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(triangles.len() <= 173, "a tenth of 1728: {}", triangles.len());
        assert!(positions.iter().all(|p| p.x >= -1e-4 && p.x <= 30.0 + 1e-4 && p.z <= 10.0 + 1e-4), "the block's shape");
    }
}
