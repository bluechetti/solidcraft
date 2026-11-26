//! A shape fitted straight to a point cloud's points, with no surface made first: the same
//! solvers as a scan's fit, their samples the points (each weighing the same) and each point's
//! direction the normal the file states or the one read from its nearest neighbours. With a
//! tolerance asked for, a random-sample consensus first picks the shape most points lie on, so a
//! few stray points (the table under a part, a hand in the capture) do not pull it.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use solidcraft_geom::inertia::{Tensor3, principal};

use super::{FitError, Fitted, Kind, MAX_LEAN, SOLVE_LIMIT, Samples, Shape, solved};
use crate::Scan;
use crate::poisson::PointGrid;

/// The neighbours (itself with them) a point's normal is read from.
const NEIGHBOURS: usize = 12;

/// The fewest points a shape is fitted to.
const FEWEST: usize = 6;

/// Random subsets tried, each of [`SUBSET`] points.
const TRIALS: usize = 300;
const SUBSET: usize = 12;

/// What a fit to points tells: the shape, how well it fits and where its middle is.
#[derive(Clone, Debug, PartialEq)]
pub struct PointsFitted {
    pub shape: Shape,
    /// The points it was fitted to.
    pub points: usize,
    /// With a tolerance asked for: the points within it (the rest are left out of what follows);
    /// else all of them.
    pub inliers: usize,
    /// The inliers' distance from the shape: root mean square and largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// The spread of the inliers' signed distances from the shape, largest less smallest, in mm.
    pub form_error: f64,
    /// How far round its axis a cylinder's or a cone's points go, in radians (`None` otherwise).
    pub arc: Option<f64>,
    pub middle: Vec3,
    pub half_length: f64,
}

impl PointsFitted {
    /// The fit as a scan's fit reads, for what keeps a shape: no triangles, no area.
    pub fn as_fitted(&self) -> Fitted {
        Fitted {
            shape: self.shape,
            triangles: Vec::new(),
            area: 0.0,
            rms: self.rms,
            max: self.max,
            form_error: self.form_error,
            arc: self.arc,
            middle: self.middle,
            left_out: self.points - self.inliers,
            half_length: self.half_length,
        }
    }
}

/// The unit normal of the plane of least squares through each point and its nearest neighbours,
/// facing either way (zero where the neighbours are in a line or too few).
pub fn normals_of(points: &[Vec3]) -> Vec<Vec3> {
    if points.len() < NEIGHBOURS {
        return vec![Vec3::ZERO; points.len()];
    }
    let grid = PointGrid::sized_for(points, NEIGHBOURS as f64);
    let mut near = Vec::new();
    points
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            grid.nearest(p, NEIGHBOURS, Some(i as u32), &mut near);
            let around: Vec<Vec3> = near.iter().map(|&(_, k)| points[k as usize]).chain(std::iter::once(p)).collect();
            let middle = around.iter().fold(Vec3::ZERO, |a, &q| a + q) / around.len() as f64;
            let mut spread: Tensor3 = [[0.0; 3]; 3];
            for q in &around {
                let d = *q - middle;
                let d = [d.x, d.y, d.z];
                for (row, di) in spread.iter_mut().zip(d) {
                    for (cell, dj) in row.iter_mut().zip(d) {
                        *cell += di * dj;
                    }
                }
            }
            principal(&spread, 0.0).and_then(|(_, axes)| axes[0].normalized()).unwrap_or(Vec3::ZERO)
        })
        .collect()
}

/// The shape of `kind` fitted to `points` (with `normals`, one each, when they are known; else
/// read from the neighbours); with `tolerance` the shape most points lie on within it.
pub fn fit_points(points: &[Vec3], normals: Option<&[Vec3]>, kind: Kind, tolerance: Option<f64>) -> Result<PointsFitted, FitError> {
    let normals = normals.filter(|n| n.len() == points.len());
    let wanted: Vec<usize> = (0..points.len()).filter(|&i| points[i].x.is_finite() && points[i].y.is_finite() && points[i].z.is_finite()).collect();
    if wanted.len() < FEWEST {
        return Err(FitError::NoFit);
    }
    // An even sample of a large cloud fits as well and far faster.
    let chosen = crate::thinned(&wanted, SOLVE_LIMIT);
    let sampled: Vec<Vec3> = chosen.iter().map(|&i| points[i]).collect();
    let directions = match normals {
        Some(n) => chosen.iter().map(|&i| n[i]).collect(),
        None => normals_of(&sampled),
    };
    let mut shape = solve(&sampled, &directions, kind).ok_or(FitError::NoFit)?;
    let mut keep = vec![true; points.len()];
    if let Some(tolerance) = tolerance.filter(|t| *t > 0.0 && t.is_finite()) {
        shape = consensus(&sampled, &directions, kind, tolerance, shape);
        // The shape settled on the points that lie on it.
        let inside: Vec<usize> = (0..sampled.len()).filter(|&k| shape.distance(sampled[k]).abs() <= tolerance).collect();
        if inside.len() >= FEWEST {
            let (p, n): (Vec<Vec3>, Vec<Vec3>) = inside.iter().map(|&k| (sampled[k], directions[k])).unzip();
            if let Some(again) = solve(&p, &n, kind) {
                shape = again;
            }
        }
        for &i in &wanted {
            keep[i] = shape.distance(points[i]).abs() <= tolerance;
        }
    }
    let on: Vec<Vec3> = wanted.iter().filter(|&&i| keep[i]).map(|&i| points[i]).collect();
    if on.len() < FEWEST {
        return Err(FitError::NoFit);
    }
    Ok(summarize(shape, on, wanted.len()))
}

/// [`fit_region`](super::fit_region) on the triangles of `region` that lie on the shape most of
/// them lie on within `tolerance` (a random-sample consensus), the rest left out and counted in
/// [`Fitted::left_out`]: a hand-chosen region with a stray piece in it (the table the part stood on)
/// does not pull the shape.
pub fn fit_region_robust(scan: &Scan, region: &[usize], kind: Kind, tolerance: f64) -> Result<Fitted, FitError> {
    let mut region: Vec<usize> = region.iter().copied().filter(|&t| scan.corners(t).is_some()).collect();
    region.sort_unstable();
    region.dedup();
    if region.len() < 3 {
        return Err(FitError::NotOnTheScan);
    }
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(FitError::NoFit);
    }
    // The corners of an even sample of the triangles, each facing as its triangle does.
    let mut points = Vec::new();
    let mut directions = Vec::new();
    for &t in &crate::thinned(&region, SOLVE_LIMIT / 3) {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let n = (b - a).cross(c - a).normalized().unwrap_or(Vec3::ZERO);
        for p in [a, b, c] {
            points.push(p);
            directions.push(n);
        }
    }
    let start = solve(&points, &directions, kind).ok_or(FitError::NoFit)?;
    let shape = consensus(&points, &directions, kind, tolerance, start);
    let on = |shape: &Shape, t: usize| {
        scan.corners(t).is_some_and(|[a, b, c]| {
            let n = (b - a).cross(c - a).normalized().unwrap_or(Vec3::ZERO);
            [a, b, c].iter().all(|&p| super::points::on(shape, p, n, tolerance))
        })
    };
    let mut inside: Vec<usize> = region.iter().copied().filter(|&t| on(&shape, t)).collect();
    if inside.len() < 3 {
        return Err(FitError::NoFit);
    }
    // Settled on the triangles that lie on it.
    let settled = super::solve_with(scan, &inside, kind, SOLVE_LIMIT).unwrap_or(shape);
    let again: Vec<usize> = region.iter().copied().filter(|&t| on(&settled, t)).collect();
    if again.len() >= 3 {
        inside = again;
    }
    let shape = super::solve_with(scan, &inside, kind, SOLVE_LIMIT).ok_or(FitError::NoFit)?;
    let left_out = region.len() - inside.len();
    let mut fitted = super::summarize(scan, inside, shape)?;
    fitted.left_out = left_out;
    Ok(fitted)
}

/// The shape of `kind` fitted to the points, each facing as its direction says.
pub(super) fn solve(points: &[Vec3], directions: &[Vec3], kind: Kind) -> Option<Shape> {
    if points.len() < FEWEST {
        return None;
    }
    let origin = points.iter().fold(Vec3::ZERO, |a, &p| a + p) / points.len() as f64;
    let weight = 1.0 / points.len() as f64;
    let (lo, hi) = points.iter().fold((points[0], points[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let mut normals = [[0.0f64; 3]; 3];
    for n in directions {
        let n = [n.x, n.y, n.z];
        for (row, ni) in normals.iter_mut().zip(n) {
            for (cell, nj) in row.iter_mut().zip(n) {
                *cell += weight * ni * nj;
            }
        }
    }
    let samples = Samples {
        origin,
        points: points.iter().map(|&p| (p - origin, weight)).collect(),
        directions: directions.to_vec(),
        normals,
        size: (hi - lo).len(),
    };
    solved(&samples, kind)
}

/// Whether the point lies on the shape within `tolerance`, facing as the shape does there when its
/// normal is known.
pub(super) fn on(shape: &Shape, p: Vec3, n: Vec3, tolerance: f64) -> bool {
    shape.distance(p).abs() <= tolerance
        && (n.len2() < 0.5 || shape.normal_at(p).is_none_or(|s| s.dot(n).abs() >= MAX_LEAN.cos()))
}

/// How many points are on the shape.
fn support(shape: &Shape, points: &[Vec3], directions: &[Vec3], tolerance: f64) -> usize {
    points.iter().zip(directions).filter(|&(&p, &n)| on(shape, p, n, tolerance)).count()
}

/// The shape of the most support among `start` and those fitted to random subsets.
pub(super) fn consensus(points: &[Vec3], directions: &[Vec3], kind: Kind, tolerance: f64, start: Shape) -> Shape {
    let (mut best, mut most) = (start, support(&start, points, directions, tolerance));
    let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ points.len() as u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let pick = SUBSET.min(points.len());
    for _ in 0..TRIALS {
        let chosen: Vec<usize> = (0..pick).map(|_| (next() % points.len() as u64) as usize).collect();
        let (p, n): (Vec<Vec3>, Vec<Vec3>) = chosen.iter().map(|&k| (points[k], directions[k])).unzip();
        let Some(shape) = solve(&p, &n, kind) else { continue };
        let votes = support(&shape, points, directions, tolerance);
        if votes > most {
            (best, most) = (shape, votes);
        }
    }
    best
}

/// How well `shape` fits `on` (the points that count) and where its middle is, as a scan's fit
/// reads them.
fn summarize(shape: Shape, on: Vec<Vec3>, points: usize) -> PointsFitted {
    let count = on.len() as f64;
    let (mut squares, mut max, mut signed) = (0.0, 0.0f64, (f64::INFINITY, f64::NEG_INFINITY));
    let mut along: (f64, f64) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut sum = Vec3::ZERO;
    for &p in &on {
        let d = shape.distance(p);
        squares += d * d;
        max = max.max(d.abs());
        signed = (signed.0.min(d), signed.1.max(d));
        sum += p;
        if let Shape::Cylinder { point: from, axis, .. } | Shape::Cone { apex: from, axis, .. } = shape {
            let s = (p - from).dot(axis);
            along = (along.0.min(s), along.1.max(s));
        }
    }
    let arc = match shape {
        Shape::Cylinder { point: from, axis, .. } | Shape::Cone { apex: from, axis, .. } => {
            let u = axis.any_perp().normalized().unwrap_or(Vec3::X);
            let off_axis = |p: Vec3| ((p - from) - axis * (p - from).dot(axis)).len();
            let least = super::NEAR_AXIS * on.iter().map(|&p| off_axis(p)).fold(0.0, f64::max);
            let telling = on.iter().copied().filter(|&p| off_axis(p) > least);
            crate::pipe::angular_extent(telling, from, axis, u).map(|(a, b)| (b - a).clamp(0.0, 2.0 * std::f64::consts::PI))
        }
        _ => None,
    };
    let centre = sum / count;
    let (middle, half_length) = match shape {
        Shape::Plane { point, normal } => (centre - normal * (centre - point).dot(normal), 0.0),
        Shape::Cylinder { point, axis, .. } => (point + axis * ((along.0 + along.1) / 2.0), (along.1 - along.0) / 2.0),
        Shape::Sphere { center, .. } | Shape::Torus { center, .. } => (center, 0.0),
        Shape::Cone { apex, axis, .. } => (apex + axis * ((along.0 + along.1) / 2.0), (along.1 - along.0) / 2.0),
    };
    PointsFitted { shape, points, inliers: on.len(), rms: (squares / count).sqrt(), max, form_error: signed.1 - signed.0, arc, middle, half_length }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grid of `n` × `n` squares of side 2 at height `z`, as triangles.
    fn grid(n: u32, z: f64, positions: &mut Vec<Vec3>, triangles: &mut Vec<[u32; 3]>) {
        let base = positions.len() as u32;
        for j in 0..=n {
            for i in 0..=n {
                positions.push(Vec3::new(f64::from(i) * 2.0, f64::from(j) * 2.0, z));
            }
        }
        let at = |i: u32, j: u32| base + j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }

    /// A plate and a table 8 mm under it, both in the region asked for: fitted as it is the plane
    /// leans between them; robustly it lies on the plate and counts the table left out.
    #[test]
    fn a_region_with_a_stray_piece_is_fitted_to_the_part() {
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        grid(20, 5.0, &mut positions, &mut triangles);
        let plate = triangles.len();
        grid(8, -3.0, &mut positions, &mut triangles);
        let scan = Scan::new(&positions, &triangles);
        let all: Vec<usize> = (0..triangles.len()).collect();
        let loose = super::super::fit_region(&scan, &all, Kind::Plane).unwrap();
        let Shape::Plane { point, .. } = loose.shape else { panic!() };
        assert!(point.z < 4.5, "pulled down: {point:?}");
        let firm = fit_region_robust(&scan, &all, Kind::Plane, 0.1).unwrap();
        let Shape::Plane { point, normal } = firm.shape else { panic!() };
        assert!((point.z - 5.0).abs() < 1e-6 && normal.dot(Vec3::Z).abs() > 0.99999, "{point:?} {normal:?}");
        assert_eq!(firm.triangles.len(), plate);
        assert_eq!(firm.left_out, triangles.len() - plate);
        assert_eq!(loose.left_out, 0);
        assert_eq!(fit_region_robust(&scan, &all, Kind::Plane, 0.0), Err(FitError::NoFit));
        assert_eq!(fit_region_robust(&scan, &[], Kind::Plane, 0.1), Err(FitError::NotOnTheScan));
    }

    /// A cylinder of radius 10 about Z from z 0 to 40, `rings` × `around` points, `sweep` radians
    /// of it, its points nudged by a repeatable `noise` along the radius.
    fn cylinder(rings: u32, around: u32, sweep: f64, noise: f64) -> Vec<Vec3> {
        let mut out = Vec::new();
        for j in 0..rings {
            for i in 0..around {
                let (z, a) = (40.0 * f64::from(j) / f64::from(rings - 1), sweep * f64::from(i) / f64::from(around));
                let r = 10.0 + noise * (((i * 7 + j * 13) % 11) as f64 / 5.0 - 1.0);
                out.push(Vec3::new(r * a.cos(), r * a.sin(), z));
            }
        }
        out
    }

    fn plane(n: u32, noise: f64) -> Vec<Vec3> {
        (0..n * n)
            .map(|k| {
                let (i, j) = (k % n, k / n);
                Vec3::new(f64::from(i) * 2.0, f64::from(j) * 2.0, 5.0 + noise * (((i * 5 + j * 3) % 7) as f64 / 3.0 - 1.0))
            })
            .collect()
    }

    #[test]
    fn a_plane_of_points_is_fitted_with_its_form_error() {
        let points = plane(20, 0.02);
        let fit = fit_points(&points, None, Kind::Plane, None).unwrap();
        let Shape::Plane { normal, point } = fit.shape else { panic!("{:?}", fit.shape) };
        assert!(normal.dot(Vec3::Z).abs() > 0.9999, "{normal:?}");
        assert!((point.z - 5.0).abs() < 0.01, "{point:?}");
        assert_eq!(fit.arc, None);
        assert!(fit.rms < 0.02 && fit.form_error > 0.03 && fit.form_error < 0.05, "{fit:?}");
        assert_eq!(fit.inliers, 400);
    }

    #[test]
    fn a_cylinder_of_points_is_fitted_with_the_normals_read_from_its_neighbours() {
        let points = cylinder(30, 90, 2.0 * std::f64::consts::PI, 0.0);
        let fit = fit_points(&points, None, Kind::Cylinder, None).unwrap();
        let Shape::Cylinder { axis, radius, .. } = fit.shape else { panic!("{:?}", fit.shape) };
        assert!((radius - 10.0).abs() < 0.05, "{radius}");
        assert!(axis.dot(Vec3::Z).abs() > 0.999, "{axis:?}");
        assert!((fit.half_length - 20.0).abs() < 0.5, "{}", fit.half_length);
        assert!(fit.arc.unwrap().to_degrees() > 350.0, "{:?}", fit.arc);
    }

    #[test]
    fn a_short_arc_of_points_says_so() {
        let points = cylinder(30, 40, 60f64.to_radians(), 0.0);
        let fit = fit_points(&points, None, Kind::Cylinder, None).unwrap();
        let arc = fit.arc.unwrap().to_degrees();
        assert!((arc - 60.0).abs() < 4.0, "{arc}");
    }

    #[test]
    fn a_plane_is_not_a_cylinder() {
        assert_eq!(fit_points(&plane(20, 0.0), None, Kind::Cylinder, None), Err(FitError::NoFit));
    }

    #[test]
    fn too_few_points_are_no_fit() {
        let points = [Vec3::ZERO, Vec3::X, Vec3::Y];
        assert_eq!(fit_points(&points, None, Kind::Plane, None), Err(FitError::NoFit));
        assert_eq!(fit_points(&[], None, Kind::Plane, Some(0.1)), Err(FitError::NoFit));
        let stray = [Vec3::new(f64::NAN, 0.0, 0.0); 20];
        assert_eq!(fit_points(&stray, None, Kind::Plane, None), Err(FitError::NoFit));
    }

    /// A plane with a third as many points again lying on a table 8 mm under it: fitted as it is
    /// the plane lies between them; with a tolerance it lies on the part.
    #[test]
    fn a_tolerance_leaves_stray_points_out() {
        let mut points = plane(30, 0.0);
        let table: Vec<Vec3> = points.iter().step_by(3).map(|p| Vec3::new(p.x, p.y, p.z - 8.0)).collect();
        points.extend(table);
        let loose = fit_points(&points, None, Kind::Plane, None).unwrap();
        let Shape::Plane { point, .. } = loose.shape else { panic!() };
        assert!(point.z < 4.0, "pulled down: {}", point.z);
        let firm = fit_points(&points, None, Kind::Plane, Some(0.1)).unwrap();
        let Shape::Plane { point, normal } = firm.shape else { panic!() };
        assert!((point.z - 5.0).abs() < 0.01 && normal.dot(Vec3::Z).abs() > 0.9999, "{point:?} {normal:?}");
        assert_eq!(firm.inliers, 900);
        assert_eq!(firm.points, 1200);
        assert!(firm.rms < 1e-6, "{}", firm.rms);
    }

    #[test]
    fn stray_points_do_not_move_a_cylinder_with_a_tolerance() {
        let mut points = cylinder(30, 90, 2.0 * std::f64::consts::PI, 0.01);
        points.extend((0..120).map(|k| Vec3::new(f64::from(k) * 0.4 - 24.0, 18.0, f64::from(k % 40))));
        let fit = fit_points(&points, None, Kind::Cylinder, Some(0.1)).unwrap();
        let Shape::Cylinder { radius, .. } = fit.shape else { panic!("{:?}", fit.shape) };
        assert!((radius - 10.0).abs() < 0.05, "{radius}");
        assert!(fit.inliers >= 2700 && fit.inliers < 2800, "{}", fit.inliers);
    }

    #[test]
    fn stated_normals_are_used() {
        let points = cylinder(30, 90, 2.0 * std::f64::consts::PI, 0.0);
        let normals: Vec<Vec3> = points.iter().map(|p| Vec3::new(p.x, p.y, 0.0).normalized().unwrap()).collect();
        let fit = fit_points(&points, Some(&normals), Kind::Cylinder, None).unwrap();
        let Shape::Cylinder { radius, .. } = fit.shape else { panic!() };
        assert!((radius - 10.0).abs() < 0.01, "{radius}");
    }
}
