//! The ways a scan could stand on the ground: its broad flat faces, the poses it would rest in on
//! a table, and the directions of its own axes, ranked the steadiest first ([`bases`]); and a
//! flat face grown from a triangle the person clicked ([`grow`]).

use std::f64::consts::PI;

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::hull::{hull2, hull3};
use crate::{Scan, in_plane, thinned};

/// What kind of base it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseKind {
    /// A broad flat face on the outside of the part.
    Flat,
    /// A pose the part would rest in on a table, on several places at once (the hull's faces).
    Resting,
    /// Lying along one of its own axes: on its broad side, its edge or its end.
    Axis,
    /// Standing on an end of the axis it is turned about (a mug, a shaft, a wheel).
    End,
    /// A flat face grown from a clicked triangle.
    Grown,
}

impl BaseKind {
    pub fn name(self) -> &'static str {
        match self {
            BaseKind::Flat => "flat",
            BaseKind::Resting => "resting",
            BaseKind::Axis => "axis",
            BaseKind::End => "end",
            BaseKind::Grown => "grown",
        }
    }
}

/// A way the scan could stand on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Base {
    pub kind: BaseKind,
    /// Unit, outward: the way the part faces where it would touch the ground.
    pub normal: Vec3,
    /// A point of the ground plane, under the middle of where the part touches it.
    pub anchor: Vec3,
    /// A face's area, or the area of the outline the part touches the ground by, in mm².
    pub area: f64,
    /// A face's share of the scan's surface (0 for a pose).
    pub share: f64,
    /// How far, in radians, the part must be tilted standing this way to tip over; 0 when it
    /// would not stand or this is not known.
    pub tip: f64,
    /// A grown face's distance from its fitted plane, root mean square, in mm (0 otherwise).
    pub rms: f64,
}

impl Base {
    /// How good a base it is, for ranking: how far it is from tipping, a flat face the more the
    /// more of the scan it is.
    pub fn score(&self) -> f64 {
        match self.kind {
            BaseKind::Flat => self.tip * (1.0 + 4.0 * self.share),
            _ => self.tip,
        }
    }
}

/// A scan stands on a base already when the base faces down within this (radians).
const STANDING: f64 = 2.0 * PI / 180.0;
/// A base at least this share as good as the best is among the best.
const NEAR_BEST: f64 = 0.8;

/// The base a press of Stand on Base with none named stands the scan on, of `found` (as [`bases`]
/// gives them): the one it stands on already (facing `down` within 2°) when that is among the
/// best (an end of the axis it is turned about, or nearly as steady as the steadiest), so a part
/// that stands well is not turned over or tipped for a tie or a hair's difference; else the first.
pub fn preferred(found: &[Base], down: Vec3) -> usize {
    let best = found.iter().map(Base::score).fold(0.0, f64::max);
    found.iter().position(|b| b.normal.dot(down) >= STANDING.cos() && (b.kind == BaseKind::End || b.score() >= NEAR_BEST * best)).unwrap_or(0)
}

/// The most bases [`bases`] gives.
pub const MAX_BASES: usize = 8;

/// Bases whose directions are within 15° are the same way of standing.
fn same_way() -> f64 {
    (15.0 * PI / 180.0).cos()
}

/// A face's normals may turn this far (radians) across it, beyond its noise, and it is still flat.
const MAX_TURN: f64 = 4.0 * PI / 180.0;

/// The triangles within 10° of a face's direction are the ones whose turning is measured.
fn nearly_facing() -> f64 {
    (10.0 * PI / 180.0).cos()
}

/// How far (mm) a point may be from a plane and still touch it, for a scan `extent` across.
fn touch_tolerance(extent: f64) -> f64 {
    (0.0015 * extent).max(1e-6)
}

/// The ways the scan could stand, at most [`MAX_BASES`], no two the same way: for a turned part
/// the ends of its axis first ([`turned_about`]: a mug stands on its bottom or its rim before any
/// pose its lumps or its handle would rest it in); then flat faces and resting poses ranked
/// together, the steadiest first; then the directions of its own axes.
pub fn bases(scan: &Scan) -> Vec<Base> {
    let extent = scan.extent();
    if !(extent > 0.0 && extent.is_finite()) {
        return Vec::new();
    }
    let all: Vec<Vec3> = scan.points().collect();
    let points = thinned(&all, 50_000);
    let samples = thinned(&points, 2_500);
    let (flat, centroid) = flat_faces(scan, &points, extent);
    let tolerance = touch_tolerance(extent);
    let faces = flat.into_iter().map(|mut face| {
        face.tip = rest(face.normal, &points, centroid, tolerance).map_or(0.0, |r| r.tip);
        let score = face.tip * (1.0 + 4.0 * face.share);
        (face, score)
    });
    let resting = resting_poses(&points, &samples, centroid, extent).into_iter().take(5).map(|p| (p, p.tip));
    let mut ranked: Vec<(Base, f64)> = faces.chain(resting).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let ends = turned_about(scan).map(|(on, axis)| end_bases(on, axis, &points, centroid, tolerance)).unwrap_or_default();
    let mut kept: Vec<Base> = Vec::new();
    let same = same_way();
    for base in ends.into_iter().chain(ranked.into_iter().map(|(b, _)| b)).chain(axis_bases(&samples, centroid, extent)) {
        if kept.len() < MAX_BASES && !kept.iter().any(|k| k.normal.dot(base.normal) > same) {
            kept.push(base);
        }
    }
    kept
}

/// The share of the area of a scan's sides whose normals must pass the axis for it to be a turned
/// part, and how near (the sine of the angle between a normal and the plane through the axis and
/// its point). A triangle facing along the axis within [`TURNED_END`] (a cosine) is an end, not a
/// side; the sides must be at least [`TURNED_SIDES`] of the scan.
const TURNED_SHARE: f64 = 0.6;
const TURNED_NEAR: f64 = 0.17;
const TURNED_END: f64 = 0.9;
const TURNED_SIDES: f64 = 0.2;
/// How many times the axis is fitted, each time on the triangles that turned about the last.
const TURNED_ROUNDS: usize = 3;
/// The fit is one axis when the next best line is this many times worse.
const TURNED_SHARP: f64 = 4.0;
/// The triangles the turning axis is read from, at most.
const TURNED_SAMPLE: usize = 60_000;

/// The axis the scan is turned about (a point on it and its unit direction), when most of its
/// surface is a surface of revolution about one: every normal of such a surface passes through
/// the axis, so the axis is the line that the normals' lines come nearest to all together
/// (Pottmann and Randrup's line-geometry fit: the line `(l, m)` least in `Σ w (n · m + (p × n) · l)²`
/// with `|l| = 1`, an eigenvector of a 3 × 3 matrix). What does not turn about it (a mug's handle,
/// the lumps of a rough scan) pulls the first fit a few degrees off, so it is fitted again on the
/// triangles that do, [`TURNED_ROUNDS`] times. `None` for a part that is not turned: its normals
/// do not pass one line (a block), or pass too many (a ball, a flat plate).
pub fn turned_about(scan: &Scan) -> Option<(Vec3, Vec3)> {
    let step = (scan.triangle_count() / TURNED_SAMPLE).max(1);
    let middle = scan.bounds().map(|(lo, hi)| (lo + hi) / 2.0)?;
    let extent = scan.extent();
    if !(extent > 0.0 && extent.is_finite()) {
        return None;
    }
    // About the middle and at the scan's own size, so the numbers are of one scale.
    let samples: Vec<(Vec3, Vec3, f64)> = (0..scan.triangle_count())
        .step_by(step)
        .filter_map(|t| {
            let [a, b, c] = scan.corners(t)?;
            let cross = (b - a).cross(c - a);
            let normal = cross.normalized()?;
            let at = ((a + b + c) / 3.0 - middle) / extent;
            (at.is_finite() && normal.is_finite()).then_some((at, normal, cross.len() / 2.0))
        })
        .collect();
    let total: f64 = samples.iter().map(|s| s.2).sum();
    if samples.len() < 16 || !(total > 0.0) {
        return None;
    }
    // A side triangle turns about the axis when its normal lies within a few degrees of the plane
    // through the axis and its own point; an end (facing along the axis) says nothing either way.
    let side = |n: Vec3, direction: Vec3| n.dot(direction).abs() < TURNED_END;
    let about = |p: Vec3, n: Vec3, point: Vec3, direction: Vec3| {
        let out = p - point;
        let radial = out - direction * out.dot(direction);
        radial.normalized().is_some_and(|r| n.dot(direction.cross(r)).abs() <= TURNED_NEAR)
    };
    let mut used = vec![true; samples.len()];
    let mut found = None;
    for _ in 0..TURNED_ROUNDS {
        let (point, direction, sharp) = axis_of_normals(&samples, &used)?;
        found = Some((point, direction, sharp));
        for (slot, (p, n, _)) in used.iter_mut().zip(&samples) {
            *slot = !side(*n, direction) || about(*p, *n, point, direction);
        }
    }
    let (point, direction, sharp) = found?;
    // Most of its sides must turn about it. Its ends alone do not make it a turned part (a slab's
    // two broad faces pass any line square to them), and a part with hardly any side is none to
    // stand on an end.
    let sides: Vec<&(Vec3, Vec3, f64)> = samples.iter().filter(|(_, n, _)| side(*n, direction)).collect();
    let side_area: f64 = sides.iter().map(|s| s.2).sum();
    let turning: f64 = sides.iter().filter(|(p, n, _)| about(*p, *n, point, direction)).map(|s| s.2).sum();
    (sharp && side_area >= TURNED_SIDES * total && turning >= TURNED_SHARE * side_area).then(|| (middle + point * extent, direction))
}

/// The line the normals of the `used` samples (place, unit normal, area) come nearest to all
/// together: a point on it, its unit direction, and whether it is one line and no other (a ball's
/// normals pass every line through its middle). `None` when the normals do not spread in all
/// three directions (any line square to them all would do).
fn axis_of_normals(samples: &[(Vec3, Vec3, f64)], used: &[bool]) -> Option<(Vec3, Vec3, bool)> {
    // Σ w v vᵀ for v = (p × n, n), in blocks: a (moments), b (mixed), c (normals).
    let (mut a, mut b, mut c) = ([[0.0f64; 3]; 3], [[0.0f64; 3]; 3], [[0.0f64; 3]; 3]);
    let mut total = 0.0;
    for ((p, n, w), _) in samples.iter().zip(used).filter(|(_, used)| **used) {
        let m = p.cross(*n);
        let (m, n) = ([m.x, m.y, m.z], [n.x, n.y, n.z]);
        total += w;
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += w * m[i] * m[j];
                b[i][j] += w * m[i] * n[j];
                c[i][j] += w * n[i] * n[j];
            }
        }
    }
    let inverse = inverse3(&c)?;
    let spread = solidcraft_geom::inertia::principal(&c, 0.0)?.0;
    if !(total > 0.0 && spread[0] > 0.02 * total) {
        return None;
    }
    // a − b c⁻¹ bᵀ: its eigenvector of the least eigenvalue is the axis's direction.
    let mut reduced = a;
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                for l in 0..3 {
                    reduced[i][j] -= b[i][k] * inverse[k][l] * b[j][l];
                }
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&reduced, 0.0)?;
    let direction = axes[0].normalized()?;
    let sharp = values[1] > TURNED_SHARP * values[0].max(0.0) + 1e-12 * total;
    // The moment m = −c⁻¹ bᵀ l, and the point of the line nearest the origin, l × m.
    let l = [direction.x, direction.y, direction.z];
    let mut moment = [0.0f64; 3];
    for (k, slot) in moment.iter_mut().enumerate() {
        for i in 0..3 {
            for j in 0..3 {
                *slot -= inverse[k][j] * b[i][j] * l[i];
            }
        }
    }
    Some((direction.cross(Vec3::new(moment[0], moment[1], moment[2])), direction, sharp))
}

/// The inverse of a symmetric 3 × 3 matrix; `None` when it has none to speak of.
fn inverse3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det = m[0][0] * cofactor(0, 0) + m[0][1] * cofactor(0, 1) + m[0][2] * cofactor(0, 2);
    let scale = m.iter().flatten().fold(0.0f64, |s, x| s.max(x.abs()));
    if !(det.abs() > 1e-12 * scale * scale * scale) {
        return None;
    }
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = cofactor(c, r) / det;
        }
    }
    Some(out)
}

/// A turned part standing on each end of its axis (through `on` along `axis`), the axis over the
/// middle of the base: how steady it is there when it would stay (a mug on its bottom or its
/// rim), else simply that end down (a top's point: it is still the way the part is looked at).
/// The end with more of the part touching first.
fn end_bases(on: Vec3, axis: Vec3, points: &[Vec3], centroid: Vec3, tolerance: f64) -> Vec<Base> {
    let mut ends: Vec<Base> = [axis, -axis]
        .into_iter()
        .filter_map(|normal| {
            let lowest = points.iter().map(|p| p.dot(normal)).fold(f64::NEG_INFINITY, f64::max);
            let anchor = on + normal * (lowest - on.dot(normal));
            let (area, tip) = rest(normal, points, centroid, tolerance).map_or((0.0, 0.0), |r| (r.area, r.tip));
            lowest.is_finite().then_some(Base { kind: BaseKind::End, normal, anchor, area, share: 0.0, tip, rms: 0.0 })
        })
        .collect();
    ends.sort_by(|a, b| b.area.total_cmp(&a.area));
    ends
}

#[derive(Clone, Copy)]
struct Patch {
    normal: Vec3,
    area: f64,
}

/// Broad flat faces on the outside of the part (the triangles gathered by the way they face; a
/// patch whose normals turn steadily across it is curved and left out), and the area-weighted
/// middle of the surface.
fn flat_faces(scan: &Scan, points: &[Vec3], extent: f64) -> (Vec<Base>, Vec3) {
    let middle = scan.bounds().map_or(Vec3::ZERO, |(lo, hi)| (lo + hi) / 2.0);
    let step = (scan.triangle_count() / 100_000).max(1);
    let triangles = || (0..scan.triangle_count()).step_by(step).filter_map(|t| scan.corners(t));
    let mut patches: Vec<Patch> = Vec::new();
    let (mut total, mut weighted) = (0.0, Vec3::ZERO);
    for [a, b, c] in triangles() {
        let cross = (b - a).cross(c - a);
        let length = cross.len();
        if !(length > 1e-12 && length.is_finite()) {
            continue;
        }
        let area = length / 2.0;
        let centre = (a + b + c) / 3.0;
        let mut normal = cross / length;
        if normal.dot(centre - middle) < 0.0 {
            normal = -normal;
        }
        total += area;
        weighted += centre * area;
        match patches.iter_mut().find(|p| p.normal.dot(normal) > 0.95) {
            Some(p) => *p = Patch { normal: (p.normal * p.area + normal * area).normalized().unwrap_or(p.normal), area: p.area + area },
            None => patches.push(Patch { normal, area }),
        }
    }
    if !(total > 0.0) {
        return (Vec::new(), middle);
    }
    let centroid = weighted / total;
    let band = (extent * 0.025).max(1e-5);
    let facing_limit = nearly_facing();
    patches.sort_by(|a, b| b.area.total_cmp(&a.area));
    let mut found: Vec<Base> = patches
        .iter()
        .take(24)
        .filter(|patch| patch.area / total >= 0.03)
        .filter_map(|patch| {
            let n = patch.normal;
            let farthest = points.iter().map(|p| p.dot(n)).fold(f64::NEG_INFINITY, f64::max);
            // A face must lie on the outside of the shape, not merely point that way.
            let (mut support, mut support_weighted) = (0.0, Vec3::ZERO);
            let (mut centres, mut corners, mut facing) = (Vec::new(), Vec::new(), Vec::new());
            for [a, b, c] in triangles() {
                let cross = (b - a).cross(c - a);
                let area = cross.len() / 2.0;
                let centre = (a + b + c) / 3.0;
                if !(area > 1e-12 && (cross / (2.0 * area)).dot(n).abs() > 0.95 && farthest - centre.dot(n) <= band) {
                    continue;
                }
                support += area;
                support_weighted += centre * area;
                centres.push(centre);
                corners.extend([a, b, c]);
                let normal = cross / (2.0 * area);
                let along = if normal.dot(n) < 0.0 { -normal } else { normal };
                if along.dot(n) > facing_limit {
                    facing.push((centre, along, area));
                }
            }
            if !(support / total >= 0.03 && turn(&facing, n) <= MAX_TURN) {
                return None;
            }
            // The anchor: the supporting triangle nearest the face's middle, on the outermost plane;
            // or the middle of the face's outline when the face fills most of it.
            let mid = support_weighted / support;
            let nearest = centres.iter().copied().min_by(|a, b| a.dist(mid).total_cmp(&b.dist(mid))).unwrap_or(mid);
            let mut anchor = nearest + n * (farthest - nearest.dot(n));
            if let Some((outline_middle, outline_area)) = outline(&thinned(&corners, 6_000), n)
                && support >= 0.8 * outline_area
            {
                anchor = outline_middle + n * (farthest - outline_middle.dot(n));
            }
            // A large scan is read every `step` triangles: the area is the sample's, scaled back up.
            Some(Base { kind: BaseKind::Flat, normal: n, anchor, area: support * step as f64, share: support / total, tip: 0.0, rms: 0.0 })
        })
        .collect();
    found.sort_by(|a, b| b.area.total_cmp(&a.area));
    let same = same_way();
    let mut kept: Vec<Base> = Vec::new();
    for face in found {
        if kept.len() < 6 && !kept.iter().any(|k| k.normal.dot(face.normal) > same) {
            kept.push(face);
        }
    }
    (kept, centroid)
}

/// The middle of the convex outline of `points` seen along `normal` (the mean of its corners), and
/// its area; `None` for points that do not spread over a plane.
fn outline(points: &[Vec3], normal: Vec3) -> Option<(Vec3, f64)> {
    let (u, v) = in_plane(normal);
    let flat: Vec<Vec2> = points.iter().map(|p| Vec2::new(p.dot(u), p.dot(v))).collect();
    let hull = hull2(&flat);
    if hull.len() < 3 {
        return None;
    }
    let corners: Vec<Vec2> = hull.iter().filter_map(|&i| flat.get(i).copied()).collect();
    let area = polygon_area(&corners);
    let middle = hull.iter().filter_map(|&i| points.get(i).copied()).fold(Vec3::ZERO, |s, p| s + p) / hull.len() as f64;
    Some((middle, area.abs()))
}

fn polygon_area(corners: &[Vec2]) -> f64 {
    let n = corners.len();
    (0..n).map(|i| corners[i].cross(corners[(i + 1) % n])).sum::<f64>() / 2.0
}

/// How far the normals of `samples` (centre, normal, area) turn across them, in radians: their
/// sideways part fitted as a linear function of the place, area-weighted, and twice the rms of the
/// fitted part. A flat face's normals scatter with its noise but do not turn.
fn turn(samples: &[(Vec3, Vec3, f64)], normal: Vec3) -> f64 {
    let Some(n) = normal.normalized() else { return 0.0 };
    let (u, v) = in_plane(n);
    let flat = |p: Vec3| [p.dot(u), p.dot(v)];
    let weight: f64 = samples.iter().map(|s| s.2).sum();
    if !(samples.len() >= 3 && weight > 0.0) {
        return 0.0;
    }
    let mean = |f: &dyn Fn(&(Vec3, Vec3, f64)) -> Vec3| {
        let m = samples.iter().fold([0.0; 2], |acc, s| {
            let q = flat(f(s));
            [acc[0] + q[0] * s.2, acc[1] + q[1] * s.2]
        });
        [m[0] / weight, m[1] / weight]
    };
    let (place, side) = (mean(&|s| s.0), mean(&|s| s.1));
    // spread = Σ w x xᵀ and turning = Σ w m xᵀ, row by row.
    let (mut spread, mut turning) = ([[0.0f64; 2]; 2], [[0.0f64; 2]; 2]);
    for s in samples {
        let (x, m) = (flat(s.0), flat(s.1));
        let x = [x[0] - place[0], x[1] - place[1]];
        let m = [m[0] - side[0], m[1] - side[1]];
        for r in 0..2 {
            for c in 0..2 {
                spread[r][c] += s.2 * x[r] * x[c];
                turning[r][c] += s.2 * m[r] * x[c];
            }
        }
    }
    let size = spread[0][0] + spread[1][1];
    if !(size > 0.0) {
        return 0.0;
    }
    // A face that is a line of triangles has no spread across it: a little is added to invert it.
    spread[0][0] += 1e-9 * size;
    spread[1][1] += 1e-9 * size;
    let det = spread[0][0] * spread[1][1] - spread[0][1] * spread[1][0];
    if !(det.abs() > 0.0) {
        return 0.0;
    }
    let inverse = [[spread[1][1] / det, -spread[0][1] / det], [-spread[1][0] / det, spread[0][0] / det]];
    let mul = |a: [[f64; 2]; 2], b: [[f64; 2]; 2]| {
        let mut out = [[0.0; 2]; 2];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = a[r][0] * b[0][c] + a[r][1] * b[1][c];
            }
        }
        out
    };
    let rate = mul(turning, inverse);
    let rate_t = [[rate[0][0], rate[1][0]], [rate[0][1], rate[1][1]]];
    let fitted = mul(mul(rate, spread), rate_t);
    2.0 * ((fitted[0][0] + fitted[1][1]).max(0.0) / weight).sqrt()
}

/// Each face of the hull of a sample of the points is a plane the part can stand on: those it would
/// stay on, the steadiest first, no two the same way, measured again against all the points.
fn resting_poses(points: &[Vec3], samples: &[Vec3], centroid: Vec3, extent: f64) -> Vec<Base> {
    let input: Vec<Vec3> = samples.iter().copied().chain(thinned(points, 10_000)).collect();
    let Some(faces) = hull3(&input, (1e-9 * extent).max(1e-12)) else { return Vec::new() };
    let tolerance = touch_tolerance(extent);
    let mut tried: Vec<Vec3> = Vec::new();
    let mut found: Vec<Base> = Vec::new();
    for face in &faces {
        // The many small faces of one curved or flat patch lie in nearly one plane: one try each.
        if tried.iter().any(|t| t.dot(face.normal) > 0.99999) {
            continue;
        }
        tried.push(face.normal);
        found.extend(rest(face.normal, &input, centroid, tolerance));
    }
    found.sort_by(|a, b| b.tip.total_cmp(&a.tip));
    let same = same_way();
    let mut kept: Vec<Base> = Vec::new();
    for pose in found {
        if kept.len() < MAX_BASES && !kept.iter().any(|k| k.normal.dot(pose.normal) > same) {
            kept.push(pose);
        }
    }
    kept.into_iter().map(|pose| rest(pose.normal, points, centroid, tolerance).unwrap_or(pose)).collect()
}

/// The pose standing with its underside facing `normal`, if the part would stay there: its centre
/// of mass (taken as `centroid`) straight above the outline it touches the ground by, at least 2°
/// from tipping.
fn rest(normal: Vec3, points: &[Vec3], centroid: Vec3, tolerance: f64) -> Option<Base> {
    let normal = normal.normalized()?;
    let lowest = points.iter().map(|p| p.dot(normal)).fold(f64::NEG_INFINITY, f64::max);
    let contact: Vec<Vec3> = points.iter().copied().filter(|p| p.dot(normal) >= lowest - tolerance).collect();
    if contact.len() < 3 {
        return None;
    }
    let mean = contact.iter().fold(Vec3::ZERO, |s, p| s + *p) / contact.len() as f64;
    let (u, v) = in_plane(normal);
    let flat: Vec<Vec2> = contact.iter().map(|p| Vec2::new((*p - mean).dot(u), (*p - mean).dot(v))).collect();
    let hull: Vec<Vec2> = hull2(&flat).into_iter().filter_map(|i| flat.get(i).copied()).collect();
    if hull.len() < 3 {
        return None;
    }
    let area = polygon_area(&hull);
    if !(area > 1e-9) {
        return None;
    }
    // The centre of mass straight down onto the ground, against each side of the outline.
    let mass = Vec2::new((centroid - mean).dot(u), (centroid - mean).dot(v));
    let mut margin = f64::INFINITY;
    for (i, &a) in hull.iter().enumerate() {
        let b = hull[(i + 1) % hull.len()];
        let edge = b - a;
        let length = edge.len();
        if length > 1e-12 {
            margin = margin.min(edge.cross(mass - a) / length);
        }
    }
    let height = lowest - centroid.dot(normal);
    if !(margin > 0.0 && height > 1e-9) {
        return None;
    }
    let tip = margin.atan2(height);
    if !(tip > 2.0 * PI / 180.0) {
        return None;
    }
    let anchor = mean + normal * (lowest - mean.dot(normal));
    Some(Base { kind: BaseKind::Resting, normal, anchor, area, share: 0.0, tip, rms: 0.0 })
}

/// Lying flat on its broad side, on its edge, on its end: the six directions of the part's own
/// axes, each axis the way round with more points touching first.
fn axis_bases(samples: &[Vec3], centroid: Vec3, extent: f64) -> Vec<Base> {
    if samples.len() < 4 {
        return Vec::new();
    }
    let mut spread = [[0.0f64; 3]; 3];
    for p in samples {
        let d = *p - centroid;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let Some((values, axes)) = solidcraft_geom::inertia::principal(&spread, 0.0) else { return Vec::new() };
    if !(values[2] > 1e-12) {
        return Vec::new();
    }
    let tolerance = (0.012 * extent).max(1e-6);
    let mut out = Vec::new();
    // Thinnest first: lying on its broad side, then on its edge, then on its end.
    for axis in axes {
        let Some(axis) = axis.normalized() else { continue };
        let mut sides: Vec<(Base, usize)> = [axis, -axis]
            .into_iter()
            .filter_map(|normal| {
                let lowest = samples.iter().map(|p| p.dot(normal)).fold(f64::NEG_INFINITY, f64::max);
                let touching: Vec<Vec3> = samples.iter().copied().filter(|p| p.dot(normal) >= lowest - tolerance).collect();
                if touching.is_empty() {
                    return None;
                }
                let mean = touching.iter().fold(Vec3::ZERO, |s, p| s + *p) / touching.len() as f64;
                let anchor = mean + normal * (lowest - mean.dot(normal));
                Some((Base { kind: BaseKind::Axis, normal, anchor, area: 0.0, share: 0.0, tip: 0.0, rms: 0.0 }, touching.len()))
            })
            .collect();
        sides.sort_by_key(|side| std::cmp::Reverse(side.1));
        out.extend(sides.into_iter().map(|(b, _)| b));
    }
    out
}

/// The triangles that would touch the ground standing on `base`: those wholly within a little of
/// its ground plane, to show where the part rests.
pub fn touching(scan: &Scan, base: &Base) -> Vec<usize> {
    let reach = 2.0 * touch_tolerance(scan.extent());
    let Some(n) = base.normal.normalized() else { return Vec::new() };
    let ground = base.anchor.dot(n);
    (0..scan.triangle_count()).filter(|&t| scan.corners(t).is_some_and(|c| c.iter().all(|p| ground - p.dot(n) <= reach))).collect()
}

/// Why a face could not be grown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrowError {
    /// The seed is not a triangle of the scan with an area.
    NotOnTheScan,
    /// The face curves: it is a strip of a curved wall, not a flat face.
    Curved,
}

impl std::fmt::Display for GrowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            GrowError::NotOnTheScan => "pick a point on the scan, on the face it should stand on",
            GrowError::Curved => "the scan curves there, so it is not a flat face: pick the flat face the part stands on",
        })
    }
}

/// A neighbour's normal may lean this far (radians) from the plane fitted to the face so far.
const GROW_ANGLE: f64 = 20.0 * PI / 180.0;

/// A plane fitted to a face: a point of it, its unit normal (either way), and the mean square
/// distance of the face's points from it.
#[derive(Clone, Copy)]
struct Plane {
    point: Vec3,
    normal: Vec3,
    spread: f64,
}

/// The area-weighted moments of a face's triangles (each corner a third of its triangle's area).
#[derive(Clone, Copy, Default)]
struct Moments {
    weight: f64,
    sum: Vec3,
    outer: [[f64; 3]; 3],
}

impl Moments {
    fn add(&mut self, [a, b, c]: [Vec3; 3]) {
        let w = (b - a).cross(c - a).len() / 6.0;
        if !(w > 0.0 && w.is_finite()) {
            return;
        }
        for p in [a, b, c] {
            self.weight += w;
            self.sum += p * w;
            let q = [p.x, p.y, p.z];
            for (row, qi) in self.outer.iter_mut().zip(q) {
                for (cell, qj) in row.iter_mut().zip(q) {
                    *cell += w * qi * qj;
                }
            }
        }
    }

    /// The plane of least squares: through the middle, square to the direction the points spread
    /// least in. `None` with no area.
    fn plane(&self) -> Option<Plane> {
        if !(self.weight > 0.0) {
            return None;
        }
        let mean = self.sum / self.weight;
        let m = [mean.x, mean.y, mean.z];
        let mut covariance = [[0.0f64; 3]; 3];
        for (r, row) in covariance.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = self.outer[r][c] / self.weight - m[r] * m[c];
            }
        }
        let (values, axes) = solidcraft_geom::inertia::principal(&covariance, 0.0)?;
        Some(Plane { point: mean, normal: axes[0].normalized()?, spread: values[0].max(0.0) })
    }
}

/// The triangle of the scan nearest `point`, for a pick.
pub fn nearest_triangle(scan: &Scan, point: Vec3) -> Option<usize> {
    (0..scan.triangle_count())
        .filter_map(|t| {
            let [a, b, c] = scan.corners(t)?;
            Some((t, point.dist(solidcraft_geom::compare::closest_on_triangle(point, a, b, c))))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(t, _)| t)
}

/// The flat face grown from triangle `seed`: it spreads across edges to every connected triangle
/// that lies in the plane fitted to the face so far (fitted again each time the face doubles), then
/// once more from the seed against the final plane. Its outward side is away from the rest of the
/// scan, and its anchor is on the plane through its outermost point, under its middle.
pub fn grow(scan: &Scan, seed: usize) -> Result<Base, GrowError> {
    grow_face(scan, seed).map(|(base, _)| base)
}

/// The ground through the points `at` picked on the scan (three or more: feet, rails, the rim
/// of a bowl): the plane fitted to them, the part on its inner side. Its anchor is the points'
/// middle; its area the area they span. `None` for fewer than three points, or points in a line.
pub fn through(scan: &Scan, at: &[Vec3]) -> Option<Base> {
    if at.len() < 3 || at.iter().any(|p| ![p.x, p.y, p.z].iter().all(|x| x.is_finite())) {
        return None;
    }
    let middle = at.iter().fold(Vec3::ZERO, |m, p| m + *p) / at.len() as f64;
    let mut spread = [[0.0; 3]; 3];
    for p in at {
        let d = *p - middle;
        let d = [d.x, d.y, d.z];
        for (i, row) in spread.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += d[i] * d[j];
            }
        }
    }
    let (moments, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    // Points in a line spread one way only: no plane through them.
    if moments[1] <= 1e-9 * moments[2].max(1e-30) {
        return None;
    }
    let mut normal = axes[0].normalized()?;
    // Outward from the part: the scan lies on the other side.
    let n = scan.positions.len().max(1);
    let step = (n / 20_000).max(1);
    let lean: f64 =
        scan.positions.iter().step_by(step).filter(|p| [p.x, p.y, p.z].iter().all(|x| x.is_finite())).map(|p| (*p - middle).dot(normal)).sum();
    if lean > 0.0 {
        normal = -normal;
    }
    let u = normal.any_perp().normalized()?;
    let v = normal.cross(u);
    let flat: Vec<Vec2> = at.iter().map(|p| Vec2::new((*p - middle).dot(u), (*p - middle).dot(v))).collect();
    let hull: Vec<Vec2> = hull2(&flat).into_iter().filter_map(|i| flat.get(i).copied()).collect();
    let area = 0.5 * (0..hull.len()).map(|i| hull[i].cross(hull[(i + 1) % hull.len()])).sum::<f64>().abs();
    let rms = (at.iter().map(|p| (*p - middle).dot(normal).powi(2)).sum::<f64>() / at.len() as f64).sqrt();
    Some(Base { kind: BaseKind::Resting, normal, anchor: middle, area, share: 0.0, tip: 0.0, rms })
}

/// [`grow`], with the face's triangles (the seed first), to show which face it is.
pub fn grow_face(scan: &Scan, seed: usize) -> Result<(Base, Vec<usize>), GrowError> {
    let seed_corners = scan.corners(seed).ok_or(GrowError::NotOnTheScan)?;
    let [a, b, c] = seed_corners;
    let seed_normal = (b - a).cross(c - a).normalized().ok_or(GrowError::NotOnTheScan)?;
    let tolerance = touch_tolerance(scan.extent());
    let neighbours = edge_neighbours(scan);
    // Moments about the seed, so that a scan far from the origin keeps its precision.
    let origin = (a + b + c) / 3.0;
    let local = |[a, b, c]: [Vec3; 3]| [a - origin, b - origin, c - origin];
    let cos_limit = GROW_ANGLE.cos();
    let spread = |start: Plane, refits: bool| -> (Vec<usize>, Plane) {
        let mut plane = start;
        let mut moments = Moments::default();
        moments.add(local(seed_corners));
        let mut seen = vec![false; scan.triangle_count()];
        if let Some(s) = seen.get_mut(seed) {
            *s = true;
        }
        let mut found = vec![seed];
        let mut next_fit = 16;
        let mut head = 0;
        while let Some(&triangle) = found.get(head) {
            head += 1;
            for neighbour in neighbours.of(scan, triangle) {
                match seen.get_mut(neighbour) {
                    Some(s) if !*s => *s = true,
                    _ => continue,
                }
                let Some(corners) = scan.corners(neighbour) else { continue };
                let [a, b, c] = local(corners);
                // Either winding; a sliver with no normal is judged by where it is alone.
                if let Some(n) = (b - a).cross(c - a).normalized()
                    && n.dot(plane.normal).abs() < cos_limit
                {
                    continue;
                }
                if !(((a + b + c) / 3.0 - plane.point).dot(plane.normal).abs() <= tolerance) {
                    continue;
                }
                found.push(neighbour);
                moments.add([a, b, c]);
                if refits && found.len() >= next_fit {
                    if let Some(fitted) = moments.plane() {
                        plane = fitted;
                    }
                    next_fit *= 2;
                }
            }
        }
        (found, moments.plane().unwrap_or(plane))
    };
    let (_, first) = spread(Plane { point: Vec3::ZERO, normal: seed_normal, spread: 0.0 }, true);
    let (triangles, plane) = spread(first, false);

    // Curved, by the same measure as the flat faces: the normals of the triangles facing nearly the
    // plane's way, against where they are.
    let facing_limit = nearly_facing();
    let step = (triangles.len() / 50_000).max(1);
    let mut area = 0.0;
    let mut facing = Vec::new();
    for (k, corners) in triangles.iter().filter_map(|&t| scan.corners(t)).enumerate() {
        let [a, b, c] = corners;
        let cross = (b - a).cross(c - a);
        let twice = cross.len();
        area += twice / 2.0;
        if k % step != 0 || !(twice > 0.0 && twice.is_finite()) {
            continue;
        }
        let n = cross / twice;
        let along = if n.dot(plane.normal) < 0.0 { -n } else { n };
        if along.dot(plane.normal) > facing_limit {
            facing.push(((a + b + c) / 3.0, along, twice / 2.0));
        }
    }
    if triangles.len() >= 3 && turn(&facing, plane.normal) > MAX_TURN {
        return Err(GrowError::Curved);
    }
    // Outward is the side of the plane the rest of the scan is not on.
    let middle = origin + plane.point;
    let mut outward = plane.normal;
    let side: f64 = scan.points().step_by((scan.positions.len() / 20_000).max(1)).map(|p| (p - middle).dot(outward)).sum();
    if side > 0.0 {
        outward = -outward;
    }
    let outermost = triangles.iter().filter_map(|&t| scan.corners(t)).flatten().map(|p| p.dot(outward)).fold(f64::NEG_INFINITY, f64::max);
    let anchor = middle + outward * (outermost - middle.dot(outward));
    let total: f64 = (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).map(|[a, b, c]| (b - a).cross(c - a).len() / 2.0).sum();
    let share = if total > 0.0 { area / total } else { 0.0 };
    Ok((Base { kind: BaseKind::Grown, normal: outward, anchor, area, share, tip: 0.0, rms: plane.spread.sqrt() }, triangles))
}

/// The triangles across each edge: found through each position's triangles (one flat list,
/// built in two passes), so a scan of millions of triangles needs no hashing.
pub(crate) struct EdgeNeighbours {
    /// Where each position's triangles start in `triangles` (one more entry than positions).
    starts: Vec<u32>,
    triangles: Vec<u32>,
}

impl EdgeNeighbours {
    /// The triangles sharing an edge with triangle `t`.
    pub(crate) fn of<'a>(&'a self, scan: &'a Scan, t: usize) -> impl Iterator<Item = usize> + 'a {
        let corners = scan.triangles.get(t).copied();
        corners.into_iter().flat_map(move |[a, b, c]| {
            [(a, b), (b, c), (c, a)]
                .into_iter()
                .flat_map(move |(p, q)| self.around(p).filter(move |&n| n != t && scan.triangles.get(n).is_some_and(|tri| tri.contains(&q))))
        })
    }

    /// The triangles that use position `p`.
    fn around(&self, p: u32) -> impl Iterator<Item = usize> + '_ {
        let p = p as usize;
        let (from, to) = match (self.starts.get(p), self.starts.get(p + 1)) {
            (Some(&a), Some(&b)) => (a as usize, b as usize),
            _ => (0, 0),
        };
        self.triangles.get(from..to).unwrap_or_default().iter().map(|&t| t as usize)
    }
}

pub(crate) fn edge_neighbours(scan: &Scan) -> EdgeNeighbours {
    let n = scan.positions.len();
    let mut starts = vec![0u32; n + 1];
    for tri in scan.triangles {
        for &p in tri {
            if let Some(s) = starts.get_mut(p as usize + 1) {
                *s += 1;
            }
        }
    }
    for i in 0..n {
        starts[i + 1] += starts[i];
    }
    let mut fill = starts.clone();
    let mut triangles = vec![0u32; starts[n] as usize];
    for (t, tri) in scan.triangles.iter().enumerate() {
        for &p in tri {
            if let Some(slot) = fill.get_mut(p as usize)
                && let Some(cell) = triangles.get_mut(*slot as usize)
            {
                *cell = t as u32;
                *slot += 1;
            }
        }
    }
    EdgeNeighbours { starts, triangles }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::stand_on;
    use crate::test_meshes::{cuboid, fine_cuboid, turned};

    /// Standing on a slab's broad face, the triangles of that face touch the ground and nothing else does.
    #[test]
    fn the_triangles_touching_the_ground_are_the_base_face() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(80.0, 50.0, 10.0), 4);
        let scan = Scan::new(&positions, &triangles);
        let found = bases(&scan);
        let touching = touching(&scan, &found[0]);
        assert_eq!(touching.len(), 2 * 4 * 4, "the one face's triangles");
        let normal = found[0].normal;
        assert!(
            touching
                .iter()
                .all(|&t| scan.corners(t).is_some_and(|[a, b, c]| (b - a).cross(c - a).normalized().is_some_and(|m| m.dot(normal).abs() > 0.999)))
        );
        let seed = touching[0];
        let (grown, faces) = grow_face(&scan, seed).unwrap();
        assert_eq!(faces.len(), 32);
        assert!(grown.normal.dot(normal) > 0.999);
    }

    /// A cup turned and moved anywhere is read as turned about its own axis; a box, a slab (whose
    /// broad faces alone would pass any line square to them) and a ball (which has no one axis)
    /// are not.
    #[test]
    fn a_turned_part_is_read_about_its_axis_and_a_box_a_slab_and_a_ball_are_not() {
        use crate::test_meshes::revolved;
        // A cup: its bottom, its wall leaning out, open at the top.
        let profile: Vec<[f64; 2]> =
            [[0.0, 0.0], [0.0, 30.0]].into_iter().chain((1..=12).map(|i| [5.0 * f64::from(i), 30.0 + 0.4 * f64::from(i)])).collect();
        let (positions, triangles) = revolved(&profile, 64, false);
        let (axis, shift) = (Vec3::new(0.3, -0.5, 0.8).normalized().unwrap(), Vec3::new(40.0, -12.0, 7.0));
        let moved = turned(&positions, axis, 0.7, shift);
        let (on, direction) = turned_about(&Scan::new(&moved, &triangles)).expect("turned");
        let its_axis = turned(&[Vec3::ZERO, Vec3::Z], axis, 0.7, shift);
        let along = its_axis[1] - its_axis[0];
        assert!(direction.dot(along).abs() > 0.5f64.to_radians().cos(), "{direction:?} against {along:?}");
        let off = on - its_axis[0];
        assert!((off - along * off.dot(along)).len() < 0.05, "on the axis: {on:?}");
        for size in [Vec3::new(40.0, 40.0, 40.0), Vec3::new(80.0, 50.0, 10.0), Vec3::new(30.0, 20.0, 60.0)] {
            let (p, t) = fine_cuboid(Vec3::ZERO, size, 8);
            assert!(turned_about(&Scan::new(&p, &t)).is_none(), "a box {size:?}");
        }
        let ball: Vec<[f64; 2]> = (0..=24).map(|i| std::f64::consts::PI * f64::from(i) / 24.0).map(|a| [-20.0 * a.cos(), 20.0 * a.sin()]).collect();
        let (p, t) = revolved(&ball, 48, false);
        assert!(turned_about(&Scan::new(&p, &t)).is_none(), "a ball has no one axis");
    }

    /// A cup's first bases are the two ends of its axis, and a part that already stands on one of
    /// the best is left standing on it: the cup upright or upside down stays so, a slab on either
    /// broad face (they tie) is not turned over; lying on a worse side, each goes to the first.
    #[test]
    fn a_part_that_stands_well_is_left_as_it_stands() {
        use crate::test_meshes::revolved;
        let profile: Vec<[f64; 2]> =
            [[0.0, 0.0], [0.0, 30.0]].into_iter().chain((1..=12).map(|i| [5.0 * f64::from(i), 30.0 + 0.4 * f64::from(i)])).collect();
        let (positions, triangles) = revolved(&profile, 64, false);
        let cup = bases(&Scan::new(&positions, &triangles));
        assert!(cup.len() >= 2 && cup[0].kind == BaseKind::End && cup[1].kind == BaseKind::End, "{cup:?}");
        assert!(cup[0].normal.dot(cup[1].normal) < -0.999 && cup[0].normal.z.abs() > 0.999, "{cup:?}");
        for down in [-Vec3::Z, Vec3::Z] {
            let chosen = &cup[preferred(&cup, down)];
            assert!(chosen.kind == BaseKind::End && chosen.normal.dot(down) > 0.999, "{down:?}: {chosen:?}");
        }
        assert_eq!(preferred(&cup, Vec3::X), 0, "lying on its side: stood on the first");
        // Standing on an end puts the axis over the origin.
        let stood = crate::place::stand(cup[0].normal, cup[0].anchor).unwrap();
        let foot = stood.apply(Vec3::ZERO);
        assert!(foot.x.abs() < 1e-6 && foot.y.abs() < 1e-6, "{foot:?}");

        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(80.0, 50.0, 10.0), 4);
        let slab = bases(&Scan::new(&positions, &triangles));
        assert!(slab.iter().all(|b| b.kind != BaseKind::End), "{slab:?}");
        for down in [-Vec3::Z, Vec3::Z] {
            assert!(slab[preferred(&slab, down)].normal.dot(down) > 0.999, "the broad face it stands on, either way up");
        }
        assert_eq!(preferred(&slab, Vec3::X), 0, "on its end it is stood on a broad face");
        assert!(slab[0].normal.z.abs() > 0.999);
    }

    /// A scan of more than 100 000 triangles is read a sample at a time: a face's area is still
    /// the whole face's.
    #[test]
    fn a_large_scans_face_area_is_the_whole_faces() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(80.0, 50.0, 10.0), 130);
        assert!(triangles.len() >= 200_000, "read every second triangle");
        let found = bases(&Scan::new(&positions, &triangles));
        let flat = found.iter().find(|b| b.kind == BaseKind::Flat).unwrap();
        assert!((flat.area - 4000.0).abs() < 0.02 * 4000.0, "{}", flat.area);
    }

    /// A slab 80 × 50 × 10: its broad faces come first, each a flat face, and standing on the first
    /// puts a broad face down on the grid. Turned anywhere first, the answer is the same.
    #[test]
    fn a_slab_stands_on_a_broad_face() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(80.0, 50.0, 10.0), 4);
        let axis = Vec3::new(1.0, 0.4, -0.3).normalized().unwrap();
        let moved = turned(&positions, axis, 1.1, Vec3::new(-30.0, 12.0, 40.0));
        let scan = Scan::new(&moved, &triangles);
        let found = bases(&scan);
        assert!(!found.is_empty() && found.len() <= MAX_BASES);
        let first = found[0];
        assert_eq!(first.kind, BaseKind::Flat);
        assert!((first.area - 4000.0).abs() < 1.0, "a broad face: {first:?}");
        let stand = stand_on(first.normal, first.anchor).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| stand.apply(*p)).collect();
        let lo = placed.iter().map(|p| p.z).fold(f64::MAX, f64::min);
        let hi = placed.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        assert!(lo.abs() < 1e-9 && (hi - 10.0).abs() < 1e-9, "lying flat on the grid: {lo} to {hi}");
        // No two bases the same way.
        for (i, a) in found.iter().enumerate() {
            assert!(found[i + 1..].iter().all(|b| a.normal.dot(b.normal) <= same_way()));
        }
    }

    /// A box with one corner missing still rests; a scan of nothing has no base.
    #[test]
    fn bases_of_odd_scans_do_not_fail() {
        assert!(bases(&Scan::new(&[], &[])).is_empty());
        let (positions, triangles) = cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0));
        let broken: Vec<[u32; 3]> = triangles.iter().copied().chain([[0, 1, 99]]).collect();
        assert!(!bases(&Scan::new(&positions, &broken)).is_empty(), "a triangle naming no position is skipped");
    }

    /// A face grown from a clicked point on the top of a box is the top, facing up, with its anchor on
    /// it; a point on a wide curved wall is refused.
    #[test]
    fn a_face_grows_from_a_click_and_a_curved_wall_is_refused() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0), 6);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(20.0, 15.0, 20.0)).unwrap();
        let face = grow(&scan, seed).unwrap();
        assert_eq!(face.kind, BaseKind::Grown);
        assert!(face.normal.dot(Vec3::Z) > 0.999999, "outward is up: {face:?}");
        assert!((face.anchor.z - 20.0).abs() < 1e-9);
        assert!((face.area - 1200.0).abs() < 1e-6 && face.rms < 1e-9);

        // A cylinder wall R 200 in 720 strips: each strip is flat and its neighbours lie within the
        // tolerance, but the face grown across them turns.
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let sides = 720u32;
        for k in 0..sides {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
            for z in [0.0, 30.0] {
                positions.push(Vec3::new(200.0 * a.cos(), 200.0 * a.sin(), z));
            }
        }
        for k in 0..sides {
            let (a, b) = (2 * k, 2 * ((k + 1) % sides));
            triangles.push([a, b, b + 1]);
            triangles.push([a, b + 1, a + 1]);
        }
        let scan = Scan::new(&positions, &triangles);
        assert_eq!(grow(&scan, 0), Err(GrowError::Curved));
        assert_eq!(grow(&scan, 9999), Err(GrowError::NotOnTheScan));
    }
}
