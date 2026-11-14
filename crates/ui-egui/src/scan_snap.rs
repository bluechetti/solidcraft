//! One snapping for every Scan-tab tool that takes a click on a scan: New Free Form's corners,
//! Curve on Scan's points, Flat Template's outline and Fit Patches' click. A click lands on the
//! nearest target within a few pixels, in this order: a point of a curve already drawn on the scan
//! (a 3D sketch's fit points and ends), a corner, edge or crease of the scan itself, or a vertex
//! of its mesh; the view shows the target under the pointer before the click, so it is never a
//! surprise. The suppress-snap chord (Ctrl or Alt by default) clicks exactly where the pointer is,
//! and Preferences › Tools › Clicks on a scan snap turns it off.

use std::cell::RefCell;

use egui::{Color32, Painter};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::select::topology_for_snap;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::Kind;
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
    Vertex,
    /// Flat Template: the foot on the wall's plane of where the click is (nothing nearer to snap to).
    Plane,
}

impl Target {
    /// The label shown beside the marker.
    fn label(self) -> &'static str {
        solidcraft_ui_text::tr(match self {
            Target::Curve => "ui.scan_snap_curve",
            Target::Corner => "ui.scan_snap_corner",
            Target::Boundary => "ui.scan_snap_edge",
            Target::Crease => "ui.scan_snap_crease",
            Target::Vertex => "ui.scan_snap_vertex",
            Target::Plane => "ui.scan_snap_plane",
        })
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
    let topology = topology_for_snap(&app.session, body)?;
    let found = topology.snap(point, reach, CREASE_DEGREES)?;
    use solidcraft_engine::cmd::scan::select::SnapKind as K;
    Some((
        found.at,
        match found.kind {
            K::Corner => Target::Corner,
            K::Boundary => Target::Boundary,
            K::Crease => Target::Crease,
            K::Vertex => Target::Vertex,
        },
    ))
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

/// Finds what the pointer is over now (from the hits under it) for the marker; call once a frame.
pub(crate) fn track(app: &SolveApp, proj: &Proj, hits: &[crate::viewport::Hit]) {
    let found = (placing(app) && enabled(app))
        .then(|| {
            hits.iter().find_map(|h| match h {
                crate::viewport::Hit::Face { body, point, .. } => target(app, proj, body, *point),
                _ => None,
            })
        })
        .flatten();
    HOVER.with(|h| *h.borrow_mut() = found);
}

/// The marker where a click would land, with what it is.
pub(crate) fn draw(painter: &Painter, proj: &Proj) {
    let Some((at, kind)) = HOVER.with(|h| *h.borrow()) else { return };
    let Some(p) = proj.to_screen(at) else { return };
    let t = solidcraft_theme::Tokens::get();
    let colour: Color32 = t.accent;
    crate::dialogs_scan_surface::cased_dot(painter, p, 5.0, colour);
    painter.text(p + egui::vec2(9.0, -9.0), egui::Align2::LEFT_BOTTOM, kind.label(), egui::FontId::proportional(t.font_label), t.text);
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
        assert_eq!(marker, None, "no marker with snapping off");
    }
}
