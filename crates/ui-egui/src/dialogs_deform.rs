//! Deform (`solid.deform`): a body bent or twisted along one of the world's axes. Pick the body;
//! choose Bend or Twist, the axis its length runs along, for a bend the way it turns, and the
//! angle. A bend starts at the body's low end along the axis, on its side away from the way it
//! turns (its neutral line); a twist turns about the line through the middle of the body's box.
//! The cage (free-form) way is the command's (`cage`), for scripts and agents.

use serde_json::{Value, json};
use solidcraft_engine::Sel;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::selection::{BODIES, SelInput};
use crate::ui_kit::{self, choice_row};

const AXES: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const AXIS_NAMES: [&str; 3] = ["X", "Y", "Z"];

/// The dialog's values; the body is its one input.
#[derive(Clone, Debug)]
pub struct Deform {
    /// 0 bend, 1 twist.
    pub way: usize,
    /// The axis the length runs along.
    pub along: usize,
    /// A bend's way: one of the other two axes, 0 or 1 among them, and whether it turns the
    /// negative way.
    pub up: usize,
    pub down: bool,
    /// Degrees.
    pub angle: f64,
}

impl Default for Deform {
    fn default() -> Self {
        Deform { way: 0, along: 0, up: 1, down: false, angle: 90.0 }
    }
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "solid.deform").then(|| {
        let mut input = SelInput::new("Body", BODIES, false);
        if let Some(name) = app.session.selection.iter().find_map(|s| match s {
            Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.clone()),
            _ => None,
        }) {
            input.items.push(Sel::Body { name });
        }
        (Kind::Deform(Deform::default()), vec![input])
    })
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The two axes square to `along`.
fn others(along: usize) -> [usize; 2] {
    match along {
        0 => [1, 2],
        1 => [2, 0],
        _ => [0, 1],
    }
}

pub fn rows(_app: &SolveApp, ui: &mut egui::Ui, k: &mut Deform, _inputs: &[SelInput]) -> bool {
    let ways = [tr("ui.deform_bend"), tr("ui.deform_twist")];
    choice_row(ui, tr("ui.deform_way"), "deform_way", &mut k.way, &ways);
    choice_row(ui, tr("ui.deform_along"), "deform_along", &mut k.along, &AXIS_NAMES);
    if k.way == 0 {
        let [a, b] = others(k.along);
        let names = [AXIS_NAMES[a], AXIS_NAMES[b]];
        choice_row(ui, tr("ui.deform_towards"), "deform_towards", &mut k.up, &names);
        form_row(ui, tr("ui.deform_negative"), |ui| crate::dialogs::check(ui, &mut k.down));
    }
    form_row(ui, tr("ui.deform_angle"), |ui| ui_kit::angle_field(ui, &mut k.angle));
    ui_kit::text(ui, tr("ui.deform_hint"), ui_kit::Text::Hint("deform_hint"));
    false
}

pub fn commands(app: &SolveApp, k: &Deform, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let body = inputs
        .first()
        .and_then(|i| i.items.iter().find_map(|s| if let Sel::Body { name } = s { Some(name.clone()) } else { None }))
        .ok_or_else(|| tr("ui.deform_pick").to_owned())?;
    let state = app.session.model.state();
    let b = state.body(&body).and_then(|b| b.body.exact_bounds().ok()).ok_or_else(|| tr("ui.deform_pick").to_owned())?;
    let (lo, hi) = ([b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]);
    let along = AXES[k.along.min(2)];
    let angle = format!("{} deg", k.angle);
    let p = if k.way == 0 {
        let up_axis = others(k.along.min(2))[k.up.min(1)];
        let sign = if k.down { -1.0 } else { 1.0 };
        let mut up = [0.0; 3];
        up[up_axis] = sign;
        // The neutral line: the low end along the length, on the side away from the turn.
        let mut origin = [0.0; 3];
        for d in 0..3 {
            origin[d] = if d == up_axis {
                if k.down { hi[d] } else { lo[d] }
            } else if d == k.along.min(2) {
                lo[d]
            } else {
                0.5 * (lo[d] + hi[d])
            };
        }
        json!({"body": body, "bend": {"origin": origin, "along": along, "up": up, "angle": angle}})
    } else {
        let mut origin = [0.0; 3];
        for d in 0..3 {
            origin[d] = if d == k.along.min(2) { lo[d] } else { 0.5 * (lo[d] + hi[d]) };
        }
        json!({"body": body, "twist": {"origin": origin, "axis": along, "angle": angle}})
    };
    Ok(vec![("solid.deform".into(), p)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bar along X picked and bent towards Z by the dialog: the command it sends starts at the
    /// bar's low end and bottom, and OK makes the Deform feature.
    #[test]
    fn the_dialog_bends_the_body_picked() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"length": 100, "width": 10, "height": 10, "body_name": "Bar"})).unwrap();
        let features = h.app.session.doc.features.len();
        h.app.start("solid.deform");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items = vec![Sel::Body { name: "Bar".into() }];
            if let Kind::Deform(k) = &mut d.kind {
                k.up = 1; // Z among Y and Z
            }
        }
        let (k, inputs) = match h.app.dialog.as_ref() {
            Some(d) => match &d.kind {
                Kind::Deform(k) => (k.clone(), d.inputs.clone()),
                _ => panic!("Deform"),
            },
            None => panic!("the dialog"),
        };
        let (id, p) = commands(&h.app, &k, &inputs).unwrap().remove(0);
        assert_eq!(id, "solid.deform");
        assert_eq!(p["bend"]["origin"], json!([0.0, 5.0, 0.0]), "{p}");
        assert_eq!(p["bend"]["up"], json!([0.0, 0.0, 1.0]), "{p}");
        h.frames(2);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.doc.features.len(), features + 1);
    }
}
