//! Radius from Scan (`scan.radius`) in the view: the button asks for taps on rounds of the scan;
//! each tap's round is labelled with its radius, whether it is an outside or an inside round and
//! the angle it turns, and when an edge of the model runs along it a confirmation offers to fillet
//! that edge at the radius. Pressing the button again ends it.

use std::cell::RefCell;

use serde_json::json;
use solidcore_record::Vec3;

use crate::SolveApp;

/// A round a tap read.
#[derive(Clone, Debug, PartialEq)]
struct Round {
    scan: String,
    at: [f64; 3],
    radius: f64,
    convex: bool,
    swept: f64,
    middle: Vec3,
    /// The model the edge along the round is on, when one is.
    edge_on: Option<String>,
    /// Whether the fillet was offered and answered.
    answered: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum State {
    /// Waiting for a tap; the last round read, if any.
    Picking(Option<Round>),
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn state() -> Option<State> {
    STATE.with(|s| s.borrow().clone())
}

fn set(next: Option<State>) {
    STATE.with(|s| *s.borrow_mut() = next);
}

fn round() -> Option<Round> {
    match state() {
        Some(State::Picking(r)) => r,
        None => None,
    }
}

/// The button: starts asking for taps, or ends it. False: the button runs nothing more.
pub(crate) fn toggle(_app: &mut SolveApp) -> bool {
    set(if state().is_some() { None } else { Some(State::Picking(None)) });
    false
}

/// Whether a click on the scan reads a round now (the button shows pressed).
pub(crate) fn picking(_app: &SolveApp) -> bool {
    state().is_some()
}

/// A tap: the round under it read from the scan, changing nothing.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    // The scan itself, through the model: a model's sharp edge built over the scan covers its round.
    let Some((scan, q)) = crate::scan_snap::scan_under(app, proj, at) else { return };
    let at = [q.x, q.y, q.z];
    let Ok(reply) = app.run("scan.radius", json!({"scan": scan, "at": at})) else {
        set(Some(State::Picking(None)));
        return;
    };
    let middle = reply["middle"].as_array().and_then(|m| Some(Vec3::new(m.first()?.as_f64()?, m.get(1)?.as_f64()?, m.get(2)?.as_f64()?)));
    let (Some(radius), Some(middle)) = (reply["radius"].as_f64(), middle) else { return };
    set(Some(State::Picking(Some(Round {
        scan,
        at,
        radius,
        convex: reply["convex"].as_bool().unwrap_or(true),
        swept: reply["swept"].as_f64().unwrap_or(0.0),
        middle,
        edge_on: reply["edge"]["body"].as_str().map(str::to_owned),
        answered: false,
    }))));
}

/// A round's tag: its radius, outside or inside, and the angle it turns.
fn label(app: &SolveApp, r: &Round) -> String {
    let kind = solidcraft_ui_text::tr(if r.convex { "ui.scan_radius_outside" } else { "ui.scan_radius_inside" });
    solidcraft_ui_text::format(
        "ui.scan_radius_tag",
        &[("radius", &crate::prefs::show_mm(app, r.radius, 1)), ("kind", kind), ("angle", &format!("{:.0}", r.swept))],
    )
}

/// The hint while waiting for a tap, and the last round's tag.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{Stroke, vec2};
    let Some(State::Picking(last)) = state() else { return };
    let t = solidcraft_theme::Tokens::get();
    let font = egui::FontId::proportional(t.font_small);
    let hint = painter.clip_rect().center_top() + vec2(0.0, 24.0);
    painter.text(hint, egui::Align2::CENTER_TOP, solidcraft_ui_text::tr("ui.scan_radius_pick"), font.clone(), t.text);
    let Some(r) = last else { return };
    let Some(at) = proj.to_screen(r.middle) else { return };
    let galley = painter.layout_no_wrap(label(app, &r), font, t.text);
    let mut tags = crate::view_tags::Tags::default();
    let rect = tags.place(at, galley.size() + vec2(10.0, 6.0));
    painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, t.accent));
    painter.circle_filled(at, 3.0, t.accent);
    painter.rect_filled(rect, 3.0, t.panel);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
}

/// The offer once a tap's round runs along an edge of the model: OK fillets that edge.
pub(crate) fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(r) = round().filter(|r| !r.answered) else { return };
    let Some(body) = r.edge_on.clone() else { return };
    let title = solidcraft_ui_text::tr("cmd.scan_radius");
    let message = solidcraft_ui_text::format("ui.scan_radius_edge", &[("radius", &crate::prefs::show_mm(app, r.radius, 1)), ("body", &body)]);
    let (closed, action) = crate::ui_kit::Confirm::new(egui::Id::unique("sc_scan_radius"), "fillet", title).show(ctx, true, |ui| {
        crate::ui_kit::text(ui, &message, crate::ui_kit::Text::Report("radius", false));
    });
    // OK and Cancel answer it without `closed`, which is only the close button and Escape.
    if action == crate::ui_kit::Action::None && !closed {
        return;
    }
    set(Some(State::Picking(Some(Round { answered: true, ..r.clone() }))));
    if action == crate::ui_kit::Action::Ok
        && let Err(e) = app.run("scan.radius", json!({"scan": r.scan, "at": r.at, "body": body, "apply": true}))
    {
        app.set_status(e, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The button turns tapping on and off; a round read is offered for its edge once.
    #[test]
    fn the_button_taps_then_ends() {
        let mut h = crate::scenario::Harness::new();
        assert!(!picking(&h.app));
        h.app.start("scan.radius");
        assert!(picking(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.radius"));
        h.app.start("scan.radius");
        assert!(!picking(&h.app), "pressed again, it ends");
        let r =
            Round { scan: "Scan".into(), at: [0.0; 3], radius: 3.0, convex: true, swept: 90.0, middle: Vec3::ZERO, edge_on: None, answered: false };
        set(Some(State::Picking(Some(r.clone()))));
        assert!(label(&h.app, &r).contains('3'));
        assert!(!label(&h.app, &r).contains('²'), "a radius is a length: {}", label(&h.app, &r));
        set(None);
    }

    /// OK on the offer fillets the model's edge along the round (OK answers without `closed`).
    #[test]
    fn ok_fillets_the_edge_along_the_round() {
        let mut h = crate::scenario::Harness::new();
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-radius-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stl = dir.join("rounded.stl");
        h.app.run("solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Rounded"})).unwrap();
        h.app.run("solid.fillet", json!({"edges": [[30, 0, 20]], "radius": 5, "body": "Rounded"})).unwrap();
        h.app.run("file.export_stl", json!({"path": stl, "bodies": ["Rounded"], "quality": "finest"})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        h.app.run("solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Part"})).unwrap();
        let scan = h.app.run("file.insert_mesh", json!({"path": stl})).unwrap()["bodies"][0].as_str().unwrap().to_owned();
        let c = 5.0 * std::f64::consts::FRAC_PI_4.cos();
        let at = [30.0, 5.0 - c, 15.0 + c];
        let middle = Vec3::new(at[0], at[1], at[2]);
        let r = Round { scan, at, radius: 5.0, convex: true, swept: 90.0, middle, edge_on: Some("Part".into()), answered: false };
        set(Some(State::Picking(Some(r))));
        h.frames(3);
        let ok = crate::scenario::handle_at("dialog:ok").expect("the fillet is offered");
        h.call("ui.click", json!({"x": ok.x, "y": ok.y}));
        h.frames(3);
        let volume = h.app.session.model.state().body("Part").and_then(|b| b.body.volume_at(0.01));
        set(None);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(volume.is_some_and(|v| v < 48_000.0 - 100.0), "Part filleted: {volume:?}");
    }
}
