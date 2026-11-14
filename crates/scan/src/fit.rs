//! A plane, cylinder, sphere, cone or torus fitted to the part of a scan around a picked point. The region starts as
//! the triangles near the pick, the shape is fitted to it, and the region then grows across edges
//! to every connected triangle that lies on the shape (within the tolerance, and facing as the
//! shape does there), the shape fitted again each time the region doubles. Growing against the
//! shape rather than across gentle edges keeps a noisy scan's region whole.

use std::f64::consts::{FRAC_PI_2, PI};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::base::{EdgeNeighbours, edge_neighbours};
use crate::{Scan, in_plane};

/// A fitted shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A plane through `point`, square to the unit `normal`.
    Plane { point: Vec3, normal: Vec3 },
    /// A cylinder about the line through `point` along the unit `axis`.
    Cylinder { point: Vec3, axis: Vec3, radius: f64 },
    /// A sphere about `center`.
    Sphere { center: Vec3, radius: f64 },
    /// A cone from `apex` along the unit `axis`, the way its radius grows, `half_angle` radians
    /// wide (between 0 and a right angle).
    Cone { apex: Vec3, axis: Vec3, half_angle: f64 },
    /// A torus about the unit `axis` through `center`: a tube of radius `minor` round the circle of
    /// radius `major` (a chamfer's round, a fillet along a circular edge).
    Torus { center: Vec3, axis: Vec3, major: f64, minor: f64 },
}

impl Shape {
    /// The signed distance of `p` from the shape (outside a cylinder is positive).
    pub fn distance(&self, p: Vec3) -> f64 {
        match *self {
            Shape::Plane { point, normal } => (p - point).dot(normal),
            Shape::Cylinder { point, axis, radius } => {
                let v = p - point;
                (v - axis * v.dot(axis)).len() - radius
            }
            Shape::Sphere { center, radius } => p.dist(center) - radius,
            Shape::Cone { apex, axis, half_angle } => {
                let v = p - apex;
                let t = v.dot(axis);
                (v - axis * t).len() * half_angle.cos() - t * half_angle.sin()
            }
            Shape::Torus { center, axis, major, minor } => {
                let v = p - center;
                let t = v.dot(axis);
                ((v - axis * t).len() - major).hypot(t) - minor
            }
        }
    }

    /// The shape's direction at `p` (either way).
    pub fn normal_at(&self, p: Vec3) -> Option<Vec3> {
        match *self {
            Shape::Plane { normal, .. } => Some(normal),
            Shape::Cylinder { point, axis, .. } => {
                let v = p - point;
                (v - axis * v.dot(axis)).normalized()
            }
            Shape::Sphere { center, .. } => (p - center).normalized(),
            Shape::Cone { apex, axis, half_angle } => {
                let v = p - apex;
                let out = (v - axis * v.dot(axis)).normalized()?;
                (out * half_angle.cos() - axis * half_angle.sin()).normalized()
            }
            Shape::Torus { center, axis, major, .. } => {
                let v = p - center;
                let t = v.dot(axis);
                let out = (v - axis * t).normalized()?;
                (out * ((v - axis * t).len() - major) + axis * t).normalized()
            }
        }
    }

    /// Which kind of shape it is.
    pub fn kind(&self) -> Kind {
        match self {
            Shape::Plane { .. } => Kind::Plane,
            Shape::Cylinder { .. } => Kind::Cylinder,
            Shape::Sphere { .. } => Kind::Sphere,
            Shape::Cone { .. } => Kind::Cone,
            Shape::Torus { .. } => Kind::Torus,
        }
    }

    /// The shape moved by `shift`.
    pub fn moved(self, shift: Vec3) -> Shape {
        match self {
            Shape::Plane { point, normal } => Shape::Plane { point: point + shift, normal },
            Shape::Cylinder { point, axis, radius } => Shape::Cylinder { point: point + shift, axis, radius },
            Shape::Sphere { center, radius } => Shape::Sphere { center: center + shift, radius },
            Shape::Cone { apex, axis, half_angle } => Shape::Cone { apex: apex + shift, axis, half_angle },
            Shape::Torus { center, axis, major, minor } => Shape::Torus { center: center + shift, axis, major, minor },
        }
    }

    /// The round shape's axis, a point on it and its unit direction (a cylinder's, cone's or
    /// torus's), `None` for a plane or a sphere.
    pub fn axis(&self) -> Option<(Vec3, Vec3)> {
        match *self {
            Shape::Cylinder { point, axis, .. } => Some((point, axis)),
            Shape::Cone { apex, axis, .. } => Some((apex, axis)),
            Shape::Torus { center, axis, .. } => Some((center, axis)),
            Shape::Plane { .. } | Shape::Sphere { .. } => None,
        }
    }
}

/// Which kind of shape to fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plane,
    Cylinder,
    Sphere,
    Cone,
    Torus,
}

/// A shape fitted to a region of the scan, and how well it fits.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub shape: Shape,
    /// The region's triangles.
    pub triangles: Vec<usize>,
    /// The region's area, in mm².
    pub area: f64,
    /// The region's vertices' distance from the shape: root mean square and largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// The region's middle, on the shape: where to put what is made from it.
    pub middle: Vec3,
    /// How far the region reaches along a cylinder's or a cone's axis, from `middle` either way (0
    /// for the other shapes).
    pub half_length: f64,
}

/// Why nothing could be fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitError {
    /// The pick is not on the scan.
    NotOnTheScan,
    /// The region round the pick is not that shape (a cylinder on a flat face, too few points).
    NoFit,
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FitError::NotOnTheScan => "pick a point on the scan",
            FitError::NoFit => {
                "the scan there is not that shape: pick the middle of a flat face for a plane, of a round wall for a cylinder, of a slanted round wall for a cone, of a round along a circular edge for a torus"
            }
        })
    }
}

/// The most points a shape is fitted to; a larger region is sampled evenly.
const SOLVE_LIMIT: usize = 30_000;

/// A neighbour's direction may lean this far from the shape's there and still be on it.
const MAX_LEAN: f64 = 20.0 * PI / 180.0;

/// How far (mm) a point may be from the shape and still be on it, for a scan `extent` across.
pub fn tolerance(extent: f64) -> f64 {
    (0.0015 * extent).max(1e-6)
}

/// A shape must lie within this many times the scan's noise round the pick (by default).
const NOISE_MULTIPLE: f64 = 3.0;

/// Above this noise, against the typical edge round the pick, a triangle's direction is read over
/// the triangles round it ([`RINGS`] of them deep): its own turns too far with the noise for any
/// test of direction (a fifth of an edge turns it 15–20°).
const SMOOTH_ABOVE: f64 = 0.05;
const RINGS: usize = 3;

/// Fits a `kind` of shape to the region round triangle `seed`. `tolerance` is how far (mm) a
/// triangle may be from the shape and still be on it (default: [`tolerance`] of the scan, or three
/// times its noise round the pick where that is more).
pub fn fit(scan: &Scan, seed: usize, kind: Kind, tolerance: Option<f64>) -> Result<Fitted, FitError> {
    let [a, b, c] = scan.corners(seed).ok_or(FitError::NotOnTheScan)?;
    let pick = (a + b + c) / 3.0;
    let extent = scan.extent();
    let neighbours = edge_neighbours(scan);
    // The start: the connected triangles within a small reach of the pick, wider for a cylinder
    // so that the patch curves enough to show its axis.
    // On a coarse mesh the reach widens until the patch holds enough triangles to fit.
    let mut reach = match kind {
        Kind::Plane => 0.01 * extent,
        Kind::Cylinder | Kind::Sphere | Kind::Cone | Kind::Torus => 0.03 * extent,
    };
    let near = |reach: f64| connected(scan, &neighbours, seed, |t| scan.corners(t).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(pick) <= reach));
    let mut start = near(reach);
    while start.len() < 12 && reach < 0.1 * extent {
        reach *= 2.0;
        start = near(reach);
    }
    let (noise, edge) = noise_round(scan, &neighbours, &start).unwrap_or((0.0, 0.0));
    let tolerance = tolerance.filter(|t| t.is_finite() && *t > 0.0).unwrap_or_else(|| self::tolerance(extent).max(NOISE_MULTIPLE * noise));
    let facings = Facings::new(*scan, &neighbours, if edge > 0.0 && noise > SMOOTH_ABOVE * edge { RINGS } else { 0 });
    // The shape read with the directions growth reads: a noisy scan's own normals mislead an axis.
    let solve = |region: &[usize], kind: Kind| solve_facing(scan, region, kind, SOLVE_LIMIT, &facings);
    let mut shape = solve(&start, kind).ok_or(FitError::NoFit)?;
    let rules = |region: &[usize], shape: &Shape| Growth { tolerance, lean: MAX_LEAN, bend: Some(bend_slack(&facings, region, shape)) };
    // Grow against the shape, fitting again while the region grows by a tenth, then once more
    // against the final shape; the turn allowed is read again from each fit.
    let mut region = start;
    for _ in 0..24 {
        // Early only once the region is wide enough to tell: a small patch of a wide shaft looks flat too.
        if flat_cylinder(scan, &region, &shape, 0.1 * extent) {
            return Err(FitError::NoFit);
        }
        let grown = grow_by(&facings, &[seed], &shape, &rules(&region, &shape), None);
        let more = grown.len() * 10 >= region.len() * 11;
        region = grown;
        shape = solve(&region, kind).unwrap_or(shape);
        if !more {
            break;
        }
    }
    let region = grow_by(&facings, &[seed], &shape, &rules(&region, &shape), None);
    let shape = solve(&region, kind).unwrap_or(shape);
    if region.len() < 3 {
        return Err(FitError::NoFit);
    }
    if flat_cylinder(scan, &region, &shape, 0.0) {
        return Err(FitError::NoFit);
    }
    summarize(scan, region, shape)
}

/// The triangles reached from `seed` across edges whose middles are within `reach` of `at`: the
/// part of the scan round a point.
pub fn around(scan: &Scan, seed: usize, at: Vec3, reach: f64) -> Vec<usize> {
    let neighbours = edge_neighbours(scan);
    connected(scan, &neighbours, seed, |t| scan.corners(t).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(at) <= reach))
}

/// Below this, the normals of a piece weighted by area (their sum's length over the area) nearly
/// cancel: the piece wraps round (a closed part, or one scanned all round), and no one surface
/// laid over it can cover it without folding. A sheet facing one way is near 1, half a tube 0.64.
const WRAPS: f64 = 0.3;
/// How far (cosine) a triangle may turn from the way the scan faces at a click and still be part
/// of what the click means on a piece that wraps round: 60°, so the part is one over which a
/// surface lies without folding.
const FACING: f64 = 0.5;

/// The part of the scan a click at `at` on the triangle `seed` means when no radius is given: the
/// piece clicked, unless it wraps round ([`WRAPS`]), when one surface cannot cover it; then the
/// triangles reached from the click that face within 60° of the way the scan faces there.
pub fn clicked_part(scan: &Scan, seed: usize, at: Vec3) -> Vec<usize> {
    let neighbours = edge_neighbours(scan);
    let whole = connected(scan, &neighbours, seed, |t| scan.corners(t).is_some());
    let doubled = |t: usize| scan.corners(t).map(|[a, b, c]| (b - a).cross(c - a)).filter(|n| n.len().is_finite());
    let (sum, area) = whole.iter().filter_map(|&t| doubled(t)).fold((Vec3::ZERO, 0.0), |(s, a), n| (s + n, a + n.len()));
    if !(area > 0.0) || sum.len() / area > WRAPS {
        return whole;
    }
    // The way the scan faces at the click: over the triangles within a few of their sides of it,
    // so its noise does not turn it.
    let side = (area / whole.len() as f64).sqrt();
    let near = connected(scan, &neighbours, seed, |t| scan.corners(t).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(at) <= 5.0 * side));
    let Some(facing) = near.iter().filter_map(|&t| doubled(t)).fold(Vec3::ZERO, |s, n| s + n).normalized() else { return whole };
    connected(scan, &neighbours, seed, |t| doubled(t).and_then(|n| n.normalized()).is_some_and(|n| n.dot(facing) >= FACING))
}

/// The shape of `kind` fitted to all of the triangles `region`, `None` when they have too little
/// to fit or are not that shape.
pub fn shape_of(scan: &Scan, region: &[usize], kind: Kind) -> Option<Shape> {
    solve(scan, region, kind)
}

/// [`shape_of`] on an even sample of about `limit` of the region's points: faster on a large
/// region, as close where the region is the shape.
pub fn shape_of_sampled(scan: &Scan, region: &[usize], kind: Kind, limit: usize) -> Option<Shape> {
    solve_with(scan, region, kind, limit.max(16))
}

/// `shape` fitted again to the region (about `limit` of its points), settling from where it is
/// rather than starting afresh: for a region that has grown a little round the shape fitted to
/// it before. A plane is fitted afresh, which is as quick.
pub fn refined(scan: &Scan, region: &[usize], shape: &Shape, limit: usize) -> Option<Shape> {
    let s = samples(scan, region, limit.max(16))?;
    let local = shape.moved(-s.origin);
    let settled = match local {
        Shape::Plane { .. } => plane(&s)?,
        Shape::Cylinder { point, axis, radius } => {
            let (state, _) = settle(&s, CylinderState::new(point, axis, radius), cylinder_row, cylinder_step);
            Shape::Cylinder { point: state.point - state.axis * state.point.dot(state.axis), axis: state.axis, radius: state.radius }
        }
        Shape::Sphere { center, radius } => {
            let (state, _) = settle(&s, (center, radius), sphere_row, sphere_step);
            Shape::Sphere { center: state.0, radius: state.1 }
        }
        Shape::Cone { apex, axis, half_angle } => {
            let (c, _) = settle(&s, ConeState::new(apex, axis, half_angle), cone_row, cone_step);
            Shape::Cone { apex: c.apex, axis: c.axis, half_angle: c.half_angle }
        }
        Shape::Torus { center, axis, major, minor } => {
            let (t, _) = settle(&s, TorusState::new(center, axis, major, minor), torus_row, torus_step);
            Shape::Torus { center: t.center, axis: t.axis, major: t.major, minor: t.minor }
        }
    };
    Some(settled.moved(s.origin))
}

/// A cylinder's numbers as they settle for [`refined`].
#[derive(Clone, Copy)]
struct CylinderState {
    point: Vec3,
    axis: Vec3,
    radius: f64,
    e1: Vec3,
    e2: Vec3,
}

impl CylinderState {
    fn new(point: Vec3, axis: Vec3, radius: f64) -> Self {
        let (e1, e2) = in_plane(axis);
        CylinderState { point, axis, radius, e1, e2 }
    }
}

fn cylinder_row(c: &CylinderState, p: Vec3) -> (f64, [f64; 5]) {
    let d = p - c.point;
    let t = d.dot(c.axis);
    let side = d - c.axis * t;
    let Some(n) = side.normalized() else { return (-c.radius, [0.0, 0.0, 0.0, 0.0, -1.0]) };
    (side.len() - c.radius, [-t * n.dot(c.e1), -t * n.dot(c.e2), -n.dot(c.e1), -n.dot(c.e2), -1.0])
}

fn cylinder_step(c: &CylinderState, step: &[f64; 5]) -> Option<CylinderState> {
    let axis = (c.axis + c.e1 * step[0] + c.e2 * step[1]).normalized()?;
    Some(CylinderState::new(c.point + c.e1 * step[2] + c.e2 * step[3], axis, (c.radius + step[4]).max(1e-12)))
}

fn sphere_row(c: &(Vec3, f64), p: Vec3) -> (f64, [f64; 4]) {
    match (p - c.0).normalized() {
        Some(n) => (p.dist(c.0) - c.1, [-n.x, -n.y, -n.z, -1.0]),
        None => (-c.1, [0.0, 0.0, 0.0, -1.0]),
    }
}

fn sphere_step(c: &(Vec3, f64), step: &[f64; 4]) -> Option<(Vec3, f64)> {
    Some((c.0 + Vec3::new(step[0], step[1], step[2]), (c.1 + step[3]).max(1e-12)))
}

/// Whether `shape` is a "cylinder" far wider than the region it was fitted to, a region at least
/// `reach` across: a flat face, curved only by noise. Asked while the region grows too, so that a
/// flat face is refused before the region spreads over it. A cone or a torus as wide is as flat.
fn flat_cylinder(scan: &Scan, region: &[usize], shape: &Shape, reach: f64) -> bool {
    let radius = match *shape {
        Shape::Cylinder { radius, .. } | Shape::Sphere { radius, .. } => radius,
        Shape::Torus { minor, .. } => minor,
        // A cone's curve across it at the region: its radius there over the cosine.
        Shape::Cone { apex, axis, half_angle } => {
            let Some(at) = region.first().and_then(|&t| scan.corners(t)) else { return false };
            let v = at[0] - apex;
            (v - axis * v.dot(axis)).len() / half_angle.cos().max(1e-9)
        }
        Shape::Plane { .. } => return false,
    };
    let mut corners = region.iter().filter_map(|&t| scan.corners(t)).flatten();
    let Some(first) = corners.next() else { return false };
    let (lo, hi) = corners.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let across = (hi - lo).len();
    across >= reach && radius > 20.0 * across
}

/// `seed` and every triangle reached from it across edges that `keep` takes.
fn connected(scan: &Scan, neighbours: &EdgeNeighbours, seed: usize, keep: impl Fn(usize) -> bool) -> Vec<usize> {
    let mut seen = vec![false; scan.triangle_count()];
    let mut found = Vec::new();
    if let Some(s) = seen.get_mut(seed) {
        *s = true;
        found.push(seed);
    }
    let mut head = 0;
    while let Some(&t) = found.get(head) {
        head += 1;
        for n in neighbours.of(scan, t) {
            if seen.get(n).is_some_and(|s| !s) && keep(n) {
                seen[n] = true;
                found.push(n);
            }
        }
    }
    found
}

/// What a triangle must do to be on a shape as a region grows over it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Growth {
    /// How far (mm) each of its corners may be from the shape.
    pub tolerance: f64,
    /// How far (radians) its direction may lean from the shape's there.
    pub lean: f64,
    /// How far (radians) the turn from the triangle it is reached from to it may differ from the
    /// shape's turn between them: a neighbour that runs on in tangency but bends otherwise (the
    /// shaft past a round, the face beyond a fillet) is not on it. `None` asks nothing of the turn.
    pub bend: Option<f64>,
}

/// The directions triangles face as growth reads them: each its own normal on a clean scan; on a
/// noisy one, the area-weighted normal of the triangles within `rings` edges of it, each read
/// once and kept.
pub(crate) struct Facings<'a> {
    scan: Scan<'a>,
    neighbours: &'a EdgeNeighbours,
    rings: usize,
    known: std::cell::RefCell<std::collections::HashMap<usize, Option<Vec3>>>,
}

impl<'a> Facings<'a> {
    pub(crate) fn new(scan: Scan<'a>, neighbours: &'a EdgeNeighbours, rings: usize) -> Self {
        Facings { scan, neighbours, rings, known: Default::default() }
    }

    fn cross(&self, t: usize) -> Option<Vec3> {
        self.scan.corners(t).map(|[a, b, c]| (b - a).cross(c - a))
    }

    /// The way triangle `t` faces, either way round as it is wound.
    pub(crate) fn of(&self, t: usize) -> Option<Vec3> {
        if self.rings == 0 {
            return self.cross(t)?.normalized();
        }
        if let Some(known) = self.known.borrow().get(&t) {
            return *known;
        }
        let own = self.cross(t)?;
        let (mut seen, mut front, mut sum) = (vec![t], vec![t], own);
        for _ in 0..self.rings {
            let mut next = Vec::new();
            for &f in &front {
                for n in self.neighbours.of(&self.scan, f) {
                    if !seen.contains(&n) {
                        seen.push(n);
                        next.push(n);
                        if let Some(c) = self.cross(n) {
                            sum += if c.dot(own) < 0.0 { -c } else { c };
                        }
                    }
                }
            }
            front = next;
        }
        let facing = sum.normalized();
        self.known.borrow_mut().insert(t, facing);
        facing
    }
}

/// How noisy the scan is round the triangles `start`, and its typical edge there: a quadric fitted
/// to each of a few small patches spread over them, the lower middle of how far their points
/// stray from it (a patch over an edge strays far, and does not count); `None` with no patch to
/// read.
fn noise_round(scan: &Scan, neighbours: &EdgeNeighbours, start: &[usize]) -> Option<(f64, f64)> {
    const PATCH: usize = 60;
    let (mut spreads, mut edges) = (Vec::new(), Vec::new());
    for centre in crate::thinned(start, 9) {
        // The first triangles reached from the centre.
        let (mut patch, mut head) = (vec![centre], 0);
        while let Some(&t) = patch.get(head) {
            head += 1;
            for n in neighbours.of(scan, t) {
                if patch.len() < PATCH && !patch.contains(&n) {
                    patch.push(n);
                }
            }
        }
        let mut corners: Vec<u32> = patch.iter().filter_map(|&t| scan.triangles.get(t)).flatten().copied().collect();
        corners.sort_unstable();
        corners.dedup();
        let points: Vec<Vec3> = corners.iter().filter_map(|&v| scan.positions.get(v as usize).copied()).collect();
        if points.len() < 16 {
            continue;
        }
        // The quadric's six numbers take their share of the points' spread.
        let m = points.len() as f64;
        if let Some(spread) = crate::segment::quadric_spread_of(&points) {
            spreads.push(spread * (m / (m - 6.0)).sqrt());
        }
        edges.extend(patch.iter().filter_map(|&t| scan.corners(t)).flat_map(|[a, b, c]| [a.dist(b), b.dist(c), c.dist(a)]));
    }
    if spreads.is_empty() || edges.is_empty() {
        return None;
    }
    spreads.sort_by(f64::total_cmp);
    edges.sort_by(f64::total_cmp);
    Some((spreads[(spreads.len() * 3) / 10], edges[edges.len() / 2]))
}

/// The connected triangles from `seeds` (each of them taken, on the shape or not) that lie on
/// `shape` by `rules`, and only those `open` allows when it is given.
pub(crate) fn grow_by(facings: &Facings, seeds: &[usize], shape: &Shape, rules: &Growth, open: Option<&[bool]>) -> Vec<usize> {
    let (scan, neighbours) = (&facings.scan, facings.neighbours);
    let cos = rules.lean.cos();
    let tolerance = rules.tolerance;
    let on = |t: usize| -> Option<(Vec3, Option<Vec3>)> {
        let [a, b, c] = scan.corners(t)?;
        if ![a, b, c].iter().all(|p| shape.distance(*p).abs() <= tolerance) {
            return None;
        }
        let middle = (a + b + c) / 3.0;
        let own = facings.of(t);
        match (own, shape.normal_at(middle)) {
            (Some(n), Some(m)) if n.dot(m).abs() < cos => None,
            // A sliver with no direction is judged by where it is alone.
            _ => Some((middle, own)),
        }
    };
    let mut seen = vec![false; scan.triangle_count()];
    let mut found = Vec::new();
    for &seed in seeds {
        if let Some(s) = seen.get_mut(seed).filter(|s| !**s) {
            *s = true;
            found.push(seed);
        }
    }
    let mut head = 0;
    while let Some(&t) = found.get(head) {
        head += 1;
        let here = scan.corners(t).map(|[a, b, c]| ((a + b + c) / 3.0, facings.of(t)));
        for n in neighbours.of(scan, t) {
            if seen.get(n).is_none_or(|s| *s) || open.is_some_and(|o| !o.get(n).copied().unwrap_or(false)) {
                continue;
            }
            let Some((there, own)) = on(n) else { continue };
            if let (Some(slack), Some((from, Some(a))), Some(b)) = (rules.bend, here, own)
                && let Some(off) = bend_off(shape, from, a, there, b)
                && off > slack
            {
                continue;
            }
            seen[n] = true;
            found.push(n);
        }
    }
    found
}

/// How far the turn between two triangles (at `from` facing `a`, at `to` facing `b`, either way)
/// differs from the shape's turn between those places, in radians.
fn bend_off(shape: &Shape, from: Vec3, a: Vec3, to: Vec3, b: Vec3) -> Option<f64> {
    let turn = |u: Vec3, v: Vec3| u.dot(v).abs().min(1.0).acos();
    Some((turn(a, b) - turn(shape.normal_at(from)?, shape.normal_at(to)?)).abs())
}

/// The slack the turn between neighbours is allowed on `shape`, from how far it is off in
/// `region` (fitted to it): half as much again as the most it is off between nearly every pair
/// (the 995th in a thousand: a faceted mesh turns at its facets' edges, a few pairs in many), and
/// at least 2°.
pub(crate) fn bend_slack(facings: &Facings, region: &[usize], shape: &Shape) -> f64 {
    let (scan, neighbours) = (&facings.scan, facings.neighbours);
    let mut inside = vec![false; scan.triangle_count()];
    for &t in region {
        if let Some(slot) = inside.get_mut(t) {
            *slot = true;
        }
    }
    let place = |t: usize| scan.corners(t).and_then(|[a, b, c]| Some(((a + b + c) / 3.0, facings.of(t)?)));
    let mut offs = Vec::new();
    for &t in region {
        let Some((from, a)) = place(t) else { continue };
        for n in neighbours.of(scan, t).filter(|&n| n > t && inside.get(n).copied().unwrap_or(false)) {
            if let Some(off) = place(n).and_then(|(to, b)| bend_off(shape, from, a, to, b)) {
                offs.push(off);
            }
        }
    }
    (1.5 * high_share(&mut offs, 0.995)).max(2f64.to_radians())
}

/// The value `share` of the way up `values` (0 for none), reordering them.
pub(crate) fn high_share(values: &mut [f64], share: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let k = ((values.len() - 1) as f64 * share.clamp(0.0, 1.0)).round() as usize;
    *values.select_nth_unstable_by(k, f64::total_cmp).1
}

/// The region's vertices (each corner of each triangle, weighted by a third of its area) about
/// their middle, and the area-weighted sum of the triangles' `n nᵀ`.
struct Samples {
    origin: Vec3,
    points: Vec<(Vec3, f64)>,
    /// Each point's triangle's unit normal, in the order of `points`.
    directions: Vec<Vec3>,
    normals: [[f64; 3]; 3],
    /// The region's size, the diagonal of its box.
    size: f64,
}

fn samples(scan: &Scan, region: &[usize], limit: usize) -> Option<Samples> {
    samples_facing(scan, region, limit, None)
}

/// [`samples`], each triangle's direction the one `facings` reads when it is given.
fn samples_facing(scan: &Scan, region: &[usize], limit: usize, facings: Option<&Facings>) -> Option<Samples> {
    let mut points = Vec::with_capacity(region.len() * 3);
    let mut normals = [[0.0f64; 3]; 3];
    let (mut total, mut sum) = (0.0, Vec3::ZERO);
    for &t in region {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let cross = (b - a).cross(c - a);
        let area = cross.len() / 2.0;
        let Some(own) = cross.normalized() else { continue };
        let n = facings.and_then(|f| f.of(t)).unwrap_or(own);
        total += area;
        sum += (a + b + c) * (area / 3.0);
        for p in [a, b, c] {
            points.push((p, area / 3.0, n));
        }
        let n = [n.x, n.y, n.z];
        for (row, ni) in normals.iter_mut().zip(n) {
            for (cell, nj) in row.iter_mut().zip(n) {
                *cell += area * ni * nj;
            }
        }
    }
    if !(total > 0.0) || points.len() < 6 {
        return None;
    }
    let origin = sum / total;
    // An even sample of a large region fits as well and far faster.
    let points = crate::thinned(&points, limit);
    let weight: f64 = points.iter().map(|(_, w, _)| w).sum();
    let (lo, hi) = points.iter().fold((points[0].0, points[0].0), |(lo, hi), (p, _, _)| (lo.min(*p), hi.max(*p)));
    Some(Samples {
        origin,
        directions: points.iter().map(|(_, _, n)| *n).collect(),
        points: points.into_iter().map(|(p, w, _)| (p - origin, w / weight)).collect(),
        normals,
        size: (hi - lo).len(),
    })
}

/// The shape of `kind` fitted to the region, `None` when it has too little to fit or does not fit.
fn solve(scan: &Scan, region: &[usize], kind: Kind) -> Option<Shape> {
    solve_with(scan, region, kind, SOLVE_LIMIT)
}

/// [`solve`] on at most about `limit` of the region's points.
fn solve_with(scan: &Scan, region: &[usize], kind: Kind, limit: usize) -> Option<Shape> {
    let s = samples(scan, region, limit)?;
    solved(&s, kind)
}

/// [`solve_with`], each triangle facing as `facings` reads it.
fn solve_facing(scan: &Scan, region: &[usize], kind: Kind, limit: usize, facings: &Facings) -> Option<Shape> {
    let s = samples_facing(scan, region, limit, Some(facings))?;
    solved(&s, kind)
}

/// The shape of `kind` fitted to the samples.
fn solved(s: &Samples, kind: Kind) -> Option<Shape> {
    let shape = match kind {
        Kind::Plane => plane(s)?,
        Kind::Cylinder => cylinder(s)?,
        Kind::Sphere => sphere(s)?,
        Kind::Cone => cone(s)?,
        Kind::Torus => torus(s)?,
    };
    // Back from about the region's middle.
    Some(shape.moved(s.origin))
}

/// The plane of least squares: through the middle, square to the way the points spread least.
fn plane(s: &Samples) -> Option<Shape> {
    let mut spread = [[0.0f64; 3]; 3];
    for &(p, w) in &s.points {
        let p = [p.x, p.y, p.z];
        for (row, pi) in spread.iter_mut().zip(p) {
            for (cell, pj) in row.iter_mut().zip(p) {
                *cell += w * pi * pj;
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    // Points along a line spread in one direction only: no plane.
    if !(values[1] > 1e-12 * values[2].max(1e-300)) {
        return None;
    }
    Some(Shape::Plane { point: Vec3::ZERO, normal: axes[0].normalized()? })
}

/// The cylinder of least squares: the axis started square to the region's directions (a cylinder's
/// normals all lie square to its axis), the circle fitted looking along it (Kåsa's), then all five
/// numbers settled together by damped Gauss–Newton steps.
fn cylinder(s: &Samples) -> Option<Shape> {
    let (values, axes) = solidcraft_geom::inertia::principal(&s.normals, 0.0)?;
    // Normals that all point one way have no axis to find (a flat patch with no noise).
    if !(values[1] > 1e-6 * values[2]) {
        return None;
    }
    let mut axis = axes[0].normalized()?;
    let (u, v) = in_plane(axis);
    let (centre, radius) = kasa(s.points.iter().map(|&(p, w)| ([p.dot(u), p.dot(v)], w)))?;
    let mut point = u * centre[0] + v * centre[1];
    let mut radius = radius;
    let residual = |point: Vec3, axis: Vec3, radius: f64| -> f64 {
        s.points
            .iter()
            .map(|&(p, w)| {
                let d = p - point;
                let r = (d - axis * d.dot(axis)).len() - radius;
                w * r * r
            })
            .sum()
    };
    let mut cost = residual(point, axis, radius);
    let mut lambda = 1e-3;
    for _ in 0..60 {
        let (e1, e2) = in_plane(axis);
        let (mut h, mut g) = ([[0.0f64; 5]; 5], [0.0f64; 5]);
        for &(p, w) in &s.points {
            let d = p - point;
            let t = d.dot(axis);
            let side = d - axis * t;
            let rho = side.len();
            let Some(n) = side.normalized() else { continue };
            let j = [-t * n.dot(e1), -t * n.dot(e2), -n.dot(e1), -n.dot(e2), -1.0];
            let r = rho - radius;
            for a in 0..5 {
                g[a] += w * r * j[a];
                for b in 0..5 {
                    h[a][b] += w * j[a] * j[b];
                }
            }
        }
        let mut accepted = false;
        for _ in 0..10 {
            let mut damped = h;
            for (a, row) in damped.iter_mut().enumerate() {
                row[a] += lambda * row[a].max(1e-12);
            }
            let Some(step) = solve_linear(damped, g.map(|x| -x)) else { break };
            let next_axis = (axis + e1 * step[0] + e2 * step[1]).normalized()?;
            let next_point = point + e1 * step[2] + e2 * step[3];
            let next_radius = (radius + step[4]).max(1e-12);
            let next_cost = residual(next_point, next_axis, next_radius);
            if next_cost.is_finite() && next_cost <= cost {
                let gain = cost - next_cost;
                (axis, point, radius, cost) = (next_axis, next_point, next_radius, next_cost);
                lambda = (lambda * 0.3).max(1e-12);
                accepted = gain > 1e-15 * cost.max(1e-300);
                break;
            }
            lambda *= 10.0;
        }
        if !accepted {
            break;
        }
    }
    // The point on the axis nearest the region's middle.
    let point = point - axis * point.dot(axis);
    (radius.is_finite() && radius > 0.0).then_some(Shape::Cylinder { point, axis, radius })
}

/// The sphere of least squares: the algebraic sphere through the points (`|p|² + d·p + e = 0`,
/// linear), then its centre and radius settled together by damped Gauss–Newton steps on the
/// points' distances.
fn sphere(s: &Samples) -> Option<Shape> {
    let (mut a, mut b) = ([[0.0f64; 4]; 4], [0.0f64; 4]);
    for &(p, w) in &s.points {
        let row = [p.x, p.y, p.z, 1.0];
        let rhs = -p.len2();
        for i in 0..4 {
            b[i] += w * row[i] * rhs;
            for j in 0..4 {
                a[i][j] += w * row[i] * row[j];
            }
        }
    }
    let d = solve_linear(a, b)?;
    let mut center = Vec3::new(-d[0] / 2.0, -d[1] / 2.0, -d[2] / 2.0);
    let squared = center.len2() - d[3];
    if !(squared.is_finite() && squared > 0.0) {
        return None;
    }
    let mut radius = squared.sqrt();
    let residual = |c: Vec3, r: f64| s.points.iter().map(|&(p, w)| w * (p.dist(c) - r).powi(2)).sum::<f64>();
    let mut cost = residual(center, radius);
    let mut lambda = 1e-3;
    for _ in 0..60 {
        let (mut h, mut g) = ([[0.0f64; 4]; 4], [0.0f64; 4]);
        for &(p, w) in &s.points {
            let Some(n) = (p - center).normalized() else { continue };
            let j = [-n.x, -n.y, -n.z, -1.0];
            let r = p.dist(center) - radius;
            for a in 0..4 {
                g[a] += w * r * j[a];
                for b in 0..4 {
                    h[a][b] += w * j[a] * j[b];
                }
            }
        }
        let mut accepted = false;
        for _ in 0..10 {
            let mut damped = h;
            for (a, row) in damped.iter_mut().enumerate() {
                row[a] += lambda * row[a].max(1e-12);
            }
            let Some(step) = solve_linear(damped, g.map(|x| -x)) else { break };
            let (next_center, next_radius) = (center + Vec3::new(step[0], step[1], step[2]), (radius + step[3]).max(1e-12));
            let next_cost = residual(next_center, next_radius);
            if next_cost.is_finite() && next_cost <= cost {
                let gain = cost - next_cost;
                (center, radius, cost) = (next_center, next_radius, next_cost);
                lambda = (lambda * 0.3).max(1e-12);
                accepted = gain > 1e-15 * cost.max(1e-300);
                break;
            }
            lambda *= 10.0;
        }
        if !accepted {
            break;
        }
    }
    (radius.is_finite() && radius > 0.0).then_some(Shape::Sphere { center, radius })
}

/// Damped Gauss–Newton (Levenberg–Marquardt) on the samples' distances from a shape of `N`
/// numbers: `row` gives a point's distance from the shape and its derivatives by the numbers,
/// `step` the shape changed by a step of them (`None` where that leaves no shape). Where it
/// settles, and the weighted mean square distance there.
fn settle<S: Copy, const N: usize>(
    s: &Samples,
    start: S,
    row: impl Fn(&S, Vec3) -> (f64, [f64; N]),
    step: impl Fn(&S, &[f64; N]) -> Option<S>,
) -> (S, f64) {
    let cost = |state: &S| -> f64 {
        s.points
            .iter()
            .map(|&(p, w)| {
                let r = row(state, p).0;
                w * r * r
            })
            .sum()
    };
    let (mut state, mut now) = (start, cost(&start));
    let mut lambda = 1e-3;
    for _ in 0..60 {
        let (mut h, mut g) = ([[0.0f64; N]; N], [0.0f64; N]);
        for &(p, w) in &s.points {
            let (r, j) = row(&state, p);
            if !(r.is_finite() && j.iter().all(|x| x.is_finite())) {
                continue;
            }
            for a in 0..N {
                g[a] += w * r * j[a];
                for b in 0..N {
                    h[a][b] += w * j[a] * j[b];
                }
            }
        }
        let mut accepted = false;
        for _ in 0..10 {
            let mut damped = h;
            for (a, row) in damped.iter_mut().enumerate() {
                row[a] += lambda * row[a].max(1e-12);
            }
            let Some(change) = solve_linear(damped, g.map(|x| -x)) else { break };
            let Some(next) = step(&state, &change) else {
                lambda *= 10.0;
                continue;
            };
            let next_cost = cost(&next);
            if next_cost.is_finite() && next_cost <= now {
                let gain = now - next_cost;
                (state, now) = (next, next_cost);
                lambda = (lambda * 0.3).max(1e-12);
                accepted = gain > 1e-15 * now.max(1e-300);
                break;
            }
            lambda *= 10.0;
        }
        if !accepted {
            break;
        }
    }
    (state, now)
}

/// A cone's numbers as they settle: its apex, its axis with two directions square to it (along
/// which the axis tips) and its half-angle.
#[derive(Clone, Copy)]
struct ConeState {
    apex: Vec3,
    axis: Vec3,
    half_angle: f64,
    e1: Vec3,
    e2: Vec3,
}

impl ConeState {
    fn new(apex: Vec3, axis: Vec3, half_angle: f64) -> Self {
        let (e1, e2) = in_plane(axis);
        ConeState { apex, axis, half_angle, e1, e2 }
    }
}

/// The direction square to the circle the normals' tips make on the unit sphere: a cone's normals
/// all lean the same way from its axis, so their tips lie in a plane square to it.
fn circle_of_normals(s: &Samples) -> Option<Vec3> {
    let mean = s.directions.iter().fold(Vec3::ZERO, |sum, n| sum + *n) / s.directions.len().max(1) as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for n in &s.directions {
        let d = *n - mean;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    axes[0].normalized()
}

/// The cone of least squares: from each likely axis (square to the normals' spread, as a narrow
/// cone's; along it, as a wide one's; square to the circle of the normals' tips) the radius read
/// against the height along it is a line, whose slope is the half-angle and whose zero the apex;
/// all six numbers then settle together, and the closest is kept.
fn cone(s: &Samples) -> Option<Shape> {
    let (_, axes) = solidcraft_geom::inertia::principal(&s.normals, 0.0)?;
    let mut best: Option<(ConeState, f64)> = None;
    for axis in [Some(axes[0]), Some(axes[2]), circle_of_normals(s)].into_iter().flatten() {
        let Some(start) = axis.normalized().and_then(|axis| cone_start(s, axis)) else { continue };
        let (state, cost) = settle(s, start, cone_row, cone_step);
        if cost.is_finite() && best.is_none_or(|b| cost < b.1) {
            best = Some((state, cost));
        }
    }
    let (c, _) = best?;
    (c.half_angle > 0.0 && c.half_angle < FRAC_PI_2 && c.apex.is_finite()).then_some(Shape::Cone {
        apex: c.apex,
        axis: c.axis,
        half_angle: c.half_angle,
    })
}

/// A cone about `axis` through the circle the points make looking along it, from the line of
/// their radius against their height; `None` when the radius does not grow (a cylinder) or the
/// apex lies far off.
fn cone_start(s: &Samples, axis: Vec3) -> Option<ConeState> {
    let (u, v) = in_plane(axis);
    let (centre, _) = kasa(s.points.iter().map(|&(p, w)| ([p.dot(u), p.dot(v)], w)))?;
    let point = u * centre[0] + v * centre[1];
    let (mut sw, mut st, mut sr, mut stt, mut str) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(p, w) in &s.points {
        let d = p - point;
        let t = d.dot(axis);
        let rho = (d - axis * t).len();
        sw += w;
        st += w * t;
        sr += w * rho;
        stt += w * t * t;
        str += w * t * rho;
    }
    let denominator = sw * stt - st * st;
    if !(denominator > 0.0) {
        return None;
    }
    let slope = (sw * str - st * sr) / denominator;
    let offset = (sr - slope * st) / sw;
    // The axis turned the way the radius grows.
    let (axis, slope) = if slope < 0.0 { (-axis, -slope) } else { (axis, slope) };
    if !(slope > (0.2f64).to_radians().tan()) {
        return None;
    }
    let apex_t = -offset / slope;
    (apex_t.abs() < 1e4 * s.size.max(1e-9)).then(|| ConeState::new(point + axis * apex_t, axis, slope.atan().min(FRAC_PI_2 - 1e-3)))
}

fn cone_row(c: &ConeState, p: Vec3) -> (f64, [f64; 6]) {
    let v = p - c.apex;
    let t = v.dot(c.axis);
    let w = v - c.axis * t;
    let rho = w.len();
    let (sin, cos) = c.half_angle.sin_cos();
    let mut j = [0.0; 6];
    let to_apex = match w.normalized() {
        Some(n) => {
            j[0] = -cos * t * n.dot(c.e1) - sin * v.dot(c.e1);
            j[1] = -cos * t * n.dot(c.e2) - sin * v.dot(c.e2);
            -(n * cos - c.axis * sin)
        }
        None => {
            j[0] = -sin * v.dot(c.e1);
            j[1] = -sin * v.dot(c.e2);
            c.axis * sin
        }
    };
    (j[2], j[3], j[4]) = (to_apex.x, to_apex.y, to_apex.z);
    j[5] = -rho * sin - t * cos;
    (rho * cos - t * sin, j)
}

fn cone_step(c: &ConeState, step: &[f64; 6]) -> Option<ConeState> {
    let axis = (c.axis + c.e1 * step[0] + c.e2 * step[1]).normalized()?;
    let angle = (c.half_angle + step[5]).clamp(1e-3, FRAC_PI_2 - 1e-3);
    Some(ConeState::new(c.apex + Vec3::new(step[2], step[3], step[4]), axis, angle))
}

/// A torus's numbers as they settle.
#[derive(Clone, Copy)]
struct TorusState {
    center: Vec3,
    axis: Vec3,
    major: f64,
    minor: f64,
    e1: Vec3,
    e2: Vec3,
}

impl TorusState {
    fn new(center: Vec3, axis: Vec3, major: f64, minor: f64) -> Self {
        let (e1, e2) = in_plane(axis);
        TorusState { center, axis, major, minor, e1, e2 }
    }
}

/// The torus of least squares. For a tube radius tried (and each way the normals may point), the
/// points stepped back along their normals by it lie on the tube's middle circle: the radius whose
/// stepped points lie closest to a circle in space starts the solve (a sweep of 41 radii, then a
/// golden-section search between the best one's neighbours), and all seven numbers then settle.
fn torus(s: &Samples) -> Option<Shape> {
    let every = (s.points.len() / 2_000).max(1);
    let used: Vec<usize> = (0..s.points.len()).step_by(every).collect();
    if !(used.len() >= 8 && s.size > 0.0) {
        return None;
    }
    let weights: Vec<f64> = used.iter().map(|&i| s.points[i].1).collect();
    let circle = |minor: f64, sign: f64| -> Option<(TorusState, f64)> {
        let points: Vec<Vec3> = used.iter().map(|&i| s.points[i].0 - s.directions[i] * (sign * minor)).collect();
        let (center, axis, major, mean_square) = circle_in_space(&points, &weights)?;
        Some((TorusState::new(center, axis, major, minor), mean_square))
    };
    let mut best: Option<(TorusState, f64)> = None;
    for sign in [1.0, -1.0] {
        let (low, high) = ((s.size / 100.0).ln(), (3.0 * s.size).ln());
        let steps = 40;
        let width = (high - low) / f64::from(steps);
        let swept = (0..=steps)
            .filter_map(|k| {
                let r = low + width * f64::from(k);
                circle(r.exp(), sign).filter(|c| c.1.is_finite()).map(|c| (r, c.1))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((at, _)) = swept else { continue };
        let (mut a, mut b) = (at - width, at + width);
        let golden = (5.0f64.sqrt() - 1.0) / 2.0;
        let cost = |x: f64| circle(x.exp(), sign).map_or(f64::INFINITY, |c| c.1);
        let (mut c, mut d) = (b - golden * (b - a), a + golden * (b - a));
        let (mut fc, mut fd) = (cost(c), cost(d));
        for _ in 0..24 {
            if fc < fd {
                (b, d, fd) = (d, c, fc);
                c = b - golden * (b - a);
                fc = cost(c);
            } else {
                (a, c, fc) = (c, d, fd);
                d = a + golden * (b - a);
                fd = cost(d);
            }
        }
        let Some((start, _)) = circle(((a + b) / 2.0).exp(), sign) else { continue };
        if !(start.major < 1e4 * s.size && start.center.len() < 1e4 * s.size) {
            continue;
        }
        let (state, cost) = settle(s, start, torus_row, torus_step);
        if cost.is_finite() && best.is_none_or(|b| cost < b.1) {
            best = Some((state, cost));
        }
    }
    let (t, _) = best?;
    (t.minor > 0.0 && t.major.is_finite() && t.center.is_finite()).then_some(Shape::Torus {
        center: t.center,
        axis: t.axis,
        major: t.major,
        minor: t.minor,
    })
}

/// The plane weighted points spread in and the circle in it (Kåsa's): its centre, its unit axis,
/// its radius, and the points' weighted mean square distance from it. `None` for points in one
/// place or along a line.
fn circle_in_space(points: &[Vec3], weights: &[f64]) -> Option<(Vec3, Vec3, f64, f64)> {
    let total: f64 = weights.iter().sum();
    if !(total > 0.0) {
        return None;
    }
    let middle = points.iter().zip(weights).fold(Vec3::ZERO, |sum, (p, w)| sum + *p * *w) / total;
    let mut spread = [[0.0f64; 3]; 3];
    for (p, w) in points.iter().zip(weights) {
        let d = *p - middle;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += w * di * dj;
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    if !(values[2] > 0.0 && values[1] > 1e-9 * values[2]) {
        return None;
    }
    let axis = axes[0].normalized()?;
    let (u, v) = in_plane(axis);
    let (c, radius) = kasa(points.iter().zip(weights).map(|(p, w)| ([(*p - middle).dot(u), (*p - middle).dot(v)], *w)))?;
    let center = middle + u * c[0] + v * c[1];
    let square: f64 = points
        .iter()
        .zip(weights)
        .map(|(p, w)| {
            let d = *p - center;
            let t = d.dot(axis);
            let off = (d - axis * t).len() - radius;
            w * (off * off + t * t)
        })
        .sum();
    Some((center, axis, radius, square / total))
}

fn torus_row(c: &TorusState, p: Vec3) -> (f64, [f64; 7]) {
    let v = p - c.center;
    let t = v.dot(c.axis);
    let w = v - c.axis * t;
    let rho = w.len();
    let n = w.normalized().unwrap_or(c.e1);
    let q = rho - c.major;
    let tube = q.hypot(t);
    let mut j = [0.0; 7];
    if tube > 1e-300 {
        let m = (n * q + c.axis * t) / tube;
        j[0] = (-q * t * n.dot(c.e1) + t * v.dot(c.e1)) / tube;
        j[1] = (-q * t * n.dot(c.e2) + t * v.dot(c.e2)) / tube;
        (j[2], j[3], j[4]) = (-m.x, -m.y, -m.z);
        j[5] = -q / tube;
    }
    j[6] = -1.0;
    (tube - c.minor, j)
}

fn torus_step(c: &TorusState, step: &[f64; 7]) -> Option<TorusState> {
    let axis = (c.axis + c.e1 * step[0] + c.e2 * step[1]).normalized()?;
    Some(TorusState::new(c.center + Vec3::new(step[2], step[3], step[4]), axis, (c.major + step[5]).max(0.0), (c.minor + step[6]).max(1e-12)))
}

/// The circle that best fits weighted points on a plane (Kåsa's): its centre and radius.
pub(crate) fn kasa(points: impl Iterator<Item = ([f64; 2], f64)>) -> Option<([f64; 2], f64)> {
    let (mut a, mut b) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
    for ([x, y], w) in points {
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            b[i] += w * row[i] * rhs;
            for j in 0..3 {
                a[i][j] += w * row[i] * row[j];
            }
        }
    }
    let d = crate::symmetry::solve3(&a, b)?;
    let centre = [-d[0] / 2.0, -d[1] / 2.0];
    let squared = centre[0] * centre[0] + centre[1] * centre[1] - d[2];
    (squared.is_finite() && squared > 0.0).then(|| (centre, squared.sqrt()))
}

/// The `N` × `N` system `a x = b` by Gaussian elimination with partial pivoting; `None` if singular.
fn solve_linear<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if !(a[pivot][col].abs() > 1e-300) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (pivot_row, pivot_b) = (a[col], b[col]);
        for (row, rhs) in a.iter_mut().zip(b.iter_mut()).skip(col + 1) {
            let f = row[col] / pivot_row[col];
            for (cell, p) in row.iter_mut().zip(pivot_row).skip(col) {
                *cell -= f * p;
            }
            *rhs -= f * pivot_b;
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let sum: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// How well `shape` fits the region, and where its middle is.
fn summarize(scan: &Scan, triangles: Vec<usize>, shape: Shape) -> Result<Fitted, FitError> {
    let (mut area, mut sum, mut squares, mut count, mut max) = (0.0, Vec3::ZERO, 0.0, 0usize, 0.0f64);
    let mut along: (f64, f64) = (f64::INFINITY, f64::NEG_INFINITY);
    for &t in &triangles {
        let Some(corners) = scan.corners(t) else { continue };
        let [a, b, c] = corners;
        let w = (b - a).cross(c - a).len() / 2.0;
        area += w;
        sum += (a + b + c) * (w / 3.0);
        for p in corners {
            let d = shape.distance(p);
            squares += d * d;
            count += 1;
            max = max.max(d.abs());
            if let Shape::Cylinder { point: from, axis, .. } | Shape::Cone { apex: from, axis, .. } = shape {
                let s = (p - from).dot(axis);
                along = (along.0.min(s), along.1.max(s));
            }
        }
    }
    if !(area > 0.0 && count > 0) {
        return Err(FitError::NoFit);
    }
    let centre = sum / area;
    let cone_along = along;
    let (middle, half_length) = match shape {
        Shape::Plane { point, normal } => (centre - normal * (centre - point).dot(normal), 0.0),
        Shape::Cylinder { point, axis, .. } => (point + axis * ((along.0 + along.1) / 2.0), (along.1 - along.0) / 2.0),
        Shape::Sphere { center, .. } | Shape::Torus { center, .. } => (center, 0.0),
        Shape::Cone { apex, axis, .. } => {
            let (lo, hi) = cone_along;
            (apex + axis * ((lo + hi) / 2.0), (hi - lo) / 2.0)
        }
    };
    Ok(Fitted { shape, triangles, area, rms: (squares / count as f64).sqrt(), max, middle, half_length })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::nearest_triangle;
    use crate::test_meshes::{fine_cuboid, turned};

    /// A plate 60 × 40 × 8 with a round boss Ø 20 × 15 standing on it, both cut finely, turned and moved
    /// anywhere, with a little deterministic noise.
    fn plate_with_boss(noise: f64) -> (Vec<Vec3>, Vec<[u32; 3]>, Rigid) {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 40.0, 8.0), 20);
        let (sides, rows) = (96u32, 12u32);
        let base = positions.len() as u32;
        for k in 0..sides {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
            for j in 0..=rows {
                positions.push(Vec3::new(30.0 + 10.0 * a.cos(), 20.0 + 10.0 * a.sin(), 8.0 + 15.0 * f64::from(j) / f64::from(rows)));
            }
        }
        for k in 0..sides {
            let (a, b) = (base + k * (rows + 1), base + ((k + 1) % sides) * (rows + 1));
            for j in 0..rows {
                triangles.push([a + j, b + j, b + j + 1]);
                triangles.push([a + j, b + j + 1, a + j + 1]);
            }
        }
        for (i, p) in positions.iter_mut().enumerate() {
            let s = ((i as f64) * 12.9898).sin() * 43_758.545;
            *p += Vec3::new(0.0, 0.0, 1.0) * (noise * (s - s.floor() - 0.5));
        }
        let axis = Vec3::new(0.4, -0.2, 0.9).normalized().unwrap();
        let moved = turned(&positions, axis, 0.7, Vec3::new(-20.0, 55.0, 12.0));
        (moved, triangles, Rigid::turn(axis, 0.7))
    }

    use crate::place::Rigid;

    /// A click with no radius on a closed box means the side clicked (one surface cannot cover a
    /// box); on an open sheet, a bent one too, it means the whole sheet.
    #[test]
    fn a_click_on_a_part_that_wraps_round_means_the_side_facing_it() {
        let (p, t) = fine_cuboid(Vec3::ZERO, Vec3::new(40.0, 20.0, 10.0), 12);
        let scan = Scan::new(&p, &t);
        let at = Vec3::new(20.0, 10.0, 10.0);
        let seed = nearest_triangle(&scan, at).unwrap();
        let top = clicked_part(&scan, seed, at);
        assert!(!top.is_empty() && top.len() < t.len() / 2, "{} of {}", top.len(), t.len());
        for &k in &top {
            let [a, b, c] = scan.corners(k).unwrap();
            assert!((a.z - 10.0).abs() < 1e-9 && (b.z - 10.0).abs() < 1e-9 && (c.z - 10.0).abs() < 1e-9, "only the top");
        }
        // The box with its bottom left open is a tray turned over: it still wraps round.
        let open: Vec<[u32; 3]> =
            t.iter().copied().filter(|&[a, b, c]| !(p[a as usize].z == 0.0 && p[b as usize].z == 0.0 && p[c as usize].z == 0.0)).collect();
        let tray = Scan::new(&p, &open);
        let seed = nearest_triangle(&tray, at).unwrap();
        assert!(clicked_part(&tray, seed, at).len() < open.len() / 2);
        // A sheet bent through 120°: the whole of it.
        let n = 24u32;
        let at_ij = |i: u32, j: u32| {
            let a = f64::from(i) / f64::from(n) * 2.1 - 1.05;
            Vec3::new(20.0 * a.sin(), f64::from(j), 20.0 * (1.0 - a.cos()))
        };
        let mut sheet = Vec::new();
        let mut tris = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                sheet.push(at_ij(i, j));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                tris.push([k, k + 1, k + n + 2]);
                tris.push([k, k + n + 2, k + n + 1]);
            }
        }
        let bent = Scan::new(&sheet, &tris);
        let at = at_ij(2, 12);
        let seed = nearest_triangle(&bent, at).unwrap();
        assert_eq!(clicked_part(&bent, seed, at).len(), tris.len(), "a sheet is taken whole");
    }

    /// A ball R 12, half of it scanned, turned and moved anywhere: its centre and radius are found;
    /// a pick on a flat face is no sphere.
    #[test]
    fn a_sphere_fits_the_ball_picked() {
        let (rings, sides) = (24u32, 48u32);
        let mut positions = Vec::new();
        for i in 0..=rings {
            let polar = std::f64::consts::FRAC_PI_2 * f64::from(i) / f64::from(rings);
            for k in 0..sides {
                let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
                positions.push(Vec3::new(12.0 * polar.sin() * a.cos(), 12.0 * polar.sin() * a.sin(), 12.0 * polar.cos()));
            }
        }
        let mut triangles = Vec::new();
        for i in 0..rings {
            for k in 0..sides {
                let (a, b) = (i * sides + k, i * sides + (k + 1) % sides);
                triangles.push([a, b, b + sides]);
                triangles.push([a, b + sides, a + sides]);
            }
        }
        let axis = Vec3::new(0.5, 0.1, -0.3).normalized().unwrap();
        let moved = turned(&positions, axis, 0.9, Vec3::new(40.0, -10.0, 7.0));
        let scan = Scan::new(&moved, &triangles);
        let seed = nearest_triangle(&scan, moved[(rings / 2 * sides) as usize]).unwrap();
        let fitted = fit(&scan, seed, Kind::Sphere, None).unwrap();
        let Shape::Sphere { center, radius } = fitted.shape else { panic!() };
        assert!((radius - 12.0).abs() < 0.05, "{radius}");
        assert!(center.dist(Vec3::new(40.0, -10.0, 7.0)) < 0.05, "{center:?}");

        let (positions, triangles, turn) = plate_with_boss(0.02);
        let scan = Scan::new(&positions, &triangles);
        let place = |p: Vec3| turn.turned(p) + Vec3::new(-20.0, 55.0, 12.0);
        let seed = nearest_triangle(&scan, place(Vec3::new(8.0, 8.0, 8.0))).unwrap();
        assert_eq!(fit(&scan, seed, Kind::Sphere, None), Err(FitError::NoFit));
    }

    /// A shaft R 150 × 40 long, picked on its wall: its first patch looks nearly flat, but it is
    /// found, not refused as a flat face.
    #[test]
    fn a_wide_shaft_is_not_taken_for_a_flat_face() {
        let (sides, rows) = (720u32, 8u32);
        let mut positions = Vec::new();
        for k in 0..sides {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
            for j in 0..=rows {
                positions.push(Vec3::new(150.0 * a.cos(), 150.0 * a.sin(), 40.0 * f64::from(j) / f64::from(rows)));
            }
        }
        let mut triangles = Vec::new();
        for k in 0..sides {
            let (a, b) = (k * (rows + 1), ((k + 1) % sides) * (rows + 1));
            for j in 0..rows {
                triangles.push([a + j, b + j, b + j + 1]);
                triangles.push([a + j, b + j + 1, a + j + 1]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(150.0, 0.0, 20.0)).unwrap();
        let fitted = fit(&scan, seed, Kind::Cylinder, None).unwrap();
        let Shape::Cylinder { radius, axis, .. } = fitted.shape else { panic!() };
        assert!((radius - 150.0).abs() < 0.01 && axis.z.abs() > 1.0 - 1e-9, "{radius} {axis:?}");
    }

    /// A turned part, turned and moved anywhere: a shaft R 20 from 0 to 20, a 45° chamfer of 5 to
    /// a face at 25, the other end rounded R 4 into a face at 0.
    fn turned_part() -> (Vec<Vec3>, Vec<[u32; 3]>, Rigid, Vec3) {
        use crate::test_meshes::{revolved, steps};
        let mut profile = vec![[-4.0, 0.0]];
        profile.extend(steps([-4.0, 1.0], [-4.0, 15.0], 14));
        // The round about (0, 16), R 4, from the face at −4 to the shaft.
        profile.extend((0..=16).map(|i| f64::from(i) / 16.0 * std::f64::consts::FRAC_PI_2).map(|a| [-4.0 * a.cos(), 16.0 + 4.0 * a.sin()]));
        profile.extend(steps([0.0, 20.0], [20.0, 20.0], 40).into_iter().skip(1));
        profile.extend(steps([20.0, 20.0], [25.0, 15.0], 12).into_iter().skip(1));
        profile.extend(steps([25.0, 15.0], [25.0, 1.0], 14).into_iter().skip(1));
        profile.push([25.0, 0.0]);
        let (positions, triangles) = revolved(&profile, 120, false);
        let axis = Vec3::new(0.3, 0.5, 0.8).normalized().unwrap();
        let shift = Vec3::new(14.0, -6.0, 30.0);
        (turned(&positions, axis, 1.1, shift), triangles, Rigid::turn(axis, 1.1), shift)
    }

    #[test]
    fn a_cone_fits_the_chamfer_picked() {
        let (positions, triangles, turn, shift) = turned_part();
        let scan = Scan::new(&positions, &triangles);
        let place = |p: Vec3| turn.turned(p) + shift;
        let seed = nearest_triangle(&scan, place(Vec3::new(17.5, 0.0, 22.5))).unwrap();
        let fitted = fit(&scan, seed, Kind::Cone, None).unwrap();
        let Shape::Cone { apex, axis, half_angle } = fitted.shape else { panic!() };
        // The chamfer's radius shrinks along +Z: its apex at z 40, on the axis.
        assert!((half_angle.to_degrees() - 45.0).abs() < 0.2, "{}", half_angle.to_degrees());
        assert!(apex.dist(place(Vec3::new(0.0, 0.0, 40.0))) < 0.2, "{apex:?}");
        assert!(axis.dot(turn.turned(-Vec3::Z)) > 1.0 - 1e-5, "{axis:?}");
        assert!(fitted.rms < 0.05, "{}", fitted.rms);
    }

    #[test]
    fn a_torus_fits_the_round_picked() {
        let (positions, triangles, turn, shift) = turned_part();
        let scan = Scan::new(&positions, &triangles);
        let place = |p: Vec3| turn.turned(p) + shift;
        let a = std::f64::consts::FRAC_PI_4;
        let seed = nearest_triangle(&scan, place(Vec3::new(16.0 + 4.0 * a.sin(), 0.0, -4.0 * a.cos()))).unwrap();
        // The round runs into the shaft and the face in tangency, and a pick's region takes a
        // strip or two of each (Find All takes the shaft and the face first).
        let fitted = fit(&scan, seed, Kind::Torus, None).unwrap();
        let Shape::Torus { center, axis, major, minor } = fitted.shape else { panic!() };
        assert!((minor - 4.0).abs() < 0.2, "{minor}");
        assert!((major - 16.0).abs() < 0.2, "{major}");
        assert!(center.dist(place(Vec3::ZERO)) < 0.2, "{center:?}");
        // On the round's own triangles alone it is exact.
        let true_round = Shape::Torus { center: place(Vec3::ZERO), axis: turn.turned(Vec3::Z), major: 16.0, minor: 4.0 };
        let own: Vec<usize> =
            fitted.triangles.iter().copied().filter(|&t| scan.corners(t).unwrap().iter().all(|p| true_round.distance(*p).abs() < 1e-6)).collect();
        let Some(Shape::Torus { major, minor, .. }) = shape_of(&scan, &own, Kind::Torus) else { panic!() };
        assert!((minor - 4.0).abs() < 1e-6 && (major - 16.0).abs() < 1e-6, "{major} {minor}");
        assert!(axis.dot(turn.turned(Vec3::Z)).abs() > 1.0 - 1e-5, "{axis:?}");
    }

    #[test]
    fn a_plane_fits_the_face_picked_and_stops_at_its_edges() {
        let (positions, triangles, turn) = plate_with_boss(0.02);
        let scan = Scan::new(&positions, &triangles);
        let place = |p: Vec3| turn.turned(p) + Vec3::new(-20.0, 55.0, 12.0);
        // A pick on the plate's top, away from the boss.
        let seed = nearest_triangle(&scan, place(Vec3::new(8.0, 8.0, 8.0))).unwrap();
        let fitted = fit(&scan, seed, Kind::Plane, None).unwrap();
        let Shape::Plane { normal, .. } = fitted.shape else { panic!() };
        assert!(normal.dot(turn.turned(Vec3::Z)).abs() > (0.05f64).to_radians().cos(), "{normal:?}");
        assert!(fitted.shape.distance(place(Vec3::new(50.0, 35.0, 8.0))).abs() < 0.01, "on the top");
        // The whole top, 60 × 40: the boss stands on it as a mesh of its own, so the top runs under it.
        let expected = 2400.0;
        assert!((fitted.area - expected).abs() < 0.03 * expected, "{} against {expected}", fitted.area);
        assert!(fitted.rms < 0.02, "{}", fitted.rms);
        // Its noise curves it a little everywhere, but it is no cylinder.
        assert_eq!(fit(&scan, seed, Kind::Cylinder, None), Err(FitError::NoFit));
    }

    #[test]
    fn a_cylinder_fits_the_boss_picked() {
        let (positions, triangles, turn) = plate_with_boss(0.0);
        let scan = Scan::new(&positions, &triangles);
        let place = |p: Vec3| turn.turned(p) + Vec3::new(-20.0, 55.0, 12.0);
        let seed = nearest_triangle(&scan, place(Vec3::new(40.0, 20.0, 15.0))).unwrap();
        let fitted = fit(&scan, seed, Kind::Cylinder, None).unwrap();
        let Shape::Cylinder { point, axis, radius } = fitted.shape else { panic!() };
        // A 96-sided boss: its circle of best fit lies between the corners' and the sides' radii.
        assert!((radius - 10.0).abs() < 0.01, "{radius}");
        assert!(axis.dot(turn.turned(Vec3::Z)).abs() > 1.0 - 1e-9, "{axis:?}");
        let on_axis = place(Vec3::new(30.0, 20.0, 0.0));
        assert!(((on_axis - point) - axis * (on_axis - point).dot(axis)).len() < 1e-6, "through the boss's middle");
        assert!((fitted.half_length - 7.5).abs() < 0.01, "{}", fitted.half_length);
        assert_eq!(
            fit(&scan, nearest_triangle(&scan, place(Vec3::new(8.0, 8.0, 8.0))).unwrap(), Kind::Cylinder, None),
            Err(FitError::NoFit),
            "a flat face has no axis"
        );
    }

    /// A draw about as spread as a normal one of deviation 1: three uniform draws summed.
    fn draw(i: usize) -> f64 {
        let uniform = |k: usize| {
            let s = ((k as f64) * 12.9898 + 78.233).sin() * 43_758.545;
            s - s.floor() - 0.5
        };
        2.0 * (uniform(3 * i) + uniform(3 * i + 1) + uniform(3 * i + 2))
    }

    /// `positions` each moved by a noise of deviation `sigma` every way.
    fn noisy(positions: &mut [Vec3], sigma: f64) {
        for (i, p) in positions.iter_mut().enumerate() {
            *p += Vec3::new(draw(3 * i), draw(3 * i + 1), draw(3 * i + 2)) * sigma;
        }
    }

    /// A tube R 20, 30 long, scanned finely (edges about 0.7) with noise of a fifth of an edge, as a
    /// hand scanner's mesh comes before it is thinned: each triangle faces up to 20° or more off the
    /// tube. A cylinder picked on it takes nearly the whole tube, at its true radius, and a plane
    /// picked on a noisy plate the whole plate.
    #[test]
    fn a_noisy_fine_scan_is_fitted_whole() {
        let profile: Vec<[f64; 2]> = (0..=43).map(|j| [0.7 * f64::from(j), 20.0]).collect();
        let (mut positions, triangles) = crate::test_meshes::revolved(&profile, 180, false);
        noisy(&mut positions, 0.14);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(20.0, 0.0, 15.0)).unwrap();
        let fitted = fit(&scan, seed, Kind::Cylinder, None).unwrap();
        let Shape::Cylinder { axis, radius, .. } = fitted.shape else { panic!("{:?}", fitted.shape) };
        assert!((radius - 20.0).abs() < 0.05 && axis.z.abs() > 0.5f64.to_radians().cos(), "{radius} {axis:?}");
        assert!(fitted.triangles.len() > 9 * triangles.len() / 10, "{} of {}", fitted.triangles.len(), triangles.len());

        let n = 86u32;
        let mut positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(0.7 * f64::from(i), 0.7 * f64::from(j), 0.0))).collect();
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        noisy(&mut positions, 0.14);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(30.0, 30.0, 0.0)).unwrap();
        let fitted = fit(&scan, seed, Kind::Plane, None).unwrap();
        let Shape::Plane { normal, .. } = fitted.shape else { panic!("{:?}", fitted.shape) };
        assert!(normal.z.abs() > 0.2f64.to_radians().cos(), "{normal:?}");
        assert!(fitted.triangles.len() > 9 * triangles.len() / 10, "{} of {}", fitted.triangles.len(), triangles.len());
    }
}
