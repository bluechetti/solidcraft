//! Remove Markers (`scan.remove_markers`) in the view: the button asks for a click on one marker
//! (a reference sphere), every marker of its radius found is ringed, and a confirmation takes them
//! off as one Scan Clean step. Pressing the button again, or Cancel, ends it.

use std::cell::RefCell;

use serde_json::json;
use solidcore_record::Vec3;

use crate::SolveApp;

#[derive(Clone, Debug, PartialEq)]
enum State {
    /// Waiting for a click on a marker.
    Picking,
    /// The markers a click found, on which scan.
    Found { body: String, radius: f64, centres: Vec<[f64; 3]> },
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

/// The button: starts asking for a marker, or ends it. False: the button runs nothing more.
pub(crate) fn toggle(_app: &mut SolveApp) -> bool {
    set(if state().is_some() { None } else { Some(State::Picking) });
    false
}

/// Whether a click on the scan picks a marker now.
pub(crate) fn picking(_app: &SolveApp) -> bool {
    state() == Some(State::Picking)
}

/// Whether the tool is on (the button shows pressed).
pub(crate) fn active(_app: &SolveApp) -> bool {
    state().is_some()
}

/// A click while picking: the markers like the one under it found, without changing the scan.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::Face { body, point, .. } => Some((body, point)),
        _ => None,
    });
    let Some((body, q)) = hit.filter(|(b, _)| app.session.model.state().body(b).is_some_and(|b| b.body.is_mesh())) else { return };
    let Ok(reply) = app.run("scan.remove_markers", json!({"body": body, "at": [q.x, q.y, q.z], "preview": true})) else { return };
    let centres: Vec<[f64; 3]> = reply["centres"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| Some([c.get(0)?.as_f64()?, c.get(1)?.as_f64()?, c.get(2)?.as_f64()?])).collect())
        .unwrap_or_default();
    let Some(radius) = reply["radius"].as_f64().filter(|_| !centres.is_empty()) else { return };
    set(Some(State::Found { body, radius, centres }));
}

/// The hint while picking, and a ring round each marker found.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use solidcraft_engine::geom::Vec3Geom as _;
    let t = solidcraft_theme::Tokens::get();
    match state() {
        Some(State::Picking) => {
            let at = painter.clip_rect().center_top() + egui::vec2(0.0, 24.0);
            let text = solidcraft_ui_text::tr("ui.scan_markers_pick");
            painter.text(at, egui::Align2::CENTER_TOP, text, egui::FontId::proportional(t.font_small), t.text);
        }
        Some(State::Found { radius, centres, .. }) => {
            let side = proj.cam.back().any_perp();
            for c in centres {
                let c = Vec3::new(c[0], c[1], c[2]);
                let (Some(mid), Some(edge)) = (proj.to_screen(c), proj.to_screen(c + side * radius)) else { continue };
                painter.circle_stroke(mid, mid.distance(edge).max(6.0) + 3.0, egui::Stroke::new(2.0, t.warning));
            }
        }
        None => {}
    }
    let _ = app;
}

/// The confirmation once markers are found: OK takes them off.
pub(crate) fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(State::Found { body, radius, centres }) = state() else { return };
    let title = solidcraft_ui_text::tr("cmd.scan_remove_markers");
    let message = solidcraft_ui_text::format(
        "ui.scan_markers_found",
        &[("count", &centres.len().to_string()), ("diameter", &crate::prefs::show_mm(app, 2.0 * radius, 1))],
    );
    let (closed, action) = crate::ui_kit::Confirm::new(egui::Id::unique("sc_scan_markers"), "mesh_body", title).show(ctx, true, |ui| {
        crate::ui_kit::text(ui, &message, crate::ui_kit::Text::Report("markers", false));
    });
    if !closed {
        return;
    }
    set(None);
    if action == crate::ui_kit::Action::Ok {
        let _ = app.run("scan.remove_markers", json!({"body": body, "centres": centres, "radius": radius}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The button turns picking on and off; a click's markers wait for the confirmation.
    #[test]
    fn the_button_picks_then_ends() {
        let mut h = crate::scenario::Harness::new();
        assert!(!active(&h.app));
        h.app.start("scan.remove_markers");
        assert!(picking(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.remove_markers"));
        h.app.start("scan.remove_markers");
        assert!(!active(&h.app), "pressed again, it ends");
        set(Some(State::Found { body: "Scan".into(), radius: 3.0, centres: vec![[0.0, 0.0, 0.0]] }));
        assert!(active(&h.app) && !picking(&h.app), "found: waits for OK");
        set(None);
    }
}
