//! A fitted shape kept as a face trimmed to the region the scan has of it (`make: face`), built
//! from ordinary features so it stays editable and reopens anywhere:
//!
//! - a plane: the region's outline (its open edges, chained into loops and thinned) sketched on
//!   the plane and filled with Patch, the holes in it left open;
//! - a cylinder or a cone: the line of its wall across the height the region spans, sketched on
//!   a plane through the axis and turned with Surface Revolve through the angle the region spans
//!   (all the way round when it wraps);
//! - a sphere: the arc from the pole at the region's middle out to its farthest point, turned
//!   all the way round (a cap);
//! - a torus: the arc of the tube the region spans, turned through the angle it spans round the
//!   axis.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use serde_json::{Value, json};
use solidcore_record::{Plane, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::fit::{Fitted, Shape};

use super::Mesh;
use crate::params::bad;
use crate::schema::scan::ScanFitFace;
use crate::{EngineError, Result, Session};

/// The most region points read for the spans (the rest skipped evenly).
const SPAN_POINTS: usize = 60_000;
/// A gap round the axis narrower than this is no gap: the region goes all the way round.
const WRAPS_GAP: f64 = 12.0 * PI / 180.0;
/// The most corners an outline keeps after thinning.
const OUTLINE_MOST: usize = 400;
/// A hole in a plane's region smaller than this share of its outline's area is the scan's, not the
/// part's: it is filled.
const LEAST_HOLE: f64 = 0.005;

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

fn at(p: Vec3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

fn in_plane(plane: &Plane, p: Vec3) -> [f64; 2] {
    let d = p - plane.origin;
    [d.dot(plane.x), d.dot(plane.y)]
}

/// A new sketch on `plane`, left open for drawing; its id.
fn open_sketch(s: &mut Session, plane: &Plane, name: &str) -> Result<u64> {
    let made = run(s, "sketch.create", json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": s.doc.free_sketch_name(name)}))?;
    made.get("sketch").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the sketch was not made".into()))
}

/// The curve ids a draw command's reply lists.
fn curves(reply: &Value) -> Vec<Value> {
    let list = reply.get("curves").or_else(|| reply.get("created").and_then(|c| c.get("curves")));
    list.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Draws in the open sketch and finishes it, or cancels it when drawing fails.
fn drawing<T>(s: &mut Session, draw: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    match draw(s) {
        Ok(done) => {
            run(s, "sketch.finish", json!({}))?;
            Ok(done)
        }
        Err(e) => {
            let _ = run(s, "sketch.cancel", json!({}));
            Err(e)
        }
    }
}

/// The surface body a feature command made.
fn made_body(reply: &Value) -> Option<String> {
    let named = reply.get("body").and_then(Value::as_str).map(str::to_owned);
    named.or_else(|| ["created_bodies", "bodies"].iter().find_map(|k| reply.get(*k).and_then(Value::as_array)?.first()?.as_str().map(str::to_owned)))
}

/// The region's points, each once, at most [`SPAN_POINTS`] of them.
fn region_points(mesh: &Mesh, triangles: &[usize]) -> Vec<Vec3> {
    let mut seen = vec![false; mesh.positions.len()];
    let mut out = Vec::new();
    for &t in triangles {
        for &v in mesh.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), mesh.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                out.push(*p);
            }
        }
    }
    let step = out.len().div_ceil(SPAN_POINTS).max(1);
    out.into_iter().step_by(step).collect()
}

/// The span of `angles` (radians) round a circle: where it starts and how far it turns, the
/// widest gap left out; all the way round (`TAU`) when no gap is wider than [`WRAPS_GAP`].
pub(crate) fn span(angles: &mut [f64]) -> (f64, f64) {
    if angles.len() < 2 {
        return (0.0, TAU);
    }
    angles.sort_by(f64::total_cmp);
    let (mut gap, mut after) = (angles[0] + TAU - angles[angles.len() - 1], 0usize);
    for (i, w) in angles.windows(2).enumerate() {
        if w[1] - w[0] > gap {
            gap = w[1] - w[0];
            after = i + 1;
        }
    }
    if gap < WRAPS_GAP { (0.0, TAU) } else { (angles[after], TAU - gap) }
}

/// Two directions square to `axis` and to each other, right-handed with it.
fn across(axis: Vec3) -> (Vec3, Vec3) {
    let e1 = axis.any_perp().normalized().unwrap_or(Vec3::X);
    (e1, axis.cross(e1))
}

/// The region's open edges chained into closed loops (vertex ids).
fn boundary_loops(mesh: &Mesh, triangles: &[usize]) -> Vec<Vec<u32>> {
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    for &t in triangles {
        let Some(&[a, b, c]) = mesh.triangles.get(t) else { continue };
        for (p, q) in [(a, b), (b, c), (c, a)] {
            *count.entry((p.min(q), p.max(q))).or_default() += 1;
        }
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &t in triangles {
        let Some(&[a, b, c]) = mesh.triangles.get(t) else { continue };
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if count.get(&(p.min(q), p.max(q))) == Some(&1) {
                next.entry(p).or_default().push(q);
            }
        }
    }
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for start in starts {
        while let Some(first) = next.get_mut(&start).and_then(Vec::pop) {
            let mut ring = vec![start];
            let mut at_v = first;
            let mut guard = 0usize;
            while at_v != start && guard <= count.len() {
                ring.push(at_v);
                guard += 1;
                match next.get_mut(&at_v).and_then(Vec::pop) {
                    Some(v) => at_v = v,
                    None => break,
                }
            }
            if at_v == start && ring.len() >= 3 {
                loops.push(ring);
            }
        }
    }
    loops
}

/// The signed area of a closed polygon.
fn area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i][0] * ring[(i + 1) % n][1] - ring[(i + 1) % n][0] * ring[i][1]).sum::<f64>() / 2.0
}

fn off_line(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if len < 1e-12 { (p[0] - a[0]).hypot(p[1] - a[1]) } else { ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len }
}

/// The corners of the open polyline `pts` kept by Douglas-Peucker within `eps`, ends included.
fn thin_open(pts: &[[f64; 2]], eps: f64, out: &mut Vec<[f64; 2]>) {
    let mut keep = vec![false; pts.len()];
    if let Some(last) = pts.len().checked_sub(1) {
        keep[0] = true;
        keep[last] = true;
        let mut stack = vec![(0usize, last)];
        while let Some((i, j)) = stack.pop() {
            if j <= i + 1 {
                continue;
            }
            let (mut far, mut at_k) = (0.0, i);
            for k in i + 1..j {
                let d = off_line(pts[k], pts[i], pts[j]);
                if d > far {
                    far = d;
                    at_k = k;
                }
            }
            if far > eps {
                keep[at_k] = true;
                stack.push((i, at_k));
                stack.push((at_k, j));
            }
        }
    }
    out.extend(pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p));
}

/// A closed ring thinned within `eps`: split at its first corner and the corner farthest from it.
fn thin_ring(ring: &[[f64; 2]], eps: f64) -> Vec<[f64; 2]> {
    let Some(&first) = ring.first() else { return Vec::new() };
    let far = (0..ring.len()).max_by(|&a, &b| {
        let d = |k: usize| (ring[k][0] - first[0]).hypot(ring[k][1] - first[1]);
        d(a).total_cmp(&d(b))
    });
    let Some(far) = far.filter(|&f| f > 0) else { return ring.to_vec() };
    let mut out = Vec::new();
    thin_open(&ring[..=far], eps, &mut out);
    out.pop();
    let mut back: Vec<[f64; 2]> = ring[far..].to_vec();
    back.push(first);
    thin_open(&back, eps, &mut out);
    out.pop();
    out
}

/// A plane's region: its outline and the holes in it, in the plane's frame, thinned.
fn outlines(mesh: &Mesh, triangles: &[usize], plane: &Plane, tolerance: f64) -> Vec<Vec<[f64; 2]>> {
    let mut rings: Vec<Vec<[f64; 2]>> = boundary_loops(mesh, triangles)
        .iter()
        .map(|l| l.iter().filter_map(|&v| mesh.positions.get(v as usize)).map(|p| in_plane(plane, *p)).collect())
        .collect();
    rings.sort_by(|a, b| area(b).abs().total_cmp(&area(a).abs()));
    let Some(outer) = rings.first() else { return Vec::new() };
    let whole = area(outer).abs();
    let (lo, hi) =
        outer.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let size = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    // An outline of triangle edges steps in and out by about an edge: thinned within an edge's
    // length, the steps go and the corners (the points farthest out) stay where they are.
    let mut edges: Vec<f64> = outer.iter().zip(outer.iter().cycle().skip(1)).map(|(a, b)| (b[0] - a[0]).hypot(b[1] - a[1])).collect();
    edges.sort_by(f64::total_cmp);
    let edge = edges.get(edges.len() / 2).copied().unwrap_or(0.0);
    let mut eps = (2.0 * tolerance).max(0.004 * size).max(edge);
    loop {
        let thinned: Vec<Vec<[f64; 2]>> = rings
            .iter()
            .enumerate()
            .filter(|(k, r)| *k == 0 || area(r).abs() > LEAST_HOLE * whole)
            .map(|(_, r)| thin_ring(r, eps))
            .filter(|r| r.len() >= 3 && area(r).abs() > 0.0)
            .collect();
        if thinned.iter().map(Vec::len).sum::<usize>() <= OUTLINE_MOST || eps > size {
            return thinned;
        }
        eps *= 1.5;
    }
}

/// A sketch on `plane` of the outline of the scan's `triangles` and the holes in it, seen square to
/// the plane and thinned; its id.
pub(super) fn outline_sketch(s: &mut Session, mesh: &Mesh, triangles: &[usize], plane: &Plane, tolerance: f64, name: &str, cmd: &str) -> Result<u64> {
    let rings = outlines(mesh, triangles, plane, tolerance);
    if rings.is_empty() {
        return Err(bad(cmd, "the region has no outline to make a face of: it closes round on itself"));
    }
    let sketch = open_sketch(s, plane, name)?;
    drawing(s, |s| {
        for ring in &rings {
            run(s, "sketch.line", json!({"points": ring, "closed": true}))?;
        }
        Ok(())
    })?;
    Ok(sketch)
}

/// The plane the scan's `triangles` lie across: through their middle, square to the way they face
/// (their normals weighted by area).
pub(super) fn region_plane(mesh: &Mesh, triangles: &[usize]) -> Option<Plane> {
    let (mut facing, mut middle, mut total) = (Vec3::ZERO, Vec3::ZERO, 0.0);
    for &t in triangles {
        let Some(&[a, b, c]) = mesh.triangles.get(t) else { continue };
        let (Some(&a), Some(&b), Some(&c)) = (mesh.positions.get(a as usize), mesh.positions.get(b as usize), mesh.positions.get(c as usize)) else {
            continue;
        };
        let cross = (b - a).cross(c - a);
        let area = cross.len() / 2.0;
        facing += cross;
        middle += (a + b + c) / 3.0 * area;
        total += area;
    }
    let normal = facing.normalized()?;
    if !(total > 0.0) {
        return None;
    }
    let x = normal.any_perp().normalized()?;
    Some(Plane { origin: middle / total, x, y: normal.cross(x) })
}

/// The fitted shape kept as a face trimmed to its region (see the module's note).
pub(super) fn keep_face(s: &mut Session, mesh: &Mesh, fitted: &Fitted, tolerance: f64, cmd: &str) -> Result<ScanFitFace> {
    let points = region_points(mesh, &fitted.triangles);
    if points.len() < 3 {
        return Err(bad(cmd, "the region is too small to make a face of"));
    }
    match fitted.shape {
        Shape::Plane { normal, .. } => {
            let x = normal.any_perp().normalized().unwrap_or(Vec3::X);
            let plane = Plane { origin: fitted.middle, x, y: normal.cross(x) };
            let sketch = outline_sketch(s, mesh, &fitted.triangles, &plane, tolerance, "Fitted Face", cmd)?;
            let profiles = super::reconstruct::material_profiles(s, sketch);
            let mut p = json!({"sketch": sketch, "name": "Fitted Face"});
            if let Some(list) = profiles {
                p["profiles"] = json!(list);
            }
            let feature = run(s, "surface.patch", p)?;
            let surface = made_body(&feature).unwrap_or_default();
            Ok(ScanFitFace { surface, sketch, feature, sweep: None })
        }
        Shape::Cylinder { point, axis, radius } => turned(s, &points, point, axis, cmd, |_| radius),
        // From the apex, the radius grows by the half angle's tangent along the axis.
        Shape::Cone { apex, axis, half_angle } => turned(s, &points, apex, axis, cmd, |h| h.max(0.0) * half_angle.tan()),
        Shape::Sphere { center, radius } => {
            let pole = points.iter().fold(Vec3::ZERO, |sum, p| sum + (*p - center)).normalized().unwrap_or(Vec3::Z);
            let farthest = points.iter().filter_map(|p| (*p - center).normalized()).map(|d| d.dot(pole).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max);
            let polar = if farthest > PI - WRAPS_GAP { PI } else { farthest.max(1e-3) };
            let (y, _) = across(pole);
            let plane = Plane { origin: center, x: pole, y };
            let sketch = open_sketch(s, &plane, "Fitted Face")?;
            let ids = drawing(s, |s| {
                let drawn = run(s, "sketch.arc.center_point", json!({"center": [0.0, 0.0], "start": [radius, 0.0], "sweep": polar.to_degrees()}))?;
                Ok(curves(&drawn))
            })?;
            let feature = run(s, "surface.revolve", json!({"sketch": sketch, "curves": ids, "axis": "x", "name": "Fitted Face"}))?;
            let surface = made_body(&feature).unwrap_or_default();
            Ok(ScanFitFace { surface, sketch, feature, sweep: Some(360.0) })
        }
        Shape::Torus { center, axis, major, minor } => {
            let (e1, e2) = across(axis);
            let mut round: Vec<f64> = Vec::with_capacity(points.len());
            let mut tube: Vec<f64> = Vec::with_capacity(points.len());
            for p in &points {
                let v = *p - center;
                let h = v.dot(axis);
                let out = v - axis * h;
                round.push(out.dot(e2).atan2(out.dot(e1)));
                tube.push(h.atan2(out.len() - major));
            }
            let (start, sweep) = span(&mut round);
            let (tube_start, tube_sweep) = span(&mut tube);
            let mid = start + sweep / 2.0;
            let plane = Plane { origin: center, x: axis, y: e1 * mid.cos() + e2 * mid.sin() };
            let sketch = open_sketch(s, &plane, "Fitted Face")?;
            let ids = drawing(s, |s| {
                // In the sketch, x along the axis and y out from it: the tube's middle at (0, major),
                // a point of the tube `psi` round from the outside at (minor sin, major + minor cos).
                let drawn = if tube_sweep >= TAU {
                    run(s, "sketch.circle.two_point", json!({"p0": [-minor, major], "p1": [minor, major]}))?
                } else {
                    let from = [minor * tube_start.sin(), major + minor * tube_start.cos()];
                    run(s, "sketch.arc.center_point", json!({"center": [0.0, major], "start": from, "sweep": -tube_sweep.to_degrees()}))?
                };
                Ok(curves(&drawn))
            })?;
            revolve(s, sketch, ids, sweep)
        }
    }
}

/// A cylinder or a cone: the wall's line across the region's height, at `radius_at` each height
/// along the axis from `origin`, turned through the angle the region spans.
fn turned(s: &mut Session, points: &[Vec3], origin: Vec3, axis: Vec3, cmd: &str, radius_at: impl Fn(f64) -> f64) -> Result<ScanFitFace> {
    let (e1, e2) = across(axis);
    let mut round: Vec<f64> = Vec::with_capacity(points.len());
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for p in points {
        let v = *p - origin;
        let h = v.dot(axis);
        lo = lo.min(h);
        hi = hi.max(h);
        round.push(v.dot(e2).atan2(v.dot(e1)));
    }
    if !(hi - lo > 1e-6) {
        return Err(bad(cmd, "the region has no height along the axis to make a face of"));
    }
    let (start, sweep) = span(&mut round);
    let mid = start + sweep / 2.0;
    let plane = Plane { origin, x: axis, y: e1 * mid.cos() + e2 * mid.sin() };
    let sketch = open_sketch(s, &plane, "Fitted Face")?;
    let lo = if radius_at(lo) <= 0.0 && radius_at(hi) > 0.0 { lo.max(0.0) } else { lo };
    let ids = drawing(s, |s| {
        let drawn = run(s, "sketch.line", json!({"points": [[lo, radius_at(lo)], [hi, radius_at(hi)]]}))?;
        Ok(curves(&drawn))
    })?;
    revolve(s, sketch, ids, sweep)
}

/// The profile turned about the sketch's x axis through `sweep` (radians) centred on the sketch,
/// or all the way round.
fn revolve(s: &mut Session, sketch: u64, ids: Vec<Value>, sweep: f64) -> Result<ScanFitFace> {
    let mut p = json!({"sketch": sketch, "curves": ids, "axis": "x", "name": "Fitted Face"});
    if sweep < TAU {
        p["angle"] = json!(format!("{} deg", sweep.to_degrees()));
        p["symmetric"] = json!(true);
    }
    let feature = run(s, "surface.revolve", p)?;
    let surface = made_body(&feature).unwrap_or_default();
    Ok(ScanFitFace { surface, sketch, feature, sweep: Some(sweep.to_degrees().min(360.0)) })
}
