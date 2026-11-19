//! Fit Together (`scan.fit_together`): click shapes on the scan, each of the kind chosen at the
//! click (a plane, a cylinder, …); once there are two, the relations they nearly hold already are
//! worked out on a copy of the design (parallel, perpendicular, on one axis, equal radius, square
//! to a world axis) and listed, each with a check box. OK fits the shapes to their regions and to
//! the relations ticked at once and keeps each as a construction plane, axis or point.

use serde_json::{Value, json};
use solidcraft_engine::Sel;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{FACES, SelInput};
use crate::ui_kit::{self, choice_row};

/// The kinds a click makes, in the row's order.
const KINDS: [&str; 5] = ["plane", "cylinder", "sphere", "cone", "torus"];

/// The relations found for the shapes clicked: the key they were found for, and each relation
/// with whether it is ticked.
type Found = (String, Result<Vec<(Value, bool)>, String>);

/// The dialog's values; the clicks are its one input.
#[derive(Clone, Debug, Default)]
pub struct Together {
    /// The kind the next click makes.
    pub kind: usize,
    /// The kind of each click, in order.
    pub kinds: Vec<usize>,
    found: Option<Found>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.fit_together").then(|| (Kind::ScanTogether(Together { kind: 1, ..Together::default() }), vec![SelInput::new("Points", FACES, true)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The clicks: the scan and the point of each.
fn clicks(inputs: &[SelInput]) -> Vec<(String, [f64; 3])> {
    inputs
        .first()
        .map(|i| {
            i.items
                .iter()
                .filter_map(|s| match s {
                    Sel::Face { body, point, .. } => Some((body.clone(), [point.x, point.y, point.z])),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The shapes as the command takes them.
fn shapes(k: &Together, inputs: &[SelInput]) -> Vec<Value> {
    clicks(inputs).iter().zip(&k.kinds).map(|((_, p), kind)| json!({"kind": KINDS[(*kind).min(KINDS.len() - 1)], "point": p})).collect()
}

fn kind_label(name: &str) -> &'static str {
    match name {
        "plane" => tr("cmd.scan_fit_plane"),
        "cylinder" => tr("cmd.scan_fit_cylinder"),
        "sphere" => tr("cmd.scan_fit_sphere"),
        "cone" => tr("cmd.scan_fit_cone"),
        _ => tr("cmd.scan_fit_torus"),
    }
}

/// A relation's words: what it holds and between which shapes (numbered from 1) or world axes.
fn relation_label(r: &Value) -> String {
    let kind = match r["kind"].as_str().unwrap_or_default() {
        "parallel" => tr("ui.scan_together_parallel"),
        "perpendicular" => tr("ui.scan_together_perpendicular"),
        "coaxial" => tr("ui.scan_together_coaxial"),
        "tangent" => tr("ui.scan_together_tangent"),
        "mirror" => tr("ui.scan_together_mirror"),
        _ => tr("ui.scan_together_equal_radius"),
    };
    let side = |v: &Value| match v {
        Value::Number(n) => (n.as_u64().unwrap_or(0) + 1).to_string(),
        Value::String(s) => s.to_uppercase(),
        _ => String::new(),
    };
    if r.get("about").is_some_and(|a| !a.is_null()) {
        let about = [("kind", kind), ("a", &side(&r["a"])), ("b", &side(&r["b"])), ("about", &side(&r["about"]))];
        return solidcraft_ui_text::format("ui.scan_together_relation_about", &about);
    }
    solidcraft_ui_text::format("ui.scan_together_relation", &[("kind", kind), ("a", &side(&r["a"])), ("b", &side(&r["b"]))])
}

/// The relations the shapes clicked nearly hold, worked out on a copy of the design when the
/// clicks or the design changed (each shape is fitted, as its Fit tool fits it).
fn find(app: &SolveApp, k: &mut Together, inputs: &[SelInput]) {
    let shapes = shapes(k, inputs);
    if shapes.len() < 2 {
        k.found = None;
        return;
    }
    let body = clicks(inputs).first().map(|(b, _)| b.clone()).unwrap_or_default();
    let key = format!("{}|{}|{body}", app.session.revision, Value::Array(shapes.clone()));
    if k.found.as_ref().is_some_and(|(at, _)| *at == key) {
        return;
    }
    let mut copy = app.session.scratch();
    let found = copy
        .execute("scan.fit_together", &json!({"body": body, "shapes": shapes, "keep": false}))
        .map_err(|e| e.to_string())
        .map(|r| r["relations"].as_array().map(|list| list.iter().map(|x| (x.clone(), true)).collect()).unwrap_or_default());
    k.found = Some((key, found));
}

/// The dialog's rows (after its input).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Together, inputs: &[SelInput]) -> bool {
    let count = clicks(inputs).len();
    // Each click keeps the kind chosen when it was made.
    while k.kinds.len() < count {
        k.kinds.push(k.kind);
    }
    k.kinds.truncate(count);
    let options: Vec<&str> = KINDS.iter().map(|n| kind_label(n)).collect();
    choice_row(ui, tr("ui.scan_together_next"), "scan_together_kind", &mut k.kind, &options);
    for (i, kind) in k.kinds.iter().enumerate() {
        form_row(ui, &(i + 1).to_string(), |ui| ui_kit::text(ui, kind_label(KINDS[(*kind).min(KINDS.len() - 1)]), ui_kit::Text::Cell));
    }
    find(app, k, inputs);
    match k.found.as_mut().map(|(_, f)| f) {
        None => {
            ui_kit::text(ui, tr("ui.scan_together_pick"), ui_kit::Text::Hint("scan_together_pick"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_together_error", true));
        }
        Some(Ok(list)) if list.is_empty() => {
            ui_kit::text(ui, tr("ui.scan_together_none"), ui_kit::Text::Note("scan_together_none"));
        }
        Some(Ok(list)) => {
            for (r, on) in list.iter_mut() {
                form_row(ui, &relation_label(r), |ui| check(ui, on));
            }
            ui_kit::text(ui, tr("ui.scan_together_hint"), ui_kit::Text::Hint("scan_together_hint"));
        }
    }
    false
}

/// What OK runs: the shapes fitted together with the relations ticked.
pub fn commands(_app: &SolveApp, k: &Together, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let shapes = shapes(k, inputs);
    if shapes.len() < 2 {
        return Err(tr("ui.scan_together_pick").to_owned());
    }
    let body = clicks(inputs).first().map(|(b, _)| b.clone()).unwrap_or_default();
    let relations: Vec<Value> = match k.found.as_ref().map(|(_, f)| f) {
        Some(Ok(list)) => list.iter().filter(|(_, on)| *on).map(|(r, _)| r.clone()).collect(),
        Some(Err(why)) => return Err(why.clone()),
        None => return Err(tr("ui.scan_together_pick").to_owned()),
    };
    // Only what the command takes of each relation.
    let relations: Vec<Value> = relations
        .iter()
        .map(|r| {
            let mut out = json!({"kind": r["kind"], "a": r["a"]});
            for key in ["b", "about"] {
                if !r[key].is_null() {
                    out[key] = r[key].clone();
                }
            }
            out
        })
        .collect();
    Ok(vec![("scan.fit_together".into(), json!({"body": body, "shapes": shapes, "relations": relations}))])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two clicks on two faces of a box: the relations found say they are perpendicular, ticked;
    /// OK keeps two construction planes in one undo step.
    #[test]
    fn two_faces_of_a_box_are_found_perpendicular_and_kept_together() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("box.stl");
        // A box 20 × 10 × 10, each face cut into 8 × 8 squares (a plane fit needs points).
        let size = [20.0, 10.0, 10.0];
        let mut stl = String::from("solid box\n");
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in [0.0, 1.0] {
                let at = |i: usize, j: usize| {
                    let mut p = [0.0; 3];
                    p[axis] = side * size[axis];
                    p[u] = size[u] * i as f64 / 8.0;
                    p[v] = size[v] * j as f64 / 8.0;
                    p
                };
                for i in 0..8 {
                    for j in 0..8 {
                        for t in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                            stl += "facet normal 0 0 0\nouter loop\n";
                            for p in t {
                                stl += &format!("vertex {} {} {}\n", p[0], p[1], p[2]);
                            }
                            stl += "endloop\nendfacet\n";
                        }
                    }
                }
            }
        }
        std::fs::write(&path, stl + "endsolid box\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let body = h.app.session.model.state().bodies[0].name.clone();
        let features = h.app.session.doc.features.len();
        h.app.start("scan.fit_together");
        if let Some(d) = h.app.dialog.as_mut() {
            if let Kind::ScanTogether(k) = &mut d.kind {
                k.kind = 0;
            }
            for p in [[10.0, 5.0, 10.0], [20.0, 5.0, 5.0]] {
                d.inputs[0].items.push(Sel::Face { body: body.clone(), index: 0, point: solidcore_record::Vec3::new(p[0], p[1], p[2]) });
            }
        }
        h.frames(2);
        let Some(Kind::ScanTogether(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        assert_eq!(k.kinds, vec![0, 0]);
        let Some((_, Ok(found))) = &k.found else { panic!("relations found: {:?}", k.found) };
        assert!(found.iter().any(|(r, on)| r["kind"] == "perpendicular" && *on), "{found:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.doc.features.len(), features + 2, "two construction planes");
    }
}
