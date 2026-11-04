//! Editing a fitted surface by its control points: where each scan point lies on the surface (the
//! foot of its perpendicular), which of the scan's points the surface covers, how far they are from
//! it, and the surface fitted to them again with some control points held where they were put.
//!
//! A refit is linear least squares in the free control points (the weights are kept, so a rational
//! surface is as linear in its points as a polynomial one): the scan points' places on the surface
//! are found, the free points solved with the held ones as constants, a light pull toward each
//! free point's own place (so a point no scan reaches stays put) and a lighter hold on how unevenly
//! neighbours move, and that is done three times, the places found again on each new surface.

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{HPoint, NurbsSurface, basis_derivatives};
use solidcraft_geom::prelude::*;

use crate::{Scan, thinned};

/// A surface's basis at a parameter: the control points it depends on (index `j·nu + i`) and each
/// one's rational basis value and its derivatives along u and v.
struct Basis {
    cols: Vec<usize>,
    value: Vec<f64>,
    du: Vec<f64>,
    dv: Vec<f64>,
}

fn basis(surface: &NurbsSurface, u: f64, v: f64) -> Option<Basis> {
    let [pu, pv] = surface.degree();
    let [nu, nv] = surface.count();
    let [ku, kv] = surface.knots();
    let (fu, bu) = basis_derivatives(ku, pu, nu, u, 1)?;
    let (fv, bv) = basis_derivatives(kv, pv, nv, v, 1)?;
    // Each function's value and first derivative: `b[a][order]`.
    let at = |b: &[Vec<f64>], a: usize, order: usize| b.get(a).and_then(|d| d.get(order)).copied();
    let ctrl = surface.homogeneous();
    let n = (pu + 1) * (pv + 1);
    let (mut cols, mut wn, mut wu, mut wv) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for b in 0..=pv {
        for a in 0..=pu {
            let col = (fv + b) * nu + fu + a;
            let w = ctrl.get(col)?.w;
            cols.push(col);
            let (nu0, nu1, nv0, nv1) = (at(&bu, a, 0)?, at(&bu, a, 1)?, at(&bv, b, 0)?, at(&bv, b, 1)?);
            wn.push(nu0 * nv0 * w);
            wu.push(nu1 * nv0 * w);
            wv.push(nu0 * nv1 * w);
        }
    }
    let (sw, swu, swv): (f64, f64, f64) = (wn.iter().sum(), wu.iter().sum(), wv.iter().sum());
    if !(sw > 0.0) {
        return None;
    }
    let value = wn.iter().map(|x| x / sw).collect();
    let du = wn.iter().zip(&wu).map(|(n, d)| (d * sw - n * swu) / (sw * sw)).collect();
    let dv = wn.iter().zip(&wv).map(|(n, d)| (d * sw - n * swv) / (sw * sw)).collect();
    Some(Basis { cols, value, du, dv })
}

/// The surface's control points as Cartesian points (u fastest).
pub fn points(surface: &NurbsSurface) -> Vec<Vec3> {
    surface.homogeneous().iter().map(|h| h.project().unwrap_or(Vec3::ZERO)).collect()
}

/// The surface with its control points moved to `points` (u fastest), each keeping its weight.
pub fn with_points(surface: &NurbsSurface, points: &[Vec3]) -> Option<NurbsSurface> {
    let ctrl = surface.homogeneous();
    if points.len() != ctrl.len() || points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let net = ctrl.iter().zip(points).map(|(h, p)| HPoint::weighted(*p, h.w)).collect();
    NurbsSurface::from_homogeneous(surface.degree(), surface.knots().clone(), net).ok()
}

/// The point, and its derivatives along u and v, at `(u, v)`.
fn frame(surface: &NurbsSurface, net: &[Vec3], u: f64, v: f64) -> Option<(Vec3, Vec3, Vec3)> {
    let b = basis(surface, u, v)?;
    let (mut s, mut su, mut sv) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
    for (k, &c) in b.cols.iter().enumerate() {
        let p = *net.get(c)?;
        s += p * b.value[k];
        su += p * b.du[k];
        sv += p * b.dv[k];
    }
    Some((s, su, sv))
}

/// Where a point lies on a surface: its parameters, the foot of its perpendicular, and whether the
/// foot is inside the surface (not held at its edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Foot {
    pub u: f64,
    pub v: f64,
    pub point: Vec3,
    pub inside: bool,
}

/// How finely the starting guesses for a foot are sampled, each way.
const GUESSES: usize = 10;

/// Starting guesses for feet: the surface sampled on a grid.
fn samples(surface: &NurbsSurface, net: &[Vec3]) -> Vec<(f64, f64, Vec3)> {
    let [[u0, u1], [v0, v1]] = surface.domain();
    let mut out = Vec::with_capacity(GUESSES * GUESSES);
    for j in 0..GUESSES {
        for i in 0..GUESSES {
            let (u, v) = (u0 + (u1 - u0) * (i as f64 + 0.5) / GUESSES as f64, v0 + (v1 - v0) * (j as f64 + 0.5) / GUESSES as f64);
            if let Some((s, _, _)) = frame(surface, net, u, v) {
                out.push((u, v, s));
            }
        }
    }
    out
}

/// The foot of `p` on the surface, from `(u, v)`: Gauss–Newton steps on the distance, held to the
/// domain.
fn foot_from(surface: &NurbsSurface, net: &[Vec3], p: Vec3, mut u: f64, mut v: f64) -> Option<Foot> {
    let [[u0, u1], [v0, v1]] = surface.domain();
    let tiny = 1e-9 * ((u1 - u0) + (v1 - v0));
    let mut at = frame(surface, net, u, v)?;
    for _ in 0..12 {
        let (s, su, sv) = at;
        let d = s - p;
        let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
        let (gu, gv) = (su.dot(d), sv.dot(d));
        let det = a * c - b * b;
        if !(det.abs() > 1e-300) {
            break;
        }
        let (du, dv) = ((c * gu - b * gv) / det, (a * gv - b * gu) / det);
        let (nu, nv) = ((u - du).clamp(u0, u1), (v - dv).clamp(v0, v1));
        let moved = (nu - u).abs() + (nv - v).abs();
        u = nu;
        v = nv;
        at = frame(surface, net, u, v)?;
        if moved < tiny {
            break;
        }
    }
    let edge = 1e-6 * ((u1 - u0) + (v1 - v0));
    let inside = u > u0 + edge && u < u1 - edge && v > v0 + edge && v < v1 - edge;
    Some(Foot { u, v, point: at.0, inside })
}

/// The foot of `p` on `surface`.
pub fn foot(surface: &NurbsSurface, p: Vec3) -> Option<Foot> {
    let net = points(surface);
    let guesses = samples(surface, &net);
    let &(u, v, _) = guesses.iter().min_by(|a, b| a.2.dist(p).total_cmp(&b.2.dist(p)))?;
    foot_from(surface, &net, p, u, v)
}

/// The scan the surface covers, as the middles of its triangles: those whose foot is inside the
/// surface, within `reach` of it, and facing the way the surface does there (within 45°, and on the
/// side most of them face, so neither a wall the surface's edge comes near nor the far side of a
/// thin part counts), at most `limit` of them (thinned evenly).
pub fn covered(scan: &Scan, surface: &NurbsSurface, reach: f64, limit: usize) -> Vec<Vec3> {
    let net = points(surface);
    let Some(first) = net.first().copied() else { return Vec::new() };
    let (lo, hi) = net.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let pad = Vec3::new(reach, reach, reach);
    let (lo, hi) = (lo - pad, hi + pad);
    let near: Vec<(Vec3, Vec3)> = (0..scan.triangle_count())
        .filter_map(|t| {
            let [a, b, c] = scan.corners(t)?;
            let m = (a + b + c) / 3.0;
            let inside = m.x >= lo.x && m.y >= lo.y && m.z >= lo.z && m.x <= hi.x && m.y <= hi.y && m.z <= hi.z;
            inside.then(|| (b - a).cross(c - a).normalized().map(|n| (m, n)))?
        })
        .collect();
    // Thinned before the feet are found, with room for the ones that turn out not to be covered.
    let near = thinned(&near, limit.saturating_mul(3));
    let guesses = samples(surface, &net);
    let facing: Vec<(Vec3, f64)> = near
        .into_iter()
        .filter_map(|(m, n)| {
            let &(u, v, _) = guesses.iter().min_by(|a, b| a.2.dist(m).total_cmp(&b.2.dist(m)))?;
            let f = foot_from(surface, &net, m, u, v).filter(|f| f.inside && f.point.dist(m) <= reach)?;
            let (_, su, sv) = frame(surface, &net, f.u, f.v)?;
            let along = n.dot(su.cross(sv).normalized()?);
            (along.abs() >= std::f64::consts::FRAC_1_SQRT_2).then_some((m, along))
        })
        .collect();
    let sign = facing.iter().map(|(_, d)| d.signum()).sum::<f64>().signum();
    let kept: Vec<Vec3> = facing.into_iter().filter(|(_, d)| d.signum() == sign).map(|(m, _)| m).collect();
    thinned(&kept, limit)
}

/// The points' distance from the surface: root mean square and largest, in mm (0 with none).
pub fn distance(surface: &NurbsSurface, points: &[Vec3]) -> (f64, f64) {
    let net = self::points(surface);
    let guesses = samples(surface, &net);
    let (mut squares, mut max, mut n) = (0.0, 0.0f64, 0usize);
    for p in points {
        let Some(&(u, v, _)) = guesses.iter().min_by(|a, b| a.2.dist(*p).total_cmp(&b.2.dist(*p))) else { continue };
        if let Some(f) = foot_from(surface, &net, *p, u, v) {
            let d = f.point.dist(*p);
            squares += d * d;
            max = max.max(d);
            n += 1;
        }
    }
    if n == 0 { (0.0, 0.0) } else { ((squares / n as f64).sqrt(), max) }
}

/// The surface fitted again to `points` (scan points it covers), the control points `held` marks
/// (one per control point, u fastest) kept exactly where they are. Each free point moves only along
/// the way the surface faces at it, as a fit's heights do: free to go anywhere, the net could slide
/// along the surface and fold. `None` when nothing is free, no point lies on the free part, or the
/// solve fails.
pub fn refit(surface: &NurbsSurface, points: &[Vec3], held: &[bool]) -> Option<NurbsSurface> {
    let start = self::points(surface);
    if held.len() != start.len() {
        return None;
    }
    // Each free control point's place among the unknowns.
    let mut slot = vec![usize::MAX; start.len()];
    let mut free = 0usize;
    for (i, h) in held.iter().enumerate() {
        if !h {
            slot[i] = free;
            free += 1;
        }
    }
    if free == 0 || points.is_empty() {
        return None;
    }
    let directions = facing(surface, &start)?;
    let mut net = start.clone();
    let mut current = surface.clone();
    // The parameters of each point carried from round to round, from the sampled guesses at first.
    let guesses = samples(surface, &net);
    let mut params: Vec<(f64, f64)> =
        points.iter().filter_map(|p| guesses.iter().min_by(|a, b| a.2.dist(*p).total_cmp(&b.2.dist(*p))).map(|g| (g.0, g.1))).collect();
    if params.len() != points.len() {
        return None;
    }
    let [nu, nv] = surface.count();
    let id = |i: usize, j: usize| j * nu + i;
    let mut stencils: Vec<[(usize, f64); 3]> = Vec::new();
    for j in 0..nv {
        for i in 1..nu.saturating_sub(1) {
            stencils.push([(id(i - 1, j), 1.0), (id(i, j), -2.0), (id(i + 1, j), 1.0)]);
        }
    }
    for i in 0..nu {
        for j in 1..nv.saturating_sub(1) {
            stencils.push([(id(i, j - 1), 1.0), (id(i, j), -2.0), (id(i, j + 1), 1.0)]);
        }
    }
    for _ in 0..3 {
        // The places of the points on the surface as it now is.
        for (p, uv) in points.iter().zip(params.iter_mut()) {
            if let Some(f) = foot_from(&current, &net, *p, uv.0, uv.1) {
                *uv = (f.u, f.v);
            }
        }
        // Free point k is start[k] + h[k] · directions[k]; the heights h are the unknowns.
        let mut m = vec![0.0f64; free * free];
        let mut rhs = vec![0.0f64; free];
        let mut used = false;
        for (p, &(u, v)) in points.iter().zip(&params) {
            let Some(b) = basis(&current, u, v) else { continue };
            // What the surface is there with every free point at its start, taken off the point.
            let mut target = *p;
            for (k, &c) in b.cols.iter().enumerate() {
                target -= if held[c] { net[c] } else { start[c] } * b.value[k];
            }
            for (ka, &ca) in b.cols.iter().enumerate() {
                let Some(&ia) = slot.get(ca).filter(|s| **s != usize::MAX) else { continue };
                let (va, na) = (b.value[ka], directions[ca]);
                rhs[ia] += va * na.dot(target);
                for (kb, &cb) in b.cols.iter().enumerate() {
                    if let Some(&ib) = slot.get(cb).filter(|s| **s != usize::MAX) {
                        m[ia * free + ib] += va * b.value[kb] * na.dot(directions[cb]);
                        used = true;
                    }
                }
            }
        }
        if !used {
            return None;
        }
        // A light pull toward each free point's own place (one no scan point reaches stays put),
        // and a lighter hold on how unevenly neighbours move (a held point moving nothing), so one
        // the scan hardly reaches goes with its neighbours instead of staying behind as a bump.
        let trace: f64 = (0..free).map(|i| m[i * free + i]).sum::<f64>() / free as f64;
        for i in 0..free {
            m[i * free + i] += (1e-4 * trace).max(1e-12);
        }
        let smooth = 1e-4 * trace;
        for stencil in &stencils {
            for &(ca, wa) in stencil {
                let a = slot[ca];
                for &(cb, wb) in stencil {
                    let b = slot[cb];
                    if a != usize::MAX && b != usize::MAX {
                        m[a * free + b] += smooth * wa * wb;
                    }
                }
            }
        }
        let heights = crate::surface::cholesky_solve(&mut m, &rhs, free)?;
        for (i, &s) in slot.iter().enumerate() {
            if let Some(h) = heights.get(s) {
                net[i] = start[i] + directions[i] * *h;
            }
        }
        current = with_points(surface, &net)?;
    }
    Some(current)
}

/// The way the surface faces at each control point: its normal at the point's Greville parameters
/// (where the point pulls hardest), turned to agree with the surface's middle.
fn facing(surface: &NurbsSurface, net: &[Vec3]) -> Option<Vec<Vec3>> {
    let [pu, pv] = surface.degree();
    let [nu, nv] = surface.count();
    let [ku, kv] = surface.knots();
    let greville = |knots: &[f64], p: usize, i: usize| knots.get(i + 1..i + p + 1).map(|k| k.iter().sum::<f64>() / p as f64);
    let [[u0, u1], [v0, v1]] = surface.domain();
    let normal = |u: f64, v: f64| frame(surface, net, u.clamp(u0, u1), v.clamp(v0, v1)).and_then(|(_, su, sv)| su.cross(sv).normalized());
    let middle = normal((u0 + u1) / 2.0, (v0 + v1) / 2.0)?;
    let mut out = Vec::with_capacity(nu * nv);
    for j in 0..nv {
        for i in 0..nu {
            let n = normal(greville(ku, pu, i)?, greville(kv, pv, j)?).unwrap_or(middle);
            out.push(if n.dot(middle) < 0.0 { -n } else { n });
        }
    }
    Some(out)
}

/// The control points within `ring` steps (each way) of `(i, j)` on a net `count` across.
pub fn around(count: [usize; 2], i: usize, j: usize, ring: usize) -> Vec<bool> {
    let [nu, nv] = count;
    (0..nu * nv).map(|k| (k % nu).abs_diff(i) <= ring && (k / nu).abs_diff(j) <= ring).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::nearest_triangle;
    use crate::surface::fit_surface;

    /// A grid mesh of `z = f(x, y)` over `size` × `size`, `n` squares a side.
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

    fn dome(x: f64, y: f64) -> f64 {
        8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2))
    }

    /// A point of a dome's surface pushed 3 mm off comes back, with the ring round it, to within a
    /// few hundredths of the scan; a point held stays exactly where it was put.
    #[test]
    fn a_point_pushed_off_is_refitted_back_to_the_scan() {
        let (positions, triangles) = field(60.0, 60, dome);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(30.0, 30.0, 8.0)).unwrap();
        let fit = fit_surface(&scan, seed, 100.0, 8, 0.0).unwrap();
        let on = covered(&scan, &fit.surface, 2.0, 20_000);
        assert!(on.len() > 4_000, "the dome is covered: {}", on.len());
        let (rms, _) = distance(&fit.surface, &on);
        assert!(rms < 0.01, "the fit is close to start with: {rms}");

        let count = fit.surface.count();
        let (i, j) = (3, 4);
        let mut net = points(&fit.surface);
        let k = j * count[0] + i;
        net[k] += Vec3::new(0.0, 0.0, 3.0);
        let pushed = with_points(&fit.surface, &net).unwrap();
        let (off, _) = distance(&pushed, &on);
        assert!(off > 0.1, "pushed off: {off}");

        // The point and its ring refitted: back on the scan.
        let mut held: Vec<bool> = around(count, i, j, 1).iter().map(|f| !f).collect();
        let back = refit(&pushed, &on, &held).unwrap();
        let (rms, _) = distance(&back, &on);
        assert!(rms < 0.02, "back on the scan: {rms}");
        // Near where the fit had it: the refit may slide points along the surface, which the fit
        // (heights over a plane) did not.
        assert!(points(&back)[k].dist(points(&fit.surface)[k]) < 0.5, "the point is near where the fit had it");

        // Held, the pushed point stays exactly put while the ring round it follows the scan.
        held[k] = true;
        let kept = refit(&pushed, &on, &held).unwrap();
        assert_eq!(points(&kept)[k], net[k]);
        let (nearer, _) = distance(&kept, &on);
        assert!(nearer < off, "the rest follow the scan round it: {nearer} < {off}");
        assert!(refit(&pushed, &on, &vec![true; net.len()]).is_none(), "nothing free");
    }

    #[test]
    fn a_foot_is_found_on_the_surface_and_off_its_edge_is_outside() {
        let (positions, triangles) = field(60.0, 30, dome);
        let scan = Scan::new(&positions, &triangles);
        let seed = nearest_triangle(&scan, Vec3::new(30.0, 30.0, 8.0)).unwrap();
        let fit = fit_surface(&scan, seed, 100.0, 6, 0.0).unwrap();
        let f = foot(&fit.surface, Vec3::new(30.0, 30.0, 12.0)).unwrap();
        assert!(f.inside && f.point.dist(Vec3::new(30.0, 30.0, dome(30.0, 30.0))) < 0.05, "{f:?}");
        let off = foot(&fit.surface, Vec3::new(200.0, 30.0, 0.0)).unwrap();
        assert!(!off.inside, "beyond the edge: {off:?}");
    }
}
