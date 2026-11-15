//! A smooth surface fitted to the part of a scan round a picked point: a bicubic B-spline whose
//! points start at their places over the region's own plane, fitted in all three directions by
//! least squares with a smoothness term (the bending of its net) that holds the surface where the
//! scan has no points and keeps it from following the noise, each point's place then moved to its
//! foot on the surface and the net fitted again ([`crate::net_fit`]), so a region that slopes or
//! curves away from its plane is not fitted as heights over it.
//!
//! A region that wraps round (faces more than one way across its plane) cannot be one height
//! field and is refused.

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::base::{EdgeNeighbours, edge_neighbours};
use crate::{Scan, thinned};

/// The most scan points a surface is fitted to; a larger region is sampled evenly.
const POINT_LIMIT: usize = 40_000;

/// Degree in each direction.
const DEGREE: usize = 3;

/// A surface fitted to a region of the scan, and how well it fits.
#[derive(Clone, Debug)]
pub struct SurfaceFit {
    pub surface: NurbsSurface,
    /// The region's triangles.
    pub region: Vec<usize>,
    /// The region's points' distance from the surface at their feet on it: root mean square and
    /// largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// How many points it was fitted to.
    pub points: usize,
    /// Where the surface is wrong: its folds, pinches and crossings ([`crate::surface_check`]).
    pub trouble: [usize; 3],
}

/// Why no surface could be fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    NotOnTheScan,
    /// Too few points for the control points asked for.
    TooFew,
    /// The region faces more than one way across its plane (it wraps round an edge or a tube).
    Wraps,
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SurfaceError::NotOnTheScan => "pick a point on the scan, in the middle of the area to fit",
            SurfaceError::TooFew => "too few scan points there for that many control points: pick a larger radius or fewer points",
            SurfaceError::Wraps => "the area wraps round (it faces more than one way): pick a smaller radius, or an area that faces one way",
        })
    }
}

/// Fits a surface to the connected part of the scan within `radius` of triangle `seed`'s middle
/// that faces about the way the surface does there (it stops where the surface turns away),
/// with `controls` × `controls` control points (4 to 30). `smoothing` (0 to 1) weighs how smooth
/// the surface is against how closely it follows the points.
pub fn fit_surface(scan: &Scan, seed: usize, radius: f64, controls: usize, smoothing: f64) -> Result<SurfaceFit, SurfaceError> {
    let [a, b, c] = scan.corners(seed).ok_or(SurfaceError::NotOnTheScan)?;
    let pick = (a + b + c) / 3.0;
    let seed_normal = (b - a).cross(c - a).normalized().ok_or(SurfaceError::NotOnTheScan)?;
    let n = controls.clamp(DEGREE + 1, 30);
    let neighbours = edge_neighbours(scan);
    // The way the surface faces round the pick: the triangles a tenth of the radius round it,
    // area-weighted, so one noisy triangle does not set it.
    let near = within(scan, &neighbours, seed, pick, radius / 10.0, None);
    let around = near.iter().filter_map(|&t| scan.corners(t)).fold(Vec3::ZERO, |sum, [a, b, c]| sum + (b - a).cross(c - a));
    let facing = around.normalized().unwrap_or(seed_normal);
    let region = within(scan, &neighbours, seed, pick, radius, Some(facing));

    // The region's points (each position once) and its facing: each triangle's normal turned to
    // agree with the seed's, weighted by area.
    let mut seen = vec![false; scan.positions.len()];
    let mut points = Vec::new();
    let mut facing = Vec::with_capacity(region.len());
    for &t in &region {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let cross = (b - a).cross(c - a);
        let (area, normal) = (cross.len() / 2.0, cross.normalized());
        if let Some(nrm) = normal {
            facing.push((if nrm.dot(seed_normal) < 0.0 { -nrm } else { nrm }, area));
        }
        for &v in scan.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), scan.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                points.push(*p);
            }
        }
    }
    let points = thinned(&points, POINT_LIMIT);
    if points.len() < n * n {
        return Err(SurfaceError::TooFew);
    }

    // The region's plane: through its middle, square to the way its points spread least, facing
    // the way its surface does.
    let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in &points {
        let d = *p - middle;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0).ok_or(SurfaceError::TooFew)?;
    let mut normal = axes[0].normalized().ok_or(SurfaceError::TooFew)?;
    let mean_facing = facing.iter().fold(Vec3::ZERO, |s, (f, w)| s + *f * *w);
    if normal.dot(mean_facing) < 0.0 {
        normal = -normal;
    }
    let e1 = axes[2].normalized().ok_or(SurfaceError::TooFew)?;
    let e2 = normal.cross(e1);
    // More than a twentieth of it facing across or away from the plane: it wraps round.
    let total: f64 = facing.iter().map(|(_, w)| w).sum();
    let across: f64 = facing.iter().filter(|(f, _)| f.dot(normal) < 0.2).map(|(_, w)| w).sum();
    if total > 0.0 && across > 0.05 * total {
        return Err(SurfaceError::Wraps);
    }

    // Each point in the plane's frame, (u, v) on the plane and w its height.
    let frame: Vec<[f64; 3]> = points
        .iter()
        .map(|p| {
            let d = *p - middle;
            [d.dot(e1), d.dot(e2), d.dot(normal)]
        })
        .collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for [u, v, _] in &frame {
        lo = [lo[0].min(*u), lo[1].min(*v)];
        hi = [hi[0].max(*u), hi[1].max(*v)];
    }
    let pad = [(hi[0] - lo[0]) * 0.01, (hi[1] - lo[1]) * 0.01];
    let (lo, size) = ([lo[0] - pad[0], lo[1] - pad[1]], [hi[0] - lo[0] + 2.0 * pad[0], hi[1] - lo[1] + 2.0 * pad[1]]);
    if !(size[0] > 0.0 && size[1] > 0.0) {
        return Err(SurfaceError::TooFew);
    }
    // Each point's starting place on the square: where it lies over the plane.
    let samples: Vec<(Vec2, Vec3)> =
        frame.iter().zip(&points).map(|([u, v, _], p)| (Vec2::new((u - lo[0]) / size[0], (v - lo[1]) / size[1]), *p)).collect();
    // Even with no smoothing asked for, a little holds the corners the scan does not reach.
    let weight = crate::patches::smoothing_weight(smoothing);
    let fitted = crate::net_fit::fit(&samples, None, [n, n], weight, crate::net_fit::ROUNDS).ok_or(SurfaceError::TooFew)?;
    let trouble = crate::surface_check::trouble(&fitted.surface);
    Ok(SurfaceFit { surface: fitted.surface, region, rms: fitted.rms, max: fitted.max, points: samples.len(), trouble })
}

/// The control points each way [`fit_surface_close`] tries, fewest first.
const CLOSE_STEPS: &[usize] = &[6, 8, 11, 15, 20, 25, 30];

/// [`fit_surface`] with as many control points as bring it within `aim` (root mean square, mm) of
/// the scan: from 6 each way up to 30, stopping once it is that close or where more points no
/// longer cut the distance by a quarter (the scan's noise is reached). The fit and its control
/// points each way.
pub fn fit_surface_close(scan: &Scan, seed: usize, radius: f64, smoothing: f64, aim: f64) -> Result<(SurfaceFit, usize), SurfaceError> {
    let mut best: Option<(SurfaceFit, usize)> = None;
    for &n in CLOSE_STEPS {
        let fit = match fit_surface(scan, seed, radius, n, smoothing) {
            Ok(fit) => fit,
            // Too few points for more control points: the last that fitted is the closest.
            Err(SurfaceError::TooFew) if best.is_some() => break,
            Err(e) => return Err(e),
        };
        if best.as_ref().is_some_and(|(b, _)| fit.rms >= 0.75 * b.rms) {
            break;
        }
        let close = fit.rms <= aim;
        best = Some((fit, n));
        if close {
            break;
        }
    }
    best.ok_or(SurfaceError::TooFew)
}

/// The cosine of the most a triangle may face away from the way the surface faces at the pick and
/// still be grown into: past it the surface has turned (round an edge, onto a plate's far side).
const MOST_TURN: f64 = 0.34; // about 70°

/// The connected triangles from `seed` whose middle is within `radius` of `pick`, and, with
/// `facing`, that face within about 70° of it: the area stops where the surface turns away, so on a
/// thin plate it keeps to the face picked instead of running round the edge onto the other.
fn within(scan: &Scan, neighbours: &EdgeNeighbours, seed: usize, pick: Vec3, radius: f64, facing: Option<Vec3>) -> Vec<usize> {
    let mut seen = vec![false; scan.triangle_count()];
    let mut found = Vec::new();
    if let Some(s) = seen.get_mut(seed) {
        *s = true;
        found.push(seed);
    }
    let mut head = 0;
    while let Some(&t) = found.get(head) {
        head += 1;
        for nb in neighbours.of(scan, t) {
            if seen.get(nb).is_some_and(|s| !s) {
                seen[nb] = true;
                let faces = |[a, b, c]: [Vec3; 3]| facing.is_none_or(|f| (b - a).cross(c - a).normalized().is_some_and(|n| n.dot(f) > MOST_TURN));
                if scan.corners(nb).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(pick) <= radius && faces([a, b, c])) {
                    found.push(nb);
                }
            }
        }
    }
    found
}

/// A clamped, uniform cubic knot vector on 0..1 for `n` control points.
pub(crate) fn clamped_knots(n: usize) -> Vec<f64> {
    let inner = n - DEGREE;
    let mut knots = vec![0.0; DEGREE + 1];
    knots.extend((1..inner).map(|i| i as f64 / inner as f64));
    knots.extend(std::iter::repeat_n(1.0, DEGREE + 1));
    knots
}

/// The symmetric positive definite system `m x = rhs` (`k` × `k`, row by row) by Cholesky; `None`
/// when it is not positive definite.
pub(crate) fn cholesky_solve(m: &mut [f64], rhs: &[f64], k: usize) -> Option<Vec<f64>> {
    if !cholesky_factor(m, k) {
        return None;
    }
    cholesky_back(m, rhs, k)
}

/// `m` (`k` × `k`, row by row, symmetric) factored in place into `L Lᵀ`, its lower triangle `L`;
/// false when it is not positive definite. One factoring answers any number of right-hand sides
/// ([`cholesky_back`]).
pub(crate) fn cholesky_factor(m: &mut [f64], k: usize) -> bool {
    if m.len() < k * k {
        return false;
    }
    for j in 0..k {
        let mut d = m[j * k + j];
        for p in 0..j {
            d -= m[j * k + p] * m[j * k + p];
        }
        if !(d > 0.0) {
            return false;
        }
        let d = d.sqrt();
        m[j * k + j] = d;
        for i in j + 1..k {
            let mut s = m[i * k + j];
            for p in 0..j {
                s -= m[i * k + p] * m[j * k + p];
            }
            m[i * k + j] = s / d;
        }
    }
    true
}

/// `x` with `L Lᵀ x = rhs`, `l` as [`cholesky_factor`] left it; `None` when the answer is not finite.
pub(crate) fn cholesky_back(l: &[f64], rhs: &[f64], k: usize) -> Option<Vec<f64>> {
    if l.len() < k * k || rhs.len() < k {
        return None;
    }
    let mut y = vec![0.0; k];
    for i in 0..k {
        let s: f64 = (0..i).map(|p| l[i * k + p] * y[p]).sum();
        y[i] = (rhs[i] - s) / l[i * k + i];
    }
    let mut x = vec![0.0; k];
    for i in (0..k).rev() {
        let s: f64 = (i + 1..k).map(|p| l[p * k + i] * x[p]).sum();
        x[i] = (y[i] - s) / l[i * k + i];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::nearest_triangle;
    use crate::test_meshes::turned;

    /// A grid mesh of the height field `z = f(x, y)` over `size` × `size`, `n` squares a side.
    fn field(size: f64, n: u32, f: impl Fn(f64, f64) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (size * f64::from(i) / f64::from(n), size * f64::from(j) / f64::from(n));
                positions.push(Vec3::new(x, y, f(x, y)));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        (positions, triangles)
    }

    /// A gently curved dome, turned and moved anywhere: the surface follows it within a hundredth
    /// of a millimetre, and its control net lies over the region.
    #[test]
    fn a_smooth_dome_is_fitted_closely() {
        let (positions, triangles) = field(60.0, 60, |x, y| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2)));
        let axis = Vec3::new(0.3, 0.8, 0.2).normalized().unwrap();
        let moved = turned(&positions, axis, 0.6, Vec3::new(10.0, -40.0, 25.0));
        let scan = Scan::new(&moved, &triangles);
        let seed = nearest_triangle(&scan, moved[(30 * 61 + 30) as usize]).unwrap();
        let fit = fit_surface(&scan, seed, 100.0, 8, 0.0).unwrap();
        assert!(fit.rms < 0.01 && fit.max < 0.05, "rms {} max {}", fit.rms, fit.max);
        assert_eq!(fit.surface.count(), [8, 8]);
        // A point on the dome's top is on the surface.
        let [[u0, u1], [v0, v1]] = fit.surface.domain();
        let mid = fit.surface.eval((u0 + u1) / 2.0, (v0 + v1) / 2.0).unwrap();
        let top = moved[(30 * 61 + 30) as usize];
        assert!(mid.dist(top) < 2.0, "the middle of the surface is over the middle of the dome: {mid:?} {top:?}");
    }

    /// Fitted closely, a wavy sheet takes more control points than a plain one, and stops at the
    /// first count that comes within the aim.
    #[test]
    fn close_fitting_adds_control_points_until_within_the_aim() {
        let (positions, triangles) = field(60.0, 60, |x, y| 3.0 * (x / 9.0).sin() * (y / 11.0).cos());
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(30.0, 30.0, 0.0)).unwrap();
        let six = fit_surface(&scan, seed, 100.0, 6, 0.0).unwrap();
        let (close, n) = fit_surface_close(&scan, seed, 100.0, 0.0, 0.02).unwrap();
        assert!(n > 6 && close.rms <= 0.02 && close.rms < six.rms, "{n}: {} against {}", close.rms, six.rms);
        assert_eq!(close.surface.count(), [n, n]);
        let (_, plain) = fit_surface_close(&scan, seed, 100.0, 0.0, 10.0).unwrap();
        assert_eq!(plain, 6, "already within a loose aim");
    }

    /// Smoothing trades closeness for smoothness: with noise, a smoother surface is farther from the
    /// points but never wild.
    #[test]
    fn smoothing_holds_the_surface_against_noise() {
        let noisy = |x: f64, y: f64| 0.002 * (x - 20.0) * (y - 20.0) + 0.05 * ((x * 7.3).sin() * (y * 5.1).cos());
        let (positions, triangles) = field(40.0, 50, noisy);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(20.0, 20.0, 0.0)).unwrap();
        let close = fit_surface(&scan, seed, 100.0, 12, 0.0).unwrap();
        let smooth = fit_surface(&scan, seed, 100.0, 12, 1.0).unwrap();
        assert!(close.rms < smooth.rms && smooth.max < 0.5, "{} {} {}", close.rms, smooth.rms, smooth.max);
    }

    /// On a thin plate the area grown from a pick on its top keeps to the top, however large the
    /// radius: it does not run round the edge onto the underside.
    #[test]
    fn on_a_thin_plate_the_area_keeps_to_the_face_picked() {
        let (positions, triangles) = crate::test_meshes::fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 40.0, 2.0), 20);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(30.0, 20.0, 2.0)).unwrap();
        let fit = fit_surface(&scan, seed, 100.0, 8, 0.1).unwrap();
        assert!(fit.rms < 1e-6 && fit.max < 1e-6, "the flat top: {} {}", fit.rms, fit.max);
        let [[u0, u1], [v0, v1]] = fit.surface.domain();
        assert!((fit.surface.eval((u0 + u1) / 2.0, (v0 + v1) / 2.0).unwrap().z - 2.0).abs() < 1e-6, "on the top");
    }

    /// On a tube the area keeps to the side facing the pick instead of wrapping round it.
    #[test]
    fn on_a_tube_the_area_keeps_to_one_side() {
        let (sides, rows) = (48u32, 10u32);
        let mut positions = Vec::new();
        for k in 0..sides {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
            for j in 0..=rows {
                positions.push(Vec3::new(10.0 * a.cos(), 10.0 * a.sin(), 3.0 * f64::from(j)));
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
        // The area keeps to the side of the tube facing the pick, which one surface can be.
        let fit = fit_surface(&scan, 0, 100.0, 6, 0.1).unwrap();
        let [a, b, c] = scan.corners(0).unwrap();
        let facing = (b - a).cross(c - a).normalized().unwrap();
        assert!(
            fit.region.iter().all(|&t| scan.corners(t).is_some_and(|[a, b, c]| (b - a).cross(c - a).normalized().unwrap().dot(facing) > 0.2)),
            "one side"
        );
        assert!(fit.region.len() < triangles.len() / 2 && fit.rms < 0.2, "{} of {}, rms {}", fit.region.len(), triangles.len(), fit.rms);
        assert_eq!(fit_surface(&scan, 99_999, 100.0, 6, 0.1).unwrap_err(), SurfaceError::NotOnTheScan);
    }
}
