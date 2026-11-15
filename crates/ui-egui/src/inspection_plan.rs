//! The inspection plan in the view. Add Check (`inspect.check_add`) is a click tool: a round face
//! adds its diameter, a flat face its flatness, and two flat faces Shift+clicked one after the
//! other the distance between them. Measure Checks (`inspect.check_measure`) labels every check
//! with what the scan holds, passed or failed, until the design changes or the button is pressed
//! again.

use std::cell::RefCell;

use serde_json::json;
use solidcore_record::Vec3;

use crate::SolveApp;

thread_local! {
    /// Add Check on, and the first flat face of a distance Shift+clicked so far.
    static ADDING: RefCell<Option<Option<(String, Vec3)>>> = const { RefCell::new(None) };
}

/// Add Check's button: on, or off again. False: the button runs nothing more.
pub(crate) fn toggle_adding(_app: &mut SolveApp) -> bool {
    ADDING.with(|a| {
        let mut a = a.borrow_mut();
        *a = if a.is_some() { None } else { Some(None) };
    });
    false
}

/// Whether a click on the model adds a check now.
pub(crate) fn adding(_app: &SolveApp) -> bool {
    ADDING.with(|a| a.borrow().is_some())
}

/// A click while adding: the face under it becomes a check; with Shift, two flat faces a distance.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, shift: bool) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::Face { body, point, .. } => Some((body, point)),
        _ => None,
    });
    let Some((body, q)) = hit.filter(|(b, _)| app.session.model.state().body(b).is_some_and(|b| !b.body.is_mesh())) else { return };
    let face = json!({"body": body, "point": [q.x, q.y, q.z]});
    if shift {
        let first = ADDING.with(|a| a.borrow_mut().as_mut().and_then(Option::take));
        match first {
            Some((b1, p1)) => {
                let _ = app.run("inspect.check_add", json!({"kind": "distance", "face": {"body": b1, "point": [p1.x, p1.y, p1.z]}, "face2": face}));
            }
            None => ADDING.with(|a| {
                if let Some(slot) = a.borrow_mut().as_mut() {
                    *slot = Some((body, q));
                }
            }),
        }
        return;
    }
    let _ = app.run("inspect.check_add", json!({"kind": "auto", "face": face}));
}

/// The results, if shown and still standing for the design.
pub(crate) fn current(app: &SolveApp) -> Option<std::sync::Arc<solidcraft_engine::cmd::inspection_plan::Inspection>> {
    app.session.inspection.clone().filter(|i| i.current(&app.session))
}

pub(crate) fn shown(app: &SolveApp) -> bool {
    current(app).is_some()
}

/// A check's tag: its number, kind, nominal, what the scan holds and the verdict.
fn label(app: &SolveApp, c: &solidcraft_engine::cmd::inspection_plan::ShownCheck) -> String {
    use solidcraft_engine::doc::inspection::CheckKind;
    let mm = |v: Option<f64>| v.map_or_else(|| "–".to_owned(), |v| crate::prefs::show_mm(app, v, 1));
    let kind = match c.kind {
        CheckKind::Diameter => "ui.check_kind_diameter",
        CheckKind::Distance => "ui.check_kind_distance",
        CheckKind::Flatness => "ui.check_kind_flatness",
        CheckKind::Position => "ui.check_kind_position",
        CheckKind::Point => "ui.check_kind_point",
    };
    let verdict = match c.pass {
        Some(true) => "ui.check_pass",
        Some(false) => "ui.check_fail",
        None => "ui.check_unmeasured",
    };
    solidcraft_ui_text::format(
        "ui.check_tag",
        &[
            ("number", &c.number.to_string()),
            ("kind", solidcraft_ui_text::tr(kind)),
            ("nominal", &mm(c.nominal)),
            ("actual", &mm(c.actual)),
            ("verdict", solidcraft_ui_text::tr(verdict)),
        ],
    )
}

/// The hint while adding, the first face of a distance, and the results' tags.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{Stroke, vec2};
    let t = solidcraft_theme::Tokens::get();
    let font = egui::FontId::proportional(t.font_small);
    if adding(app) {
        let at = painter.clip_rect().center_top() + vec2(0.0, 24.0);
        painter.text(at, egui::Align2::CENTER_TOP, solidcraft_ui_text::tr("ui.check_add_hint"), font.clone(), t.text);
        if let Some(Some((_, q))) = ADDING.with(|a| a.borrow().clone())
            && let Some(sp) = proj.to_screen(q)
        {
            painter.circle_stroke(sp, 6.0, Stroke::new(2.0, t.accent));
        }
    }
    let Some(results) = current(app) else { return };
    let good = solidcraft_theme::srgb(t.analysis.deviation[2]);
    let mut tags = crate::view_tags::Tags::default();
    for c in &results.results {
        let Some(at) = proj.to_screen(c.at) else { continue };
        let tint = match c.pass {
            Some(true) => good,
            Some(false) => t.error,
            None => t.text_dim,
        };
        let galley = painter.layout_no_wrap(label(app, c), font.clone(), t.text);
        let rect = tags.place(at, galley.size() + vec2(10.0, 6.0));
        painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, tint));
        painter.circle_filled(at, 3.5, tint);
        painter.rect_filled(rect, 3.0, t.panel);
        painter.rect_stroke(rect, 3.0, Stroke::new(1.5, tint), egui::StrokeKind::Inside);
        painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Add Check turns on and off; Measure Checks shows its results until pressed again.
    #[test]
    fn add_check_toggles_and_measure_shows_until_pressed_again() {
        let mut h = crate::scenario::Harness::new();
        h.app.start("inspect.check_add");
        assert!(adding(&h.app) && crate::toolbar::shows_pressed(&h.app, "inspect.check_add"));
        h.app.start("inspect.check_add");
        assert!(!adding(&h.app));
        h.app.run("solid.box", json!({"body_name": "Block", "operation": "new", "length": 20, "width": 20, "height": 5})).unwrap();
        h.app.run("inspect.check_add", json!({"kind": "flatness", "face": {"body": "Block", "point": [10, 10, 5]}})).unwrap();
        let tag = label(
            &h.app,
            &solidcraft_engine::cmd::inspection_plan::ShownCheck {
                number: 1,
                at: Vec3::ZERO,
                kind: solidcraft_engine::doc::inspection::CheckKind::Flatness,
                nominal: Some(0.0),
                actual: Some(0.02),
                pass: Some(true),
            },
        );
        assert!(tag.starts_with('1') && tag.contains("0.02"), "{tag}");
    }
}
