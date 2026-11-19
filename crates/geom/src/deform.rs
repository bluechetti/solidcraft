//! Deforming B-spline curves and surfaces through a map of space: a bend along a length round a
//! circle, a twist about an axis, a cage of control points (free-form deformation; Sederberg &
//! Parry, *Free-form deformation of solid geometric models*, SIGGRAPH 1986). The image of a
//! B-spline under a map that is not affine is no B-spline, so it is approximated: a cubic on the
//! original's own breakpoints (each with the continuity the original has there), its spans
//! divided evenly, interpolating the map of the original at the cubic's Greville abscissae
//! (de Boor, *A Practical Guide to Splines*, ch. XIII: the collocation matrix there is totally
//! positive and banded, so it is solved without pivoting). The parameters are the original's, so
//! a face's trims in its (u, v) stay where they were. The result is measured against the map of
//! the original at points inside every span, and the spans are divided further until the two
//! agree within the tolerance, or it is refused as needing more control points than allowed.
//! Interpolating converges with the fourth power of the span, carrying control points only with
//! the square.
//!
//! A straight line and a plane are written exactly as cubic B-splines whose control points are
//! their points at the Greville abscissae (a B-spline reproduces a linear function from those).
//!
//! Everything here is total: a map that is not finite, a spline that needs too many points or a
//! knot vector that does not fit are errors, never a panic.

use crate::bspline::{HPoint, NurbsSurface, SplineError, SplineResult, find_span};
use crate::prelude::*;
use solidcore_record::Vec3;

/// The most control points a deformed curve may have.
pub const MOST_CURVE_POINTS: usize = 4096;
/// The most control points a deformed surface may have along each direction.
pub const MOST_SURFACE_POINTS: usize = 512;
/// How many times the spans may be halved.
const MOST_HALVINGS: usize = 9;
/// Points measured inside each span, along each direction.
const SAMPLES_PER_SPAN: usize = 3;

/// A map of space.
#[derive(Clone, Debug, PartialEq)]
pub enum SpaceMap {
    /// The stretch from `origin` along the unit `along` for `length` bent round a circle towards
    /// the unit `up` (square to `along`), turning by `angle` radians over the length: the line
    /// through `origin` along `along` keeps its length (the neutral line), a point `h` along `up`
    /// from it goes round at radius `length / angle − h`. Before the stretch nothing moves; past
    /// it the rest is carried round rigidly with its end.
    Bend { origin: Vec3, along: Vec3, up: Vec3, length: f64, angle: f64 },
    /// A twist about the line through `origin` along the unit `axis` by `angle` radians over
    /// `length` along it, growing evenly; before it nothing turns, past it everything turns by
    /// the whole angle.
    Twist { origin: Vec3, axis: Vec3, length: f64, angle: f64 },
    /// A box from `lo` to `hi` with a lattice of `counts` points each way (2 to 8), `points` the
    /// lattice as moved (x fastest, then y, then z): a point of the box goes where the
    /// trivariate Bernstein polynomial of the moved lattice takes it; outside the box nothing
    /// moves.
    Cage { lo: Vec3, hi: Vec3, counts: [usize; 3], points: Vec<Vec3> },
}

/// Why something could not be deformed.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DeformError {
    #[error("the map is not usable: {0}")]
    Map(String),
    #[error("{0}")]
    Spline(#[from] SplineError),
    #[error("the deformed shape needs more control points than allowed (it stays {0:e} off at the most)")]
    TooFine(f64),
}

pub type DeformResult<T> = Result<T, DeformError>;

fn unit(v: Vec3, what: &str) -> DeformResult<Vec3> {
    v.normalized().ok_or_else(|| DeformError::Map(format!("{what} has no direction")))
}

/// Turning `v` about the unit `axis` by `angle` (Rodrigues).
fn turn(v: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

impl SpaceMap {
    /// The map checked: directions unit and square where they must be, lengths and angles
    /// finite, the lattice the size its counts say.
    pub fn checked(self) -> DeformResult<SpaceMap> {
        let finite = |v: f64, what: &str| if v.is_finite() { Ok(v) } else { Err(DeformError::Map(format!("{what} is not a number"))) };
        Ok(match self {
            SpaceMap::Bend { origin, along, up, length, angle } => {
                let along = unit(along, "the bend's length direction")?;
                let up = unit(up - along * up.dot(along), "the bend's direction (square to its length)")?;
                if !(finite(length, "the bend's length")? > 0.0) {
                    return Err(DeformError::Map("the bend's length must be above 0".into()));
                }
                if finite(angle, "the bend's angle")?.abs() > std::f64::consts::TAU {
                    return Err(DeformError::Map("the bend turns at most a full turn".into()));
                }
                SpaceMap::Bend { origin, along, up, length, angle }
            }
            SpaceMap::Twist { origin, axis, length, angle } => {
                let axis = unit(axis, "the twist's axis")?;
                if !(finite(length, "the twist's length")? > 0.0) {
                    return Err(DeformError::Map("the twist's length must be above 0".into()));
                }
                finite(angle, "the twist's angle")?;
                SpaceMap::Twist { origin, axis, length, angle }
            }
            SpaceMap::Cage { lo, hi, counts, points } => {
                if counts.iter().any(|&c| !(2..=8).contains(&c)) {
                    return Err(DeformError::Map("a cage has 2 to 8 points each way".into()));
                }
                if points.len() != counts[0] * counts[1] * counts[2] || points.iter().any(|p| !p.is_finite()) {
                    return Err(DeformError::Map("the cage's points do not fill its lattice".into()));
                }
                if !(hi.x > lo.x && hi.y > lo.y && hi.z > lo.z) {
                    return Err(DeformError::Map("the cage's box is empty".into()));
                }
                SpaceMap::Cage { lo, hi, counts, points }
            }
        })
    }

    /// A cage's lattice as it is before anything moves: its points evenly over the box.
    pub fn lattice(lo: Vec3, hi: Vec3, counts: [usize; 3]) -> Vec<Vec3> {
        let mut out = Vec::new();
        let at = |a: f64, b: f64, k: usize, n: usize| a + (b - a) * k as f64 / (n.max(2) - 1) as f64;
        for k in 0..counts[2] {
            for j in 0..counts[1] {
                for i in 0..counts[0] {
                    out.push(Vec3::new(at(lo.x, hi.x, i, counts[0]), at(lo.y, hi.y, j, counts[1]), at(lo.z, hi.z, k, counts[2])));
                }
            }
        }
        out
    }

    /// Where the map takes `p` (the map as [`SpaceMap::checked`] leaves it).
    pub fn apply(&self, p: Vec3) -> Vec3 {
        match self {
            SpaceMap::Bend { origin, along, up, length, angle } => {
                let d = p - *origin;
                let x = d.dot(*along);
                if x <= 0.0 || angle.abs() < 1e-15 {
                    return p;
                }
                let h = d.dot(*up);
                let side = d - *along * x - *up * h;
                let r = length / angle;
                let t = x.min(*length);
                let theta = t / r;
                // The neutral line's point and its frame at t along the bend.
                let centre = *origin + *up * r;
                let radial = *up * (-1.0);
                let at = centre + (radial * theta.cos() + *along * theta.sin()) * r;
                let tangent = *along * theta.cos() - radial * theta.sin();
                let inward = (centre - at).normalized().unwrap_or(*up);
                at + inward * h + side + tangent * (x - t)
            }
            SpaceMap::Twist { origin, axis, length, angle } => {
                let d = p - *origin;
                let s = d.dot(*axis).clamp(0.0, *length);
                *origin + turn(d, *axis, angle * s / length)
            }
            SpaceMap::Cage { lo, hi, counts, points } => {
                let size = *hi - *lo;
                let s = [(p.x - lo.x) / size.x, (p.y - lo.y) / size.y, (p.z - lo.z) / size.z];
                if s.iter().any(|v| !(0.0..=1.0).contains(v)) {
                    return p;
                }
                let bern = |n: usize, i: usize, t: f64| {
                    let deg = n - 1;
                    let choose = (0..i).fold(1.0, |acc, k| acc * (deg - k) as f64 / (k + 1) as f64);
                    choose * t.powi(i as i32) * (1.0 - t).powi((deg - i) as i32)
                };
                let mut out = Vec3::ZERO;
                for k in 0..counts[2] {
                    let bk = bern(counts[2], k, s[2]);
                    for j in 0..counts[1] {
                        let bj = bern(counts[1], j, s[1]) * bk;
                        for i in 0..counts[0] {
                            let w = bern(counts[0], i, s[0]) * bj;
                            if let Some(q) = points.get((k * counts[1] + j) * counts[0] + i) {
                                out += *q * w;
                            }
                        }
                    }
                }
                out
            }
        }
    }
}

/// A B-spline curve: degree, knots and homogeneous control points.
#[derive(Clone, Debug, PartialEq)]
pub struct Spline {
    pub degree: usize,
    pub knots: Vec<f64>,
    pub ctrl: Vec<HPoint>,
}

/// The `p + 1` basis functions not zero at `u` in span `span` (NURBS Book A2.2).
fn basis(knots: &[f64], span: usize, p: usize, u: f64) -> Vec<f64> {
    let k = |i: usize| knots.get(i).copied().unwrap_or(0.0);
    let mut n = vec![0.0; p + 1];
    let (mut left, mut right) = (vec![0.0; p + 1], vec![0.0; p + 1]);
    n[0] = 1.0;
    for j in 1..=p {
        left[j] = u - k(span + 1 - j);
        right[j] = k(span + j) - u;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let temp = if den != 0.0 { n[r] / den } else { 0.0 };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    n
}

impl Spline {
    /// The curve's checked: knots that fit its points and degree.
    pub fn new(degree: usize, knots: Vec<f64>, ctrl: Vec<HPoint>) -> SplineResult<Spline> {
        if degree == 0 || ctrl.len() <= degree || knots.len() != ctrl.len() + degree + 1 {
            return Err(SplineError::Invalid("a curve's knots do not fit its points and degree".into()));
        }
        crate::bspline::check_knots(&knots)?;
        if ctrl.iter().any(|h| !h.is_finite() || !(h.w > 0.0)) {
            return Err(SplineError::Invalid("control points must be finite with positive weights".into()));
        }
        Ok(Spline { degree, knots, ctrl })
    }

    /// `[t₀, t₁]`.
    pub fn domain(&self) -> [f64; 2] {
        let k = |i: usize| self.knots.get(i).copied().unwrap_or(0.0);
        [k(self.degree), k(self.ctrl.len())]
    }

    /// The point at `t`.
    pub fn eval(&self, t: f64) -> SplineResult<Vec3> {
        let p = self.degree;
        let span = find_span(&self.knots, p, self.ctrl.len(), t).ok_or_else(|| SplineError::Invalid(format!("{t} is outside the curve's domain")))?;
        let n = basis(&self.knots, span, p, t.clamp(self.domain()[0], self.domain()[1]));
        let mut acc = HPoint::ZERO;
        for (a, b) in n.iter().enumerate() {
            let c = self.ctrl.get(span - p + a).copied().unwrap_or(HPoint::ZERO);
            acc = HPoint { x: acc.x + c.x * b, y: acc.y + c.y * b, z: acc.z + c.z * b, w: acc.w + c.w * b };
        }
        acc.project().ok_or_else(|| SplineError::Invalid("the curve's weights vanish there".into()))
    }

    /// Whether every weight is one.
    pub fn polynomial(&self) -> bool {
        self.ctrl.iter().all(|h| (h.w - 1.0).abs() < 1e-14)
    }
}

/// Clamped uniform cubic knots on `[a, b]` with `spans` spans, and their Greville abscissae.
fn cubic_knots(a: f64, b: f64, spans: usize) -> (Vec<f64>, Vec<f64>) {
    let mut knots = vec![a; 4];
    for k in 1..spans {
        knots.push(a + (b - a) * k as f64 / spans as f64);
    }
    knots.extend([b; 4]);
    let greville = (0..spans + 3).map(|i| (knots[i + 1] + knots[i + 2] + knots[i + 3]) / 3.0).collect();
    (knots, greville)
}

/// The line `start + (t − a)/(b − a) · (end − start)` on `[a, b]`, exactly, as a cubic B-spline of
/// `spans` spans.
pub fn line(start: Vec3, end: Vec3, a: f64, b: f64, spans: usize) -> SplineResult<Spline> {
    if !(b > a) {
        return Err(SplineError::Invalid("a line's interval is empty".into()));
    }
    let (knots, at) = cubic_knots(a, b, spans.max(1));
    let ctrl = at.iter().map(|t| HPoint::weighted(start + (end - start) * ((t - a) / (b - a)), 1.0)).collect();
    Spline::new(3, knots, ctrl)
}

/// The plane `origin + u·u_axis + v·v_axis` over `domain`, exactly, as a bicubic B-spline of
/// `spans` spans each way.
pub fn plane(origin: Vec3, u_axis: Vec3, v_axis: Vec3, domain: [[f64; 2]; 2], spans: usize) -> SplineResult<NurbsSurface> {
    let (ku, gu) = cubic_knots(domain[0][0], domain[0][1], spans.max(1));
    let (kv, gv) = cubic_knots(domain[1][0], domain[1][1], spans.max(1));
    let mut ctrl = Vec::with_capacity(gu.len() * gv.len());
    for v in &gv {
        for u in &gu {
            ctrl.push(HPoint::weighted(origin + u_axis * *u + v_axis * *v, 1.0));
        }
    }
    NurbsSurface::from_homogeneous([3, 3], [ku, kv], ctrl)
}

/// The breakpoints of `knots` (degree `p`) with the multiplicity a cubic needs there to keep the
/// continuity the original has: `C^(p−m)` at a knot of multiplicity `m` is a cubic knot of
/// multiplicity `3 − (p − m)`, at least 1 and at most 3. The ends are left out.
fn breaks(knots: &[f64], p: usize) -> Vec<(f64, usize)> {
    let (Some(&a), Some(&b)) = (knots.get(p), knots.get(knots.len().saturating_sub(p + 1))) else { return Vec::new() };
    let mut out: Vec<(f64, usize)> = Vec::new();
    for &k in knots.iter().filter(|&&k| k > a && k < b) {
        match out.last_mut() {
            Some((at, m)) if *at == k => *m += 1,
            _ => out.push((k, 1)),
        }
    }
    out.into_iter().map(|(k, m)| (k, (3 + m).saturating_sub(p).clamp(1, 3))).collect()
}

/// Cubic knots on `[a, b]` keeping `breaks`, each span between them divided into `split` even
/// pieces.
fn refined_knots(a: f64, b: f64, breaks: &[(f64, usize)], split: usize) -> Vec<f64> {
    let mut out = vec![a; 4];
    let mut at = a;
    let stops: Vec<(f64, usize)> = breaks.iter().copied().chain(std::iter::once((b, 4))).collect();
    for (k, m) in stops {
        for i in 1..split {
            out.push(at + (k - at) * i as f64 / split as f64);
        }
        out.extend(std::iter::repeat_n(k, m));
        at = k;
    }
    out
}

/// The Greville abscissae of cubic `knots`.
fn greville(knots: &[f64]) -> Vec<f64> {
    (0..knots.len().saturating_sub(4)).map(|i| (knots[i + 1] + knots[i + 2] + knots[i + 3]) / 3.0).collect()
}

/// The cubic B-spline coefficients on `knots` taking the values `values` at `params` (one value
/// a parameter, as many as basis functions): the collocation system, each row four basis
/// functions wide, solved by banded elimination without pivoting.
fn interpolate(knots: &[f64], params: &[f64], values: &[[f64; 3]]) -> SplineResult<Vec<[f64; 3]>> {
    let n = params.len();
    if values.len() != n || knots.len() != n + 4 {
        return Err(SplineError::Invalid("interpolation sizes do not agree".into()));
    }
    // Row i: the basis functions span − 3 ..= span, stored by column.
    let mut rows: Vec<(usize, [f64; 4])> = Vec::with_capacity(n);
    for &t in params {
        let span = find_span(knots, 3, n, t).ok_or_else(|| SplineError::Invalid(format!("{t} is outside the knots")))?;
        let b = basis(knots, span, 3, t);
        rows.push((span - 3, [b[0], b[1], b[2], b[3]]));
    }
    // Banded storage: the band of width 7 about the diagonal (|col − row| ≤ 3).
    let w = 7usize;
    let mut a = vec![vec![0.0f64; w]; n];
    let col = |r: usize, c: usize| (c + 3).checked_sub(r).filter(|&k| k < w);
    for (r, (first, b)) in rows.iter().enumerate() {
        for (k, v) in b.iter().enumerate() {
            if let Some(slot) = col(r, first + k) {
                a[r][slot] = *v;
            }
        }
    }
    let mut x: Vec<[f64; 3]> = values.to_vec();
    for k in 0..n {
        let Some(pk) = col(k, k).map(|s| a[k][s]) else { return Err(SplineError::Invalid("collocation".into())) };
        if !(pk.abs() > 1e-300) {
            return Err(SplineError::Invalid("the interpolation points do not fit the knots".into()));
        }
        for r in k + 1..(k + 4).min(n) {
            let Some(rk) = col(r, k) else { continue };
            let f = a[r][rk] / pk;
            if f == 0.0 {
                continue;
            }
            for c in k..(k + 4).min(n) {
                if let (Some(rc), Some(kc)) = (col(r, c), col(k, c)) {
                    a[r][rc] -= f * a[k][kc];
                }
            }
            for d in 0..3 {
                x[r][d] -= f * x[k][d];
            }
        }
    }
    for k in (0..n).rev() {
        for c in k + 1..(k + 4).min(n) {
            if let Some(kc) = col(k, c) {
                for d in 0..3 {
                    x[k][d] -= a[k][kc] * x[c][d];
                }
            }
        }
        let Some(kk) = col(k, k) else { return Err(SplineError::Invalid("collocation".into())) };
        for d in 0..3 {
            x[k][d] /= a[k][kk];
        }
    }
    if x.iter().flatten().any(|v| !v.is_finite()) {
        return Err(SplineError::Invalid("the interpolation is not finite".into()));
    }
    Ok(x)
}

fn mapped(map: &SpaceMap, p: Vec3) -> DeformResult<[f64; 3]> {
    let q = map.apply(p);
    if !q.is_finite() {
        return Err(DeformError::Map("the map is not finite on the shape".into()));
    }
    Ok([q.x, q.y, q.z])
}

/// Parameters inside each span of `knots` (and its ends), for measuring.
fn samples(knots: &[f64]) -> Vec<f64> {
    let mut out = Vec::new();
    for w in knots.windows(2).filter(|w| w[1] > w[0]) {
        for k in 0..=SAMPLES_PER_SPAN {
            out.push(w[0] + (w[1] - w[0]) * k as f64 / (SAMPLES_PER_SPAN + 1) as f64);
        }
    }
    if let Some(last) = knots.last() {
        out.push(*last);
    }
    out
}

/// `curve` carried through `map` within `tol` (a cubic on its parameters), and how far it is
/// from the map of the curve at the points measured.
pub fn deform_curve(curve: &Spline, map: &SpaceMap, tol: f64) -> DeformResult<(Spline, f64)> {
    let [a, b] = curve.domain();
    let cuts = breaks(&curve.knots, curve.degree);
    let mut worst = f64::INFINITY;
    let mut split = 1usize;
    for _ in 0..=MOST_HALVINGS {
        let knots = refined_knots(a, b, &cuts, split);
        let params = greville(&knots);
        if params.len() > MOST_CURVE_POINTS {
            break;
        }
        let values =
            params.iter().map(|t| curve.eval(*t).map_err(DeformError::from).and_then(|p| mapped(map, p))).collect::<DeformResult<Vec<_>>>()?;
        let ctrl = interpolate(&knots, &params, &values)?.into_iter().map(|[x, y, z]| HPoint::weighted(Vec3::new(x, y, z), 1.0)).collect();
        let moved = Spline::new(3, knots.clone(), ctrl)?;
        worst = 0.0;
        for t in samples(&knots) {
            worst = worst.max(map.apply(curve.eval(t)?).dist(moved.eval(t)?));
        }
        if worst <= tol {
            return Ok((moved, worst));
        }
        split *= 2;
    }
    Err(DeformError::TooFine(worst))
}

/// `surface` carried through `map` within `tol` (a bicubic on its parameters), and how far it
/// is from the map of the surface at the points measured.
pub fn deform_surface(surface: &NurbsSurface, map: &SpaceMap, tol: f64) -> DeformResult<(NurbsSurface, f64)> {
    deform_surface_over(surface, surface.domain(), map, tol)
}

/// [`deform_surface`] over `dom`, a rectangle within the surface's domain (the part a face
/// uses): the result's domain is `dom`.
pub fn deform_surface_over(surface: &NurbsSurface, dom: [[f64; 2]; 2], map: &SpaceMap, tol: f64) -> DeformResult<(NurbsSurface, f64)> {
    let whole = surface.domain();
    let inside = (0..2).all(|d| whole[d][0] <= dom[d][0] && dom[d][0] < dom[d][1] && dom[d][1] <= whole[d][1]);
    if !inside {
        return Err(DeformError::Spline(SplineError::Invalid("the rectangle is not within the surface's domain".into())));
    }
    let deg = surface.degree();
    let within = |cuts: Vec<(f64, usize)>, d: usize| cuts.into_iter().filter(|(k, _)| *k > dom[d][0] && *k < dom[d][1]).collect::<Vec<_>>();
    let cuts = [within(breaks(&surface.knots()[0], deg[0]), 0), within(breaks(&surface.knots()[1], deg[1]), 1)];
    let mut worst = f64::INFINITY;
    let mut split = 1usize;
    for _ in 0..=MOST_HALVINGS {
        let ku = refined_knots(dom[0][0], dom[0][1], &cuts[0], split);
        let kv = refined_knots(dom[1][0], dom[1][1], &cuts[1], split);
        let (gu, gv) = (greville(&ku), greville(&kv));
        if gu.len() > MOST_SURFACE_POINTS || gv.len() > MOST_SURFACE_POINTS {
            break;
        }
        // The map of the surface at the grid of Greville points, then interpolated along u in
        // every row and along v in every column of the result.
        let mut grid: Vec<Vec<[f64; 3]>> = Vec::with_capacity(gv.len());
        for v in &gv {
            let row =
                gu.iter().map(|u| surface.eval(*u, *v).map_err(DeformError::from).and_then(|p| mapped(map, p))).collect::<DeformResult<Vec<_>>>()?;
            grid.push(interpolate(&ku, &gu, &row)?);
        }
        let mut ctrl = vec![HPoint::ZERO; gu.len() * gv.len()];
        for i in 0..gu.len() {
            let column: Vec<[f64; 3]> = grid.iter().map(|row| row[i]).collect();
            for (j, [x, y, z]) in interpolate(&kv, &gv, &column)?.into_iter().enumerate() {
                ctrl[j * gu.len() + i] = HPoint::weighted(Vec3::new(x, y, z), 1.0);
            }
        }
        let moved = NurbsSurface::from_homogeneous([3, 3], [ku.clone(), kv.clone()], ctrl)?;
        worst = 0.0;
        for u in samples(&ku) {
            for v in samples(&kv) {
                worst = worst.max(map.apply(surface.eval(u, v)?).dist(moved.eval(u, v)?));
            }
        }
        if worst <= tol {
            return Ok((moved, worst));
        }
        split *= 2;
    }
    Err(DeformError::TooFine(worst))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bend_quarter() -> SpaceMap {
        SpaceMap::Bend {
            origin: Vec3::ZERO,
            along: Vec3::new(1.0, 0.0, 0.0),
            up: Vec3::new(0.0, 0.0, 1.0),
            length: 100.0,
            angle: std::f64::consts::FRAC_PI_2,
        }
        .checked()
        .unwrap()
    }

    /// The bend keeps the neutral line's length and takes its end a quarter round: (r, 0, r)
    /// for r = 100 / (π/2); a point above the line goes round on a smaller circle.
    #[test]
    fn a_quarter_bend_takes_the_end_round() {
        let map = bend_quarter();
        let r = 100.0 / std::f64::consts::FRAC_PI_2;
        assert!(map.apply(Vec3::new(100.0, 0.0, 0.0)).dist(Vec3::new(r, 0.0, r)) < 1e-12);
        assert!(map.apply(Vec3::new(100.0, 3.0, 5.0)).dist(Vec3::new(r - 5.0, 3.0, r)) < 1e-12);
        assert_eq!(map.apply(Vec3::new(-4.0, 1.0, 2.0)), Vec3::new(-4.0, 1.0, 2.0), "before the bend nothing moves");
        // Past the bend the rest goes on straight up.
        assert!(map.apply(Vec3::new(110.0, 0.0, 0.0)).dist(Vec3::new(r, 0.0, r + 10.0)) < 1e-12);
    }

    /// A line along the bend, written as a cubic and carried through it, follows the quarter circle
    /// within the tolerance, at every point measured and between them.
    #[test]
    fn a_line_bent_follows_the_circle() {
        let map = bend_quarter();
        let line = line(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), 0.0, 1.0, 1).unwrap();
        let (bent, worst) = deform_curve(&line, &map, 1e-4).unwrap();
        assert!(worst <= 1e-4);
        let r = 100.0 / std::f64::consts::FRAC_PI_2;
        for k in 0..=50 {
            let p = bent.eval(k as f64 / 50.0).unwrap();
            let off = (p.dist(Vec3::new(0.0, 0.0, r)) - r).abs();
            assert!(off < 2e-4, "{k}: {off}");
        }
    }

    /// A plane written as a bicubic is the plane; twisted a quarter turn over its length, its edge
    /// turns as the twist says, within the tolerance.
    #[test]
    fn a_plane_is_exact_and_twists() {
        let dom = [[0.0, 10.0], [0.0, 40.0]];
        let flat = plane(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), dom, 1).unwrap();
        assert!(flat.eval(3.3, 17.0).unwrap().dist(Vec3::new(3.3, 0.0, 17.0)) < 1e-12);
        let map = SpaceMap::Twist { origin: Vec3::ZERO, axis: Vec3::new(0.0, 0.0, 1.0), length: 40.0, angle: std::f64::consts::FRAC_PI_2 }
            .checked()
            .unwrap();
        let (twisted, worst) = deform_surface(&flat, &map, 1e-3).unwrap();
        assert!(worst <= 1e-3);
        assert!(twisted.eval(10.0, 40.0).unwrap().dist(Vec3::new(0.0, 10.0, 40.0)) < 1e-3);
    }

    /// A cage left as it is moves nothing; one moved by a shift moves everything in its box by it.
    #[test]
    fn a_cage_follows_its_lattice() {
        let (lo, hi) = (Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let still = SpaceMap::Cage { lo, hi, counts: [3, 3, 3], points: SpaceMap::lattice(lo, hi, [3, 3, 3]) }.checked().unwrap();
        let p = Vec3::new(2.5, 7.0, 4.0);
        assert!(still.apply(p).dist(p) < 1e-12);
        let shifted: Vec<Vec3> = SpaceMap::lattice(lo, hi, [3, 3, 3]).into_iter().map(|q| q + Vec3::new(1.0, 2.0, 3.0)).collect();
        let moved = SpaceMap::Cage { lo, hi, counts: [3, 3, 3], points: shifted }.checked().unwrap();
        assert!(moved.apply(p).dist(p + Vec3::new(1.0, 2.0, 3.0)) < 1e-12);
    }

    #[test]
    fn unusable_maps_are_refused() {
        let bad = [
            SpaceMap::Bend { origin: Vec3::ZERO, along: Vec3::ZERO, up: Vec3::new(0.0, 0.0, 1.0), length: 1.0, angle: 1.0 },
            SpaceMap::Bend { origin: Vec3::ZERO, along: Vec3::new(1.0, 0.0, 0.0), up: Vec3::new(1.0, 0.0, 0.0), length: 1.0, angle: 1.0 },
            SpaceMap::Twist { origin: Vec3::ZERO, axis: Vec3::new(0.0, 0.0, 1.0), length: 0.0, angle: 1.0 },
            SpaceMap::Cage { lo: Vec3::ZERO, hi: Vec3::new(1.0, 1.0, 1.0), counts: [2, 2, 9], points: Vec::new() },
        ];
        for m in bad {
            assert!(m.clone().checked().is_err(), "{m:?}");
        }
    }
}
