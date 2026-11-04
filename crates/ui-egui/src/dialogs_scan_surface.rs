//! Edit Surface (`scan.edit_surface`): the control net of a surface Fit Surface made is drawn over
//! the view with a handle at each point. A handle dragged moves its point in the plane of the
//! screen (keeping its depth along the view), or, with Snap to scan on, onto the scan under the
//! pointer; with Refit on, the points round it (or all but those moved by hand) are fitted to the
//! scan again as it moves, the dragged one held where it is put. The surface it would make is drawn
//! as lines across it, and how far it is from the scan is shown as it moves. OK keeps the net as
//! one undo step; Reset Points puts it back as it was.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::surface::{self as edit, Covered, EditableSurface};
use solidcraft_engine::geom::bspline::NurbsSurface;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit::{self, choice_row};

/// What a drag refits (0 nothing): the ring round the point dragged, or every point not moved by
/// hand.
pub const REFIT_AROUND: usize = 1;
pub const REFIT_ALL: usize = 2;

/// How many rings round the dragged point Refit Around frees.
const RING: usize = 2;

/// A handle's half size, and how near the pointer must be pressed to take it, in pixels.
const HANDLE: f32 = 4.0;
const GRAB: f32 = 10.0;

/// The most scan points a drag's preview refits to and measures: the full set (as the command
/// uses) once it is let go.
const PREVIEW_POINTS: usize = 2_500;

/// The surface drawn as this many lines each way, each sampled at this many points.
const LINES: usize = 10;
const SAMPLES: usize = 40;

/// The Edit Surface dialog's values.
#[derive(Clone, Debug)]
pub struct EditSurface {
    /// A dragged point goes onto the scan under the pointer.
    pub snap: bool,
    /// What a drag refits: 0 nothing, [`REFIT_AROUND`] or [`REFIT_ALL`].
    pub refit: usize,
    loaded: Option<Result<Loaded, String>>,
    /// The net as edited so far, and the points moved by hand.
    net: Vec<Vec3>,
    moved: Vec<bool>,
    /// The surface shown (the net, or a drag's preview of it) and its net.
    shown: Option<NurbsSurface>,
    shown_net: Vec<Vec3>,
    /// The scan's distance from the surface shown: root mean square and largest.
    fit: Option<(f64, f64)>,
    chosen: Option<usize>,
    drag: Option<Drag>,
}

impl Default for EditSurface {
    fn default() -> Self {
        EditSurface {
            snap: false,
            refit: REFIT_AROUND,
            loaded: None,
            net: Vec::new(),
            moved: Vec::new(),
            shown: None,
            shown_net: Vec::new(),
            fit: None,
            chosen: None,
            drag: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Loaded {
    target: EditableSurface,
    covered: Option<Covered>,
    /// An even sample of the covered points, for previews while a handle is dragged.
    sample: Arc<Vec<Vec3>>,
    revision: u64,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    index: usize,
    /// Where the point was when the drag began: the depth it keeps.
    from: Vec3,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.edit_surface").then(|| (Kind::ScanEditSurface(EditSurface::default()), Vec::new()))
}

/// Load the surface and the scan under it when the dialog opens, or again when the design has
/// changed under it (an undo, say); what was moved is dropped then.
fn load(app: &SolveApp, k: &mut EditSurface) {
    let current = matches!(&k.loaded, Some(Ok(l)) if l.revision == app.session.revision) || matches!(&k.loaded, Some(Err(_)));
    if current {
        return;
    }
    let cmd = "scan.edit_surface";
    let loaded = edit::editable(&app.session, None, cmd).map_err(|e| e.to_string()).map(|target| {
        let reach = edit::default_reach(&target.surface);
        let covered = edit::covered(&app.session, None, &target.surface, reach, cmd).ok().filter(|c| !c.points.is_empty());
        let all = covered.as_ref().map(|c| c.points.as_slice()).unwrap_or_default();
        let sample = Arc::new(all.iter().step_by(all.len() / PREVIEW_POINTS + 1).copied().collect());
        Loaded { target, covered, sample, revision: app.session.revision }
    });
    let surface = loaded.as_ref().ok().map(|l| l.target.surface.clone());
    k.loaded = Some(loaded);
    if let Some(surface) = surface {
        k.net = edit::control_points(&surface);
        k.moved = vec![false; k.net.len()];
        k.chosen = None;
        k.drag = None;
        show(k, surface, false);
    }
}

fn loaded(k: &EditSurface) -> Option<&Loaded> {
    k.loaded.as_ref()?.as_ref().ok()
}

/// Show `surface`, with how far the scan is from it (measured on the sample while `dragging`).
fn show(k: &mut EditSurface, surface: NurbsSurface, dragging: bool) {
    k.fit = loaded(k).and_then(|l| {
        let c = l.covered.as_ref()?;
        Some(edit::distance(&surface, if dragging { &l.sample } else { &c.points }))
    });
    k.shown_net = edit::control_points(&surface);
    k.shown = Some(surface);
}

/// The surface with point `index` at `to`, and, as Refit says, the points round it or all those not
/// moved by hand fitted to the scan again with it held.
fn preview(k: &EditSurface, index: usize, to: Vec3) -> Option<NurbsSurface> {
    let l = loaded(k)?;
    let mut net = k.net.clone();
    *net.get_mut(index)? = to;
    let mut moved = k.moved.clone();
    moved[index] = true;
    let surface = edit::with_points(&l.target.surface, &net)?;
    let points = l.sample.as_slice();
    let count = surface.count();
    let held: Vec<bool> = match k.refit {
        REFIT_AROUND => edit::around(count, index % count[0], index / count[0], RING).iter().zip(&moved).map(|(near, m)| !near || *m).collect(),
        REFIT_ALL => moved,
        _ => return Some(surface),
    };
    Some(edit::refit(&surface, points, &held).unwrap_or(surface))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface) -> bool {
    load(app, k);
    let l = match &k.loaded {
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_edit_error", true));
            return false;
        }
        Some(Ok(l)) => l.clone(),
        None => return false,
    };
    let [nu, nv] = l.target.surface.count();
    let name = format!("{} ({nu} × {nv})", l.target.body);
    form_row(ui, crate::i18n::tr_label("Surface"), |ui| ui_kit::text(ui, &name, ui_kit::Text::Cell));
    if l.covered.is_some() {
        form_row(ui, tr("ui.scan_edit_snap"), |ui| check(ui, &mut k.snap));
        let labels = [tr("ui.scan_edit_refit_off"), tr("ui.scan_edit_refit_around"), tr("ui.scan_edit_refit_all")];
        choice_row(ui, tr("ui.scan_edit_refit"), "scan_edit_refit", &mut k.refit, &labels);
    } else {
        ui_kit::text(ui, tr("ui.scan_edit_no_scan"), ui_kit::Text::Hint("scan_edit_no_scan"));
    }
    let moved = k.moved.iter().filter(|m| **m).count().to_string();
    form_row(ui, tr("ui.scan_edit_moved"), |ui| ui_kit::text(ui, &moved, ui_kit::Text::Cell));
    if let Some((rms, max)) = k.fit {
        let fit = crate::i18n::format(
            "ui.scan_edit_distance_value",
            &[("rms", &crate::prefs::show_mm(app, rms, 3)), ("max", &crate::prefs::show_mm(app, max, 3))],
        );
        form_row(ui, tr("ui.scan_edit_distance"), |ui| ui_kit::text(ui, &fit, ui_kit::Text::Cell));
    }
    let edited = k.moved.iter().any(|m| *m);
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_reset"), ui_kit::Button::Enabled(edited))).clicked() {
        k.net = edit::control_points(&l.target.surface);
        k.moved = vec![false; k.net.len()];
        k.chosen = None;
        show(k, l.target.surface.clone(), false);
    }
    ui_kit::text(ui, tr("ui.scan_edit_hint"), ui_kit::Text::Hint("scan_edit_hint"));
    false
}

/// What OK runs: the net as edited.
pub fn commands(k: &EditSurface) -> Result<Vec<(String, Value)>, String> {
    let l = k.loaded.as_ref().ok_or("still loading the surface")?.as_ref().map_err(Clone::clone)?;
    if !k.moved.iter().any(|m| *m) {
        return Err(crate::i18n::tr("ui.scan_edit_nothing").into());
    }
    let points: Vec<[f64; 3]> = k.net.iter().map(|p| [p.x, p.y, p.z]).collect();
    Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "points": points}))])
}

fn dialog(app: &mut SolveApp) -> Option<&mut EditSurface> {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(Kind::ScanEditSurface(k)) => Some(k),
        _ => None,
    }
}

/// Whether the dialog shows handles (the next primary press on one takes it).
pub(crate) fn editing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if loaded(k).is_some())
}

/// Whether a handle is being dragged: the drag neither selects nor turns the view.
pub(crate) fn holds_pointer(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if k.drag.is_some())
}

/// A primary press at `at`: takes the handle under it, if any.
pub(crate) fn grab(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) -> bool {
    let Some(k) = dialog(app) else { return false };
    let nearest = k
        .shown_net
        .iter()
        .enumerate()
        .filter_map(|(i, p)| proj.to_screen(*p).map(|s| (i, s.distance(at))))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let Some((index, _)) = nearest else { return false };
    // A drag starts from the net as shown (a refit the last drag made is kept).
    k.net = k.shown_net.clone();
    k.drag = Some(Drag { index, from: k.net[index] });
    k.chosen = Some(index);
    true
}

/// The handle dragged to `to` on the screen.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &crate::viewport::Proj, to: egui::Pos2) {
    let Some((drag, snap, scan)) = dialog(app).and_then(|k| Some((k.drag?, k.snap, loaded(k)?.covered.as_ref().map(|c| c.scan.clone())))) else {
        return;
    };
    // Onto the scan under the pointer, or in the plane of the screen through where the point was.
    let on_scan = if snap && let Some(scan) = scan {
        crate::viewport::pick_cached(app, proj, to).into_iter().find_map(|h| match h {
            crate::viewport::Hit::Face { body, point, .. } if body == scan => Some(point),
            _ => None,
        })
    } else {
        None
    };
    let place = on_scan.or_else(|| {
        let (o, d) = proj.ray(to);
        let n = proj.cam.back();
        let den = d.dot(n);
        (den.abs() > 1e-12).then(|| o + d * ((drag.from - o).dot(n) / den))
    });
    let Some(k) = dialog(app) else { return };
    if let Some(surface) = place.and_then(|p| preview(k, drag.index, p)) {
        show(k, surface, true);
    }
}

/// The drag let go: the net as previewed is kept and the point counts as moved by hand.
pub(crate) fn release(app: &mut SolveApp) {
    let Some(k) = dialog(app) else { return };
    let Some(drag) = k.drag.take() else { return };
    if k.shown_net.len() == k.net.len() && k.shown_net.get(drag.index) != k.net.get(drag.index) {
        k.net = k.shown_net.clone();
        k.moved[drag.index] = true;
    }
    // Measured again on every covered point, as the command will.
    if let Some(surface) = k.shown.clone() {
        show(k, surface, false);
    }
}

/// The surface shown, its control net and the handles, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanEditSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(surface) = &k.shown else { return };
    let t = crate::theme::Tokens::get();
    let [[u0, u1], [v0, v1]] = surface.domain();
    let line = egui::Stroke::new(1.5, t.surface_accent);
    let at = |i: usize, n: usize, a: f64, b: f64| a + (b - a) * i as f64 / n as f64;
    for along_u in [true, false] {
        for i in 0..=LINES {
            let curve: Option<Vec<egui::Pos2>> = (0..=SAMPLES)
                .map(|j| {
                    let (u, v) =
                        if along_u { (at(j, SAMPLES, u0, u1), at(i, LINES, v0, v1)) } else { (at(i, LINES, u0, u1), at(j, SAMPLES, v0, v1)) };
                    surface.eval(u, v).ok().and_then(|p| proj.to_screen(p))
                })
                .collect();
            if let Some(curve) = curve {
                painter.add(egui::Shape::line(curve, line));
            }
        }
    }
    let [nu, nv] = surface.count();
    let screen: Vec<Option<egui::Pos2>> = k.shown_net.iter().map(|p| proj.to_screen(*p)).collect();
    let net = egui::Stroke::new(1.0, t.text.gamma_multiply(0.45));
    for j in 0..nv {
        for i in 0..nu {
            let here = screen.get(j * nu + i).copied().flatten();
            for next in [(i + 1 < nu).then(|| j * nu + i + 1), (j + 1 < nv).then(|| (j + 1) * nu + i)].into_iter().flatten() {
                if let (Some(a), Some(b)) = (here, screen.get(next).copied().flatten()) {
                    painter.line_segment([a, b], net);
                }
            }
        }
    }
    let [r, g, b] = t.analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    let moved_fill = egui::Color32::from_rgb(r, g, b);
    for (i, s) in screen.iter().enumerate() {
        let Some(s) = s else { continue };
        let fill = if k.chosen == Some(i) {
            t.accent
        } else if k.moved.get(i).copied().unwrap_or(false) {
            moved_fill
        } else {
            t.text
        };
        let rect = egui::Rect::from_center_size(*s, egui::vec2(2.0 * HANDLE, 2.0 * HANDLE));
        painter.rect_filled(rect, 1.0, fill);
        painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, t.panel), egui::StrokeKind::Outside);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A dome scanned as a grid, `n` squares a side over 60 mm, as an STL.
    fn dome_stl(n: u32) -> String {
        let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        let at = |i: u32, j: u32| {
            let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
            [x, y, z(x, y)]
        };
        let mut stl = String::from("solid d\n");
        for i in 0..n {
            for j in 0..n {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for [x, y, z] in tri {
                        stl += &format!("vertex {x} {y} {z}\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        stl + "endsolid d\n"
    }

    fn kind(h: &crate::scenario::Harness) -> &EditSurface {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanEditSurface(k)) => k,
            _ => panic!("the Edit Surface dialog"),
        }
    }

    /// A handle of a fitted surface dragged up through the viewport's own input moves its point and
    /// refits the ring round it; OK keeps the edit as one undo step, which puts it back.
    #[test]
    fn a_handle_dragged_in_the_view_moves_its_point_and_ok_keeps_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [30, 30, 8], "radius": 25, "controls": 6})).unwrap();
        h.step(&json!({"view_anim": "front"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert!(editing(&h.app), "handles shown");
        let start = kind(&h).shown_net.clone();
        let fit = kind(&h).fit.expect("the scan's distance from the surface");
        assert!(fit.0 < 0.15, "fitted closely to start with: {fit:?}");

        // The point at [2, 2] dragged 6 mm up in the front view.
        let k = 2 * 6 + 2;
        let (from, to) = (start[k], start[k] + Vec3::new(0.0, 0.0, 6.0));
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h);
        assert!(after.drag.is_none() && after.moved[k], "moved by hand");
        assert!(after.net[k].z - from.z > 4.0, "followed the pointer up: {:?} → {:?}", from, after.net[k]);
        let ring = (0..36).filter(|&i| i != k && after.net[i].dist(start[i]) > 1e-6).count();
        assert!(ring > 0, "the ring round it refitted as it moved");
        assert!(after.fit.is_some_and(|f| f.0 > fit.0), "farther from the scan now");
        let edited = after.net.clone();

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none());
        let kept = edit::editable(&h.app.session, Some("Surface1"), "test").unwrap();
        let net = edit::control_points(&kept.surface);
        assert!(net.iter().zip(&edited).all(|(a, b)| a.dist(*b) < 1e-6), "the net as edited, kept");
        h.app.run("edit.undo", json!({})).unwrap();
        let undone = edit::editable(&h.app.session, Some("Surface1"), "test").unwrap();
        assert!(edit::control_points(&undone.surface)[k].dist(from) < 1e-6, "one undo puts it back");
    }
}
