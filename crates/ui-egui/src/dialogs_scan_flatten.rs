//! Flatten (`scan.flatten`): click the part of the scan to unroll (the piece there, the face grown
//! from the click, or only as far as the radius round the click), or select it first
//! ([`crate::scan_area`]); the region is marked on the scan and the pattern drawn where
//! it will lie, beside the scan on XY, with how far it is from the scan (its area, each edge's
//! stretch, triangles turned over) before OK sketches its outline.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::flatten::flattened;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit;

/// The marks' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 10;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 2.5;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Flatten dialog's values (the click on the scan is its selection input).
#[derive(Clone, Debug)]
pub struct Flatten {
    /// The part of the scan unrolled.
    pub area: crate::scan_area::Area,
    pub relax: bool,
    shown: Option<Box<Shown>>,
}

impl Default for Flatten {
    fn default() -> Self {
        Flatten { area: crate::scan_area::Area::default(), relax: true, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Marked, String>,
}

#[derive(Clone, Debug)]
struct Marked {
    stats: solidcraft_engine::cmd::scan::flatten::Stats,
    triangles: usize,
    /// The pattern's outline where it will lie.
    outline: Vec<Vec<Vec3>>,
    version: u64,
    dots: Arc<Vec<u8>>,
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let k = Flatten { area: crate::scan_area::Area::opening(app, false), ..Flatten::default() };
    (id == "scan.flatten").then(|| (Kind::ScanFlatten(k), vec![SelInput::new("Point", FACES, false)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// What `scan.flatten` is sent.
fn params(app: &SolveApp, k: &Flatten, inputs: &[SelInput]) -> Option<Value> {
    let clicked = match items(inputs, 0).first() {
        Some(Sel::Face { body, point, .. }) => Some((body.as_str(), *point)),
        _ => None,
    };
    let mut p = k.area.params(app, clicked)?;
    p["relax"] = json!(k.relax);
    Some(p)
}

/// Flatten again when the click, the values or the design have changed.
fn look(app: &SolveApp, k: &mut Flatten, inputs: &[SelInput]) {
    let Some(p) = params(app, k, inputs) else {
        k.shown = None;
        return;
    };
    // What is selected is not in the revision: its stamp is.
    let key = format!("{p}|{}|{}", app.session.revision, solidcraft_engine::cmd::scan::select::stamp(&app.session));
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = flattened(&app.session, &p, "scan.flatten").map_err(|e| e.to_string()).map(|(body, flat, shift)| {
        let state = app.session.model.state();
        let rgb = solidcraft_theme::Tokens::get().accent.to_array();
        let mut dots = Vec::new();
        if let Some((positions, _)) = state.body(&body).and_then(|b| b.body.triangle_mesh()) {
            // At most a few hundred thousand marks: every so many points on a vast region.
            let step = (flat.vertices.len() / 400_000).max(1);
            for &v in flat.vertices.iter().step_by(step) {
                if let Some(p) = positions.get(v as usize) {
                    cloud_dot(&mut dots, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], rgb: [rgb[0], rgb[1], rgb[2]], ..Dot::default() });
                }
            }
        }
        let outline = flat.outline.iter().map(|lp| lp.iter().map(|q| Vec3::new(q.x + shift.x, q.y + shift.y, 0.0)).collect()).collect();
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Marked { stats: flat.stats, triangles: flat.triangles.len(), outline, version, dots: Arc::new(dots) }
    });
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The dialog's rows (after the click).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Flatten, inputs: &[SelInput]) -> bool {
    crate::scan_area::row(app, ui, "scan_flatten_area", &mut k.area);
    form_row(ui, tr("ui.scan_flatten_relax"), |ui| check(ui, &mut k.relax));
    look(app, k, inputs);
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let st = &m.stats;
            let line = solidcraft_ui_text::format(
                "ui.scan_flatten_found",
                &[
                    ("triangles", &m.triangles.to_string()),
                    ("area", &format!("{:.1}", st.area)),
                    ("change", &format!("{:.2}", 100.0 * (st.flat_area / st.area.max(1e-300) - 1.0))),
                    ("rms", &format!("{:.2}", 100.0 * st.strain_rms)),
                    ("max", &format!("{:.1}", 100.0 * st.strain_max)),
                ],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_flatten_found"));
            if st.flipped > 0 {
                let folds = solidcraft_ui_text::format("ui.scan_flatten_flipped", &[("flipped", &st.flipped.to_string())]);
                ui_kit::text(ui, &folds, ui_kit::Text::Report("scan_flatten_flipped", true));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_flatten_error", true));
        }
        None => {
            ui_kit::text(ui, crate::scan_area::waiting(&k.area, "ui.scan_flatten_hint"), ui_kit::Text::Hint("scan_flatten_hint"));
        }
    }
    false
}

/// What OK runs.
pub fn commands(app: &SolveApp, k: &Flatten, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(app, k, inputs).ok_or_else(|| crate::scan_area::waiting(&k.area, "ui.scan_flatten_hint").to_owned())?;
    k.shown.as_ref().ok_or("still flattening")?.result.as_ref().map_err(Clone::clone)?;
    Ok(vec![("scan.flatten".into(), p)])
}

/// The pattern's outline where it will lie, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanFlatten(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok(m)) = k.shown.as_ref().map(|s| &s.result) else { return };
    let stroke = egui::Stroke::new(1.5, solidcraft_theme::Tokens::get().accent);
    for lp in &m.outline {
        let step = (lp.len() / 20_000).max(1);
        let pts: Vec<egui::Pos2> = lp.iter().step_by(step).chain(lp.first()).filter_map(|q| proj.to_screen(*q)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    }
}

/// The region marked for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFlatten(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A sheet bent a quarter round a cylinder, as an STL.
    fn bent_stl() -> String {
        let at = |i: u32, j: u32| {
            let a = f64::from(i) / 20.0 * std::f64::consts::FRAC_PI_2;
            [25.0 * a.sin(), f64::from(j) * 1.5, 25.0 * (1.0 - a.cos())]
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

    /// A click on the bent sheet flattens it before anything changes, the pattern drawn and
    /// measured; OK sketches the pattern's outline.
    #[test]
    fn a_click_flattens_the_piece_and_ok_sketches_its_outline() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("bent.stl");
        std::fs::write(&path, bent_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flatten");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Face { body: scan, index: 0, point: Vec3::new(10.0, 15.0, 2.2) });
        }
        h.frames(3);
        let Some(Kind::ScanFlatten(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flatten dialog") };
        let m = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).expect("flattened");
        assert!(m.stats.strain_max < 0.02 && m.stats.flipped == 0, "{:?}", m.stats);
        assert!(gpu_marks(&h.app).is_some_and(|c| !c.dots.is_empty()));
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let made = h.app.session.doc.features.iter().any(|f| f.name == "Flat Pattern");
        assert!(made, "the pattern's sketch");
    }

    /// Each way of choosing the area flattens its own part of the sheet: grown from the click, less
    /// than the whole piece; round the click, less again; and with part selected first the dialog
    /// opens on What is selected and flattens that with no click.
    #[test]
    fn every_way_of_choosing_the_area_flattens_its_part() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("bent.stl");
        std::fs::write(&path, bent_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flatten");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Face { body: scan.clone(), index: 0, point: Vec3::new(0.3, 15.0, 0.0) });
        }
        let count = |h: &mut crate::scenario::Harness, way: usize| {
            if let Some(Kind::ScanFlatten(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
                (k.area.way, k.area.radius) = (way, 6.0);
            }
            h.frames(3);
            let Some(Kind::ScanFlatten(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flatten dialog") };
            k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).map_or(0, |m| m.triangles)
        };
        let piece = count(&mut h, crate::scan_area::PIECE);
        let grown = count(&mut h, crate::scan_area::GROW);
        let round = count(&mut h, crate::scan_area::ROUND);
        assert!(piece == 800 && (0..piece).contains(&grown) && (0..grown).contains(&round) && round > 0, "{piece} {grown} {round}");
        h.app.dialog = None;
        let half: Vec<usize> = (0..300).collect();
        h.app.run("scan.select", json!({"body": scan, "triangles": half})).unwrap();
        h.app.start("scan.flatten");
        h.frames(3);
        let Some(Kind::ScanFlatten(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flatten dialog") };
        assert_eq!(k.area.way, crate::scan_area::SELECTED);
        assert_eq!(k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).map(|m| m.triangles), Some(300));
    }
}
