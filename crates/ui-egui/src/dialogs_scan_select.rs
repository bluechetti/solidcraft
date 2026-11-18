//! Select on Scan (`scan.select`): choosing part of a scan for the tools that work on part of one.
//! The tools: Brush (drag over the scan; each place takes what is within the radius, across the
//! scan), Polygon (click its corners round what is seen, then Apply or click the first again),
//! Lasso (drag round what is seen), Region (click: the smooth region there, to the creases), Shape
//! (click: the shape Find All found there) and Inside Curve (click a closed 3D curve drawn on the
//! scan: what is inside it, not reaching the far side). Each adds to, takes from or replaces what is
//! selected, as Selection says. All, None, Invert, Grow and Shrink change it at once. Keep puts it
//! in the design by a name, to open again from the list; each kept one can be deleted. What is
//! selected shows on the scan in the selection colour; it is a view, not an undo step.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::curve as engine_curve;
use solidcraft_engine::cmd::scan::select as engine_select;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit::{self, choice_row};

/// The tools, in the order the dialog offers them.
const TOOLS: [&str; 6] = [
    "ui.scan_select_brush",
    "ui.scan_select_polygon",
    "ui.scan_select_lasso",
    "ui.scan_select_region",
    "ui.scan_select_shape",
    "ui.scan_select_inside",
];
const BRUSH: usize = 0;
const POLYGON: usize = 1;
const LASSO: usize = 2;
const REGION: usize = 3;
const SHAPE: usize = 4;
const INSIDE: usize = 5;

/// What the tools do to what is selected.
const MODES: [&str; 3] = ["ui.scan_select_replace", "ui.scan_select_add", "ui.scan_select_remove"];

/// The places of a brush stroke sent at once while it is drawn.
const STROKE_CHUNK: usize = 6;

/// How near the first corner a click must be to close the polygon, in pixels.
const CLOSE: f32 = 10.0;

/// The Select on Scan dialog's values.
#[derive(Clone, Debug)]
pub struct ScanSelect {
    pub tool: usize,
    pub mode: usize,
    /// The brush's radius, in mm (0 until the scan's size gives it one).
    pub radius: f64,
    /// Polygon and Lasso take what is behind the near side too.
    pub through: bool,
    /// Shape: every shape Find All found like the one clicked (holes of its size, faces facing
    /// its way), not that one alone.
    pub like: bool,
    /// How sharply a region may bend between neighbouring triangles, in degrees.
    pub angle: f64,
    /// The name to keep the selection by.
    pub name: String,
    /// The keys held as the pointer acted: the mode they stand for instead of `mode` (Shift adds,
    /// Ctrl and Alt take away), as in every Scan-tab tool.
    keys: Option<usize>,
    /// The polygon's corners or the lasso's points, on the screen.
    corners: Vec<egui::Pos2>,
    /// The brush stroke's places not yet sent, and whether the stroke has sent any.
    stroke: Vec<Vec3>,
    stroked: bool,
    /// A command asked for by a button, run on the next frame.
    asked: Option<(String, Value)>,
    /// What the last command said, when it was refused.
    error: Option<String>,
}

impl Default for ScanSelect {
    fn default() -> Self {
        ScanSelect {
            tool: BRUSH,
            mode: 0,
            radius: 0.0,
            through: false,
            like: false,
            angle: engine_select::REGION_ANGLE,
            name: String::new(),
            keys: None,
            corners: Vec::new(),
            stroke: Vec::new(),
            stroked: false,
            asked: None,
            error: None,
        }
    }
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.select").then(|| (Kind::ScanSelect(ScanSelect::default()), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

fn dialog(app: &mut SolveApp) -> Option<&mut ScanSelect> {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(Kind::ScanSelect(k)) => Some(k),
        _ => None,
    }
}

/// Whether the dialog takes the viewport's clicks and drags on the scan.
pub(crate) fn active(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSelect(_)))
}

fn mode_word(mode: usize) -> &'static str {
    ["replace", "add", "remove"][mode.min(2)]
}

/// The mode a pointer action uses: the one the keys held stand for, else the dialog's.
fn mode_now(k: &ScanSelect) -> &'static str {
    mode_word(k.keys.unwrap_or(k.mode))
}

/// Takes the keys held this frame (call before the clicks and drags are read): Shift adds, Ctrl and
/// Alt take away.
pub(crate) fn keys(app: &mut SolveApp, mods: egui::Modifiers) {
    if let Some(k) = dialog(app) {
        k.keys = crate::scan_how::How::of(mods).select_mode();
    }
}

/// The wheel turned with Ctrl+Shift held over the view: the brush's radius grows or shrinks, and the
/// view does not zoom: the same keys as the Smoothing Brush of Edit Surface. Before the camera reads
/// the wheel.
pub(crate) fn radius_wheel(app: &mut SolveApp, ctx: &egui::Context, rect: egui::Rect) {
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
    if !over || !app.preferences.edit_surface_brush_keys {
        return;
    }
    let Some(k) = dialog(app).filter(|k| k.tool == BRUSH) else { return };
    let line = ctx.options(|o| o.input_options.line_scroll_speed);
    let (turned, points) = ctx.input(|i| {
        let (mut turned, mut points) = (false, 0.0f32);
        for e in &i.events {
            let egui::Event::MouseWheel { unit, delta, modifiers, phase, .. } = e else { continue };
            if !((modifiers.ctrl || modifiers.command) && modifiers.shift && !modifiers.alt) {
                continue;
            }
            turned = true;
            if *phase == egui::TouchPhase::Move {
                points += match unit {
                    egui::MouseWheelUnit::Point => delta.x + delta.y,
                    egui::MouseWheelUnit::Line => (delta.x + delta.y) * line,
                    egui::MouseWheelUnit::Page => (delta.x + delta.y) * 400.0,
                };
            }
        }
        (turned, points)
    });
    if !turned {
        return;
    }
    if points != 0.0 && points.is_finite() {
        k.radius = (k.radius * f64::from((0.0025 * points).exp())).clamp(1e-3, 1e4);
    }
    crate::navigation::claim_wheel(ctx);
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut ScanSelect) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    if k.radius <= 0.0 {
        let scan = solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.select").ok();
        let size = scan.map_or(0.0, |s| engine_curve::size(&app.session, &s));
        k.radius = if size > 0.0 { (size / 50.0).max(0.1) } else { 2.0 };
    }
    let tools: Vec<&str> = TOOLS.iter().map(|t| tr(t)).collect();
    let before = k.tool;
    choice_row(ui, tr("ui.scan_select_tool"), "scan_select_tool", &mut k.tool, &tools);
    if k.tool != before {
        k.corners.clear();
    }
    let modes: Vec<&str> = MODES.iter().map(|m| tr(m)).collect();
    choice_row(ui, tr("ui.scan_select_mode"), "scan_select_mode", &mut k.mode, &modes);
    match k.tool {
        BRUSH => {
            form_row(ui, tr("ui.scan_select_radius"), |ui| ui_kit::length_field(ui, &mut k.radius, &units, decimals));
            k.radius = k.radius.clamp(1e-3, 1e4);
        }
        POLYGON | LASSO => {
            form_row(ui, tr("ui.scan_select_through"), |ui| check(ui, &mut k.through));
        }
        REGION => {
            form_row(ui, tr("ui.scan_select_angle"), |ui| ui_kit::number_field(ui, &mut k.angle, 1.0..=90.0, 0.5, "°"));
        }
        SHAPE => {
            form_row(ui, tr("ui.scan_select_like"), |ui| check(ui, &mut k.like));
        }
        _ => {}
    }
    if k.tool == POLYGON {
        let can = k.corners.len() >= 3;
        let apply = ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_select_apply_polygon"), ui_kit::Button::Enabled(can))).clicked();
        if apply {
            k.asked = Some(("polygon".into(), Value::Null));
        }
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_select_undo_corner"), ui_kit::Button::Enabled(!k.corners.is_empty()))).clicked() {
            k.corners.pop();
        }
    }
    let now = engine_select::selected(&app.session);
    let summary = match &now {
        Some(s) => solidcraft_ui_text::format("ui.scan_select_count", &[("triangles", &s.count.to_string()), ("of", &s.selected.len().to_string())]),
        None => tr("ui.scan_select_nothing").to_owned(),
    };
    form_row(ui, tr("ui.scan_select_selected"), |ui| ui_kit::text(ui, &summary, ui_kit::Text::Cell));
    if let Some(s) = &now {
        let area = crate::prefs::show_mm(app, s.area, 2);
        form_row(ui, tr("ui.scan_select_area"), |ui| ui_kit::text(ui, &area, ui_kit::Text::Cell));
    }
    let ask = |k: &mut ScanSelect, p: Value| k.asked = Some(("scan.select".into(), p));
    for line in [
        &[
            ("ui.scan_select_all", json!({"all": true})),
            ("ui.scan_select_none", json!({"none": true})),
            ("ui.scan_select_invert", json!({"invert": true})),
        ][..],
        &[("ui.scan_select_grow", json!({"grow": 1})), ("ui.scan_select_shrink", json!({"shrink": 1}))][..],
    ] {
        ui_kit::row(ui, "", |ui| {
            ui.horizontal(|ui| {
                for (label, p) in line {
                    let needs = *label != "ui.scan_select_all";
                    if ui_kit::button(ui, tr(label), ui_kit::Button::Enabled(!needs || now.is_some())).clicked() {
                        ask(k, p.clone());
                    }
                }
            })
            .response
        });
    }
    // Keeping it in the design, and the kept ones.
    form_row(ui, tr("ui.scan_select_name"), |ui| ui_kit::text_field(ui, &mut k.name, tr("ui.scan_select_name_hint")).0);
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_select_keep"), ui_kit::Button::Enabled(now.is_some()))).clicked() {
        let name = k.name.trim().to_owned();
        k.asked = Some(("scan.keep_selection".into(), if name.is_empty() { json!({}) } else { json!({ "name": name }) }));
    }
    for kept in &app.session.doc.scan_selections {
        let line = solidcraft_ui_text::format("ui.scan_select_kept", &[("name", &kept.name), ("triangles", &kept.count().to_string())]);
        ui_kit::row(ui, &line, |ui| {
            ui.horizontal(|ui| {
                if ui_kit::button(ui, tr("ui.scan_select_open"), ui_kit::Button::Enabled(true)).clicked() {
                    k.asked = Some(("scan.select".into(), json!({"saved": kept.name, "mode": mode_now(k)})));
                }
                if ui_kit::button(ui, tr("ui.scan_select_delete"), ui_kit::Button::Enabled(true)).clicked() {
                    k.asked = Some(("scan.delete_selection".into(), json!({"name": kept.name})));
                }
            })
            .response
        });
    }
    if let Some(why) = &k.error {
        ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_select_error", true));
    }
    let hint = match k.tool {
        BRUSH => "ui.scan_select_hint_brush",
        POLYGON => "ui.scan_select_hint_polygon",
        LASSO => "ui.scan_select_hint_lasso",
        REGION => "ui.scan_select_hint_region",
        SHAPE => "ui.scan_select_hint_shape",
        _ => "ui.scan_select_hint_inside",
    };
    ui_kit::text(ui, tr(hint), ui_kit::Text::Hint("scan_select_hint"));
    false
}

/// OK closes the dialog: what is selected stays selected.
pub fn commands(_k: &ScanSelect) -> Result<Vec<(String, Value)>, String> {
    Ok(Vec::new())
}

/// Run `command` with `p`, keeping why it was refused for the dialog to say.
fn run(app: &mut SolveApp, command: &str, p: Value) {
    let said = app.run(command, p).err();
    if let Some(k) = dialog(app) {
        k.error = said;
    }
}

/// The scan under `at` on the screen, and the place of it there.
fn scan_at(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<(String, Vec3)> {
    crate::scan_snap::scan_hit(app, proj, at, None)
}

/// The polygon `corners` (on the screen) as places on the plane through the view's target square
/// to it, with the way the view looks and, for a perspective view, its eye.
fn on_plane(app: &SolveApp, proj: &crate::viewport::Proj, corners: &[egui::Pos2]) -> Option<(Vec<Vec3>, Vec3, Option<Vec3>)> {
    let toward = proj.cam.back();
    let target = app.cam.target;
    let mut places = Vec::with_capacity(corners.len());
    let mut origins = Vec::with_capacity(corners.len());
    for &c in corners {
        let (o, d) = proj.ray(c);
        let den = d.dot(toward);
        if den.abs() < 1e-12 {
            return None;
        }
        places.push(o + d * ((target - o).dot(toward) / den));
        origins.push(o);
    }
    let eye = origins.first().copied().filter(|o| origins.iter().all(|q| q.dist(*o) < 1e-6));
    Some((places, -toward, eye))
}

fn polygon_command(app: &SolveApp, proj: &crate::viewport::Proj, k: &ScanSelect) -> Option<Value> {
    let (places, direction, eye) = on_plane(app, proj, &k.corners)?;
    let points: Vec<[f64; 3]> = places.iter().map(|p| [p.x, p.y, p.z]).collect();
    let mut polygon = json!({"points": points, "direction": [direction.x, direction.y, direction.z], "through": k.through});
    if let Some(e) = eye {
        polygon["eye"] = json!([e.x, e.y, e.z]);
    }
    Some(json!({"polygon": polygon, "mode": mode_now(k)}))
}

/// Each frame: the command a button asked for.
pub(crate) fn tick(app: &mut SolveApp, proj: &crate::viewport::Proj) {
    let Some(k) = dialog(app) else { return };
    let Some((command, p)) = k.asked.take() else { return };
    if command == "polygon" {
        let k = k.clone();
        let p = polygon_command(app, proj, &k);
        if let Some(k) = dialog(app) {
            k.corners.clear();
        }
        if let Some(p) = p {
            run(app, "scan.select", p);
        }
        return;
    }
    run(app, &command, p);
}

/// A click at `at`: Region and Shape select there; Polygon adds a corner (on the first, with
/// three or more, it closes and selects); Brush brushes once.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let Some(k) = dialog(app).map(|k| k.clone()) else { return };
    match k.tool {
        POLYGON => {
            let first = k.corners.first().copied();
            if k.corners.len() >= 3 && first.is_some_and(|f| f.distance(at) <= CLOSE) {
                if let Some(k) = dialog(app) {
                    k.asked = Some(("polygon".into(), Value::Null));
                }
            } else if let Some(k) = dialog(app) {
                k.corners.push(at);
            }
        }
        LASSO => {}
        INSIDE => {
            let curve = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
                crate::viewport::Hit::SketchCurve { sketch, id, .. } => Some((sketch, id)),
                _ => None,
            });
            let state = app.session.model.state();
            let curve = curve.filter(|(sketch, id)| state.sketch(*sketch).is_some_and(|ss| ss.sketch.wire_index(id).is_some()));
            drop(state);
            if let Some((sketch, id)) = curve {
                run(app, "scan.select_inside_curve", json!({"sketch": sketch, "curves": [id], "mode": mode_now(&k)}));
            }
        }
        _ => {
            let Some((body, point)) = scan_at(app, proj, at) else { return };
            let place = json!([point.x, point.y, point.z]);
            let p = match k.tool {
                // Up to the creases sharper than the angle, as its hint says: a whole hole's wall.
                REGION => json!({"body": body, "region": {"point": place, "angle": k.angle, "until": "crease"}, "mode": mode_now(&k)}),
                SHAPE => json!({"body": body, "shape": {"point": place, "like": k.like}, "mode": mode_now(&k)}),
                _ => json!({"body": body, "brush": {"points": [place], "radius": format!("{} mm", k.radius)}, "mode": mode_now(&k)}),
            };
            run(app, "scan.select", p);
        }
    }
}

/// The brush stroke's places so far sent: the first of a stroke replaces in Replace, the rest add.
fn send_stroke(app: &mut SolveApp, body: Option<String>) {
    let Some(k) = dialog(app) else { return };
    if k.stroke.is_empty() {
        return;
    }
    let points: Vec<[f64; 3]> = std::mem::take(&mut k.stroke).iter().map(|p| [p.x, p.y, p.z]).collect();
    let mode = if k.keys.unwrap_or(k.mode) == 0 && k.stroked { "add" } else { mode_now(k) };
    k.stroked = true;
    let mut p = json!({"brush": {"points": points, "radius": format!("{} mm", k.radius)}, "mode": mode});
    if let Some(b) = body {
        p["body"] = json!(b);
    }
    run(app, "scan.select", p);
}

/// A primary drag, now at `to`: Brush brushes along the scan under it; Lasso draws its loop.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &crate::viewport::Proj, from: egui::Pos2, to: egui::Pos2) {
    let Some(tool) = dialog(app).map(|k| k.tool) else { return };
    match tool {
        BRUSH => {
            let Some((body, point)) = scan_at(app, proj, to) else { return };
            let full = dialog(app).is_some_and(|k| {
                k.stroke.push(point);
                k.stroke.len() >= STROKE_CHUNK
            });
            if full {
                send_stroke(app, Some(body));
            }
        }
        LASSO => {
            if let Some(k) = dialog(app) {
                if k.corners.is_empty() {
                    k.corners.push(from);
                }
                if k.corners.last().is_some_and(|l| l.distance(to) >= 3.0) {
                    k.corners.push(to);
                }
            }
        }
        _ => {}
    }
}

/// The drag let go: the brush's last places sent, or the lasso selecting what is inside it.
pub(crate) fn drag_end(app: &mut SolveApp) {
    let Some(k) = dialog(app) else { return };
    match k.tool {
        BRUSH => {
            send_stroke(app, None);
            if let Some(k) = dialog(app) {
                k.stroked = false;
            }
        }
        LASSO if k.corners.len() >= 3 => k.asked = Some(("polygon".into(), Value::Null)),
        LASSO => k.corners.clear(),
        _ => {}
    }
}

/// The polygon or lasso being drawn, and the brush under the pointer.
pub(crate) fn draw(app: &mut SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj, hover: Option<egui::Pos2>) {
    let Some(k) = dialog(app).map(|k| (k.tool, k.corners.clone(), k.radius)) else { return };
    let t = solidcraft_theme::Tokens::get();
    let (tool, corners, radius) = k;
    if corners.len() >= 2 {
        let mut line = corners.clone();
        if tool == LASSO {
            line.push(corners[0]);
        } else if let Some(h) = hover {
            line.push(h);
        }
        painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, t.accent)));
    }
    for (i, c) in corners.iter().enumerate().filter(|_| tool == POLYGON) {
        painter.circle_filled(*c, 3.0, if i == 0 { t.accent } else { t.text });
    }
    if tool == BRUSH
        && let Some(h) = hover
        && let Some((_, at)) = scan_at(app, proj, h)
        && let Some(side) = proj.cam.back().cross(Vec3::Z).normalized().or_else(|| proj.cam.back().cross(Vec3::X).normalized())
        && let (Some(a), Some(b)) = (proj.to_screen(at), proj.to_screen(at + side * radius))
    {
        painter.circle_stroke(a, a.distance(b), egui::Stroke::new(1.5, t.accent));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A box 30 × 20 × 10, its faces cut fine, as an STL.
    fn box_stl(n: u32) -> String {
        let size = [30.0f64, 20.0, 10.0];
        let mut stl = String::from("solid b\n");
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let at = |di: u32, dj: u32| {
                        let mut p = [0.0; 3];
                        p[axis] = if high { size[axis] } else { 0.0 };
                        p[u] = size[u] * f64::from(i + di) / f64::from(n);
                        p[v] = size[v] * f64::from(j + dj) / f64::from(n);
                        p
                    };
                    let quad = [at(0, 0), at(1, 0), at(1, 1), at(0, 1)];
                    for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                        let tri = if high { tri } else { [tri[0], tri[2], tri[1]] };
                        stl += "facet normal 0 0 0\nouter loop\n";
                        for [x, y, z] in tri {
                            stl += &format!("vertex {x} {y} {z}\n");
                        }
                        stl += "endloop\nendfacet\n";
                    }
                }
            }
        }
        stl + "endsolid b\n"
    }

    fn kind(h: &mut crate::scenario::Harness) -> &mut ScanSelect {
        dialog(&mut h.app).expect("the Select on Scan dialog")
    }

    /// The keys held with a click choose as they do in every Scan-tab tool: Shift adds to the
    /// selection, Ctrl takes the region from it, and a plain click replaces it.
    #[test]
    fn shift_adds_and_ctrl_takes_away_with_a_click() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("box.stl");
        std::fs::write(&path, box_stl(24)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.select");
        h.frames(2);
        kind(&mut h).tool = REGION;
        h.frames(1);
        let count = |h: &crate::scenario::Harness| engine_select::selected(&h.app.session).map_or(0, |s| s.count);
        h.step(&json!({"click": {"world": [15.0, 10.0, 10.0]}})).unwrap();
        h.frames(2);
        let top = count(&h);
        assert!(top > 0);
        h.step(&json!({"click": {"world": [15.0, 10.0, 10.0]}, "ctrl": true})).unwrap();
        h.frames(2);
        assert_eq!(count(&h), 0, "Ctrl took the region away");
        h.step(&json!({"click": {"world": [15.0, 10.0, 10.0]}, "shift": true})).unwrap();
        h.frames(2);
        assert_eq!(count(&h), top, "Shift added it again");
        h.step(&json!({"click": {"world": [15.0, 10.0, 10.0]}, "shift": true})).unwrap();
        h.frames(2);
        assert_eq!(count(&h), top, "added to what was chosen, not doubled");
        assert_eq!(kind(&mut h).mode, 0, "the dialog's own mode is not changed by the keys");
    }

    /// Through the viewport's own input on a box seen from above: a click with Region takes its
    /// top; a lasso round the left half, Remove, takes that half away; the buttons invert and keep
    /// it, and a kept selection opens again.
    #[test]
    fn a_region_a_lasso_and_the_buttons_select_on_a_box() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("box.stl");
        std::fs::write(&path, box_stl(24)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.select");
        h.frames(2);
        assert!(active(&h.app));
        kind(&mut h).tool = REGION;
        h.frames(1);
        h.step(&json!({"click": {"world": [15.0, 10.0, 10.0]}})).unwrap();
        h.frames(2);
        let top = engine_select::selected(&h.app.session).expect("the top selected");
        let of = top.selected.len();
        assert_eq!(top.count, of / 6, "one face of six: {}", top.count);
        drop(top);
        kind(&mut h).tool = LASSO;
        kind(&mut h).mode = 2;
        h.frames(1);
        let corners = [[-2.0, -2.0], [14.0, -2.0], [14.0, 22.0], [-2.0, 22.0]].map(|[x, y]| json!({"world": [x, y, 10.0]}));
        h.step(&json!({"drag": [corners[0], corners[1], corners[2], corners[3], corners[0]], "steps": 6})).unwrap();
        h.frames(3);
        let half = engine_select::selected(&h.app.session).expect("some of the top left");
        assert!(half.count > 0 && half.count < of / 6, "the left part taken away: {}", half.count);
        let left = half.count;
        drop(half);
        kind(&mut h).asked = Some(("scan.select".into(), json!({"invert": true})));
        h.frames(2);
        assert_eq!(engine_select::selected(&h.app.session).map(|s| s.count), Some(of - left), "inverted");
        kind(&mut h).name = "Rest".into();
        kind(&mut h).asked = Some(("scan.keep_selection".into(), json!({"name": "Rest"})));
        h.frames(2);
        assert_eq!(h.app.session.doc.scan_selections.len(), 1, "kept: {:?}", kind(&mut h).error);
        kind(&mut h).asked = Some(("scan.select".into(), json!({"none": true})));
        h.frames(2);
        assert!(engine_select::selected(&h.app.session).is_none());
        kind(&mut h).asked = Some(("scan.select".into(), json!({"saved": "Rest"})));
        h.frames(2);
        assert_eq!(engine_select::selected(&h.app.session).map(|s| s.count), Some(of - left), "opened again");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.frames(2);
        assert!(h.app.dialog.is_none());
        assert!(engine_select::selected(&h.app.session).is_some(), "the selection stays");
    }
}
