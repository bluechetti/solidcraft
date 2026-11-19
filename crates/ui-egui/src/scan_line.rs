//! Length on Scan (`scan.measure_line`) in the view: the button asks for clicks along the scan;
//! each click is a dot, and the line between the dots lies on the scan (the way across it the cut
//! of the plane through two dots and the view takes), labelled with its length along the surface,
//! what the dots alone would read straight, and, when it is an arc, its radius. A stretch that
//! could not follow the surface (across a hole) is drawn dashed in the warning colour. Pressing
//! the button again ends it; the line changes nothing in the design.

use std::cell::RefCell;

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::SolveApp;

/// The line being measured.
#[derive(Clone, Debug, Default, PartialEq)]
struct Line {
    scan: String,
    /// The dots clicked, on the scan.
    dots: Vec<[f64; 3]>,
    /// The direction the first dot was clicked from: the line keeps it as the view turns.
    view: Option<[f64; 3]>,
    /// What the command read: each stretch's way and whether it followed the surface.
    stretches: Vec<(Vec<Vec3>, bool)>,
    length: f64,
    straight: f64,
    round: Option<(f64, f64)>,
    /// Why the last click read nothing.
    error: Option<String>,
}

thread_local! {
    static STATE: RefCell<Option<Line>> = const { RefCell::new(None) };
}

fn state() -> Option<Line> {
    STATE.with(|s| s.borrow().clone())
}

fn set(next: Option<Line>) {
    STATE.with(|s| *s.borrow_mut() = next);
}

/// The button: starts a line, or ends it. False: the button runs nothing more.
pub(crate) fn toggle(_app: &mut SolveApp) -> bool {
    set(if state().is_some() { None } else { Some(Line::default()) });
    false
}

/// Whether a click on the scan adds a dot now (the button shows pressed).
pub(crate) fn picking(_app: &SolveApp) -> bool {
    state().is_some()
}

/// The points of `v` (`[[x, y, z], …]`).
fn points(v: &Value) -> Vec<Vec3> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some(Vec3::new(p.first()?.as_f64()?, p.get(1)?.as_f64()?, p.get(2)?.as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The line measured again with `line`'s dots.
fn measured(app: &mut SolveApp, mut line: Line) -> Line {
    line.error = None;
    if line.dots.len() < 2 {
        line.stretches.clear();
        return line;
    }
    let mut p = json!({"body": line.scan, "points": line.dots, "path": true});
    if let Some(v) = line.view {
        p["view"] = json!(v);
    }
    match app.run("scan.measure_line", p) {
        Ok(r) => {
            line.stretches = r["stretches"]
                .as_array()
                .map(|s| s.iter().map(|st| (points(&st["path"]), st["followed"].as_bool().unwrap_or(true))).collect())
                .unwrap_or_default();
            line.length = r["length"].as_f64().unwrap_or(0.0);
            line.straight = r["straight"].as_f64().unwrap_or(0.0);
            line.round = r.get("round").and_then(|o| Some((o["radius"].as_f64()?, o["turned"].as_f64()?)));
        }
        Err(e) => {
            line.dots.pop();
            line.error = Some(e);
        }
    }
    line
}

/// A click: a dot where the scan is under it, and the line measured again. A click on another
/// scan starts a new line there.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let Some(mut line) = state() else { return };
    let Some((scan, q)) = crate::scan_snap::scan_under(app, proj, at) else { return };
    if line.scan != scan {
        line = Line { scan, ..Line::default() };
    }
    if line.dots.is_empty() {
        let (_, d) = proj.ray(at);
        line.view = Some([d.x, d.y, d.z]);
    }
    line.dots.push([q.x, q.y, q.z]);
    set(Some(measured(app, line)));
}

/// The line's tag: its length along the surface, straight between the dots, and its round.
fn label(app: &SolveApp, line: &Line) -> String {
    let mut text = solidcraft_ui_text::format(
        "ui.scan_line_tag",
        &[("length", &crate::prefs::show_mm(app, line.length, 2)), ("straight", &crate::prefs::show_mm(app, line.straight, 2))],
    );
    if let Some((radius, turned)) = line.round {
        text.push('\n');
        text.push_str(&solidcraft_ui_text::format(
            "ui.scan_line_round",
            &[("radius", &crate::prefs::show_mm(app, radius, 2)), ("angle", &format!("{turned:.0}"))],
        ));
    }
    text
}

/// The hint, the line on the scan, its dots, and its tag at the last dot.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{Stroke, vec2};
    let Some(line) = state() else { return };
    let t = solidcraft_theme::Tokens::get();
    let font = egui::FontId::proportional(t.font_small);
    let hint = painter.clip_rect().center_top() + vec2(0.0, 24.0);
    let message = line.error.clone().unwrap_or_else(|| solidcraft_ui_text::tr("ui.scan_line_pick").to_owned());
    painter.text(hint, egui::Align2::CENTER_TOP, message, font.clone(), if line.error.is_some() { t.error } else { t.text });
    for (path, followed) in &line.stretches {
        let screen: Vec<egui::Pos2> = path.iter().filter_map(|p| proj.to_screen(*p)).collect();
        if *followed {
            painter.add(egui::Shape::line(screen, Stroke::new(2.0, t.accent)));
        } else {
            painter.extend(egui::Shape::dashed_line(&screen, Stroke::new(2.0, t.warning), 6.0, 4.0));
        }
    }
    let dots: Vec<egui::Pos2> = line.dots.iter().filter_map(|d| proj.to_screen(Vec3::new(d[0], d[1], d[2]))).collect();
    for d in &dots {
        painter.circle_filled(*d, 3.5, t.accent);
    }
    if line.stretches.is_empty() {
        return;
    }
    let Some(at) = dots.last().copied() else { return };
    let galley = painter.layout_no_wrap(label(app, &line), font, t.text);
    let mut tags = crate::view_tags::Tags::default();
    let rect = tags.place(at, galley.size() + vec2(10.0, 6.0));
    painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, t.accent));
    painter.rect_filled(rect, 3.0, t.panel);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The button starts a line and ends it; two dots on a flat sheet read the straight length,
    /// and nothing in the design changes.
    #[test]
    fn two_dots_read_the_length_on_the_scan() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("sheet.obj");
        let mut obj = String::new();
        for (x, y) in [(0.0, 0.0), (40.0, 0.0), (40.0, 40.0), (0.0, 40.0)] {
            obj += &format!("v {x} {y} 0\n");
        }
        obj += "f 1 2 3\nf 1 3 4\n";
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        let revision = h.app.session.revision;
        h.app.start("scan.measure_line");
        assert!(picking(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.measure_line"));
        let line = Line { scan: scan.clone(), dots: vec![[5.0, 5.0, 0.0], [35.0, 25.0, 0.0]], ..Line::default() };
        let line = measured(&mut h.app, line);
        assert!(line.error.is_none(), "{:?}", line.error);
        let straight = (30.0f64 * 30.0 + 20.0 * 20.0).sqrt();
        assert!((line.length - straight).abs() < 1e-6 && (line.straight - straight).abs() < 1e-9, "{line:?}");
        assert_eq!(line.stretches.len(), 1);
        assert!(line.stretches[0].1, "followed");
        assert!(label(&h.app, &line).contains("36.06"), "{}", label(&h.app, &line));
        assert_eq!(h.app.session.revision, revision, "nothing changes");
        h.app.start("scan.measure_line");
        assert!(!picking(&h.app), "pressed again, it ends");
    }
}
