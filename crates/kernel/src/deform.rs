//! Deform: a body carried through a map of space ([`SpaceMap`]: bent along a length round a
//! circle, twisted about an axis, or moved with a cage of control points). Every face's surface
//! and every edge's curve becomes a cubic B-spline on its own parameters that follows the map
//! within the tolerance ([`solidcraft_geom::deform`]); a plane and a straight line are first
//! written exactly as cubics. Because the parameters do not change, each face's trims (its
//! pcurves in its (u, v)) and each edge's interval stay as they were, and the topology is the
//! body's own. The vertices go where the map takes them.
//!
//! A face's edges and the face itself are approximated apart, so where an edge lies on a face
//! the two are a little apart: each edge's tolerance grows to cover the largest gap measured
//! between its curve and the surface along its pcurve on each face it bounds (half again, for
//! what lies between the points measured), and each vertex's to cover its edges'. Refused, with
//! the reason: a face on a surface held only as itself (a procedural blend, a revolution, a
//! surface of intersection) and an edge on a curve of intersection or a conic with no NURBS form.

use solidcore_record::Vec3;
use solidcore_record::brep::{Brep, Curve, Curve2, Face, Surface};
use solidcraft_geom::bspline::{HPoint, NurbsSurface};
pub use solidcraft_geom::deform::SpaceMap;
use solidcraft_geom::deform::{self, Spline};
use solidcraft_geom::prelude::*;

use crate::body::Body;
use crate::{KernelError, Result};

/// Points measured along each edge use for the tolerances.
const GAP_SAMPLES: usize = 24;

fn v3(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

fn failed(what: impl std::fmt::Display) -> KernelError {
    KernelError::Invalid(format!("deform: {what}"))
}

fn unsupported(what: &str) -> KernelError {
    KernelError::Invalid(format!("not supported yet: deforming {what}"))
}

/// A face's surface as a B-spline surface on the face's own (u, v).
fn surface_of(face: &Face) -> Result<NurbsSurface> {
    match face.parameters() {
        Surface::Plane { origin, u_axis, v_axis } => deform::plane(v3(*origin), v3(*u_axis), v3(*v_axis), face.domain, 1).map_err(failed),
        Surface::PlaneThrough { origin, u_point, v_point } => {
            let o = v3(*origin);
            deform::plane(o, v3(*u_point) - o, v3(*v_point) - o, face.domain, 1).map_err(failed)
        }
        Surface::Spline { u_knots, v_knots, points } => {
            let rows = points.len();
            let cols = points.first().map_or(0, Vec::len);
            let p = u_knots.len().checked_sub(rows + 1).ok_or_else(|| failed("a surface's knots do not fit its points"))?;
            let q = v_knots.len().checked_sub(cols + 1).ok_or_else(|| failed("a surface's knots do not fit its points"))?;
            let mut ctrl = vec![HPoint::ZERO; rows * cols];
            for (i, row) in points.iter().enumerate() {
                for (j, c) in row.iter().enumerate() {
                    if let Some(slot) = ctrl.get_mut(j * rows + i) {
                        *slot = HPoint::weighted(v3(*c), 1.0);
                    }
                }
            }
            NurbsSurface::from_homogeneous([p, q], [u_knots.clone(), v_knots.clone()], ctrl).map_err(failed)
        }
        Surface::RationalSpline { u_knots, v_knots, homogeneous } => {
            let rows = homogeneous.len();
            let cols = homogeneous.first().map_or(0, Vec::len);
            let p = u_knots.len().checked_sub(rows + 1).ok_or_else(|| failed("a surface's knots do not fit its points"))?;
            let q = v_knots.len().checked_sub(cols + 1).ok_or_else(|| failed("a surface's knots do not fit its points"))?;
            let mut ctrl = vec![HPoint::ZERO; rows * cols];
            for (i, row) in homogeneous.iter().enumerate() {
                for (j, c) in row.iter().enumerate() {
                    if let Some(slot) = ctrl.get_mut(j * rows + i) {
                        *slot = HPoint { x: c[0], y: c[1], z: c[2], w: c[3] };
                    }
                }
            }
            NurbsSurface::from_homogeneous([p, q], [u_knots.clone(), v_knots.clone()], ctrl).map_err(failed)
        }
        _ => Err(unsupported("a face on a surface held only as itself (a blend, a revolution, a ball patch)")),
    }
}

/// An edge's curve as a B-spline curve on its own parameter, over `interval`.
fn curve_of(curve: &Curve, interval: [f64; 2]) -> Result<Spline> {
    match curve {
        Curve::Line { start, end } => {
            let (s, e) = (v3(*start), v3(*end));
            let at = |t: f64| s + (e - s) * t;
            deform::line(at(interval[0]), at(interval[1]), interval[0], interval[1], 1).map_err(failed)
        }
        Curve::Polyline { points } => {
            let n = points.len();
            if n < 2 {
                return Err(failed("a polyline of fewer than two points"));
            }
            let mut knots = vec![0.0];
            knots.extend((0..n).map(|i| i as f64));
            knots.push((n - 1) as f64);
            Spline::new(1, knots, points.iter().map(|p| HPoint::weighted(v3(*p), 1.0)).collect()).map_err(failed)
        }
        Curve::Spline { knots, points } => {
            let p = knots.len().checked_sub(points.len() + 1).ok_or_else(|| failed("a curve's knots do not fit its points"))?;
            Spline::new(p, knots.clone(), points.iter().map(|c| HPoint::weighted(v3(*c), 1.0)).collect()).map_err(failed)
        }
        Curve::RationalSpline { knots, homogeneous } => {
            let p = knots.len().checked_sub(homogeneous.len() + 1).ok_or_else(|| failed("a curve's knots do not fit its points"))?;
            Spline::new(p, knots.clone(), homogeneous.iter().map(|c| HPoint { x: c[0], y: c[1], z: c[2], w: c[3] }).collect()).map_err(failed)
        }
        Curve::Circle { .. } | Curve::Ellipse { .. } => Err(unsupported("an edge on a conic held without its NURBS form")),
        Curve::SurfaceIntersection { .. } => Err(unsupported("an edge on a curve of intersection")),
    }
}

/// A pcurve as a planar spline (its (u, v) in x and y).
fn pcurve_of(c: &Curve2) -> Result<Spline> {
    match c {
        Curve2::Spline { knots, points } => {
            let p = knots.len().checked_sub(points.len() + 1).ok_or_else(|| failed("a pcurve's knots do not fit its points"))?;
            Spline::new(p, knots.clone(), points.iter().map(|q| HPoint::weighted(Vec3::new(q[0], q[1], 0.0), 1.0)).collect()).map_err(failed)
        }
        Curve2::RationalSpline { knots, homogeneous } => {
            let p = knots.len().checked_sub(homogeneous.len() + 1).ok_or_else(|| failed("a pcurve's knots do not fit its points"))?;
            Spline::new(p, knots.clone(), homogeneous.iter().map(|q| HPoint { x: q[0], y: q[1], z: 0.0, w: q[2] }).collect()).map_err(failed)
        }
    }
}

/// The box round a face's pcurves' control points, a millionth of its size larger each way,
/// within the face's domain; `None` for a face with no edge loop.
fn used_box(face: &Face) -> Option<[[f64; 2]; 2]> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for lp in &face.loops {
        let solidcore_record::brep::Loop::Edges { edges } = lp else { continue };
        for used in edges {
            let points: Vec<[f64; 2]> = match &used.pcurve {
                Curve2::Spline { points, .. } => points.clone(),
                Curve2::RationalSpline { homogeneous, .. } => homogeneous.iter().filter(|h| h[2] > 0.0).map(|h| [h[0] / h[2], h[1] / h[2]]).collect(),
            };
            for q in points {
                for d in 0..2 {
                    lo[d] = lo[d].min(q[d]);
                    hi[d] = hi[d].max(q[d]);
                }
            }
        }
    }
    let mut out = face.domain;
    for d in 0..2 {
        if !(lo[d].is_finite() && hi[d].is_finite() && hi[d] > lo[d]) {
            return None;
        }
        let pad = 1e-6 * (hi[d] - lo[d]);
        out[d] = [(lo[d] - pad).max(face.domain[d][0]), (hi[d] + pad).min(face.domain[d][1])];
    }
    Some(out)
}

fn surface_record(s: &NurbsSurface) -> Surface {
    let [nu, nv] = s.count();
    let points = (0..nu).map(|i| (0..nv).map(|j| s.point(i, j).project().map_or([0.0; 3], |p| [p.x, p.y, p.z])).collect()).collect();
    Surface::Spline { u_knots: s.knots()[0].clone(), v_knots: s.knots()[1].clone(), points }
}

fn curve_record(c: &Spline) -> Curve {
    Curve::Spline { knots: c.knots.clone(), points: c.ctrl.iter().map(|h| h.project().map_or([0.0; 3], |p| [p.x, p.y, p.z])).collect() }
}

/// `body` carried through `map`, every face and edge within `tolerance` of the map of the body.
pub fn deform(body: &Body, map: &SpaceMap, tolerance: f64) -> Result<Body> {
    let _span = solidcraft_profile::span("kernel", "deform");
    body.require_brep("deform")?;
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(KernelError::Invalid("deform: the tolerance must be a length above 0".into()));
    }
    let map = map.clone().checked().map_err(failed)?;
    let mut rec: Brep = body.record().clone();
    let mut surfaces: Vec<NurbsSurface> = Vec::with_capacity(rec.faces.len());
    for face in &mut rec.faces {
        let surface = surface_of(face)?;
        // Only what the face uses: the box round its pcurves (each lies within its control
        // points), a hair larger, within its domain. A plane's domain can reach far past the
        // face, over places the map bends sharply that the face never meets.
        let used = used_box(face).unwrap_or(face.domain);
        let (moved, _) = deform::deform_surface_over(&surface, used, &map, tolerance).map_err(failed)?;
        face.surface = surface_record(&moved);
        face.nurbs = None;
        face.domain = used;
        surfaces.push(moved);
    }
    let mut curves: Vec<Spline> = Vec::with_capacity(rec.edges.len());
    for edge in &mut rec.edges {
        let (moved, _) = deform::deform_curve(&curve_of(edge.parameters(), edge.interval)?, &map, tolerance).map_err(failed)?;
        edge.curve = curve_record(&moved);
        edge.nurbs = None;
        curves.push(moved);
    }
    for vertex in &mut rec.vertices {
        let p = map.apply(v3(vertex.point));
        if !p.is_finite() {
            return Err(failed("the map is not finite at a vertex"));
        }
        vertex.point = [p.x, p.y, p.z];
    }
    // Each edge's tolerance covers its gap to every face it bounds, along its pcurve there.
    let mut gaps = vec![0.0f64; rec.edges.len()];
    for (f, face) in rec.faces.iter().enumerate() {
        let Some(surface) = surfaces.get(f) else { continue };
        let dom = surface.domain();
        for lp in &face.loops {
            let solidcore_record::brep::Loop::Edges { edges } = lp else { continue };
            for used in edges {
                let e = used.edge as usize;
                let (Some(curve), Some(edge)) = (curves.get(e), rec.edges.get(e)) else { continue };
                let pcurve = pcurve_of(&used.pcurve)?;
                let [a, b] = edge.interval;
                for k in 0..=GAP_SAMPLES {
                    let t = a + (b - a) * k as f64 / GAP_SAMPLES as f64;
                    let uv = pcurve.eval(t).map_err(failed)?;
                    let (u, v) = (uv.x.clamp(dom[0][0], dom[0][1]), uv.y.clamp(dom[1][0], dom[1][1]));
                    let on_face = surface.eval(u, v).map_err(failed)?;
                    let on_edge = curve.eval(t).map_err(failed)?;
                    if let Some(g) = gaps.get_mut(e) {
                        *g = g.max(on_face.dist(on_edge));
                    }
                }
            }
        }
    }
    for (edge, gap) in rec.edges.iter_mut().zip(&gaps) {
        edge.tolerance = edge.tolerance.max(1.5 * gap);
    }
    // Each vertex's covers its edges' ends and tolerances.
    let mut reach = vec![0.0f64; rec.vertices.len()];
    for (e, edge) in rec.edges.iter().enumerate() {
        let Some(curve) = curves.get(e) else { continue };
        for (end, &v) in edge.interval.iter().zip(&edge.vertices) {
            let Some(vertex) = rec.vertices.get(v as usize) else { continue };
            let off = curve.eval(*end).map_err(failed)?.dist(v3(vertex.point));
            if let Some(r) = reach.get_mut(v as usize) {
                *r = r.max(1.5 * off).max(edge.tolerance);
            }
        }
    }
    for (vertex, r) in rec.vertices.iter_mut().zip(&reach) {
        vertex.tolerance = vertex.tolerance.max(*r);
    }
    Body::from_record(rec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bar 100 × 10 × 10 bent a quarter round about its bottom edge's line: its bottom keeps
    /// its length, its top goes round 10 nearer the centre, so its volume is 100 · 10 · (10 − 10²
    /// / (2r)) for r = 100 / (π/2); a twist of a square bar keeps its volume.
    #[test]
    fn a_bent_bar_and_a_twisted_one_keep_the_volume_they_should() {
        let bar = crate::box_solid(Vec3::ZERO, Vec3::new(100.0, 10.0, 10.0)).unwrap();
        let quarter = std::f64::consts::FRAC_PI_2;
        let bend =
            SpaceMap::Bend { origin: Vec3::ZERO, along: Vec3::new(1.0, 0.0, 0.0), up: Vec3::new(0.0, 0.0, 1.0), length: 100.0, angle: quarter };
        let bent = deform(&bar, &bend, 1e-4).unwrap();
        let r = 100.0 / quarter;
        let want = 100.0 * 10.0 * (10.0 - 100.0 / (2.0 * r));
        let got = crate::measure(&bent).unwrap().volume;
        assert!((got - want).abs() < 0.05, "{got} against {want}");
        let twist = SpaceMap::Twist { origin: Vec3::new(0.0, 5.0, 5.0), axis: Vec3::new(1.0, 0.0, 0.0), length: 100.0, angle: quarter };
        let twisted = deform(&bar, &twist, 1e-4).unwrap();
        let v = crate::measure(&twisted).unwrap().volume;
        assert!((v - 10_000.0).abs() < 0.5, "{v}");
    }
}
