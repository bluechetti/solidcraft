//! Slice Surface (`scan.slice_surface`): choose the surface (or none, for the one made last) and
//! the plane, a construction plane such as a symmetry plane or a flat face. The plane is drawn
//! over the picture with an arrow to the side kept, and the dialog says how many loops of the cut
//! a cap closes before anything is changed; Keep turns the side round, Cap closes the cut with flat
//! faces sewn in, so that a part made round with free forms becomes a solid.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::slice::{self, Keep, Slice};
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{BODIES, FACES, PLANES, SelInput, SelectionExt as _, items};
use crate::ui_kit::{self, choice_row};

/// The Slice Surface dialog's values (the surface and the plane are its selection inputs).
#[derive(Clone, Debug)]
pub struct SliceSurface {
    /// 0: the side the plane faces; 1: the other.
    pub keep: usize,
    pub cap: bool,
    shown: Option<Box<Shown>>,
}

impl Default for SliceSurface {
    fn default() -> Self {
        SliceSurface { keep: 0, cap: true, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (Option<String>, String, usize, u64),
    /// What the slice would do, and the surface's middle (to draw the plane at).
    result: Result<(Slice, Vec3), String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.slice_surface").then(|| {
        (
            Kind::ScanSliceSurface(SliceSurface::default()),
            vec![SelInput::new("Surface", BODIES, false), SelInput::new("Plane", PLANES | FACES, false)],
        )
    })
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The surface chosen, if one is.
fn surface(inputs: &[SelInput]) -> Option<String> {
    items(inputs, 0).body_names().into_iter().next()
}

/// The plane chosen, as `scan.slice_surface` takes it: a plane by its name, or a flat face's.
fn plane(app: &SolveApp, inputs: &[SelInput]) -> Option<Value> {
    match items(inputs, 1).first()? {
        Sel::Plane { name } => Some(json!(name)),
        Sel::Face { body, index, point } => {
            let (_, n) = crate::dialogs::planar_face(&app.session, body, *index)?;
            Some(json!({"origin": [point.x, point.y, point.z], "normal": [n.x, n.y, n.z]}))
        }
        _ => None,
    }
}

/// Work out the slice again when the choices or the design have changed.
fn look(app: &SolveApp, k: &mut SliceSurface, inputs: &[SelInput]) {
    let Some(plane) = plane(app, inputs) else {
        k.shown = None;
        return;
    };
    let named = surface(inputs);
    let key = (named.clone(), plane.to_string(), k.keep, app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let cmd = "scan.slice_surface";
    let keep = if k.keep == 1 { Keep::Back } else { Keep::Front };
    let result = slice::slice_target(&app.session, named.as_deref(), cmd)
        .and_then(|body| slice::sliced(&app.session, &body, &plane, keep, cmd))
        .map_err(|e| e.to_string())
        .map(|found| {
            let state = app.session.model.state();
            let b = state.body(&found.body).map(|b| b.body.outline_bounds());
            let middle = b.map_or(found.plane.origin, |b| (b.min + b.max) * 0.5);
            (found, middle)
        });
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The dialog's rows (after the surface and plane inputs).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut SliceSurface, inputs: &[SelInput]) -> bool {
    look(app, k, inputs);
    let sides = [tr("ui.scan_slice_front"), tr("ui.scan_slice_back")];
    choice_row(ui, tr("ui.scan_slice_keep"), "scan_slice_keep", &mut k.keep, &sides);
    form_row(ui, tr("ui.scan_slice_cap"), |ui| check(ui, &mut k.cap));
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((found, _))) => {
            let loops = found.loops.len().to_string();
            let line = format!("{} · {loops}", found.body);
            form_row(ui, tr("ui.scan_slice_loops"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Cell));
            if k.cap && found.loops.is_empty() {
                ui_kit::text(ui, tr("ui.scan_slice_open"), ui_kit::Text::Report("scan_slice_open", true));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_slice_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_slice_hint"), ui_kit::Text::Hint("scan_slice_hint"));
    false
}

/// What OK runs: the slice as shown.
pub fn commands(app: &SolveApp, k: &SliceSurface, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let plane = plane(app, inputs).ok_or_else(|| tr("ui.scan_slice_hint").to_owned())?;
    let (found, _) = k.shown.as_ref().ok_or("still slicing")?.result.as_ref().map_err(Clone::clone)?;
    if k.cap && found.loops.is_empty() {
        return Err(tr("ui.scan_slice_open").into());
    }
    let keep = if k.keep == 1 { "back" } else { "front" };
    Ok(vec![("scan.slice_surface".into(), json!({"surface": found.body, "plane": plane, "keep": keep, "cap": k.cap}))])
}

/// The plane, square round the surface's middle, an arrow to the side kept, and the loops a cap
/// closes, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanSliceSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((found, middle))) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    let pl = &found.plane;
    let n = pl.normal();
    let centre = *middle - n * pl.height(*middle);
    let half = 0.6 * found.size;
    let corners: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)]
        .iter()
        .filter_map(|(a, b)| proj.to_screen(centre + pl.x * (a * half) + pl.y * (b * half)))
        .collect();
    painter.add(egui::Shape::line(corners, egui::Stroke::new(1.5, t.accent)));
    let side = if (found.keep - pl.origin).dot(n) >= 0.0 { n } else { -n };
    if let (Some(a), Some(b)) = (proj.to_screen(centre), proj.to_screen(centre + side * (0.25 * found.size))) {
        painter.arrow(a, b - a, egui::Stroke::new(2.0, t.accent));
    }
    for q in found.loops.iter().flatten().filter_map(|q| proj.to_screen(*q)) {
        painter.circle_filled(q, 3.0, t.accent);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A flat strip as an STL, so the design has a scan.
    fn strip_stl() -> String {
        "solid s\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 60 0 0\nvertex 60 60 0\nendloop\nendfacet\nendsolid s\n".into()
    }

    /// Four flat free forms round a square tube, merged: the XY plane picked, the dialog finds
    /// the one loop to cap before anything changes; OK caps the tube's end.
    #[test]
    fn the_plane_picked_shows_the_loop_to_cap_and_ok_caps_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("strip.stl");
        std::fs::write(&path, strip_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        for corners in [
            json!([[0, 0, -20], [20, 0, -20], [20, 0, 20], [0, 0, 20]]),
            json!([[20, 0, -20], [20, 20, -20], [20, 20, 20], [20, 0, 20]]),
            json!([[20, 20, -20], [0, 20, -20], [0, 20, 20], [20, 20, 20]]),
            json!([[0, 20, -20], [0, 0, -20], [0, 0, 20], [0, 20, 20]]),
        ] {
            h.app.run("scan.free_form", json!({"corners": corners, "fit": "off"})).unwrap();
        }
        h.app.run("scan.merge_surfaces", json!({})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.slice_surface");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[1].items.push(Sel::Plane { name: "XY".into() });
        }
        h.frames(3);
        let Some(Kind::ScanSliceSurface(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Slice Surface dialog") };
        let Some(Ok((found, _))) = k.shown.as_ref().map(|s| s.result.clone()) else {
            panic!("sliced: {:?}", k.shown.as_ref().map(|s| s.result.as_ref().err()))
        };
        assert_eq!((found.body.as_str(), found.loops.len()), ("FreeForm1", 1));
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let faces = h.app.session.model.state().body("FreeForm1").map(|b| b.body.face_count());
        assert_eq!(faces, Some(5), "four sides and a cap");
    }
}
