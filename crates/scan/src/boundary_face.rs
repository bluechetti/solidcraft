//! Fit from a boundary curve: a Free Form face whose four sides follow four curves drawn on the
//! scan (Curve on Scan curves), and whose inside is fitted to the scan.
//!
//! [`order`] finds the closed loop the four curves make and turns it into a [`Boundary`]: side 0
//! runs from corner A to B, side 1 from B to C, side 2 from C to D and side 3 from D back to A.
//! [`face`] makes the bilinearly blended (Coons) patch over those sides, as a cubic clamped
//! B-spline net like every other Free Form face, with the outer ring of control points the least
//! squares cubic of each side, then moves the inside's points to the scan with
//! [`crate::surface_edit::refit`], the ring held.
//!
//! Orientation: u runs along sides 0 and 2 (A to B, and D to C), v along sides 3 and 1 (A to D,
//! and B to C). The surface normal `S_u x S_v` is therefore the right-handed normal of the loop
//! A, B, C, D (it points toward a viewer who sees the loop run counter-clockwise). Reverse the
//! order of the curves, or draw them the other way round, to flip it.

use std::fmt;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{NurbsSurface, basis_derivatives};
use solidcraft_geom::prelude::*;

use crate::free_form::net_surface;
use crate::surface::{cholesky_solve, clamped_knots};
use crate::surface_edit;

/// The points of a face's net each way, at least and at most.
pub const LEAST_COUNT: usize = 4;
pub const MOST_COUNT: usize = 24;

/// The default gap, as a fraction of the curves' size, when none is given.
const RELATIVE_GAP: f64 = 0.02;
/// The most scan points a fit looks at (the rest are thinned evenly).
const MOST_SCAN: usize = 20_000;
/// Points per side when a side of a face is read as a polyline.
const SIDE_SAMPLES: usize = 65;

/// Why a boundary or a face could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryError {
    /// There were not four curves (the number given).
    NotFourCurves(usize),
    /// A curve has fewer than two points.
    TooShort,
    /// A point is not a finite number.
    NotFinite,
    /// The curves do not close into one loop.
    OpenLoop,
    /// A corner has more than two curves meeting at it.
    TooManyAtCorner,
    /// A side has no length (all its points are one point).
    DegenerateSide,
    /// The net's size is outside `LEAST_COUNT..=MOST_COUNT`.
    BadCount,
    /// The surface could not be built.
    FitFailed,
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BoundaryError::NotFourCurves(n) => write!(f, "a boundary needs four curves, not {n}"),
            BoundaryError::TooShort => write!(f, "a curve needs at least two points"),
            BoundaryError::NotFinite => write!(f, "a curve has a point that is not a number"),
            BoundaryError::OpenLoop => write!(f, "the four curves do not close into a loop"),
            BoundaryError::TooManyAtCorner => write!(f, "more than two curves meet at one corner"),
            BoundaryError::DegenerateSide => write!(f, "a side of the boundary has no length"),
            BoundaryError::BadCount => write!(f, "the face needs {LEAST_COUNT} to {MOST_COUNT} points each way"),
            BoundaryError::FitFailed => write!(f, "the face could not be fitted"),
        }
    }
}

impl std::error::Error for BoundaryError {}

/// Four curves round a face, in order: side 0 runs A to B, side 1 B to C, side 2 C to D, side 3
/// D to A, each end of a side the same point as the start of the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Boundary {
    pub sides: [Vec<Vec3>; 4],
}

fn first(c: &[Vec3]) -> Option<Vec3> {
    c.first().copied()
}

fn last(c: &[Vec3]) -> Option<Vec3> {
    c.last().copied()
}

/// The four `curves`, in any order and direction, as a closed loop. Curve ends within `gap` of
/// each other meet (`gap <= 0`: 2 % of the curves' bounding box); the shared corners are put at
/// the middle of the ends that meet there.
pub fn order(curves: &[Vec<Vec3>], gap: f64) -> Result<Boundary, BoundaryError> {
    if curves.len() != 4 {
        return Err(BoundaryError::NotFourCurves(curves.len()));
    }
    if curves.iter().any(|c| c.len() < 2) {
        return Err(BoundaryError::TooShort);
    }
    if curves.iter().flatten().any(|p| !p.is_finite()) {
        return Err(BoundaryError::NotFinite);
    }
    let gap = if gap.is_finite() && gap > 0.0 {
        gap
    } else {
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for p in curves.iter().flatten() {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        (RELATIVE_GAP * hi.dist(lo)).max(1e-9)
    };
    // The ends of each curve: (curve, at its end?, point).
    let end = |c: usize, at_end: bool| -> Option<Vec3> {
        let curve = curves.get(c)?;
        if at_end { last(curve) } else { first(curve) }
    };
    // No end has two other curves near it.
    for c in 0..4 {
        for at_end in [false, true] {
            let Some(p) = end(c, at_end) else { return Err(BoundaryError::TooShort) };
            let near = (0..4).filter(|&o| o != c && [false, true].iter().any(|&e| end(o, e).is_some_and(|q| q.dist(p) <= gap))).count();
            if near > 1 {
                return Err(BoundaryError::TooManyAtCorner);
            }
        }
    }
    // Walk the loop from curve 0, forward.
    let mut used = [false; 4];
    let mut chain: Vec<(usize, bool)> = vec![(0, false)];
    used[0] = true;
    for _ in 1..4 {
        let &(c, reversed) = chain.last().ok_or(BoundaryError::OpenLoop)?;
        let tail = end(c, !reversed).ok_or(BoundaryError::OpenLoop)?;
        let mut best: Option<(f64, usize, bool)> = None;
        for o in 0..4 {
            if used.get(o).copied().unwrap_or(true) {
                continue;
            }
            for rev in [false, true] {
                let Some(q) = end(o, rev) else { continue };
                let d = q.dist(tail);
                if d <= gap && best.is_none_or(|b| d < b.0) {
                    best = Some((d, o, rev));
                }
            }
        }
        let (_, o, rev) = best.ok_or(BoundaryError::OpenLoop)?;
        // A curve that is reversed starts at its own end.
        if let Some(u) = used.get_mut(o) {
            *u = true;
        }
        chain.push((o, rev));
    }
    let &(c0, r0) = chain.first().ok_or(BoundaryError::OpenLoop)?;
    let &(cl, rl) = chain.last().ok_or(BoundaryError::OpenLoop)?;
    let (head, tail) = (end(c0, r0).ok_or(BoundaryError::OpenLoop)?, end(cl, !rl).ok_or(BoundaryError::OpenLoop)?);
    if head.dist(tail) > gap {
        return Err(BoundaryError::OpenLoop);
    }
    let mut sides: Vec<Vec<Vec3>> = Vec::with_capacity(4);
    for &(c, rev) in &chain {
        let mut side = curves.get(c).cloned().ok_or(BoundaryError::OpenLoop)?;
        if rev {
            side.reverse();
        }
        sides.push(side);
    }
    // Snap each corner: the middle of the end of one side and the start of the next.
    for k in 0..4 {
        let next = (k + 1) % 4;
        let (a, b) = match (sides.get(k).and_then(|s| last(s)), sides.get(next).and_then(|s| first(s))) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err(BoundaryError::OpenLoop),
        };
        let mid = (a + b) * 0.5;
        if let Some(p) = sides.get_mut(k).and_then(|s| s.last_mut()) {
            *p = mid;
        }
        if let Some(p) = sides.get_mut(next).and_then(|s| s.first_mut()) {
            *p = mid;
        }
    }
    let [a, b, c, d]: [Vec<Vec3>; 4] = sides.try_into().map_err(|_| BoundaryError::OpenLoop)?;
    Ok(Boundary { sides: [a, b, c, d] })
}

/// The length of a polyline.
fn length(line: &[Vec3]) -> f64 {
    line.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// The point at arc length `s` along `line` (clamped to its ends), with its cumulative lengths.
fn at_length(line: &[Vec3], cumulative: &[f64], s: f64) -> Option<Vec3> {
    let n = line.len();
    let i = cumulative.partition_point(|c| *c < s).clamp(1, n.checked_sub(1)?);
    let (a, b) = (*line.get(i - 1)?, *line.get(i)?);
    let (s0, s1) = (*cumulative.get(i - 1)?, *cumulative.get(i)?);
    let t = if s1 > s0 { ((s - s0) / (s1 - s0)).clamp(0.0, 1.0) } else { 0.0 };
    Some(a + (b - a) * t)
}

/// `line` resampled to `n` points evenly along its length.
fn resample(line: &[Vec3], n: usize) -> Option<Vec<Vec3>> {
    let mut cumulative = Vec::with_capacity(line.len());
    let mut run = 0.0;
    for (i, p) in line.iter().enumerate() {
        if i > 0 {
            run += line.get(i - 1)?.dist(*p);
        }
        cumulative.push(run);
    }
    let total = run;
    if !(total > 0.0) || !total.is_finite() || n < 2 {
        return None;
    }
    (0..n).map(|k| at_length(line, &cumulative, total * k as f64 / (n - 1) as f64)).collect()
}

/// The `n` control points of a clamped cubic through the ends of `line` and, by least squares,
/// near the rest. The places of the samples on the curve start as their share of `line`'s length
/// and are found again on the curve as fitted a few times.
fn edge_controls(line: &[Vec3], n: usize) -> Option<Vec<Vec3>> {
    let m = (8 * n).max(64);
    let samples = resample(line, m)?;
    let mut params: Vec<f64> = (0..m).map(|k| k as f64 / (m - 1) as f64).collect();
    let mut controls = solve_edge(&samples, &params, n)?;
    for _ in 0..4 {
        let dense: Vec<(f64, Vec3)> = (0..=DENSE)
            .filter_map(|k| {
                let t = k as f64 / DENSE as f64;
                curve_at(&controls, t).map(|p| (t, p))
            })
            .collect();
        for (t, q) in params.iter_mut().zip(&samples) {
            if let Some(&(best, _)) = dense.iter().min_by(|a, b| a.1.dist(*q).total_cmp(&b.1.dist(*q))) {
                *t = best;
            }
        }
        // The ends stay the ends.
        if let Some(t) = params.first_mut() {
            *t = 0.0;
        }
        if let Some(t) = params.last_mut() {
            *t = 1.0;
        }
        controls = solve_edge(&samples, &params, n)?;
    }
    Some(controls)
}

/// Samples of a curve when its parameters are found again.
const DENSE: usize = 800;

/// The point of the clamped cubic with `controls` at `t`.
fn curve_at(controls: &[Vec3], t: f64) -> Option<Vec3> {
    let n = controls.len();
    let (start, ders) = basis_derivatives(&clamped_knots(n), 3, n, t, 0)?;
    let mut p = Vec3::ZERO;
    for (i, d) in ders.iter().enumerate() {
        p += *controls.get(start + i)? * d.first().copied().unwrap_or(0.0);
    }
    Some(p)
}

/// The control points (the first and last at the samples' ends) of the clamped cubic nearest the
/// `samples` at `params`, by least squares.
fn solve_edge(samples: &[Vec3], params: &[f64], n: usize) -> Option<Vec<Vec3>> {
    let knots = clamped_knots(n);
    let (a, b) = (*samples.first()?, *samples.last()?);
    let free = n.checked_sub(2)?;
    let mut mat = vec![0.0; free * free];
    let mut rhs = vec![Vec3::ZERO; free];
    for (p, &t) in samples.iter().zip(params) {
        let (start, ders) = basis_derivatives(&knots, 3, n, t, 0)?;
        let weights: Vec<(usize, f64)> = ders.iter().enumerate().map(|(i, d)| (start + i, d.first().copied().unwrap_or(0.0))).collect();
        let w_of = |idx: usize| weights.iter().find(|w| w.0 == idx).map_or(0.0, |w| w.1);
        let rest = *p - a * w_of(0) - b * w_of(n - 1);
        for &(ia, wa) in &weights {
            if ia == 0 || ia == n - 1 {
                continue;
            }
            let ra = ia - 1;
            *rhs.get_mut(ra)? += rest * wa;
            for &(ib, wb) in &weights {
                if ib == 0 || ib == n - 1 {
                    continue;
                }
                *mat.get_mut(ra * free + ib - 1)? += wa * wb;
            }
        }
    }
    for i in 0..free {
        *mat.get_mut(i * free + i)? += 1e-9;
    }
    let mut out = vec![a];
    let mut solved = [Vec::new(), Vec::new(), Vec::new()];
    for (axis, column) in solved.iter_mut().enumerate() {
        let r: Vec<f64> = rhs.iter().map(|v| [v.x, v.y, v.z][axis]).collect();
        *column = cholesky_solve(&mut mat.clone(), &r, free)?;
    }
    for i in 0..free {
        out.push(Vec3::new(*solved[0].get(i)?, *solved[1].get(i)?, *solved[2].get(i)?));
    }
    out.push(b);
    Some(out)
}

/// A face over `boundary` fitted to `scan`, `count[0]` control points along u and `count[1]` along
/// v (each `LEAST_COUNT..=MOST_COUNT`), cubic with clamped even knots like every Free Form face.
/// With no scan points it is the Coons patch the sides bound.
pub fn face(boundary: &Boundary, scan: &[Vec3], count: [usize; 2]) -> Result<NurbsSurface, BoundaryError> {
    let [nu, nv] = count;
    if !(LEAST_COUNT..=MOST_COUNT).contains(&nu) || !(LEAST_COUNT..=MOST_COUNT).contains(&nv) {
        return Err(BoundaryError::BadCount);
    }
    if boundary.sides.iter().any(|s| s.len() < 2) {
        return Err(BoundaryError::TooShort);
    }
    if boundary.sides.iter().flatten().any(|p| !p.is_finite()) {
        return Err(BoundaryError::NotFinite);
    }
    let biggest = boundary.sides.iter().map(|s| length(s)).fold(0.0, f64::max);
    if boundary.sides.iter().any(|s| !(length(s) > 1e-9 * biggest) || !(length(s) > 0.0)) || !biggest.is_finite() {
        return Err(BoundaryError::DegenerateSide);
    }
    let fit = |side: usize, n: usize, reversed: bool| -> Result<Vec<Vec3>, BoundaryError> {
        let line = boundary.sides.get(side).ok_or(BoundaryError::TooShort)?;
        let mut c = edge_controls(line, n).ok_or(BoundaryError::FitFailed)?;
        if reversed {
            c.reverse();
        }
        Ok(c)
    };
    // Bottom (v = 0) and top (v = 1) rows along u; left (u = 0) and right (u = 1) along v.
    let (bottom, top) = (fit(0, nu, false)?, fit(2, nu, true)?);
    let (right, left) = (fit(1, nv, false)?, fit(3, nv, true)?);
    let corner = |row: &[Vec3], at_end: bool| if at_end { last(row) } else { first(row) }.ok_or(BoundaryError::FitFailed);
    let (c00, c10, c01, c11) = (corner(&bottom, false)?, corner(&bottom, true)?, corner(&top, false)?, corner(&top, true)?);
    let mut net = Vec::with_capacity(nu * nv);
    for j in 0..nv {
        let t = j as f64 / (nv - 1) as f64;
        for i in 0..nu {
            let s = i as f64 / (nu - 1) as f64;
            let (bi, ti) = (*bottom.get(i).ok_or(BoundaryError::FitFailed)?, *top.get(i).ok_or(BoundaryError::FitFailed)?);
            let (lj, rj) = (*left.get(j).ok_or(BoundaryError::FitFailed)?, *right.get(j).ok_or(BoundaryError::FitFailed)?);
            let ruled_v = bi * (1.0 - t) + ti * t;
            let ruled_u = lj * (1.0 - s) + rj * s;
            let bilinear = c00 * ((1.0 - s) * (1.0 - t)) + c10 * (s * (1.0 - t)) + c01 * ((1.0 - s) * t) + c11 * (s * t);
            net.push(ruled_v + ruled_u - bilinear);
        }
    }
    let coons = net_surface(&net, count).ok_or(BoundaryError::FitFailed)?;
    let scan = thinned(scan);
    if scan.is_empty() || nu < 4 || nv < 4 {
        return Ok(coons);
    }
    // Only the scan near the patch: its feet inside, within the patch's size.
    let reach = biggest;
    let near: Vec<Vec3> =
        scan.into_iter().filter(|p| surface_edit::foot(&coons, *p).is_some_and(|f| f.inside && f.point.dist(*p) <= reach)).collect();
    let held: Vec<bool> = (0..nu * nv)
        .map(|k| {
            let (i, j) = (k % nu, k / nu);
            i == 0 || j == 0 || i + 1 == nu || j + 1 == nv
        })
        .collect();
    Ok(surface_edit::refit(&coons, &near, &held).unwrap_or(coons))
}

/// The finite points of `scan`, at most `MOST_SCAN` of them (taken evenly).
fn thinned(scan: &[Vec3]) -> Vec<Vec3> {
    let step = scan.len().div_ceil(MOST_SCAN).max(1);
    scan.iter().step_by(step).filter(|p| p.is_finite()).copied().collect()
}

/// The four sides of `face` as polylines, in the order of a [`Boundary`]: A to B along u at the
/// start of v, B to C along v at the end of u, C to D along u back at the end of v, D to A along
/// v back at the start of u (A, B, C, D the face's corners at (0,0), (1,0), (1,1), (0,1)).
pub fn sides_of(face: &NurbsSurface) -> [Vec<Vec3>; 4] {
    let [[u0, u1], [v0, v1]] = face.domain();
    let line = |f: &dyn Fn(f64) -> (f64, f64)| -> Vec<Vec3> {
        (0..SIDE_SAMPLES)
            .filter_map(|k| {
                let (u, v) = f(k as f64 / (SIDE_SAMPLES - 1) as f64);
                face.eval(u, v).ok()
            })
            .collect()
    };
    [
        line(&|t| (u0 + (u1 - u0) * t, v0)),
        line(&|t| (u1, v0 + (v1 - v0) * t)),
        line(&|t| (u1 - (u1 - u0) * t, v1)),
        line(&|t| (u0, v1 - (v1 - v0) * t)),
    ]
}

/// The distance from `p` to the polyline `line`.
fn to_line(p: Vec3, line: &[Vec3]) -> f64 {
    let mut best = f64::INFINITY;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let l2 = ab.dot(ab);
        let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min((a + ab * t).dist(p));
    }
    if line.len() == 1 {
        best = line.first().map_or(best, |a| a.dist(p));
    }
    best
}

/// The largest distance from the boundary's sides to the face's four sides (infinite when the
/// face has no sides to measure to).
pub fn edge_deviation(boundary: &Boundary, face: &NurbsSurface) -> f64 {
    let sides = sides_of(face);
    let mut worst = 0.0f64;
    for (curve, side) in boundary.sides.iter().zip(&sides) {
        for p in curve {
            worst = worst.max(to_line(*p, side));
        }
        for w in curve.windows(2) {
            worst = worst.max(to_line((w[0] + w[1]) * 0.5, side));
        }
    }
    if worst.is_nan() { f64::INFINITY } else { worst }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dome(x: f64, y: f64) -> f64 {
        3.0 * (1.0 - (x * x + y * y) / 2.0).max(-1.0)
    }

    fn p(x: f64, y: f64) -> Vec3 {
        Vec3::new(x, y, dome(x, y))
    }

    /// The four sides of the square [-1, 1]² on the dome, shuffled and some reversed.
    fn curves() -> Vec<Vec<Vec3>> {
        let n = 20;
        let t = |k: usize| -1.0 + 2.0 * k as f64 / n as f64;
        let bottom: Vec<Vec3> = (0..=n).map(|k| p(t(k), -1.0)).collect();
        let right: Vec<Vec3> = (0..=n).map(|k| p(1.0, t(k))).collect();
        let top: Vec<Vec3> = (0..=n).map(|k| p(t(k), 1.0)).collect();
        let left: Vec<Vec3> = (0..=n).map(|k| p(-1.0, t(k))).collect();
        let rev = |mut c: Vec<Vec3>| {
            c.reverse();
            c
        };
        vec![rev(top), left, rev(right), bottom]
    }

    fn scan() -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in 0..=40 {
            for j in 0..=40 {
                let (x, y) = (-1.0 + i as f64 * 0.05, -1.0 + j as f64 * 0.05);
                let bump = 0.4 * (x * std::f64::consts::FRAC_PI_2).cos().powi(2) * (y * std::f64::consts::FRAC_PI_2).cos().powi(2);
                out.push(p(x, y) + Vec3::new(0.0, 0.0, bump));
            }
        }
        out
    }

    fn rms(face: &NurbsSurface, scan: &[Vec3]) -> f64 {
        surface_edit::distance(face, scan).0
    }

    #[test]
    fn order_recovers_the_loop() {
        let b = order(&curves(), 0.0).expect("loop");
        for k in 0..4 {
            assert_eq!(b.sides[k].last(), b.sides[(k + 1) % 4].first());
        }
        // Side 0 started on the curve given second (left), so the loop runs left, top, right, bottom... whatever the
        // start, every side is one of the input curves.
        let total: f64 = b.sides.iter().map(|s| length(s)).sum();
        let given: f64 = curves().iter().map(|s| length(s)).sum();
        assert!((total - given).abs() < 1e-6);
    }

    #[test]
    fn order_errors() {
        let c = curves();
        assert_eq!(order(&c[..3], 0.0), Err(BoundaryError::NotFourCurves(3)));
        let mut open = c.clone();
        open[1].truncate(10);
        assert_eq!(order(&open, 0.0), Err(BoundaryError::OpenLoop));
        let mut nan = c.clone();
        nan[0][3] = Vec3::new(f64::NAN, 0.0, 0.0);
        assert_eq!(order(&nan, 0.0), Err(BoundaryError::NotFinite));
        let mut short = c.clone();
        short[2] = vec![Vec3::ZERO];
        assert_eq!(order(&short, 0.0), Err(BoundaryError::TooShort));
        // A fifth curve ending at a corner makes three meet there.
        let mut crowded = c.clone();
        crowded[3] = vec![p(-1.0, -1.0), p(1.0, -1.0)];
        crowded[2] = vec![p(-1.0, -1.0), p(1.0, -1.0)];
        assert!(order(&crowded, 0.0).is_err());
    }

    #[test]
    fn sides_follow_the_curves() {
        let b = order(&curves(), 0.0).expect("loop");
        let f = face(&b, &[], [8, 8]).expect("face");
        assert!(edge_deviation(&b, &f) < 0.02, "{}", edge_deviation(&b, &f));
        let f = face(&b, &scan(), [8, 8]).expect("face");
        assert!(edge_deviation(&b, &f) < 0.02, "{}", edge_deviation(&b, &f));
    }

    #[test]
    fn the_inside_fits_the_dome() {
        let b = order(&curves(), 0.0).expect("loop");
        let s = scan();
        let coons = face(&b, &[], [8, 8]).expect("coons");
        let fitted = face(&b, &s, [8, 8]).expect("fitted");
        let (before, after) = (rms(&coons, &s), rms(&fitted, &s));
        assert!(after < before * 0.5 && after < 0.05, "{before} -> {after}");
    }

    #[test]
    fn hostile_input_never_panics() {
        let b = order(&curves(), 0.0).expect("loop");
        assert!(face(&b, &[], [3, 8]).is_err());
        assert!(face(&b, &[], [8, 99]).is_err());
        let nan = Boundary { sides: [vec![Vec3::new(f64::NAN, 0.0, 0.0); 2], b.sides[1].clone(), b.sides[2].clone(), b.sides[3].clone()] };
        assert!(face(&nan, &[], [6, 6]).is_err());
        let zero = Boundary { sides: [vec![Vec3::ZERO; 3], vec![Vec3::ZERO; 3], vec![Vec3::ZERO; 3], vec![Vec3::ZERO; 3]] };
        assert_eq!(face(&zero, &[], [6, 6]), Err(BoundaryError::DegenerateSide));
        let junk = vec![Vec3::new(f64::NAN, 1.0, f64::INFINITY), Vec3::new(1e300, 1e300, 1e300)];
        assert!(face(&b, &junk, [6, 6]).is_ok());
        assert!(order(&[vec![], vec![], vec![], vec![]], f64::NAN).is_err());
    }

    #[test]
    fn the_normal_follows_the_loop() {
        let n = 5;
        let sq = |a: (f64, f64), b: (f64, f64)| vec![Vec3::new(a.0, a.1, 0.0), Vec3::new(b.0, b.1, 0.0)];
        let _ = n;
        let b = Boundary { sides: [sq((0.0, 0.0), (1.0, 0.0)), sq((1.0, 0.0), (1.0, 1.0)), sq((1.0, 1.0), (0.0, 1.0)), sq((0.0, 1.0), (0.0, 0.0))] };
        let f = face(&b, &[], [4, 4]).expect("face");
        let (_, su, sv) = surface_edit::frame(&f, &surface_edit::points(&f), 0.5, 0.5).expect("frame");
        assert!(su.cross(sv).z > 0.0);
    }
}
