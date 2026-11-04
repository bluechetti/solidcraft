//! A smooth surface fitted to the part of a scan round a picked point: a bicubic B-spline over the
//! region's own plane, its control points on an even grid in that plane and their heights fitted
//! by least squares, with a smoothness term (second differences of the heights) that holds the
//! surface where the scan has no points and keeps it from following the noise.
//!
//! A region that wraps round (faces more than one way across its plane) cannot be one height
//! field and is refused.

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{NurbsSurface, basis_derivatives};
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
    /// The region's points' distance from the surface along its plane's normal: root mean square
    /// and largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// How many points it was fitted to.
    pub points: usize,
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

/// Fits a surface to the connected part of the scan within `radius` of triangle `seed`'s middle,
/// with `controls` × `controls` control points (4 to 30). `smoothing` (0 to 1) weighs how smooth
/// the surface is against how closely it follows the points.
pub fn fit_surface(scan: &Scan, seed: usize, radius: f64, controls: usize, smoothing: f64) -> Result<SurfaceFit, SurfaceError> {
    let [a, b, c] = scan.corners(seed).ok_or(SurfaceError::NotOnTheScan)?;
    let pick = (a + b + c) / 3.0;
    let seed_normal = (b - a).cross(c - a).normalized().ok_or(SurfaceError::NotOnTheScan)?;
    let n = controls.clamp(DEGREE + 1, 30);
    let neighbours = edge_neighbours(scan);
    let region = within(scan, &neighbours, seed, pick, radius);

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
    let knots = clamped_knots(n);

    // Least squares for the heights: Σ (Σ Nᵢ(s) Nⱼ(t) hᵢⱼ − w)², plus the smoothness term.
    let k = n * n;
    let mut m = vec![0.0f64; k * k];
    let mut rhs = vec![0.0f64; k];
    let mut rows: Vec<(f64, [f64; 16], [usize; 16])> = Vec::with_capacity(frame.len());
    for [u, v, w] in &frame {
        let (s, t) = (((u - lo[0]) / size[0]).clamp(0.0, 1.0), ((v - lo[1]) / size[1]).clamp(0.0, 1.0));
        let Some((cols, vals)) = basis_row(&knots, n, s, t) else { continue };
        for a in 0..16 {
            rhs[cols[a]] += vals[a] * w;
            for b in 0..16 {
                m[cols[a] * k + cols[b]] += vals[a] * vals[b];
            }
        }
        rows.push((*w, vals, cols));
    }
    let trace: f64 = (0..k).map(|i| m[i * k + i]).sum::<f64>() / k as f64;
    // Even with no smoothing asked for, a little holds the corners the scan does not reach.
    let lambda = trace * (1e-4 + smoothing.clamp(0.0, 1.0) * 0.5);
    let id = |i: usize, j: usize| j * n + i;
    let mut add_term = |terms: &[(usize, f64)]| {
        for &(a, wa) in terms {
            for &(b, wb) in terms {
                m[a * k + b] += lambda * wa * wb;
            }
        }
    };
    for j in 0..n {
        for i in 1..n - 1 {
            add_term(&[(id(i - 1, j), 1.0), (id(i, j), -2.0), (id(i + 1, j), 1.0)]);
            add_term(&[(id(j, i - 1), 1.0), (id(j, i), -2.0), (id(j, i + 1), 1.0)]);
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            add_term(&[(id(i, j), 1.0), (id(i + 1, j), -1.0), (id(i, j + 1), -1.0), (id(i + 1, j + 1), 1.0)]);
        }
    }
    let heights = cholesky_solve(&mut m, &rhs, k).ok_or(SurfaceError::TooFew)?;

    // How far the points are from it, along the normal.
    let (mut squares, mut max) = (0.0f64, 0.0f64);
    for (w, vals, cols) in &rows {
        let fitted: f64 = (0..16).map(|a| vals[a] * heights[cols[a]]).sum();
        let d = w - fitted;
        squares += d * d;
        max = max.max(d.abs());
    }
    let rms = (squares / rows.len().max(1) as f64).sqrt();

    // Control points: on the grid of the Greville points (so the plane's u and v are reproduced
    // exactly), at their heights.
    let greville: Vec<f64> = (0..n).map(|i| (knots[i + 1] + knots[i + 2] + knots[i + 3]) / 3.0).collect();
    let net: Vec<Vec3> = (0..k)
        .map(|idx| {
            let (i, j) = (idx % n, idx / n);
            middle + e1 * (lo[0] + greville[i] * size[0]) + e2 * (lo[1] + greville[j] * size[1]) + normal * heights[idx]
        })
        .collect();
    let surface = NurbsSurface::new([DEGREE, DEGREE], [knots.clone(), knots], &net, &vec![1.0; k]).map_err(|_| SurfaceError::TooFew)?;
    Ok(SurfaceFit { surface, region, rms, max, points: rows.len() })
}

/// The connected triangles from `seed` whose middle is within `radius` of `pick`.
fn within(scan: &Scan, neighbours: &EdgeNeighbours, seed: usize, pick: Vec3, radius: f64) -> Vec<usize> {
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
                if scan.corners(nb).is_some_and(|[a, b, c]| ((a + b + c) / 3.0).dist(pick) <= radius) {
                    found.push(nb);
                }
            }
        }
    }
    found
}

/// A clamped, uniform cubic knot vector on 0..1 for `n` control points.
fn clamped_knots(n: usize) -> Vec<f64> {
    let inner = n - DEGREE;
    let mut knots = vec![0.0; DEGREE + 1];
    knots.extend((1..inner).map(|i| i as f64 / inner as f64));
    knots.extend(std::iter::repeat_n(1.0, DEGREE + 1));
    knots
}

/// The 16 control points a point (s, t) depends on and their weights Nᵢ(s) Nⱼ(t).
fn basis_row(knots: &[f64], n: usize, s: f64, t: f64) -> Option<([usize; 16], [f64; 16])> {
    // The index of the first function alive at the parameter, and each one's value (element 0).
    let (fu, bu) = basis_derivatives(knots, DEGREE, n, s, 0)?;
    let (fv, bv) = basis_derivatives(knots, DEGREE, n, t, 0)?;
    let (mut cols, mut vals) = ([0usize; 16], [0.0f64; 16]);
    for a in 0..=DEGREE {
        for b in 0..=DEGREE {
            cols[b * 4 + a] = (fv + b) * n + fu + a;
            vals[b * 4 + a] = bu.get(a)?.first()? * bv.get(b)?.first()?;
        }
    }
    Some((cols, vals))
}

/// The symmetric positive definite system `m x = rhs` (`k` × `k`, row by row) by Cholesky; `None`
/// when it is not positive definite.
pub(crate) fn cholesky_solve(m: &mut [f64], rhs: &[f64], k: usize) -> Option<Vec<f64>> {
    for j in 0..k {
        let mut d = m[j * k + j];
        for p in 0..j {
            d -= m[j * k + p] * m[j * k + p];
        }
        if !(d > 0.0) {
            return None;
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
    let mut y = vec![0.0; k];
    for i in 0..k {
        let s: f64 = (0..i).map(|p| m[i * k + p] * y[p]).sum();
        y[i] = (rhs[i] - s) / m[i * k + i];
    }
    let mut x = vec![0.0; k];
    for i in (0..k).rev() {
        let s: f64 = (i + 1..k).map(|p| m[p * k + i] * x[p]).sum();
        x[i] = (y[i] - s) / m[i * k + i];
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

    /// A tube wraps round: no one surface over a plane can be it.
    #[test]
    fn an_area_that_wraps_round_is_refused() {
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
        assert_eq!(fit_surface(&scan, 0, 100.0, 6, 0.1).unwrap_err(), SurfaceError::Wraps);
        assert_eq!(fit_surface(&scan, 99_999, 100.0, 6, 0.1).unwrap_err(), SurfaceError::NotOnTheScan);
    }
}
