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
    // The scan itself, through the model: a model built over the scan covers its markers.
    let Some((body, q)) = crate::scan_snap::scan_under(app, proj, at) else { return };
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
    // OK and Cancel answer it without `closed`, which is only the close button and Escape.
    if action == crate::ui_kit::Action::None && !closed {
        return;
    }
    set(None);
    if action == crate::ui_kit::Action::Ok
        && let Err(e) = app.run("scan.remove_markers", json!({"body": body, "centres": centres, "radius": radius}))
    {
        app.set_status(e, true);
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

    /// A floor 100 mm square with balls of radius 3 standing in it, as an STL.
    fn floor_with_markers_stl(centres: &[[f64; 2]]) -> String {
        let mut stl = String::from("solid floor\n");
        let mut facet = |t: [[f64; 3]; 3]| {
            stl += "facet normal 0 0 0\nouter loop\n";
            for [x, y, z] in t {
                stl += &format!("vertex {x} {y} {z}\n");
            }
            stl += "endloop\nendfacet\n";
        };
        let n = 40;
        let at = |i: usize, j: usize| [100.0 * i as f64 / n as f64, 100.0 * j as f64 / n as f64, 0.0];
        for i in 0..n {
            for j in 0..n {
                facet([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                facet([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        for &[cx, cy] in centres {
            let (rings, segments) = (20, 40);
            let p = |i: usize, j: usize| {
                let (phi, th) = (std::f64::consts::PI * i as f64 / rings as f64, std::f64::consts::TAU * j as f64 / segments as f64);
                [cx + 3.0 * phi.sin() * th.cos(), cy + 3.0 * phi.sin() * th.sin(), 3.0 * phi.cos()]
            };
            for i in 0..rings {
                for j in 0..segments {
                    facet([p(i, j), p(i + 1, j), p(i + 1, j + 1)]);
                    facet([p(i, j), p(i + 1, j + 1), p(i, j + 1)]);
                }
            }
        }
        stl + "endsolid floor\n"
    }

    /// A model built over the scan covers it: a click on a marker still finds the markers, and OK
    /// takes them off (OK answers the confirmation without `closed`).
    #[test]
    fn a_click_through_the_model_finds_the_markers_and_ok_removes_them() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("markers.stl");
        std::fs::write(&path, floor_with_markers_stl(&[[20.0, 20.0], [70.0, 30.0], [40.0, 75.0]])).unwrap();
        let scan = h.app.run("file.insert_mesh", json!({"path": path})).unwrap()["bodies"][0].as_str().unwrap().to_owned();
        h.app.run("solid.box", json!({"length": 100, "width": 100, "height": 10, "body_name": "Part"})).unwrap();
        let triangles = |h: &crate::scenario::Harness| {
            h.app.session.model.state().body(&scan).and_then(|b| b.body.triangle_mesh().map(|(_, t)| t.len())).unwrap_or(0)
        };
        let before = triangles(&h);
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.remove_markers");
        h.frames(2);
        h.step(&json!({"click": {"world": [20.0, 20.0, 10.0]}})).unwrap();
        h.frames(2);
        let found = match state() {
            Some(State::Found { centres, .. }) => centres.len(),
            other => panic!("the markers under the model were found: {other:?}"),
        };
        assert_eq!(found, 3);
        h.frames(2);
        let ok = crate::scenario::handle_at("dialog:ok").expect("the removal is offered");
        h.call("ui.click", json!({"x": ok.x, "y": ok.y}));
        h.frames(3);
        let after = triangles(&h);
        set(None);
        assert!(!active(&h.app), "OK ends the tool");
        assert_ne!(after, before, "the markers are taken off");
    }
}
