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

use crate::place::Rigid;
use crate::{Scan, thinned};

/// A surface's basis at a parameter: the control points it depends on (index `j·nu + i`) and each
/// one's rational basis value and its derivatives along u and v.
pub(crate) struct Basis {
    pub(crate) cols: Vec<usize>,
    pub(crate) value: Vec<f64>,
    pub(crate) du: Vec<f64>,
    pub(crate) dv: Vec<f64>,
}

pub(crate) fn basis(surface: &NurbsSurface, u: f64, v: f64) -> Option<Basis> {
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
pub(crate) fn frame(surface: &NurbsSurface, net: &[Vec3], u: f64, v: f64) -> Option<(Vec3, Vec3, Vec3)> {
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
pub(crate) fn samples(surface: &NurbsSurface, net: &[Vec3]) -> Vec<(f64, f64, Vec3)> {
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
pub(crate) fn foot_from(surface: &NurbsSurface, net: &[Vec3], p: Vec3, mut u: f64, mut v: f64) -> Option<Foot> {
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
    covered_facing(scan, surface, reach, limit).0
}

/// [`covered`], and which way the scan's outside is from the surface: 1 along the surface's own
/// normal (u × v), -1 against it.
pub fn covered_facing(scan: &Scan, surface: &NurbsSurface, reach: f64, limit: usize) -> (Vec<Vec3>, f64) {
    let net = points(surface);
    let Some(first) = net.first().copied() else { return (Vec::new(), 1.0) };
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
    (thinned(&kept, limit), if sign < 0.0 { -1.0 } else { 1.0 })
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

/// The way the surface faces at control point `index` (u fastest): its normal where the point
/// pulls hardest, the way a refit moves it.
pub fn normal_at(surface: &NurbsSurface, index: usize) -> Option<Vec3> {
    facing(surface, &points(surface))?.get(index).copied()
}

/// The rigid move that takes the net `from` onto the net `to` (`count` across, u fastest), if
/// there is one. A surface moved by a
/// later feature has its imported net moved so.
pub fn placement(from: &[Vec3], to: &[Vec3], count: [usize; 2]) -> Option<Rigid> {
    (from.len() == count[0] * count[1]).then_some(())?;
    Rigid::between(from, to)
}

/// The control points within `ring` steps (each way) of `(i, j)` on a net `count` across.
pub fn around(count: [usize; 2], i: usize, j: usize, ring: usize) -> Vec<bool> {
    let [nu, nv] = count;
    (0..nu * nv).map(|k| (k % nu).abs_diff(i) <= ring && (k / nu).abs_diff(j) <= ring).collect()
}

/// How much of a move a control point `distance` from the nearest point moved takes, with a
/// falloff of `radius`: all of it at the points moved, none from the radius out, and a smooth step
/// between (one less the smoothstep `3t² − 2t³` of `t = distance / radius`, level at both ends).
/// With no radius only the points moved move.
pub fn falloff_weight(distance: f64, radius: f64) -> f64 {
    if !(radius > 0.0) {
        return if distance <= 0.0 { 1.0 } else { 0.0 };
    }
    let t = (distance / radius).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// How much of a move each point of `net` takes when the points `chosen` are moved with a falloff
/// of `radius` (mm, from the nearest one chosen, as the net is): 1 at those chosen, none where
/// `held` (a held point chosen does not move either).
pub fn falloff_weights(net: &[Vec3], chosen: &[usize], held: &[bool], radius: f64) -> Vec<f64> {
    let chosen: Vec<Vec3> = chosen.iter().filter_map(|&i| net.get(i).copied()).collect();
    net.iter()
        .enumerate()
        .map(|(i, p)| {
            if held.get(i).copied().unwrap_or(false) || chosen.is_empty() {
                return 0.0;
            }
            falloff_weight(chosen.iter().map(|c| c.dist(*p)).fold(f64::INFINITY, f64::min), radius)
        })
        .collect()
}

/// The net with the points `moves` put where they go and the others within `radius` of them
/// carried along by their [`falloff_weight`] of the nearest one's distance, each by the moves of
/// the points moved blended by how near each is, so that one move carries its neighbours exactly
/// as far as their weight says (a soft selection). Points `held` stay. The net, and which points
/// the falloff carried (not those moved).
pub fn with_falloff(net: &[Vec3], moves: &[(usize, Vec3)], held: &[bool], radius: f64) -> (Vec<Vec3>, Vec<bool>) {
    let mut out = net.to_vec();
    let mut carried = vec![false; net.len()];
    // Each move as where its point was and how far it goes.
    let from: Vec<(Vec3, Vec3)> = moves.iter().filter_map(|&(i, to)| net.get(i).map(|p| (*p, to - *p))).collect();
    if radius > 0.0 && !from.is_empty() {
        for (i, p) in net.iter().enumerate() {
            if held.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (mut sum, mut delta, mut nearest) = (0.0, Vec3::ZERO, f64::INFINITY);
            for (at, d) in &from {
                let gap = at.dist(*p);
                let w = falloff_weight(gap, radius);
                sum += w;
                delta += *d * w;
                nearest = nearest.min(gap);
            }
            let w = falloff_weight(nearest, radius);
            if w > 0.0 && sum > 0.0 {
                out[i] = *p + delta * (w / sum);
                carried[i] = true;
            }
        }
    }
    // The points moved go exactly where they were put.
    for &(i, to) in moves {
        if let Some(q) = out.get_mut(i) {
            *q = to;
            carried[i] = false;
        }
    }
    (out, carried)
}

/// The Greville places of the surface's control points (u fastest): where each pulls hardest, kept
/// in the domain.
pub fn greville_places(surface: &NurbsSurface) -> Vec<(f64, f64)> {
    let [nu, nv] = surface.count();
    let [pu, pv] = surface.degree();
    let [ku, kv] = surface.knots();
    let [[u0, u1], [v0, v1]] = surface.domain();
    let at = |k: &[f64], i: usize, p: usize| if p == 0 { k.get(i).copied().unwrap_or(0.0) } else { crate::net::greville(k, i, p) };
    (0..nv).flat_map(|j| (0..nu).map(move |i| (i, j))).map(|(i, j)| (at(ku, i, pu).clamp(u0, u1), at(kv, j, pv).clamp(v0, v1))).collect()
}

/// The steps in (u, v) that carry each control point's Greville place about `along[0]` mm along u
/// and `along[1]` along v on the surface (each by the surface's speed there), for [`slid`].
pub fn slide_steps(surface: &NurbsSurface, along: [f64; 2]) -> Vec<(f64, f64)> {
    let net = points(surface);
    greville_places(surface)
        .into_iter()
        .map(|(u, v)| {
            let Some((_, su, sv)) = frame(surface, &net, u, v) else { return (0.0, 0.0) };
            let per = |speed: f64, mm: f64| if speed > 1e-12 && mm.is_finite() { mm / speed } else { 0.0 };
            (per(su.len(), along[0]), per(sv.len(), along[1]))
        })
        .collect()
}

/// Slide (after Plasticity's): each control point that takes a share (`weights`) moved along the
/// surface rather than through space, by that share of the way the surface goes from its Greville
/// place to that place shifted by `step` for it (kept in the domain): `P + w · (S(g + Δ) − S(g))`.
/// So the net slides over the surface and the surface keeps its shape as far as its points allow.
/// `None` when the surface cannot be evaluated.
pub fn slid(surface: &NurbsSurface, weights: &[f64], step: impl Fn(usize) -> (f64, f64)) -> Option<Vec<Vec3>> {
    let net = points(surface);
    let [[u0, u1], [v0, v1]] = surface.domain();
    greville_places(surface)
        .into_iter()
        .enumerate()
        .map(|(i, (u, v))| {
            let p = *net.get(i)?;
            let w = weights.get(i).copied().unwrap_or(0.0);
            if !(w > 0.0) {
                return Some(p);
            }
            let (du, dv) = step(i);
            let (du, dv) = (if du.is_finite() { du } else { 0.0 }, if dv.is_finite() { dv } else { 0.0 });
            let (from, _, _) = frame(surface, &net, u, v)?;
            let (to, _, _) = frame(surface, &net, (u + du).clamp(u0, u1), (v + dv).clamp(v0, v1))?;
            Some(p + (to - from) * w)
        })
        .collect()
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

    /// A net turned and moved is found to be so, and the move undone; a net bent is no move.
    #[test]
    fn the_placement_of_a_moved_net_is_found() {
        let net: Vec<Vec3> = (0..20).map(|k| Vec3::new(f64::from(k % 5) * 3.0, f64::from(k / 5) * 2.0, 0.1 * f64::from(k * k % 7))).collect();
        let by = Rigid::turn(Vec3::new(0.2, 0.5, 0.8).normalized().unwrap(), 0.7);
        let by = Rigid { translation: Vec3::new(4.0, -2.0, 9.0), ..by };
        let moved: Vec<Vec3> = net.iter().map(|p| by.apply(*p)).collect();
        let found = placement(&net, &moved, [5, 4]).unwrap();
        assert!(net.iter().zip(&moved).all(|(p, q)| found.apply(*p).dist(*q) < 1e-9 && found.inverse().apply(*q).dist(*p) < 1e-9));
        let mut bent = moved.clone();
        bent[7] += Vec3::new(0.0, 0.0, 0.5);
        assert!(placement(&net, &bent, [5, 4]).is_none());
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

    /// A flat bicubic net, `n` points a side, 10 mm apart, clamped knots on [0, 1].
    fn flat(n: usize) -> NurbsSurface {
        let mut knots = vec![0.0; 4];
        knots.extend((1..n - 3).map(|k| k as f64 / (n - 3) as f64));
        knots.extend([1.0; 4]);
        let net: Vec<Vec3> = (0..n * n).map(|k| Vec3::new((k % n) as f64 * 10.0, (k / n) as f64 * 10.0, 0.0)).collect();
        NurbsSurface::new([3, 3], [knots.clone(), knots], &net, &vec![1.0; n * n]).unwrap()
    }

    #[test]
    fn the_falloff_takes_all_at_the_point_none_past_the_radius_and_half_half_way() {
        assert_eq!(falloff_weight(0.0, 10.0), 1.0);
        assert_eq!(falloff_weight(10.0, 10.0), 0.0);
        assert_eq!(falloff_weight(25.0, 10.0), 0.0);
        assert!((falloff_weight(5.0, 10.0) - 0.5).abs() < 1e-12);
        assert_eq!(falloff_weight(0.0, 0.0), 1.0, "no radius: the point moved only");
        assert_eq!(falloff_weight(1.0, 0.0), 0.0);
        assert_eq!(falloff_weight(1.0, f64::NAN), 0.0);
        let net = points(&flat(5));
        let w = falloff_weights(&net, &[12], &[false; 25], 15.0);
        assert_eq!(w[12], 1.0);
        assert!(w[13] > 0.0 && w[13] < 1.0 && w[14] == 0.0, "{w:?}");
        let held = (0..25).map(|k| k == 13).collect::<Vec<_>>();
        assert_eq!(falloff_weights(&net, &[12], &held, 15.0)[13], 0.0, "held");
    }

    #[test]
    fn a_point_moved_with_a_falloff_carries_its_neighbours_less_the_further_they_are() {
        let net = points(&flat(5));
        let up = Vec3::new(0.0, 0.0, 6.0);
        let (moved, carried) = with_falloff(&net, &[(12, net[12] + up)], &[false; 25], 20.0);
        assert_eq!(moved[12], net[12] + up, "the point goes where it was put");
        assert!(!carried[12]);
        // One step away (10 mm): half; two steps (20 mm): none; across (14.1 mm): less than half.
        assert!((moved[13].z - 3.0).abs() < 1e-9 && carried[13], "{:?}", moved[13]);
        assert_eq!(moved[14], net[14]);
        assert!(!carried[14]);
        assert!(moved[18].z > 0.0 && moved[18].z < 3.0);
        // A held neighbour stays; with no radius nothing but the point moves.
        let held = (0..25).map(|k| k == 13).collect::<Vec<_>>();
        assert_eq!(with_falloff(&net, &[(12, net[12] + up)], &held, 20.0).0[13], net[13]);
        let (alone, none) = with_falloff(&net, &[(12, net[12] + up)], &[false; 25], 0.0);
        assert!(alone.iter().enumerate().all(|(k, p)| k == 12 || *p == net[k]) && !none.iter().any(|c| *c));
        // Two points moved the same way carry the one between them as the nearer one alone would;
        // moved opposite ways, their moves cancel there.
        let (two, _) = with_falloff(&net, &[(11, net[11] + up), (13, net[13] + up)], &[false; 25], 20.0);
        assert!((two[12].z - 3.0).abs() < 1e-9, "{:?}", two[12]);
        let (apart, _) = with_falloff(&net, &[(11, net[11] + up), (13, net[13] - up)], &[false; 25], 20.0);
        assert!(apart[12].z.abs() < 1e-9, "{:?}", apart[12]);
    }

    #[test]
    fn a_net_slid_along_a_flat_surface_moves_in_its_plane_by_the_length_asked() {
        let s = flat(6);
        let n = s.count()[0] * s.count()[1];
        let steps = slide_steps(&s, [3.0, 0.0]);
        let middle = 2 * 6 + 2;
        let slid = slid(&s, &vec![1.0; n], |i| steps[i]).unwrap();
        let before = points(&s);
        // Inside the net, every point goes about 3 mm along u (by the surface's speed where it
        // starts) and stays in the plane.
        let d = slid[middle] - before[middle];
        assert!((d.x - 3.0).abs() < 0.3 && d.y.abs() < 1e-9 && d.z.abs() < 1e-12, "{d:?}");
        // Points at the far edge are held by the edge of the domain.
        let last = 5;
        assert!((slid[last] - before[last]).len() < 1e-9);
        // Only the points with a share move; a share of a half goes half way.
        let mut w = vec![0.0; n];
        w[middle] = 0.5;
        let half = super::slid(&s, &w, |i| steps[i]).unwrap();
        assert!(((half[middle] - before[middle]).x - d.x / 2.0).abs() < 1e-9);
        assert!(half.iter().zip(&before).enumerate().all(|(k, (a, b))| k == middle || a == b));
        // Hostile steps leave a point where it is.
        let still = super::slid(&s, &vec![1.0; n], |_| (f64::NAN, f64::INFINITY)).unwrap();
        assert!(still.iter().zip(&before).all(|(a, b)| a.is_finite() && (a.x - b.x).abs() < 1e-9));
    }

    #[test]
    fn greville_places_lie_in_the_domain_one_a_point() {
        let s = flat(7);
        let g = greville_places(&s);
        assert_eq!(g.len(), 49);
        assert_eq!(g[0], (0.0, 0.0));
        assert_eq!(g[48], (1.0, 1.0));
        assert!(g.windows(2).take(6).all(|w| w[1].0 > w[0].0));
    }
}
