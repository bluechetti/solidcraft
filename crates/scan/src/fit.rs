//! A plane or a cylinder fitted to the part of a scan around a picked point. The region starts as
//! the triangles near the pick, the shape is fitted to it, and the region then grows across edges
//! to every connected triangle that lies on the shape (within the tolerance, and facing as the
//! shape does there), the shape fitted again each time the region doubles. Growing against the
//! shape rather than across gentle edges keeps a noisy scan's region whole.

use std::f64::consts::PI;

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
        }
    }

    /// The shape's direction at `p` (either way).
    fn normal_at(&self, p: Vec3) -> Option<Vec3> {
        match *self {
            Shape::Plane { normal, .. } => Some(normal),
            Shape::Cylinder { point, axis, .. } => {
                let v = p - point;
                (v - axis * v.dot(axis)).normalized()
            }
        }
    }
}

/// Which kind of shape to fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plane,
    Cylinder,
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
    /// How far the region reaches along a cylinder's axis, from `middle` either way (0 for a plane).
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
            FitError::NoFit => "the scan there is not that shape: pick the middle of a flat face for a plane, of a round wall for a cylinder",
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

/// Fits a `kind` of shape to the region round triangle `seed`. `tolerance` is how far (mm) a
/// triangle may be from the shape and still be on it (default: [`tolerance`] of the scan).
pub fn fit(scan: &Scan, seed: usize, kind: Kind, tolerance: Option<f64>) -> Result<Fitted, FitError> {
    let [a, b, c] = scan.corners(seed).ok_or(FitError::NotOnTheScan)?;
    let pick = (a + b + c) / 3.0;
    let extent = scan.extent();
    let tolerance = tolerance.filter(|t| t.is_finite() && *t > 0.0).unwrap_or_else(|| self::tolerance(extent));
    let neighbours = edge_neighbours(scan);
    // The start: the connected triangles within a small reach of the pick, wider for a cylinder
    // so that the patch curves enough to show its axis.
    // On a coarse mesh the reach widens until the patch holds enough triangles to fit.
    let mut reach = match kind {
        Kind::Plane => 0.01 * extent,
        Kind::Cylinder => 0.03 * extent,
    };
    let near = |reach: f64| connected(scan, &neighbours, seed, |t| scan.corners(t).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(pick) <= reach));
    let mut start = near(reach);
    while start.len() < 12 && reach < 0.1 * extent {
        reach *= 2.0;
        start = near(reach);
    }
    let mut shape = solve(scan, &start, kind).ok_or(FitError::NoFit)?;
    // Grow against the shape, fitting again each time the region doubles, then once more against
    // the final shape.
    let mut region = start;
    for _ in 0..12 {
        // Early only once the region is wide enough to tell: a small patch of a wide shaft looks flat too.
        if flat_cylinder(scan, &region, &shape, 0.1 * extent) {
            return Err(FitError::NoFit);
        }
        let grown = grow(scan, &neighbours, seed, &shape, tolerance);
        let doubled = grown.len() >= 2 * region.len();
        region = grown;
        shape = solve(scan, &region, kind).unwrap_or(shape);
        if !doubled {
            break;
        }
    }
    let region = grow(scan, &neighbours, seed, &shape, tolerance);
    let shape = solve(scan, &region, kind).unwrap_or(shape);
    if region.len() < 3 {
        return Err(FitError::NoFit);
    }
    if flat_cylinder(scan, &region, &shape, 0.0) {
        return Err(FitError::NoFit);
    }
    summarize(scan, region, shape)
}

/// Whether `shape` is a "cylinder" far wider than the region it was fitted to, a region at least
/// `reach` across: a flat face, curved only by noise. Asked while the region grows too, so that a
/// flat face is refused before the region spreads over it.
fn flat_cylinder(scan: &Scan, region: &[usize], shape: &Shape, reach: f64) -> bool {
    let Shape::Cylinder { radius, .. } = *shape else { return false };
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

/// The connected triangles from `seed` that lie on `shape`: their middle within `tolerance` of it
/// and their direction within [`MAX_LEAN`] of its direction there.
fn grow(scan: &Scan, neighbours: &EdgeNeighbours, seed: usize, shape: &Shape, tolerance: f64) -> Vec<usize> {
    let cos = MAX_LEAN.cos();
    connected(scan, neighbours, seed, |t| {
        let Some([a, b, c]) = scan.corners(t) else { return false };
        let middle = (a + b + c) / 3.0;
        if !(shape.distance(middle).abs() <= tolerance) {
            return false;
        }
        match ((b - a).cross(c - a).normalized(), shape.normal_at(middle)) {
            (Some(n), Some(m)) => n.dot(m).abs() >= cos,
            // A sliver with no direction is judged by where it is alone.
            _ => true,
        }
    })
}

/// The region's vertices (each corner of each triangle, weighted by a third of its area) about
/// their middle, and the area-weighted sum of the triangles' `n nᵀ`.
struct Samples {
    origin: Vec3,
    points: Vec<(Vec3, f64)>,
    normals: [[f64; 3]; 3],
}

fn samples(scan: &Scan, region: &[usize]) -> Option<Samples> {
    let mut points = Vec::with_capacity(region.len() * 3);
    let mut normals = [[0.0f64; 3]; 3];
    let (mut total, mut sum) = (0.0, Vec3::ZERO);
    for &t in region {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let cross = (b - a).cross(c - a);
        let area = cross.len() / 2.0;
        let Some(n) = cross.normalized() else { continue };
        total += area;
        sum += (a + b + c) * (area / 3.0);
        for p in [a, b, c] {
            points.push((p, area / 3.0));
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
    let points = crate::thinned(&points, SOLVE_LIMIT);
    let weight: f64 = points.iter().map(|(_, w)| w).sum();
    Some(Samples { origin, points: points.into_iter().map(|(p, w)| (p - origin, w / weight)).collect(), normals })
}

/// The shape of `kind` fitted to the region, `None` when it has too little to fit or does not fit.
fn solve(scan: &Scan, region: &[usize], kind: Kind) -> Option<Shape> {
    let s = samples(scan, region)?;
    let shape = match kind {
        Kind::Plane => plane(&s)?,
        Kind::Cylinder => cylinder(&s)?,
    };
    // Back from about the region's middle.
    Some(match shape {
        Shape::Plane { point, normal } => Shape::Plane { point: point + s.origin, normal },
        Shape::Cylinder { point, axis, radius } => Shape::Cylinder { point: point + s.origin, axis, radius },
    })
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
            let Some(step) = solve5(damped, g.map(|x| -x)) else { break };
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

/// The circle that best fits weighted points on a plane (Kåsa's): its centre and radius.
fn kasa(points: impl Iterator<Item = ([f64; 2], f64)>) -> Option<([f64; 2], f64)> {
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

/// The 5 × 5 system `a x = b` by Gaussian elimination with partial pivoting; `None` if singular.
fn solve5(mut a: [[f64; 5]; 5], mut b: [f64; 5]) -> Option<[f64; 5]> {
    for col in 0..5 {
        let pivot = (col..5).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
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
    let mut x = [0.0; 5];
    for row in (0..5).rev() {
        let sum: f64 = (row + 1..5).map(|k| a[row][k] * x[k]).sum();
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
            if let Shape::Cylinder { point, axis, .. } = shape {
                let s = (p - point).dot(axis);
                along = (along.0.min(s), along.1.max(s));
            }
        }
    }
    if !(area > 0.0 && count > 0) {
        return Err(FitError::NoFit);
    }
    let centre = sum / area;
    let (middle, half_length) = match shape {
        Shape::Plane { point, normal } => (centre - normal * (centre - point).dot(normal), 0.0),
        Shape::Cylinder { point, axis, .. } => (point + axis * ((along.0 + along.1) / 2.0), (along.1 - along.0) / 2.0),
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
}
