//! Fillet from Scan (`scan.fillet`): pick a model's edges; the round the scan shows along them is
//! measured as the edges, the search width and the rounding change (the radius, how well a round
//! fits against the sharp edge, and how it varies along the edges), and OK fillets the edges at
//! the radius measured.

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::fillet::{self as measuring, REACH, STEP};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::selection::{EDGES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The rounding steps offered: exact, then to 0.1, 0.5 and 1 mm.
const STEPS: [f64; 4] = [0.0, 0.1, 0.5, 1.0];

/// The Fillet from Scan dialog's values (the edges are its selection input).
#[derive(Clone, Debug)]
pub struct FilletScan {
    pub reach: f64,
    /// Index into [`STEPS`].
    pub step: usize,
    shown: Option<Box<Shown>>,
}

impl Default for FilletScan {
    fn default() -> Self {
        FilletScan { reach: REACH, step: STEPS.iter().position(|s| *s == STEP).unwrap_or(2), shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Value, String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.fillet").then(|| (Kind::ScanFillet(FilletScan::default()), vec![SelInput::new("Edges", EDGES, true)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// What `scan.fillet` is sent, from the edges picked and the values.
fn params(k: &FilletScan, inputs: &[SelInput]) -> Option<Value> {
    let mut body = None;
    let mut edges = Vec::new();
    for x in items(inputs, 0) {
        if let Sel::Edge { body: b, point, .. } = x {
            body.get_or_insert_with(|| b.clone());
            edges.push(json!([point.x, point.y, point.z]));
        }
    }
    (!edges.is_empty()).then(|| json!({"edges": edges, "body": body, "reach": k.reach, "step": STEPS[k.step.min(STEPS.len() - 1)]}))
}

/// Measure again when the edges, the values or the design have changed.
fn look(app: &SolveApp, k: &mut FilletScan, inputs: &[SelInput]) {
    let Some(p) = params(k, inputs) else {
        k.shown = None;
        return;
    };
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = measuring::measured(&app.session, &p, "scan.fillet").map_err(|e| e.to_string()).map(|(_, _, _, m)| {
        json!({"radius": m.radius, "fitted": m.fitted, "rms": m.rms, "sharp": m.sharp_rms, "low": m.station_low, "high": m.station_high, "on": m.on_round, "points": m.points})
    });
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The dialog's rows (after the edges).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FilletScan, inputs: &[SelInput]) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_fillet_reach"), |ui| ui_kit::length_field(ui, &mut k.reach, &units, decimals));
    k.reach = k.reach.max(0.1);
    let steps = [tr("ui.scan_fillet_exact"), "0.1", "0.5", "1"];
    choice_row(ui, tr("ui.scan_fillet_step"), "scan_fillet_step", &mut k.step, &steps);
    look(app, k, inputs);
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let n = |key: &str| m.get(key).and_then(Value::as_f64).unwrap_or(0.0);
            let line = solidcraft_ui_text::format(
                "ui.scan_fillet_found",
                &[
                    ("radius", &format!("{:.3}", n("radius"))),
                    ("fitted", &format!("{:.3}", n("fitted"))),
                    ("low", &format!("{:.3}", n("low"))),
                    ("high", &format!("{:.3}", n("high"))),
                    ("rms", &format!("{:.3}", n("rms"))),
                    ("sharp", &format!("{:.3}", n("sharp"))),
                    ("on", &format!("{}", n("on") as usize)),
                    ("points", &format!("{}", n("points") as usize)),
                ],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_fillet_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_fillet_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_fillet_hint"), ui_kit::Text::Hint("scan_fillet_hint"));
        }
    }
    false
}

/// What OK runs: the edges filleted at the radius measured.
pub fn commands(k: &FilletScan, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(k, inputs).ok_or_else(|| tr("ui.scan_fillet_hint").to_owned())?;
    k.shown.as_ref().ok_or("still measuring")?.result.as_ref().map_err(Clone::clone)?;
    Ok(vec![("scan.fillet".into(), p)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A block whose top front edge is rounded at 3 mm, meshed finely and written as an STL.
    fn rounded_stl(h: &mut crate::scenario::Harness) -> std::path::PathBuf {
        h.app.run("solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Rounded"})).unwrap();
        h.app.run("solid.fillet", json!({"edges": [[30, 0, 20]], "radius": 3, "body": "Rounded"})).unwrap();
        let mesh = {
            let state = h.app.session.model.state();
            let body = state.body("Rounded").unwrap();
            body.body.mesh(&solidcraft_engine::kernel::MeshTolerance { chord: 0.002, angle: Some(2f64.to_radians()) }).unwrap()
        };
        let mut out = String::from("solid r\n");
        for t in &mesh.triangles {
            out.push_str("facet normal 0 0 0\nouter loop\n");
            for &i in t {
                let p = mesh.positions[i as usize];
                out.push_str(&format!("vertex {} {} {}\n", p.x, p.y, p.z));
            }
            out.push_str("endloop\nendfacet\n");
        }
        out.push_str("endsolid r\n");
        h.app.run("edit.undo", json!({})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        let path = h.dir.join("rounded.stl");
        std::fs::write(&path, out).unwrap();
        path
    }

    /// The edge picked is measured at its 3 mm round before anything changes; OK fillets it.
    #[test]
    fn the_edge_picked_is_measured_and_ok_fillets_it() {
        let mut h = crate::scenario::Harness::new();
        let path = rounded_stl(&mut h);
        h.app.run("solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Part"})).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let faces = h.app.session.model.state().body("Part").map(|b| b.body.face_count());
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.fillet");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Edge { body: "Part".into(), index: 0, point: solidcore_record::Vec3::new(30.0, 0.0, 20.0) });
        }
        h.frames(3);
        let Some(Kind::ScanFillet(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fillet from Scan dialog") };
        let measured = k.shown.as_ref().map(|s| s.result.clone()).expect("measured");
        let radius = measured.as_ref().ok().and_then(|m| m.get("radius")).and_then(Value::as_f64);
        assert_eq!(radius, Some(3.0), "{measured:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.model.state().body("Part").map(|b| b.body.face_count()), faces.map(|f| f + 1), "the round face");
    }
}
