//! New Free Form (`scan.free_form`, SurfaceCraft's after QuickSurface's): a surface made by hand on
//! the scan, a cubic net over four corners clicked round its first face, fitted to the scan as it
//! is made (Follow the Scan), or only to its overall shape so it spans openings and ribs (Overall
//! Shape), or left flat over the corners (Off). It is kept as a surface body of its own, so Edit
//! Surface drags its points, snaps them to the scan and refits them, and Edit Net changes its net.

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;
use solidcraft_scan::free_form;
pub use solidcraft_scan::free_form::{Fitting, flat_plane};
pub use solidcraft_scan::merge::{Merged, Seam};
pub use solidcraft_scan::wrap::{Cylinder, Wrap};

use super::length;
use crate::params::bad;
use crate::schema::scan::{ScanAddFaceResult, ScanFreeFormResult, ScanMergeSurfacesResult, ScanMirrorFreeFormResult, ScanWrapSurfaceResult};
use crate::{EngineError, Result, Session};

/// How a new face meets the scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The net lies flat over the corners.
    Off,
    /// The face is fitted to the scan under it.
    Follow,
    /// The same, the places far off it left out: it spans openings, dents and ribs.
    Overall,
    /// Exactly flat: in the one plane fitted to the scan under it (the places far off it left out,
    /// so a rib or a dent does not tilt it), the corners laid on that plane. For a panel or a wall
    /// that only has to be flat, not to follow every bump.
    Flat,
}

/// A face made, before it is kept: the scan it was fitted to, the surface and how it fits: the
/// places it was fitted to, those met on the scan and those kept, and how far the scan under it is
/// from it, root mean square and largest (mm; the places' own distance where no scan is under it).
#[derive(Clone, Debug)]
pub struct Made {
    pub scan: Option<String>,
    pub surface: NurbsSurface,
    pub places: usize,
    pub met: usize,
    pub kept: usize,
    pub rms: f64,
    pub max: f64,
}

/// How far the scan `scan` under `surface` is from it, root mean square and largest: the scan, not
/// the places the surface was fitted to, so it is what Edit Surface shows for it after. `fallback`
/// for both with no scan under it.
fn scan_distance(s: &Session, scan: &str, surface: &NurbsSurface, fallback: f64, cmd: &str) -> (f64, f64) {
    match super::surface::covered(s, Some(scan), surface, super::surface::default_reach(surface), cmd) {
        Ok(c) if !c.points.is_empty() => solidcraft_scan::surface_edit::distance(surface, &c.points),
        _ => (fallback, fallback),
    }
}

/// The face over `corners`: fitted to the scan `body` (or the selected or only one) unless `fit`
/// is off, places met no farther than `reach` from it when given.
pub fn make(s: &Session, body: Option<&str>, corners: [Vec3; 4], fit: Fit, fitting: Fitting, reach: Option<f64>) -> Result<Made> {
    let cmd = "scan.free_form";
    if corners.iter().any(|c| !c.is_finite()) {
        return Err(bad(cmd, "the corners must be finite points"));
    }
    let count = fitting.count.clamp(free_form::LEAST_POINTS, free_form::MOST_POINTS);
    if fit == Fit::Off {
        let surface = free_form::surface(&free_form::net_over(corners, count), count).ok_or_else(|| bad(cmd, "the corners make no face"))?;
        return Ok(Made { scan: None, surface, places: 0, met: 0, kept: 0, rms: 0.0, max: 0.0 });
    }
    let (scan, on) = super::curve::projector(s, body, cmd)?;
    let reach = reach.unwrap_or(f64::INFINITY);
    let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along, reach)).collect::<Vec<_>>();
    let fitting = Fitting { count, overall: fit == Fit::Overall, ..fitting };
    let fitted = if fit == Fit::Flat {
        free_form::flattened(corners, &fitting, snap)
            .ok_or_else(|| bad(cmd, "no flat face could be fitted over those corners: the scan under them makes no plane within the snap distance"))?
    } else {
        free_form::fitted(corners, &fitting, snap)
            .ok_or_else(|| bad(cmd, "no face could be fitted over those corners: click them round a face, not in a line"))?
    };
    if fitted.met == 0 {
        return Err(bad(cmd, "the scan is nowhere near those corners within the snap distance"));
    }
    let (rms, max) = scan_distance(s, &scan, &fitted.surface, fitted.rms, cmd);
    Ok(Made { scan: Some(scan), surface: fitted.surface, places: fitted.places, met: fitted.met, kept: fitted.kept, rms, max })
}

/// `p` put on the scan no farther than `reach`: along `along` where that line meets the scan,
/// else at the scan's nearest place.
fn snapped(on: &solidcraft_scan::curve::Projector, p: Vec3, along: Option<Vec3>, reach: f64) -> Option<Vec3> {
    along.and_then(|d| on.along(p, d)).or_else(|| on.nearest(p)).filter(|(_, d)| *d <= reach).map(|(q, _)| q)
}

/// The corners: four points [x, y, z], in order round the face.
fn corners_param(p: &Value, cmd: &str) -> Result<[Vec3; 4]> {
    let list =
        p.get("corners").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`corners` must list four points [x, y, z], in order round the face"))?;
    let points: Vec<Vec3> =
        list.iter().map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, "a corner must be [x, y, z] of numbers"))).collect::<Result<_>>()?;
    <[Vec3; 4]>::try_from(points).map_err(|_| bad(cmd, "`corners` must be four points, in order round the face"))
}

/// A face laid flat on the chosen `plane` over `corners`, `count` points each way: no scan needed.
pub fn made_on_plane(corners: [Vec3; 4], count: usize, plane: solidcraft_scan::symmetric::MirrorPlane) -> Result<Made> {
    let cmd = "scan.free_form";
    if corners.iter().any(|c| !c.is_finite()) {
        return Err(bad(cmd, "the corners must be finite points"));
    }
    let surface = free_form::flat_over(corners, count, plane.point, plane.normal)
        .ok_or_else(|| bad(cmd, "the corners make no face on that plane: click them round a face, not in a line"))?;
    Ok(Made { scan: None, surface, places: 0, met: 0, kept: 0, rms: 0.0, max: 0.0 })
}

pub(super) fn new_free_form(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.free_form";
    let corners = corners_param(p, cmd)?;
    let plane = match p.get("plane").filter(|v| !v.is_null()) {
        Some(v) => Some(mirror_plane_param(s, Some(v), cmd)?),
        None => None,
    };
    let fit = match crate::params::str_(p, "fit").unwrap_or(if plane.is_some() { "flat" } else { "follow" }) {
        "off" => Fit::Off,
        "follow" => Fit::Follow,
        "overall" => Fit::Overall,
        "flat" => Fit::Flat,
        other => return Err(bad(cmd, format!("`fit` must be off, follow, overall or flat, not `{other}`"))),
    };
    if plane.is_some() && fit != Fit::Flat {
        return Err(bad(cmd, "a `plane` lays the face flat on it: leave `fit` out, or give `flat`"));
    }
    let count = match p.get("count").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_u64()
            .filter(|n| (free_form::LEAST_POINTS as u64..=free_form::MOST_POINTS as u64).contains(n))
            .ok_or_else(|| bad(cmd, "`count` must be a whole number from 4 to 12"))? as usize,
        None => free_form::LEAST_POINTS,
    };
    let stiffness = match p.get("stiffness").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`stiffness` must be a number from 0 to 1"))?,
        None => 0.0,
    };
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => Some(length(s, p, "reach", 1.0, cmd)?),
        None => None,
    };
    let body = crate::params::str_(p, "body").map(str::to_owned);
    let made = match plane {
        Some(plane) => made_on_plane(corners, count, plane)?,
        None => make(s, body.as_deref(), corners, fit, Fitting { count, stiffness, overall: false }, reach)?,
    };
    let file = made.scan.as_deref().map_or_else(|| "free form".to_owned(), |scan| format!("{scan} (free form)"));
    let (name, feature) = super::surface::keep_surface(s, &made.surface, "FreeForm", "Free Form", &file, cmd)?;
    let reply = ScanFreeFormResult {
        body: made.scan,
        surface: name,
        controls: made.surface.count(),
        places: made.places,
        met: made.met,
        kept: made.kept,
        rms: made.rms,
        max: made.max,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub use solidcraft_scan::network::Side;

/// The faces with one added on the open `side` of face `face` out to `far`, `rows` points across,
/// fitted to the scan `body` (or the selected or only one), places met no farther than `reach`
/// from it when given: as Add Face (`scan.add_face`) and Edit Surface's Add Face make it. Also
/// the scan's name.
#[allow(clippy::too_many_arguments)]
pub fn added_face(
    s: &Session,
    body: Option<&str>,
    faces: &[NurbsSurface],
    face: usize,
    side: Side,
    far: [Vec3; 2],
    rows: usize,
    reach: Option<f64>,
) -> Result<(String, Vec<NurbsSurface>)> {
    let cmd = "scan.add_face";
    let joins = solidcraft_scan::network::joins(faces).map_err(|e| bad(cmd, e.to_string()))?;
    let (scan, on) = super::curve::projector(s, body, cmd)?;
    let reach = reach.unwrap_or(f64::INFINITY);
    let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along, reach)).collect::<Vec<_>>();
    let made = solidcraft_scan::network::with_face(faces, &joins, face, side, far, rows, snap).map_err(|e| bad(cmd, e.to_string()))?;
    Ok((scan, made))
}

/// A plane to mirror in, from `v`: `YZ`, `XZ` or `XY` (through the origin), a construction plane's
/// name, or `{point | origin, normal}`.
pub fn mirror_plane_param(s: &Session, v: Option<&Value>, cmd: &str) -> Result<solidcraft_scan::symmetric::MirrorPlane> {
    use solidcraft_scan::symmetric::MirrorPlane;
    const WANT: &str = "`YZ`, `XZ`, `XY`, a construction plane's name, or {point, normal}";
    let of = |pl: &solidcore_record::Plane| MirrorPlane::new(pl.origin, pl.x.cross(pl.y));
    match v.filter(|v| !v.is_null()) {
        Some(Value::String(name)) => {
            let name = name.trim();
            if let Some(pl) = solidcore_record::Plane::named(name) {
                return of(&pl).ok_or_else(|| bad(cmd, "the plane has no direction"));
            }
            let found = crate::view::construction_planes(s).into_iter().find(|(_, n, _)| n == name).map(|(_, _, pl)| pl);
            match found {
                Some(pl) => of(&pl).ok_or_else(|| bad(cmd, format!("the construction plane `{name}` has no direction"))),
                None => Err(bad(cmd, format!("no plane `{name}`: a plane is {WANT}"))),
            }
        }
        Some(v) if v.is_object() => {
            let point = v.get("point").or_else(|| v.get("origin")).and_then(crate::params::vec3);
            let normal = v.get("normal").and_then(crate::params::vec3);
            match (point, normal) {
                (Some(point), Some(normal)) => MirrorPlane::new(point, normal).ok_or_else(|| bad(cmd, "`plane.normal` has no direction")),
                _ => Err(bad(
                    cmd,
                    format!("a plane given as an object needs `point` (or `origin`) and `normal`, each [x, y, z] of numbers; a plane is {WANT}"),
                )),
            }
        }
        _ => Err(bad(cmd, format!("`plane` is needed: {WANT}"))),
    }
}

pub(super) fn mirror_free_form(s: &mut Session, p: &Value) -> Result<Value> {
    use solidcraft_scan::symmetric::{self, How};
    let cmd = "scan.mirror_free_form";
    let plane = mirror_plane_param(s, p.get("plane"), cmd)?;
    let target = super::surface::editable(s, crate::params::str_(p, "surface"), cmd)?;
    let gap = match crate::params::expr(p, "gap") {
        Some(_) => length(s, p, "gap", 1.0, cmd)?,
        None => target.faces.iter().map(super::surface::default_reach).fold(1.0, f64::max),
    };
    if !(gap.is_finite() && gap > 0.0) {
        return Err(bad(cmd, "`gap` must be a length above 0"));
    }
    let (faces, pairs, how) = symmetric::mirrored(&target.faces, plane, gap).map_err(|e| bad(cmd, e.to_string()))?;
    let (how, added, onto) = match how {
        How::Already => ("already", 0, 0),
        How::Mirrored { added, onto } => {
            super::surface::write_faces(s, &target, &faces, cmd)?;
            ("mirrored", added, onto)
        }
    };
    // The design keeps the plane: Edit Surface opens with Symmetry on in it, and edits keep it.
    let joins = super::network_edit::joins_for(s, &target, &faces, cmd)?;
    super::network_edit::keep(s, &target, &faces, &joins, Some(Some(plane)));
    let reply = ScanMirrorFreeFormResult {
        surface: target.body,
        faces: faces.len(),
        how: how.into(),
        added,
        onto,
        pairs: pairs.pairs(),
        on_plane: pairs.on_plane_count(),
        kink: solidcraft_scan::network::worst_kink(&faces, &joins),
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Two points `[x, y, z]` under `key`.
fn two_points(p: &Value, key: &str, cmd: &str, what: &str) -> Result<[Vec3; 2]> {
    let list = p.get(key).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{key}` must be two points [x, y, z]: {what}")))?;
    let points: Vec<Vec3> = list
        .iter()
        .map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, format!("a point of `{key}` must be [x, y, z] of numbers"))))
        .collect::<Result<_>>()?;
    <[Vec3; 2]>::try_from(points).map_err(|_| bad(cmd, format!("`{key}` must be two points: {what}")))
}

pub(super) fn add_face(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.add_face";
    let [a, b] = two_points(p, "edge", cmd, "the ends of the open edge to build on")?;
    let far = two_points(p, "far", cmd, "the new face's far corners")?;
    let rows = match p.get("rows").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_u64()
            .filter(|n| (free_form::LEAST_POINTS as u64..=free_form::MOST_POINTS as u64).contains(n))
            .ok_or_else(|| bad(cmd, "`rows` must be a whole number from 4 to 12"))? as usize,
        None => free_form::LEAST_POINTS,
    };
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => Some(length(s, p, "reach", 1.0, cmd)?),
        None => None,
    };
    let target = super::surface::editable(s, crate::params::str_(p, "surface"), cmd)?;
    let joins = solidcraft_scan::network::joins(&target.faces).map_err(|e| bad(cmd, e.to_string()))?;
    let ((face, side), off) =
        solidcraft_scan::network::open_side_near(&target.faces, &joins, a, b).ok_or_else(|| bad(cmd, "the surface has no open edge to build on"))?;
    // Near enough: the two ends off by less than the edge is long, all told.
    let places = solidcraft_scan::network::network_points(&target.faces, &joins);
    let ends = joins.side(&target.faces, face, side);
    let long = match (ends.first(), ends.last()) {
        (Some(a), Some(b)) => places[*a].dist(places[*b]),
        _ => 0.0,
    };
    if !(off <= long) {
        return Err(bad(cmd, "`edge` is not near the ends of an open edge of the surface: give the two corners of the edge to build on"));
    }
    let body = crate::params::str_(p, "body").map(str::to_owned);
    // With Symmetry (given, or the plane the design keeps) the face's mirror image comes too.
    let before = super::network_edit::joins_for(s, &target, &target.faces, cmd)?;
    let (pairs, symmetry) = super::network_edit::edit_pairs(s, &target, &before, p, cmd)?;
    let (scan, made) = added_face(s, body.as_deref(), &target.faces, face, side, far, rows, reach)?;
    let new = made.len() - 1;
    let gap = target.faces.iter().map(super::surface::default_reach).fold(1.0, f64::max);
    let (made, symmetry) = match &pairs {
        Some(pairs) => match solidcraft_scan::symmetric::with_twin_face_after(&target.faces, &made, pairs.plane, gap) {
            Some(both) => (both, symmetry),
            None => (made, Some(None)),
        },
        None => (made, symmetry),
    };
    super::surface::write_faces(s, &target, &made, cmd)?;
    let joins = super::network_edit::joins_for(s, &target, &made, cmd)?;
    super::network_edit::keep(s, &target, &made, &joins, symmetry);
    let (rms, max) = match super::surface::covered(s, Some(&scan), &made[new], super::surface::default_reach(&made[new]), cmd) {
        Ok(c) if !c.points.is_empty() => solidcraft_scan::surface_edit::distance(&made[new], &c.points),
        _ => (0.0, 0.0),
    };
    let reply = ScanAddFaceResult {
        surface: target.body,
        faces: made.len(),
        twin: (made.len() > new + 1).then_some(made.len() - 1),
        face: new,
        controls: made[new].count(),
        body: scan,
        rms,
        max,
        kink: solidcraft_scan::network::worst_kink(&made, &joins),
        irregular: joins.irregular().count(),
        sharp: {
            let held: std::collections::HashSet<usize> = joins.smooth.iter().map(|t| t.seam).collect();
            joins.seams.iter().enumerate().filter(|(i, m)| (m.a == new || m.b == new) && !held.contains(i)).count()
        },
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// A net wrapped on a tube-like part of a scan, before it is kept: the scan, the cylinder it was
/// laid on, how far the part is from that on average, and the face.
#[derive(Clone, Debug)]
pub struct Wrapped {
    pub scan: String,
    pub cylinder: Cylinder,
    pub off: f64,
    pub made: Made,
    /// Where the net folds, pinches and passes through itself (Surface Check on it).
    pub trouble: [usize; 3],
    /// For a net that goes the whole way round: how far apart its two ends are (it is open there).
    pub seam: Option<f64>,
}

/// The part's reach round the point when none is given: a fifth of the scan's size.
pub fn default_wrap_reach(s: &Session, body: Option<&str>) -> Result<f64> {
    Ok(super::default_surface_radius(s, body)? * 2.0)
}

/// The net `scan.wrap_surface` would wrap on the part of the scan `body` within `reach` of `point`.
pub fn wrapped(s: &Session, body: Option<&str>, point: Vec3, reach: f64, wrap: Wrap) -> Result<Wrapped> {
    let cmd = "scan.wrap_surface";
    if !point.is_finite() {
        return Err(bad(cmd, "`point` must be a finite point"));
    }
    if !(reach.is_finite() && reach > 0.0) {
        return Err(bad(cmd, "`reach` must be a length above 0"));
    }
    let (name, on) = super::curve::projector(s, body, cmd)?;
    let state = s.model.state();
    let (positions, triangles) =
        state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let scan = solidcraft_scan::Scan::new(positions, triangles);
    let seed = solidcraft_scan::base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let snap = |ps: &[Vec3], limit: f64| ps.iter().map(|p| on.nearest(*p).filter(|(_, d)| *d <= limit).map(|(q, _)| q)).collect::<Vec<_>>();
    let w = solidcraft_scan::wrap::wrap(&scan, seed, point, reach, &wrap, snap).map_err(|e| bad(cmd, e.to_string()))?;
    let f = w.fitted;
    let trouble = solidcraft_scan::surface_check::trouble(&f.surface);
    let (rms, max) = scan_distance(s, &name, &f.surface, f.rms, cmd);
    let seam = round_seam(&f.surface, &w.cylinder);
    let made = Made { scan: Some(name.clone()), surface: f.surface, places: f.places, met: f.met, kept: f.kept, rms, max };
    Ok(Wrapped { scan: name, cylinder: w.cylinder, off: w.off, made, trouble, seam })
}

/// For a net that goes the whole way round its cylinder (its first and last columns at about the
/// same angle round the axis, as a wrap of a whole part is, open at a seam at its back): how far
/// apart those two sides are on average, mm. `None` for a net that goes only part of the way.
pub fn round_seam(surface: &NurbsSurface, c: &Cylinder) -> Option<f64> {
    use solidcraft_scan::merge::Side;
    let angle = |side: Side| -> Option<f64> {
        let p = side.at(surface, 0.5)? - c.point;
        let off = p - c.axis * p.dot(c.axis);
        let (x, y) = {
            let any = if c.axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let u = c.axis.cross(any).normalized()?;
            (u, c.axis.cross(u))
        };
        Some(off.dot(y).atan2(off.dot(x)))
    };
    let (a, b) = (angle(Side::FirstColumn)?, angle(Side::LastColumn)?);
    let apart = (b - a).sin().atan2((b - a).cos()).abs();
    if apart > 15f64.to_radians() {
        return None;
    }
    let gaps: Vec<f64> = (0..=8)
        .filter_map(|k| {
            let t = f64::from(k) / 8.0;
            Some(Side::FirstColumn.at(surface, t)?.dist(Side::LastColumn.at(surface, t)?))
        })
        .collect();
    (!gaps.is_empty()).then(|| gaps.iter().sum::<f64>() / gaps.len() as f64)
}

pub(super) fn wrap_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.wrap_surface";
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let axis = match crate::params::str_(p, "axis").unwrap_or("fitted") {
        "fitted" => None,
        "x" | "X" => Some(Vec3::X),
        "y" | "Y" => Some(Vec3::Y),
        "z" | "Z" => Some(Vec3::Z),
        other => return Err(bad(cmd, format!("`axis` must be fitted, x, y or z, not `{other}`"))),
    };
    let details = match crate::params::str_(p, "details").unwrap_or("medium") {
        "low" => 0,
        "medium" => 1,
        "fine" => 2,
        other => return Err(bad(cmd, format!("`details` must be low, medium or fine, not `{other}`"))),
    };
    let arc = match crate::params::expr(p, "arc") {
        Some(e) => {
            let e = s.doc.with_design_unit(e.as_str(), solidcraft_doc::expr::Kind::Angle);
            let radians = s.doc.eval(&e, solidcraft_doc::expr::Kind::Angle).map_err(|e| bad(cmd, format!("arc: {e}")))?;
            if !(radians.is_finite() && (0.0..=std::f64::consts::TAU + 1e-9).contains(&radians)) {
                return Err(bad(cmd, "`arc` must be an angle from 0 to 360°"));
            }
            (radians > 0.0).then_some(radians)
        }
        None => None,
    };
    let stiffness = match p.get("stiffness").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`stiffness` must be a number from 0 to 1"))?,
        None => 0.0,
    };
    let body = super::mesh_body_name(s, crate::params::str_(p, "body"), cmd)?;
    let point = point.or_else(|| super::picked_point(s, &body)).ok_or_else(|| bad(cmd, "pick a point on the part of the scan to wrap"))?;
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => length(s, p, "reach", 1.0, cmd)?,
        None => default_wrap_reach(s, Some(&body))?,
    };
    let w = wrapped(s, Some(&body), point, reach, Wrap { axis, details, arc, stiffness })?;
    let trouble = w.trouble;
    let (name, feature) = super::surface::keep_surface(s, &w.made.surface, "FreeForm", "Wrap Surface", &format!("{body} (wrap)"), cmd)?;
    let at = |v: Vec3| [v.x, v.y, v.z];
    let reply = ScanWrapSurfaceResult {
        body,
        surface: name,
        controls: w.made.surface.count(),
        point: at(w.cylinder.point),
        axis: at(w.cylinder.axis),
        radius: w.cylinder.radius,
        off: w.off,
        places: w.made.places,
        met: w.made.met,
        rms: w.made.rms,
        max: w.made.max,
        seam: w.seam,
        folds: trouble[0],
        pinches: trouble[1],
        crossings: trouble[2],
        warning: wrap_warning(trouble, stiffness),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// What to tell someone about a wrap that folds, pinches or passes through itself: where its net
/// follows rims and ribs it is too coarse for. Stiffness smooths them out (on the online test
/// scans 0.05 to 0.1 left none, a little further from the scan); finer Details fold more, and
/// Merge Surfaces cannot sew a folded seam. `None` when it does none of them.
pub(super) fn wrap_warning(trouble: [usize; 3], stiffness: f64) -> Option<String> {
    (trouble.iter().sum::<usize>() > 0).then(|| {
        let raise = if stiffness < 0.1 { "raise Stiffness a little (0.05 to 0.1)" } else { "raise Stiffness further" };
        let how = match trouble {
            [0, 0, _] => "passes through itself",
            [0, _, 0] => "pinches",
            _ => "folds over itself",
        };
        format!(
            "the surface {how} ({}) where the net follows rims and ribs it is too coarse for: {raise}, or wrap a smaller reach; Merge Surfaces cannot sew a folded seam, and finer Details fold more",
            super::surface::trouble_text(trouble)
        )
    })
}

/// The surfaces Merge works on: those named, else the selected ones, else every surface in the
/// design whose net can be edited. One alone is merged with itself: a wrap closed at its seam.
pub fn merge_candidates(s: &Session, names: Option<&[String]>, cmd: &str) -> Result<Vec<super::surface::EditableSurface>> {
    if let Some(names) = names {
        let mut seen: Vec<&str> = Vec::new();
        let mut out = Vec::new();
        for name in names {
            if seen.contains(&name.as_str()) {
                continue;
            }
            seen.push(name);
            out.push(super::surface::editable(s, Some(name), cmd)?);
        }
        return Ok(out);
    }
    let mut selected: Vec<String> = Vec::new();
    for x in &s.selection {
        if let crate::Sel::Body { name } | crate::Sel::Face { body: name, .. } | crate::Sel::Edge { body: name, .. } = x
            && !selected.contains(name)
        {
            selected.push(name.clone());
        }
    }
    let chosen: Vec<_> = selected.iter().filter_map(|n| super::surface::editable(s, Some(n), cmd).ok()).collect();
    if !chosen.is_empty() {
        return Ok(chosen);
    }
    let names: Vec<String> = s.model.state().bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.clone()).collect();
    Ok(names.iter().filter_map(|n| super::surface::editable(s, Some(n), cmd).ok()).collect())
}

/// The gap a merge closes when none is given: a tenth of the shortest side of the nets, measured
/// along it (a net wrapped round a part has corners close across its seam).
pub fn default_gap(surfaces: &[NurbsSurface]) -> f64 {
    let shortest =
        surfaces.iter().flat_map(|s| solidcraft_scan::merge::SIDES.map(|side| side.length(s))).filter(|l| *l > 0.0).fold(f64::INFINITY, f64::min);
    if shortest.is_finite() { 0.1 * shortest } else { 1.0 }
}

/// Sides this many gaps apart or nearer are worth a wider gap, said when Merge finds no seam.
const NEAR_ENOUGH: f64 = 3.0;

/// Which surface each face of the merge belongs to: a surface's faces, in order, surface by surface.
pub fn face_owners(targets: &[super::surface::EditableSurface]) -> Vec<usize> {
    targets.iter().enumerate().flat_map(|(i, t)| std::iter::repeat_n(i, t.faces.len())).collect()
}

/// The surfaces as Merge would make them meet, within `gap` (default [`default_gap`]), and the
/// gap used. Every face of every surface is taken (a Free Form of several faces whole), so the
/// merged faces are one per face, surface by surface ([`face_owners`]); the seams are those the
/// merge makes, not the sides a Free Form's faces already share.
pub fn merged(s: &Session, names: Option<&[String]>, gap: Option<f64>) -> Result<(Vec<super::surface::EditableSurface>, Merged, f64)> {
    let cmd = "scan.merge_surfaces";
    let targets = merge_candidates(s, names, cmd)?;
    if targets.is_empty() {
        return Err(bad(cmd, "the design has no surface to merge: make them with New Free Form, Wrap Surface or Fit Surface"));
    }
    let surfaces: Vec<NurbsSurface> = targets.iter().flat_map(|t| t.faces.iter().cloned()).collect();
    if surfaces.len() > solidcraft_scan::network::MOST_FACES {
        return Err(bad(
            cmd,
            format!("the surfaces have {} faces together; a Free Form has at most {}", surfaces.len(), solidcraft_scan::network::MOST_FACES),
        ));
    }
    let owners = face_owners(&targets);
    let gap = gap.unwrap_or_else(|| default_gap(&surfaces));
    if !(gap.is_finite() && gap > 0.0) {
        return Err(bad(cmd, "`gap` must be a length above 0"));
    }
    let mut made = solidcraft_scan::merge::merge(&surfaces, gap).map_err(|e| match e {
        // How far off the nearest sides are, and how wide a gap reaches them.
        solidcraft_scan::merge::MergeError::NoSeam => match solidcraft_scan::merge::nearest_sides(&surfaces) {
            Some((apart, own)) => {
                // A gap to reach them is worth giving only when they are near: sides far apart
                // made one would bend the surfaces out of shape.
                let reach = if apart <= NEAR_ENOUGH * gap { format!(", or give a gap of {:.2} mm or more", 1.05 * apart) } else { String::new() };
                if own && surfaces.len() == 1 {
                    bad(
                        cmd,
                        format!(
                            "the surface does not close round on itself: its opposite sides are {apart:.2} mm apart on average, more than the gap ({gap:.2} mm); wrap a part that goes round its axis (on one that does not, the net's back meets other parts of the scan){reach}"
                        ),
                    )
                } else {
                    bad(
                        cmd,
                        format!(
                            "no side of one surface runs along a side of another within the gap ({gap:.2} mm): the nearest two are {apart:.2} mm apart on average; put their corners together{reach}"
                        ),
                    )
                }
            }
            None => bad(cmd, e.to_string()),
        },
        e => bad(cmd, e.to_string()),
    })?;
    // The sides a Free Form's own faces share already are not seams the merge makes.
    let own = |m: &Seam| m.a != m.b && owners.get(m.a) == owners.get(m.b) && m.gap <= 1e-9;
    made.seams.retain(|m| !own(m));
    if made.seams.is_empty() {
        return Err(bad(cmd, format!("no side of one surface runs along a side of another within the gap ({gap:.2} mm)")));
    }
    Ok((targets, made, gap))
}

/// The merged faces as one Free Form: a face closed on its own seam cut into two halves there,
/// so that each seam is between two faces (the kernel never pairs the edges of one face).
fn one_network(made: &Merged, cmd: &str) -> Result<Vec<NurbsSurface>> {
    let mut faces = Vec::with_capacity(made.surfaces.len() + 1);
    for (i, surface) in made.surfaces.iter().enumerate() {
        match made.seams.iter().find(|m| m.a == i && m.b == i) {
            Some(seam) => {
                let (a, b) = solidcraft_scan::merge::halves(surface, 1 - seam.a_side.along()).map_err(|e| bad(cmd, e.to_string()))?;
                faces.push(a);
                faces.push(b);
            }
            None => faces.push(surface.clone()),
        }
    }
    Ok(faces)
}

/// Why sewing `surfaces` (`what`: "the seam", "the surfaces") failed, in words rather than the
/// kernel's test names: the joined surface folding over itself where its sides were brought
/// together (by Surface Check), else what the kernel's check refused.
pub(super) fn sewing_refusal(what: &str, surfaces: &[&NurbsSurface], kernel: &str, cmd: &str) -> EngineError {
    let trouble = surfaces.iter().map(|f| solidcraft_scan::surface_check::trouble(f)).fold([0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
    if trouble.iter().sum::<usize>() > 0 {
        return bad(
            cmd,
            format!(
                "{what} could not be sewn: the surface folds over itself ({}): a wrap whose net follows rims and ribs too fine for it (wrap it again with Stiffness 0.05 to 0.1), or sides brought together that ran apart (Edit Surface, or merge with a smaller gap)",
                super::surface::trouble_text(trouble)
            ),
        );
    }
    bad(cmd, format!("{what} could not be sewn: {}", kernel_words(kernel)))
}

/// What the kernel's check refused, in words: each kind of failure it names, once.
fn kernel_words(kernel: &str) -> String {
    let kinds = [
        ("NonMonotonic", "the control net doubles back on itself"),
        ("CurveCusp", "an edge curve kinks sharply"),
        ("Bad Gap", "an edge does not lie on the faces it joins"),
        ("Bad Edge loc", "an edge does not lie on the faces it joins"),
        ("ntersect", "the surface passes through itself"),
    ];
    let mut said: Vec<&str> = Vec::new();
    for (name, words) in kinds {
        if kernel.contains(name) && !said.contains(&words) {
            said.push(words);
        }
    }
    if said.is_empty() { "the kernel's check refused the joined surface".to_string() } else { said.join("; ") }
}

pub(super) fn merge_surfaces(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.merge_surfaces";
    let names = match p.get("surfaces").filter(|v| !v.is_null()) {
        Some(v) => Some(
            v.as_array()
                .filter(|a| a.len() <= 64)
                .ok_or_else(|| bad(cmd, "`surfaces` must list up to 64 surface body names"))?
                .iter()
                .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| bad(cmd, "`surfaces` must list surface body names")))
                .collect::<Result<Vec<String>>>()?,
        ),
        None => None,
    };
    let gap = match crate::params::expr(p, "gap") {
        Some(_) => Some(length(s, p, "gap", 1.0, cmd)?),
        None => None,
    };
    let stitch = crate::params::bool_(p, "stitch").unwrap_or(true);
    let (targets, made, _) = merged(s, names.as_deref(), gap)?;
    let owners = face_owners(&targets);
    let bodies: Vec<String> = targets.iter().map(|t| t.body.clone()).collect();
    let run = |s: &mut Session, id: &str, p: Value| -> Result<Value> {
        super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
    };
    let (feature, faces) = if stitch {
        // One Free Form: every face, kept in the first surface's own feature, edited with Edit
        // Surface from then on; the other surfaces are taken out of the timeline from here on.
        let faces = one_network(&made, cmd)?;
        let joins = solidcraft_scan::network::joins(&faces).map_err(|e| bad(cmd, format!("the merged faces make no Free Form: {e}")))?;
        if !solidcraft_scan::network::connected(&faces, &joins) {
            return Err(bad(cmd, "the surfaces do not all meet: a surface none of the others runs along is left out of the choice"));
        }
        let keeper = targets.first().ok_or_else(|| bad(cmd, "no surface"))?;
        let refs: Vec<&NurbsSurface> = faces.iter().collect();
        super::surface::write_faces(s, keeper, &faces, cmd).map_err(|e| sewing_refusal("the surfaces", &refs, &e.to_string(), cmd))?;
        let others: Vec<String> = bodies.iter().skip(1).cloned().collect();
        let feature = if others.is_empty() { None } else { Some(run(s, "solid.remove", serde_json::json!({ "bodies": others }))?) };
        (feature, Some(faces.len()))
    } else {
        // Each surface written back as it now meets the others, still a body of its own.
        for (i, target) in targets.iter().enumerate() {
            let own: Vec<NurbsSurface> = made.surfaces.iter().zip(&owners).filter(|(_, o)| **o == i).map(|(f, _)| f.clone()).collect();
            if own != target.faces {
                super::surface::write_faces(s, target, &own, cmd)?;
            }
        }
        (None, None)
    };
    let reply = ScanMergeSurfacesResult {
        body: stitch.then(|| bodies[0].clone()),
        surfaces: bodies,
        seams: made.seams.len(),
        gap: made.seams.iter().map(|m| m.gap).fold(0.0, f64::max),
        faces,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
mod sewing_tests {
    /// The kernel's refusal of a seam on T-LESS 01 (a wrap merged round on itself), in words: no
    /// test numbers, owners or offsets, each kind once.
    #[test]
    fn a_sewing_refusal_is_said_in_words() {
        let kernel = "the operation failed: SMLib stitch: SMLib's check refuses the result: 2 failed tests:\nSmBSplineSurface::Test #5 Bad NonMonotonic CPts: SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface [owner SmBSplineSurface]\nSmBSplineCurve::Test #14 Bad CurveCusp: 3D BSplineCurve Warning: Extremely small radius of curvature (offset=0.0028879039, tolerance=0.0076947937) [owner SmBSplineCurve]";
        let words = super::kernel_words(kernel);
        assert_eq!(words, "the control net doubles back on itself; an edge curve kinks sharply; the surface passes through itself");
        assert!(!words.contains("Sm") && !words.contains('#'));
        assert_eq!(super::kernel_words("something else"), "the kernel's check refused the joined surface");
    }
}
