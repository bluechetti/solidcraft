//! Hole Table (`inspect.hole_table`) in the view: each hole the table read is numbered where it
//! starts, with its diameters and whether it goes through, until the design changes or the button
//! is pressed again.

use crate::SolveApp;

/// The table, if one is shown and still stands for the design.
pub(crate) fn current(app: &SolveApp) -> Option<std::sync::Arc<solidcraft_engine::cmd::hole_table::HoleTable>> {
    app.session.hole_table.clone().filter(|t| t.current(&app.session))
}

/// Whether the numbers are shown: the Hole Table button shows pressed.
pub(crate) fn shown(app: &SolveApp) -> bool {
    current(app).is_some()
}

/// A hole's tag: its number, its diameters from where it starts, and through, blind or closed.
fn label(app: &SolveApp, h: &solidcraft_engine::cmd::hole_table::ShownHole) -> String {
    let sizes: Vec<String> = h.diameters.iter().map(|d| format!("Ø{}", crate::prefs::show_mm(app, *d, 1))).collect();
    let kind = match h.kind {
        "through" => "ui.hole_table_through",
        "blind" => "ui.hole_table_blind",
        _ => "ui.hole_table_closed",
    };
    solidcraft_ui_text::format(
        "ui.hole_table_tag",
        &[("number", &h.number.to_string()), ("sizes", &sizes.join(" / ")), ("kind", solidcraft_ui_text::tr(kind))],
    )
}

/// The numbers over the view while the table is shown.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{Rect, Stroke, vec2};
    let Some(table) = current(app) else { return };
    let t = solidcraft_theme::Tokens::get();
    let font = egui::FontId::proportional(t.font_small);
    for h in &table.holes {
        let Some(at) = proj.to_screen(h.at) else { continue };
        let galley = painter.layout_no_wrap(label(app, h), font.clone(), t.text);
        let rect = Rect::from_min_size(at + vec2(10.0, -10.0 - galley.size().y - 6.0), galley.size() + vec2(10.0, 6.0));
        painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, t.accent));
        painter.circle_filled(at, 3.0, t.accent);
        painter.rect_filled(rect, 3.0, t.panel);
        painter.rect_stroke(rect, 3.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A plate with a hole: the button numbers it, an edit takes the numbers away, and pressed
    /// while shown the button takes them away.
    #[test]
    fn the_button_numbers_the_holes_and_takes_them_away() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"body_name": "Plate", "operation": "new", "length": 40, "width": 20, "height": 5})).unwrap();
        h.app.run("solid.hole", json!({"position": [10, 10, 5], "diameter": 6})).unwrap();
        h.app.start("inspect.hole_table");
        let table = current(&h.app).expect("shown");
        assert_eq!(table.holes.len(), 1);
        let tag = label(&h.app, &table.holes[0]);
        assert!(tag.starts_with('1') && tag.contains("Ø6") && tag.contains("through"), "{tag}");
        assert!(crate::toolbar::shows_pressed(&h.app, "inspect.hole_table"));
        h.app.start("inspect.hole_table");
        assert!(!shown(&h.app), "pressed again, the numbers go");
        h.app.start("inspect.hole_table");
        h.app.run("solid.box", json!({"body_name": "Other", "operation": "new", "length": 3, "width": 3, "height": 3, "x": 80})).unwrap();
        assert!(!shown(&h.app), "an edit makes the numbers stale");
    }
}
