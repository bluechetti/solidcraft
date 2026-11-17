//! One snapping for every Scan-tab tool that takes a click on a scan: New Free Form's corners,
//! Curve on Scan's points, Flat Template's outline and Fit Patches' click. A click lands on the
//! nearest target within a few pixels, in this order: a point of a curve already drawn on the scan
//! (a 3D sketch's fit points and ends), or a corner, edge or crease of the scan itself; with none
//! near it lands exactly where the pointer is on the scan (a scan's own vertices are noise, not
//! targets). A click just past the scan's outline lands on the scan, not in the air beyond it. The
//! view always shows where the click will land: a ring on the scan, and a dot with what it is when
//! it snaps. On a scan too large to read its edges on the spot, they are read beside the window
//! once a tool starts placing points on it. The suppress-snap chord (Ctrl or Alt by default)
//! clicks exactly where the pointer is, and Preferences › Tools › Clicks on a scan snap turns it
//! off.

use std::cell::RefCell;

use egui::{Color32, Painter};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::select::topology_for_snap;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::Kind;
use crate::scan_job;
use crate::viewport::Proj;

/// How near the pointer a target is taken, in pixels.
pub(crate) const SNAP_PX: f32 = 10.0;
/// The angle two triangles meet at that makes a crease a target, in degrees.
const CREASE_DEGREES: f64 = 30.0;

/// What a click would land on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Target {
    /// A point of a curve drawn on the scan.
    Curve,
    Corner,
    Boundary,
    Crease,
    /// Nothing to snap to: the place on the scan under the pointer.
    Surface,
    /// Flat Template: the foot on the wall's plane of where the click is (nothing nearer to snap to).
    Plane,
}

impl Target {
    /// The label shown beside the marker (none on the plain surface).
    fn label(self) -> Option<&'static str> {
        Some(solidcraft_ui_text::tr(match self {
            Target::Curve => "ui.scan_snap_curve",
            Target::Corner => "ui.scan_snap_corner",
            Target::Boundary => "ui.scan_snap_edge",
            Target::Crease => "ui.scan_snap_crease",
            Target::Surface => return None,
            Target::Plane => "ui.scan_snap_plane",
        }))
    }
}

thread_local! {
    /// The target under the pointer, for the marker.
    static HOVER: RefCell<Option<(Vec3, Target)>> = const { RefCell::new(None) };
}

/// Whether the open dialog is placing points on a scan with clicks.
pub(crate) fn placing(app: &SolveApp) -> bool {
    match app.dialog.as_ref().map(|d| &d.kind) {
        Some(Kind::ScanCurve(_) | Kind::ScanFlatTemplate(_) | Kind::ScanFitPatches(_)) => true,
        Some(Kind::ScanFreeForm(_)) => crate::dialogs_scan_free_form::drawing(app),
        _ => false,
    }
}

/// Whether clicks snap now: on in Preferences, the suppress chord not held.
pub(crate) fn enabled(app: &SolveApp) -> bool {
    app.preferences.scan_click_snap && !crate::drag_snap::suppressed()
}

/// How far `at` is from the pointer's snap reach, in mm: what [`SNAP_PX`] is there.
pub(crate) fn reach_mm(proj: &Proj, at: Vec3) -> Option<f64> {
    let (right, ..) = proj.cam.basis();
    let a = proj.to_screen(at)?;
    let b = proj.to_screen(at + right)?;
    let per_mm = f64::from(a.distance(b));
    (per_mm > 1e-9).then(|| f64::from(SNAP_PX) / per_mm)
}

/// The points of the curves drawn on scans (3D sketches): each curve's fit points and ends.
fn curve_points(app: &SolveApp) -> Vec<Vec3> {
    let state = app.session.model.state();
    let mut out = Vec::new();
    for s in state.sketches.iter().filter(|s| s.sketch.view.three_d) {
        for w in &s.sketch.wires {
            out.extend(w.fit.iter().copied());
            out.extend(w.pts.first().copied());
            out.extend(w.pts.last().copied());
        }
    }
    out
}

/// What `point` on the scan `body` snaps to, if anything is within reach. In Flat Template, once
/// the wall's plane is known, the target is laid onto that plane (where the template's corner will
/// be), and a click with nothing nearer lands on the plane's foot under it.
pub(crate) fn target(app: &SolveApp, proj: &Proj, body: &str, point: Vec3) -> Option<(Vec3, Target)> {
    let found = on_scan(app, proj, body, point);
    let Some((origin, normal)) = crate::dialogs_scan_template::wall_plane(app) else { return found };
    let foot = |p: Vec3| p - normal * (p - origin).dot(normal);
    Some(found.map_or((foot(point), Target::Plane), |(at, kind)| (foot(at), kind)))
}

/// What `point` on the scan `body` snaps to on the scan itself.
fn on_scan(app: &SolveApp, proj: &Proj, body: &str, point: Vec3) -> Option<(Vec3, Target)> {
    let reach = reach_mm(proj, point)?;
    let on_screen = proj.to_screen(point)?;
    // A curve's point, judged on the screen so a point behind the scan's surface is not taken.
    let curve = curve_points(app)
        .into_iter()
        .filter_map(|p| proj.to_screen(p).map(|q| (q.distance(on_screen), p)))
        .filter(|(d, p)| *d <= SNAP_PX && p.dist(point) <= 4.0 * reach)
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if let Some((_, at)) = curve {
        return Some((at, Target::Curve));
    }
    let Some(topology) = topology_for_snap(&app.session, body) else {
        warm(app, body);
        return None;
    };
    // Reach across the line of sight, as the pointer's is on the screen.
    let toward = proj.ray(on_screen).1;
    let found = topology.snap_seen(point, Some(toward), reach, CREASE_DEGREES)?;
    use solidcraft_engine::cmd::scan::select::SnapKind as K;
    let kind = match found.kind {
        K::Corner => Target::Corner,
        K::Boundary => Target::Boundary,
        K::Crease => Target::Crease,
        // A vertex of a scan's mesh is where the scanner happened to put a point: snapping to it
        // only moves the click off where it was aimed.
        K::Vertex => return None,
    };
    Some((found.at, kind))
}

/// Scans larger than this have their edges read for snapping beside the window, once (smaller
/// ones are read on the spot by [`topology_for_snap`]); larger than [`WARM_LIMIT`] not at all.
const WARM_LIMIT: usize = 30_000_000;

thread_local! {
    /// The scan whose edges are being read beside the window, by its name and triangle count.
    static WARMING: RefCell<Option<((String, usize), scan_job::Job<(), ()>)>> = const { RefCell::new(None) };
}

/// Starts reading the edges of the large scan `body` beside the window, so clicks on it snap once
/// they are read; does nothing while they are being read, or for a scan past [`WARM_LIMIT`].
fn warm(app: &SolveApp, body: &str) {
    let Some(count) = app.session.model.state().body(body).and_then(|b| b.body.kept_mesh()).map(|m| m.triangles.len()) else { return };
    if count > WARM_LIMIT {
        return;
    }
    let key = (body.to_owned(), count);
    let busy = WARMING.with(|w| {
        let mut w = w.borrow_mut();
        match w.as_ref().map(|(k, job)| (k == &key, job.poll())) {
            Some((true, scan_job::Poll::Working)) => true,
            // Read, or the worker is gone: a later hover tries again only if it is still missing.
            Some((_, scan_job::Poll::Done(()) | scan_job::Poll::Lost)) | Some((false, scan_job::Poll::Working)) | None => {
                *w = None;
                false
            }
        }
    });
    if busy {
        return;
    }
    let name = body.to_owned();
    if let Some(job) = scan_job::spawn(app, (), move |s| {
        let _ = solidcraft_engine::cmd::scan::select::topology(s, &name, "scan.snap");
    }) {
        WARMING.with(|w| *w.borrow_mut() = Some((key, job)));
    }
}

/// The scan right under the screen point `at` (only `only`, when given), and the place on it,
/// read through any model in front: a model is often built over its scan and covers it. Only
/// scans (mesh bodies) the view shows count.
pub(crate) fn scan_hit(app: &SolveApp, proj: &Proj, at: egui::Pos2, only: Option<&str>) -> Option<(String, Vec3)> {
    ray_hit(app, proj, at, only).map(|(_, body, point, _)| (body, point))
}

/// [`scan_hit`] with the triangle hit, its index in the scan's mesh.
pub(crate) fn scan_hit_triangle(app: &SolveApp, proj: &Proj, at: egui::Pos2, only: Option<&str>) -> Option<(String, Vec3, usize)> {
    ray_hit(app, proj, at, only).map(|(_, body, point, triangle)| (body, point, triangle))
}

/// [`scan_hit`] with how far along the view ray it is and the triangle hit.
fn ray_hit(app: &SolveApp, proj: &Proj, at: egui::Pos2, only: Option<&str>) -> Option<(f64, String, Vec3, usize)> {
    let state = app.session.model.state();
    let (o, d) = proj.ray(at);
    let mut best: Option<(f64, String, Vec3, usize)> = None;
    for b in state.bodies.iter().filter(|b| b.body.is_mesh() && only.is_none_or(|n| n == b.name) && app.body_visible(&b.name)) {
        let (m, place) = b.mesh_in_place();
        let Some((lo, ld)) = crate::viewport::local_ray(&place, o, d) else { continue };
        let index = app.viewport.pick_index.of(&m);
        if let Some((t, triangle)) = crate::pick_index::raycast(index.as_deref(), &m, lo, ld)
            && best.as_ref().is_none_or(|(bt, ..)| t < *bt)
        {
            best = Some((t, b.name.clone(), o + d * t, triangle));
        }
    }
    best
}

/// The scan under the screen point `at`, and the place on it: the scan right under the pointer
/// ([`scan_hit`], through any model), else the nearest one within [`SNAP_PX`] on the screen (a
/// click just past a scan's outline lands on the scan, not in the air beyond it).
pub(crate) fn scan_under(app: &SolveApp, proj: &Proj, at: egui::Pos2) -> Option<(String, Vec3)> {
    if let Some(found) = scan_hit(app, proj, at, None) {
        return Some(found);
    }
    // Rings round the pointer, nearest first: the first ring that touches a scan gives the place
    // nearest the pointer on it.
    const SPOKES: usize = 12;
    for radius in [SNAP_PX * 0.4, SNAP_PX * 0.7, SNAP_PX] {
        let found = (0..SPOKES)
            .filter_map(|k| {
                let a = std::f32::consts::TAU * k as f32 / SPOKES as f32;
                ray_hit(app, proj, at + egui::vec2(a.cos(), a.sin()) * radius, None)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, body, point, _)) = found {
            return Some((body, point));
        }
    }
    None
}

/// `point` on the scan `body` as a click lands: on its snap target, or where it is.
pub(crate) fn snapped(app: &SolveApp, proj: &Proj, body: &str, point: Vec3) -> Vec3 {
    if !enabled(app) {
        return point;
    }
    target(app, proj, body, point).map_or(point, |(at, _)| at)
}

/// The same for a selection a dialog input is about to take: a place on a scan's face.
pub(crate) fn snapped_sel(app: &SolveApp, proj: &Proj, sel: solidcraft_engine::Sel) -> solidcraft_engine::Sel {
    use solidcraft_engine::Sel;
    if !placing(app) {
        return sel;
    }
    match sel {
        Sel::Face { body, index, point } if app.session.model.state().body(&body).is_some_and(|b| b.body.is_mesh()) => {
            let point = snapped(app, proj, &body, point);
            Sel::Face { body, index, point }
        }
        other => other,
    }
}

/// Finds where a click would land now, for the marker; call once a frame. `pointer` is where the
/// pointer is over the view, and `hits` what is under it: a body's face in front of the scans takes
/// the click as it is (no marker).
pub(crate) fn track(app: &SolveApp, proj: &Proj, pointer: Option<egui::Pos2>, hits: &[crate::viewport::Hit]) {
    let found = placing(app)
        .then(|| {
            let front = hits.iter().find_map(|h| match h {
                crate::viewport::Hit::Face { body, point, .. } => Some((body.clone(), *point)),
                _ => None,
            });
            let on_scan = |body: &str| app.session.model.state().body(body).is_some_and(|b| b.body.is_mesh());
            let (body, point) = match front {
                Some((body, _)) if !on_scan(&body) => return None,
                Some(found) => found,
                None => scan_under(app, proj, pointer?)?,
            };
            if !enabled(app) {
                return Some((point, Target::Surface));
            }
            Some(target(app, proj, &body, point).unwrap_or((point, Target::Surface)))
        })
        .flatten();
    HOVER.with(|h| *h.borrow_mut() = found);
}

/// The marker where a click would land: a ring on the scan, or a dot and what it is when it snaps.
pub(crate) fn draw(painter: &Painter, proj: &Proj) {
    let Some((at, kind)) = HOVER.with(|h| *h.borrow()) else { return };
    let Some(p) = proj.to_screen(at) else { return };
    let t = solidcraft_theme::Tokens::get();
    let colour: Color32 = t.accent;
    let Some(label) = kind.label() else {
        painter.circle_stroke(p, 6.0, egui::Stroke::new(3.5, crate::dialogs_scan_surface::casing()));
        painter.circle_stroke(p, 6.0, egui::Stroke::new(1.5, colour));
        return;
    };
    crate::dialogs_scan_surface::cased_dot(painter, p, 5.0, colour);
    let font = egui::FontId::proportional(t.font_label);
    let galley = painter.layout_no_wrap(label.to_owned(), font, t.text);
    let at = p + egui::vec2(10.0, -10.0 - galley.size().y);
    painter.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(4.0, 2.0)), t.radius_sm, t.panel);
    painter.galley(at, galley, t.text);
}

/// Where the pointer's snap would land: for tests.
#[cfg(test)]
pub(crate) fn hovered() -> Option<(Vec3, Target)> {
    HOVER.with(|h| *h.borrow())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A tube of radius 20 along Y, open at its ends, as an STL.
    fn tube_stl(n: u32) -> String {
        let at = |i: u32, j: u32| {
            let a = std::f64::consts::TAU * f64::from(i % n) / f64::from(n);
            [20.0 * a.cos(), 40.0 * f64::from(j) / f64::from(n), 20.0 * a.sin()]
        };
        let mut stl = String::from("solid t\n");
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
        stl + "endsolid t\n"
    }

    /// The first point Curve on Scan holds after a click just inside the tube's open end, seen from
    /// above: with snapping it lands on the scan's edge (y = 0), with the preference off exactly
    /// where it was clicked.
    fn first_pick(snap: bool) -> Option<(f64, Option<Target>)> {
        let mut h = crate::scenario::Harness::new();
        h.app.preferences.scan_click_snap = snap;
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, tube_stl(64)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.curve_on_scan");
        h.frames(2);
        // Hovering over it first shows the marker with what it is.
        h.step(&json!({"move": {"world": [0.0, 0.2, 20.0]}})).unwrap();
        h.frames(2);
        let marker = hovered().map(|(_, t)| t);
        h.step(&json!({"click": {"world": [0.0, 0.2, 20.0]}})).unwrap();
        h.frames(2);
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanCurve(k)) => k.picks.first().map(|p| (p.y, marker)),
            _ => None,
        }
    }

    #[test]
    fn a_click_near_the_scans_edge_lands_on_it_and_the_marker_says_so() {
        let (y, marker) = first_pick(true).expect("a point was placed");
        assert!(y.abs() < 1e-6, "on the open end: {y}");
        assert_eq!(marker, Some(Target::Boundary), "the marker named it before the click");
        let (y, marker) = first_pick(false).expect("a point was placed");
        assert!((y - 0.2).abs() < 0.05, "where it was clicked: {y}");
        assert_eq!(marker, Some(Target::Surface), "with snapping off, a ring where it lands and nothing named");
    }

    /// New Free Form, the tube seen from above: a click a few pixels past its outline puts the
    /// corner on the tube, not in the air beyond it, and the ring shows where before the click.
    #[test]
    fn a_click_just_past_the_scans_outline_lands_on_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, tube_stl(64)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.free_form");
        h.frames(2);
        h.step(&json!({"move": {"world": [20.3, 20.0, 0.0]}})).unwrap();
        h.frames(2);
        let (ring, _) = hovered().expect("a marker just past the outline");
        assert!((ring.x.hypot(ring.z) - 20.0).abs() < 0.1, "on the tube: {ring:?}");
        h.step(&json!({"click": {"world": [20.3, 20.0, 0.0]}})).unwrap();
        h.frames(2);
        let Some(Kind::ScanFreeForm(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog is open") };
        let corner = k.corners_clicked().first().copied().expect("a corner was placed");
        assert!((corner.x.hypot(corner.z) - 20.0).abs() < 0.1, "on the tube: {corner:?}");
    }
}
