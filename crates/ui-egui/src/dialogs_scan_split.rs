//! Split Mesh (`scan.split`): pick what cuts the scan, a plane or a flat face, another body or
//! scan, a 3D sketch curve, a sketch's outline, or the scan itself where a shape is to be fitted,
//! and choose what to keep: both sides (the other a scan of its own), one side, or only the
//! crossing as 3D sketch curves. The cut is worked out before anything changes: the crossing is
//! drawn over the picture, the side that becomes a new scan, or goes, is marked on the scan, and
//! the dialog says how much is on each side; an envelope's slab is marked in its own colour. With
//! nothing picked, what is selected on the scan is parted from the rest along its edge. On a large
//! scan the cut is worked out beside the window, the last one staying drawn until the new one
//! arrives, so changing a value never stops the window.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::split::{self, Keep, Planned};
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_job;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{BODIES, FACES, PLANES, PROFILES, SKETCH_LINES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The marks' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 9;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

/// The shapes a click on the scan fits, in the dialog's order.
const KINDS: [&str; 5] = ["plane", "cylinder", "sphere", "cone", "torus"];

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Split Mesh dialog's values (the cutter is its selection input).
#[derive(Clone, Debug, Default)]
pub struct SplitMesh {
    /// 0 both, 1 the positive side, 2 the negative side, 3 only the crossing as curves.
    pub keep: usize,
    /// The shape a click on the scan fits (index into [`KINDS`]).
    pub kind: usize,
    pub envelope: f64,
    pub offset: f64,
    /// Only the triangles facing the eye are cut.
    pub facing_only: bool,
    shown: Option<Box<Shown>>,
    pending: Option<Arc<scan_job::Job<String, Result<Marked, String>>>>,
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Marked, String>,
}

#[derive(Clone, Debug)]
struct Marked {
    planned: Arc<Planned>,
    version: u64,
    /// The painted triangles, for `keep`.
    dots: Arc<Vec<u8>>,
    keep: usize,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.split")
        .then(|| (Kind::ScanSplit(SplitMesh::default()), vec![SelInput::new("Cutter", PLANES | FACES | BODIES | SKETCH_LINES | PROFILES, false)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The scan the dialog cuts: the selected or only one.
fn target(app: &SolveApp) -> Option<String> {
    solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.split").ok()
}

/// Whether the pick is on the scan itself, so a shape is fitted there.
fn on_scan(app: &SolveApp, inputs: &[SelInput]) -> bool {
    matches!(items(inputs, 0).first(), Some(Sel::Face { body, .. }) if Some(body) == target(app).as_ref())
}

/// The cutter picked, as `scan.split` takes it.
fn cutter(app: &SolveApp, k: &SplitMesh, inputs: &[SelInput]) -> Option<Value> {
    let s = &app.session;
    // Nothing picked: what is selected on the scan, if anything.
    let Some(first) = items(inputs, 0).first() else {
        return solidcraft_engine::cmd::scan::select::selected(s).filter(|k| k.count > 0).map(|_| json!({"selection": true}));
    };
    let scan = target(app)?;
    let sketch_is_3d = |id: u64| s.model.state().sketch(id).is_some_and(|sk| sk.sketch.view.three_d);
    Some(match first {
        Sel::Plane { name } => json!({"plane": name}),
        Sel::Face { body, point, .. } if *body == scan => {
            json!({"shape": {"kind": KINDS[k.kind.min(KINDS.len() - 1)], "at": [point.x, point.y, point.z]}})
        }
        Sel::Face { body, index, point } => match crate::dialogs::planar_face(s, body, *index) {
            Some((_, n)) if !s.model.state().body(body).is_some_and(|b| b.body.is_mesh()) => {
                json!({"plane": {"origin": [point.x, point.y, point.z], "normal": [n.x, n.y, n.z]}})
            }
            _ => json!({"cutter": body}),
        },
        Sel::Body { name } if *name != scan => json!({"cutter": name}),
        Sel::ExternalSketchCurve { sketch, id } if sketch_is_3d(*sketch) => json!({"curve": {"sketch": sketch, "curve": id}}),
        Sel::ExternalSketchCurve { sketch, .. } | Sel::Profile { sketch, .. } => json!({"sketch": sketch}),
        _ => return None,
    })
}

/// What OK sends, from the choices.
fn params(app: &SolveApp, k: &SplitMesh, inputs: &[SelInput]) -> Option<Value> {
    let mut p = cutter(app, k, inputs)?;
    if p.get("selection").is_none() {
        p["body"] = json!(target(app)?);
    }
    p["keep"] = json!(["both", "front", "back", "curves"][k.keep.min(3)]);
    if k.envelope > 0.0 && k.keep != 3 {
        p["envelope"] = json!(format!("{} mm", k.envelope));
    }
    let reaches = p.get("curve").is_some() || p.get("sketch").is_some();
    if reaches && k.offset > 0.0 {
        p["offset"] = json!(format!("{} mm", k.offset));
    }
    if k.facing_only {
        // To about 3°: a view turned a little does not cut again.
        let back = app.cam.back();
        let r = |x: f64| (x * 20.0).round() / 20.0;
        p["facing"] = json!([r(back.x), r(back.y), r(back.z)]);
    }
    Some(p)
}

/// Work the cut out again when the choices or the design have changed.
fn look(app: &SolveApp, k: &mut SplitMesh, inputs: &[SelInput]) {
    let Some(p) = params(app, k, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    // The keep does not change the cut, only what is marked.
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    let (colours, keep) = (Colours::now(), k.keep);
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done(result) if job.key == key => {
                    k.pending = None;
                    // The keep may have changed while it worked.
                    let result = result.map(|mut m| {
                        if m.keep != k.keep {
                            remark(&mut m, k.keep, &Colours::now());
                        }
                        m
                    });
                    k.shown = Some(Box::new(Shown { key, result }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = p.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given, keep, &colours)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let result = compute(&app.session, &p, keep, &colours);
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The cut `p` makes, and its marks for `keep`.
fn compute(s: &solidcraft_engine::Session, p: &Value, keep: usize, colours: &Colours) -> Result<Marked, String> {
    let planned = split::plan(s, p, "scan.split").map_err(|e| e.to_string())?;
    let mut m = Marked { planned: Arc::new(planned), version: 0, dots: crate::scan_dots::none(), keep };
    remark(&mut m, keep, colours);
    Ok(m)
}

/// The marks' colours, read on the window's thread (the theme is its own).
#[derive(Clone, Copy, Debug)]
struct Colours {
    accent: [u8; 3],
    goes: [u8; 3],
    slab: [u8; 3],
}

impl Colours {
    fn now() -> Colours {
        let t = solidcraft_theme::Tokens::get();
        let rgb = |c: [f32; 3]| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        let accent = t.accent.to_array();
        Colours { accent: [accent[0], accent[1], accent[2]], goes: rgb(t.analysis.draft[2]), slab: rgb(t.analysis.draft[1]) }
    }
}

/// Paint the marks again for `keep`.
fn remark(m: &mut Marked, keep: usize, colours: &Colours) {
    m.keep = keep;
    m.dots = Arc::new(marks(&m.planned, keep, colours));
    m.version = VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    });
}

/// The triangles painted on the scan: the side a new scan takes (with both) or the side that goes
/// (with one side kept), and the envelope's slab.
fn marks(planned: &Planned, keep: usize, colours: &Colours) -> Vec<u8> {
    let (marked, colour) = match keep {
        0 => (-1, colours.accent),
        1 => (-1, colours.goes),
        2 => (1, colours.goes),
        _ => return Vec::new(),
    };
    let cut = &planned.cut;
    let painted = cut.side.iter().filter(|&&side| side == marked || side == 0).count();
    // At most the most a colouring paints: every so many triangles past that.
    let step = painted.div_ceil(crate::scan_dots::PAINT_MOST).max(1);
    let mut out = Vec::new();
    let at = |i: u32| cut.positions.get(i as usize).copied();
    let sides = cut.triangles.iter().zip(&cut.side).filter(|(_, side)| **side == marked || **side == 0);
    for (tri, &side) in sides.step_by(step) {
        let rgb = if side == 0 { colours.slab } else { colour };
        if let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) {
            crate::scan_dots::cloud_painted(&mut out, [a, b, c], 0.0, rgb);
        }
    }
    out
}

/// The dialog's rows (after the cutter input).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut SplitMesh, inputs: &[SelInput]) -> bool {
    if on_scan(app, inputs) {
        let kinds = [
            tr("ui.scan_split_plane"),
            tr("ui.scan_split_cylinder"),
            tr("ui.scan_split_sphere"),
            tr("ui.scan_split_cone"),
            tr("ui.scan_split_torus"),
        ];
        choice_row(ui, tr("ui.scan_split_fit"), "scan_split_fit", &mut k.kind, &kinds);
    }
    look(app, k, inputs);
    let names = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).map(|m| m.planned.sides.names());
    let side = |name: &str| {
        tr(match name {
            "behind" => "ui.scan_split_behind",
            "inside" => "ui.scan_split_inside",
            "outside" => "ui.scan_split_outside",
            "right" => "ui.scan_split_right",
            "left" => "ui.scan_split_left",
            "selected" => "ui.scan_split_selected",
            "rest" => "ui.scan_split_rest",
            _ => "ui.scan_split_in_front",
        })
    };
    let (negative, positive) = names.map_or((tr("ui.scan_split_behind"), tr("ui.scan_split_in_front")), |(n, p)| (side(n), side(p)));
    let keeps = [tr("ui.scan_split_both"), positive, negative, tr("ui.scan_split_curves")];
    let before = k.keep;
    choice_row(ui, tr("ui.scan_split_keep"), "scan_split_keep", &mut k.keep, &keeps);
    if k.keep != before {
        // The marks follow the keep; the cut stays.
        if let Some(Ok(m)) = k.shown.as_mut().map(|s| &mut s.result) {
            remark(m, k.keep, &Colours::now());
        }
    }
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    if k.keep != 3 {
        form_row(ui, tr("ui.scan_split_envelope"), |ui| ui_kit::length_field(ui, &mut k.envelope, &units, decimals));
        k.envelope = k.envelope.max(0.0);
    }
    if params(app, k, inputs).is_some_and(|p| p.get("curve").is_some() || p.get("sketch").is_some()) {
        form_row(ui, tr("ui.scan_split_offset"), |ui| ui_kit::length_field(ui, &mut k.offset, &units, decimals));
        k.offset = k.offset.max(0.0);
    }
    form_row(ui, tr("ui.scan_split_facing"), |ui| check(ui, &mut k.facing_only));
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_split_refining");
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let cut = &m.planned.cut;
            let runs = cut.runs();
            let line = solidcraft_ui_text::format(
                "ui.scan_split_found",
                &[
                    ("negative", negative),
                    ("below", &cut.count(-1).to_string()),
                    ("positive", positive),
                    ("above", &cut.count(1).to_string()),
                    ("runs", &runs.len().to_string()),
                    ("length", &format!("{:.1}", cut.length)),
                ],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_split_found"));
            if cut.count(0) > 0 {
                let slab = solidcraft_ui_text::format("ui.scan_split_slab", &[("triangles", &cut.count(0).to_string())]);
                ui_kit::text(ui, &slab, ui_kit::Text::Note("scan_split_slab"));
            }
            if cut.carried_on {
                ui_kit::text(ui, tr("ui.scan_split_carried_on"), ui_kit::Text::Hint("scan_split_carried_on"));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_split_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_split_hint"), ui_kit::Text::Hint("scan_split_hint"));
        }
    }
    false
}

/// What OK runs: the split as shown.
pub fn commands(app: &SolveApp, k: &SplitMesh, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(app, k, inputs).ok_or_else(|| tr("ui.scan_split_hint").to_owned())?;
    k.shown.as_ref().ok_or("still cutting")?.result.as_ref().map_err(Clone::clone)?;
    Keep::parse(p.get("keep").and_then(Value::as_str), "scan.split").map_err(|e| e.to_string())?;
    Ok(vec![("scan.split".into(), p)])
}

/// The crossing, over the picture, while the dialog is open.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanSplit(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok(m)) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    let stroke = egui::Stroke::new(2.0, t.accent);
    // At most some tens of thousands of pieces drawn: a long cut on a vast scan is thinned.
    let segments = &m.planned.cut.segments;
    let step = (segments.len() / 40_000).max(1);
    for [a, b] in segments.iter().step_by(step) {
        if let (Some(a), Some(b)) = (proj.to_screen(*a), proj.to_screen(*b)) {
            painter.line_segment([a, b], stroke);
        }
    }
}

/// The sides marked for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanSplit(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: m.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: m.dots.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// A point the scan's cut runs through, for tests: its first run's first point.
#[cfg(test)]
fn first_crossing(app: &SolveApp) -> Option<solidcore_record::Vec3> {
    let Some(Kind::ScanSplit(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    m.planned.cut.segments.first().map(|s| s[0])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A box of 40 × 30 × 20 as an STL of 12 facets.
    fn box_stl() -> String {
        let p = |i: u32| [if i & 1 == 0 { 0.0 } else { 40.0 }, if i & 2 == 0 { 0.0 } else { 30.0 }, if i & 4 == 0 { 0.0 } else { 20.0 }];
        let tris =
            [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        let mut out = String::from("solid b\n");
        for t in tris {
            out.push_str("facet normal 0 0 0\nouter loop\n");
            for i in t {
                let [x, y, z] = p(i);
                out.push_str(&format!("vertex {x} {y} {z}\n"));
            }
            out.push_str("endloop\nendfacet\n");
        }
        out.push_str("endsolid b\n");
        out
    }

    /// A construction plane picked: the crossing is worked out and drawn before anything
    /// changes, the side a new scan takes is marked; OK splits the scan in two.
    #[test]
    fn a_plane_picked_shows_the_cut_and_ok_splits_the_scan() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("block.stl");
        std::fs::write(&path, box_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("construct.plane.offset", json!({"base": "XY", "offset": 8, "name": "Cut Here"})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.split");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Plane { name: "Cut Here".into() });
        }
        h.frames(3);
        let at = first_crossing(&h.app).expect("the cut is worked out");
        assert!((at.z - 8.0).abs() < 1e-6, "on the plane: {at:?}");
        assert!(gpu_marks(&h.app).is_some_and(|m| !m.triangles.is_empty()), "the side below is painted");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let scans = h.app.session.model.state().bodies.iter().filter(|b| b.body.is_mesh()).count();
        assert_eq!(scans, 2, "the scan and the side behind the plane");
    }

    /// On a large scan the cut is worked out beside the window: the window goes on, the dialog says
    /// Refining…, and the cut arrives as it would have here.
    #[test]
    fn on_a_large_scan_the_cut_is_worked_out_beside_the_window() {
        crate::scan_job::force(true);
        let gate = crate::scan_job::hold();
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("block.stl");
        std::fs::write(&path, box_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("construct.plane.offset", json!({"base": "XY", "offset": 8, "name": "Cut Here"})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.split");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Plane { name: "Cut Here".into() });
        }
        h.frames(2);
        let under_way = matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSplit(k)) if k.pending.is_some() && k.shown.is_none());
        gate.open();
        for _ in 0..500 {
            h.frames(1);
            if first_crossing(&h.app).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        crate::scan_job::force(false);
        assert!(under_way, "begun beside the window");
        let at = first_crossing(&h.app).expect("the cut arrives");
        assert!((at.z - 8.0).abs() < 1e-6, "on the plane: {at:?}");
    }
}
