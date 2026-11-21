//! Wrap Scan: one patch network over a whole scan, for parts no exact face describes (cast and
//! organic parts). Design: `~/Projects/solidcraft-handoff/designs/A1-wrap-and-hybrid.md`.
//!
//! The layout and the rounds are adapted from d0pawlus/SurfaceCraft's `surface.wrap` (MIT OR
//! Apache-2.0, `geom/src/reduce.rs`, `fit/src/patch.rs`, `engine/src/cmd/wrap_rounds.rs`); the fit
//! is ours: where its patches pass through about ten points of the scan each, ours are fitted to
//! the scan by least squares, so noise averages out instead of being copied.
//!
//! 1. **Coarse mesh** ([`coarse`]): the scan reduced by [`crate::reduce::reduce_held`], every
//!    collapse held within a budget of the scan; then measured both ways (every scan point to the
//!    coarse mesh, every coarse point and face middle to the scan). Where a sample is farther than
//!    the tolerance, the budget of the scan points within two rings of it is halved and the scan
//!    reduced again from the start, up to [`ROUNDS`] times.
//! 2. **Layout**: every coarse triangle becomes three quads (a corner, the two edge midpoints
//!    beside it and the middle), so neighbouring quads share whole sides and no side ends on
//!    another's middle.
//! 3. **Sides**: each quad side is one cubic Bézier, its ends on the scan, its two inner control
//!    points fitted by least squares to the scan's points under seven places along it. A side is
//!    fitted once and used by both its quads.
//! 4. **Patches**: each quad is one bicubic Bézier with its twelve border points from its sides; the
//!    four inner points are fitted by least squares to the scan's points under an 8 × 8 grid of
//!    places, held lightly to the Coons patch of the sides where the scan says little, and the
//!    places moved to their feet once more.
//!
//! A place's "point under" is the nearest point of the scan; it counts only when that point's
//! triangle faces within about 72° of the coarse triangle (cos 0.3, as d0's), so a thin wall's far
//! side is never taken. Continuity across sides is position only (G0); tangency is a later step.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::{Scan, par, reduce, welded};

/// Rounds of reduction at most (d0's: 7).
pub const ROUNDS: usize = 7;
/// The first budget as a share of the tolerance (collapses add up; the rounds fix the rest).
const START: f64 = 0.8;
/// A budget's factor round a sample that is too far.
const TIGHTEN: f64 = 0.5;
/// Rings of scan points tightened round a sample that is too far.
const RINGS: usize = 2;
/// Scan points measured at most each round (spread evenly over the scan's points).
const SAMPLES: usize = 200_000;
/// A point under a place counts when its triangle faces the coarse triangle this much (cos).
const FACING: f64 = 0.3;
/// Places along a side whose points under it the side is fitted to.
const SIDE_PLACES: usize = 7;
/// Places across a patch, each way.
const PATCH_PLACES: usize = 8;
/// How much the Coons patch holds the inner points, as a share of the places' weight.
const HOLD: f64 = 0.02;

/// What a wrap aims for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// How far the patches may be from the scan (mm).
    pub tolerance: f64,
    /// The share of the scan's samples to be within it, both ways, for the coarse mesh (0..1).
    pub share: f64,
    /// The most patches (three a coarse triangle); a coarse mesh that needs more stops short.
    pub max_patches: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tolerance: 0.2, share: 0.99, max_patches: 12_000 }
    }
}

/// Why a wrap gives no patches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WrapError {
    /// The scan has no triangles, or the settings are out of range.
    Input(&'static str),
    /// The scan could not be reduced at all.
    Reduce,
}

impl std::fmt::Display for WrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WrapError::Input(why) => write!(f, "{why}"),
            WrapError::Reduce => write!(f, "the scan could not be reduced"),
        }
    }
}

/// Distances of one direction of a measurement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spread {
    pub samples: usize,
    /// Share within the tolerance.
    pub within: f64,
    pub p99: f64,
    pub max: f64,
}

/// The coarse mesh and how it measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Coarse {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Scan points to the coarse mesh.
    pub scan_to_coarse: Spread,
    /// Coarse points and face middles to the scan.
    pub coarse_to_scan: Spread,
    pub rounds: usize,
    /// Both ways within the tolerance for the asked share, nothing beyond twice the tolerance.
    pub met: bool,
    /// The most patches bounded the mesh: the first round was reduced to the cap regardless of the
    /// tolerance, or a later round that needed more was dropped for the last that fitted.
    pub capped: bool,
}

/// A wrap: its patches and how far they are from the scan.
#[derive(Clone, Debug)]
pub struct Wrap {
    pub faces: Vec<NurbsSurface>,
    pub coarse: Coarse,
    /// Scan points to the patches (each patch drawn as a 6 × 6 grid of triangles).
    pub scan_to_patches: Spread,
}

fn spread(d: &mut [f64], tolerance: f64) -> Spread {
    let n = d.len();
    if n == 0 {
        return Spread { samples: 0, within: 1.0, p99: 0.0, max: 0.0 };
    }
    let within = d.iter().filter(|&&x| x <= tolerance).count() as f64 / n as f64;
    let max = d.iter().copied().fold(0.0, f64::max);
    let k = ((n as f64 * 0.99).ceil() as usize).clamp(1, n) - 1;
    d.select_nth_unstable_by(k, f64::total_cmp);
    Spread { samples: n, within, p99: d[k], max }
}

/// Every `step`-th index below `n`, so at most `most` of them.
fn evenly(n: usize, most: usize) -> Vec<usize> {
    let step = n.div_ceil(most.max(1)).max(1);
    (0..n).step_by(step).collect()
}

/// The scan reduced as far as it can be within the tolerance (see the module).
pub fn coarse(scan: &Scan, settings: &Settings) -> Result<Coarse, WrapError> {
    let tol = settings.tolerance;
    if !(tol.is_finite() && tol > 0.0) || !(settings.share > 0.0 && settings.share <= 1.0) {
        return Err(WrapError::Input("the tolerance must be above zero and the share in (0, 1]"));
    }
    if scan.triangle_count() == 0 {
        return Err(WrapError::Input("the scan has no triangles"));
    }
    let tree = TriangleTree::of(scan.positions, scan.triangles);
    let rep = welded(scan.positions);
    let rings = rings(scan, &rep);
    let n = scan.positions.len();
    let used: Vec<usize> = {
        let mut u = vec![false; n];
        for t in scan.triangles {
            for &v in t {
                if let Some(x) = u.get_mut(v as usize) {
                    *x = true;
                }
            }
        }
        (0..n).filter(|&v| u[v]).collect()
    };
    let sampled: Vec<usize> = evenly(used.len(), SAMPLES).into_iter().filter_map(|k| used.get(k).copied()).collect();
    let mut budget = vec![START * tol; n];
    let mut mark = vec![0usize; n];
    let mut best: Option<Coarse> = None;
    for round in 1..=ROUNDS {
        let held = reduce::Settings { target: 4, max_error: 0.0, keep_rim: true };
        let Some(mut r) = reduce::reduce_held(scan, held, &budget, &tree) else { return Err(WrapError::Reduce) };
        let cap = (settings.max_patches / 3).max(4);
        let mut capped = false;
        if r.triangles.len() > cap {
            if let Some(b) = best.as_mut() {
                // A later round that would need more patches than allowed: keep the last that fitted.
                b.capped = true;
                break;
            }
            // The first round already needs more: reduced to the cap, the tolerance given up where
            // it must be (d0's rule), and measured as it is.
            let to_cap = reduce::Settings { target: cap, max_error: 0.0, keep_rim: true };
            r = reduce::reduce(scan, to_cap).ok_or(WrapError::Reduce)?;
            capped = true;
        }
        if r.triangles.is_empty() {
            return Err(WrapError::Reduce);
        }
        let ctree = TriangleTree::of(&r.positions, &r.triangles);
        let forward: Vec<f64> = par::map(sampled.len(), |k| ctree.nearest(scan.positions[sampled[k]]).map_or(f64::INFINITY, |(d, _)| d));
        let probes: Vec<Vec3> = r
            .positions
            .iter()
            .copied()
            .chain(r.triangles.iter().map(|t| (r.positions[t[0] as usize] + r.positions[t[1] as usize] + r.positions[t[2] as usize]) / 3.0))
            .collect();
        let backward: Vec<(f64, u32)> = par::map(probes.len(), |k| tree.nearest(probes[k]).unwrap_or((f64::INFINITY, u32::MAX)));
        let mut fd = forward.clone();
        let mut bd: Vec<f64> = backward.iter().map(|(d, _)| *d).collect();
        let (sf, sb) = (spread(&mut fd, tol), spread(&mut bd, tol));
        let met = sf.within >= settings.share && sb.within >= settings.share && sf.max <= 2.0 * tol && sb.max <= 2.0 * tol;
        let this = Coarse { positions: r.positions, triangles: r.triangles, scan_to_coarse: sf, coarse_to_scan: sb, rounds: round, met, capped };
        let done = met || capped || round == ROUNDS;
        best = Some(this);
        if done {
            break;
        }
        // Keep detail round every sample that is too far.
        for (k, &d) in forward.iter().enumerate() {
            if d > tol
                && let Some(&v) = sampled.get(k)
            {
                tighten(v, &rep, &rings, &mut budget, &mut mark, round);
            }
        }
        for &(d, t) in &backward {
            if d > tol
                && let Some(tri) = scan.triangles.get(t as usize)
            {
                for &v in tri {
                    tighten(v as usize, &rep, &rings, &mut budget, &mut mark, round);
                }
            }
        }
    }
    best.ok_or(WrapError::Reduce)
}

/// Each welded point's neighbours (by its representative).
fn rings(scan: &Scan, rep: &[u32]) -> Vec<Vec<u32>> {
    let mut r = vec![Vec::new(); scan.positions.len()];
    for t in scan.triangles {
        let t = t.map(|v| rep.get(v as usize).copied().unwrap_or(v));
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if let (Some(ra), true) = (r.get_mut(a as usize), (b as usize) < scan.positions.len()) {
                ra.push(b);
            }
            if let (Some(rb), true) = (r.get_mut(b as usize), (a as usize) < scan.positions.len()) {
                rb.push(a);
            }
        }
    }
    r
}

/// Halves the budget of `seed` and the points within [`RINGS`] rings of it, once a round; every scan
/// point welded to a tightened one gets its budget too (the reducer takes the least of them).
fn tighten(seed: usize, rep: &[u32], rings: &[Vec<u32>], budget: &mut [f64], mark: &mut [usize], round: usize) {
    let start = rep.get(seed).map_or(seed, |&r| r as usize);
    let mut frontier = vec![start];
    for _ in 0..=RINGS {
        let mut next = Vec::new();
        for &v in &frontier {
            let Some(m) = mark.get_mut(v) else { continue };
            if *m == round {
                continue;
            }
            *m = round;
            if let Some(b) = budget.get_mut(v) {
                *b *= TIGHTEN;
            }
            if let Some(r) = rings.get(v) {
                next.extend(r.iter().map(|&x| x as usize));
            }
        }
        frontier = next;
    }
}

/// The scan as something to project places onto, facing-aware.
struct Under<'a> {
    scan: &'a Scan<'a>,
    tree: TriangleTree,
}

impl Under<'_> {
    /// The nearest point of the scan to `p` whose triangle faces `facing` (cos ≥ [`FACING`]).
    fn under(&self, p: Vec3, facing: Vec3) -> Option<Vec3> {
        let (_, t) = self.tree.nearest(p)?;
        let [a, b, c] = self.scan.corners(t as usize)?;
        let n = (b - a).cross(c - a);
        let len = n.len();
        if !(len > 0.0) || (n / len).dot(facing) < FACING {
            return None;
        }
        Some(closest_on_triangle(p, a, b, c))
    }
}

fn bern(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// A cubic Bézier side from `a` to `b`: the two inner points fitted to the points under places
/// along the chord; the thirds of the chord where too few are found.
fn side(under: &Under, a: Vec3, b: Vec3, facing: Vec3) -> [Vec3; 4] {
    let (mut m11, mut m12, mut m22) = (0.0, 0.0, 0.0);
    let (mut r1, mut r2) = (Vec3::ZERO, Vec3::ZERO);
    let mut found = 0;
    for k in 1..=SIDE_PLACES {
        let t = k as f64 / (SIDE_PLACES + 1) as f64;
        let Some(q) = under.under(a + (b - a) * t, facing) else { continue };
        let w = bern(t);
        let rest = q - a * w[0] - b * w[3];
        m11 += w[1] * w[1];
        m12 += w[1] * w[2];
        m22 += w[2] * w[2];
        r1 += rest * w[1];
        r2 += rest * w[2];
        found += 1;
    }
    let thirds = [a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0)];
    let det = m11 * m22 - m12 * m12;
    if found < 3 || !(det.abs() > 1e-12) {
        return [a, thirds[0], thirds[1], b];
    }
    let p1 = (r1 * m22 - r2 * m12) / det;
    let p2 = (r2 * m11 - r1 * m12) / det;
    if !(p1.is_finite() && p2.is_finite()) {
        return [a, thirds[0], thirds[1], b];
    }
    [a, p1, p2, b]
}

/// The Coons patch of four cubic sides, as a 4 × 4 net (u fastest): `bottom` (v = 0) and `top`
/// (v = 1) run along u, `left` (u = 0) and `right` (u = 1) along v.
fn coons(bottom: &[Vec3; 4], top: &[Vec3; 4], left: &[Vec3; 4], right: &[Vec3; 4]) -> [Vec3; 16] {
    let mut net = [Vec3::ZERO; 16];
    for j in 0..4 {
        for i in 0..4 {
            let (u, v) = (i as f64 / 3.0, j as f64 / 3.0);
            let ruled_v = bottom[i] * (1.0 - v) + top[i] * v;
            let ruled_u = left[j] * (1.0 - u) + right[j] * u;
            let corners = bottom[0] * ((1.0 - u) * (1.0 - v)) + bottom[3] * (u * (1.0 - v)) + top[0] * ((1.0 - u) * v) + top[3] * (u * v);
            net[j * 4 + i] = ruled_v + ruled_u - corners;
        }
    }
    net
}

fn eval(net: &[Vec3; 16], u: f64, v: f64) -> Vec3 {
    let (bu, bv) = (bern(u), bern(v));
    let mut p = Vec3::ZERO;
    for j in 0..4 {
        for i in 0..4 {
            p += net[j * 4 + i] * (bu[i] * bv[j]);
        }
    }
    p
}

/// The four inner points (net indices 5, 6, 9, 10) fitted to the points under a grid of places,
/// held lightly to their starting places; twice, the places moved to their feet in between.
fn fit_inner(under: &Under, net: &mut [Vec3; 16], facing: Vec3) {
    const INNER: [usize; 4] = [5, 6, 9, 10];
    for _ in 0..2 {
        let mut m = [[0.0f64; 4]; 4];
        let mut r = [Vec3::ZERO; 4];
        let mut found = 0;
        for a in 0..PATCH_PLACES {
            for b in 0..PATCH_PLACES {
                let (u, v) = ((a as f64 + 0.5) / PATCH_PLACES as f64, (b as f64 + 0.5) / PATCH_PLACES as f64);
                let Some(q) = under.under(eval(net, u, v), facing) else { continue };
                let (bu, bv) = (bern(u), bern(v));
                let w: [f64; 16] = std::array::from_fn(|k| bu[k % 4] * bv[k / 4]);
                let mut rest = q;
                for k in 0..16 {
                    if !INNER.contains(&k) {
                        rest -= net[k] * w[k];
                    }
                }
                for (x, &kx) in INNER.iter().enumerate() {
                    for (y, &ky) in INNER.iter().enumerate() {
                        m[x][y] += w[kx] * w[ky];
                    }
                    r[x] += rest * w[kx];
                }
                found += 1;
            }
        }
        if found < 4 {
            return;
        }
        let hold = HOLD * found as f64 / 16.0;
        for (x, &k) in INNER.iter().enumerate() {
            m[x][x] += hold;
            r[x] += net[k] * hold;
        }
        let Some(sol) = solve4(m, r) else { return };
        if sol.iter().all(|p| p.is_finite()) {
            for (x, &k) in INNER.iter().enumerate() {
                net[k] = sol[x];
            }
        }
    }
}

/// `m x = r` for a symmetric positive definite 4 × 4 `m` (three right-hand sides at once). Indices
/// read as the textbook Cholesky does.
#[allow(clippy::needless_range_loop)]
fn solve4(mut m: [[f64; 4]; 4], r: [Vec3; 4]) -> Option<[Vec3; 4]> {
    for j in 0..4 {
        let mut d = m[j][j];
        for k in 0..j {
            d -= m[j][k] * m[j][k];
        }
        if !(d > 1e-300) {
            return None;
        }
        let d = d.sqrt();
        m[j][j] = d;
        for i in j + 1..4 {
            let mut s = m[i][j];
            for k in 0..j {
                s -= m[i][k] * m[j][k];
            }
            m[i][j] = s / d;
        }
    }
    let mut y = [Vec3::ZERO; 4];
    for i in 0..4 {
        let mut s = r[i];
        for k in 0..i {
            s -= y[k] * m[i][k];
        }
        y[i] = s / m[i][i];
    }
    let mut x = [Vec3::ZERO; 4];
    for i in (0..4).rev() {
        let mut s = y[i];
        for k in i + 1..4 {
            s -= x[k] * m[k][i];
        }
        x[i] = s / m[i][i];
    }
    Some(x)
}

/// The whole scan wrapped in patches (see the module).
pub fn wrap(scan: &Scan, settings: &Settings) -> Result<Wrap, WrapError> {
    let coarse = coarse(scan, settings)?;
    let under = Under { scan, tree: TriangleTree::of(scan.positions, scan.triangles) };
    let cp = &coarse.positions;
    // Layout points: the coarse corners, then each edge's midpoint, then each triangle's middle.
    let mut points: Vec<Vec3> = cp.clone();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut quads: Vec<([u32; 4], Vec3)> = Vec::with_capacity(coarse.triangles.len() * 3);
    for t in &coarse.triangles {
        let [a, b, c] = *t;
        let (pa, pb, pc) = (cp[a as usize], cp[b as usize], cp[c as usize]);
        let n = (pb - pa).cross(pc - pa);
        let len = n.len();
        if !(len > 0.0) {
            continue;
        }
        let facing = n / len;
        let mut m = |x: u32, y: u32| -> u32 {
            *mid.entry((x.min(y), x.max(y))).or_insert_with(|| {
                points.push((cp[x as usize] + cp[y as usize]) / 2.0);
                (points.len() - 1) as u32
            })
        };
        let (mab, mbc, mca) = (m(a, b), m(b, c), m(c, a));
        points.push((pa + pb + pc) / 3.0);
        let ct = (points.len() - 1) as u32;
        quads.push(([a, mab, ct, mca], facing));
        quads.push(([b, mbc, ct, mab], facing));
        quads.push(([c, mca, ct, mbc], facing));
    }
    // Every layout point onto the scan, facing the first quad that uses it.
    let mut facing_of = vec![Vec3::ZERO; points.len()];
    for (q, f) in &quads {
        for &p in q {
            if facing_of[p as usize] == Vec3::ZERO {
                facing_of[p as usize] = *f;
            }
        }
    }
    let placed: Vec<Vec3> = par::map(points.len(), |k| under.under(points[k], facing_of[k]).unwrap_or(points[k]));
    // Each side once, from its lower point to its higher one.
    let mut sides: HashMap<(u32, u32), Vec3> = HashMap::new();
    for (q, f) in &quads {
        for s in 0..4 {
            let (x, y) = (q[s], q[(s + 1) % 4]);
            sides.entry((x.min(y), x.max(y))).or_insert(*f);
        }
    }
    let keys: Vec<(u32, u32)> = sides.keys().copied().collect();
    let curves: Vec<[Vec3; 4]> = par::map(keys.len(), |k| {
        let (x, y) = keys[k];
        side(&under, placed[x as usize], placed[y as usize], sides[&(x, y)])
    });
    let curve: HashMap<(u32, u32), [Vec3; 4]> = keys.into_iter().zip(curves).collect();
    let along = |x: u32, y: u32| -> [Vec3; 4] {
        match curve.get(&(x.min(y), x.max(y))) {
            Some(c) if x < y => *c,
            Some(c) => [c[3], c[2], c[1], c[0]],
            None => {
                let (a, b) = (placed[x as usize], placed[y as usize]);
                [a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b]
            }
        }
    };
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let faces: Vec<Option<NurbsSurface>> = par::map(quads.len(), |k| {
        let ([q0, q1, q2, q3], facing) = quads[k];
        let (bottom, top, left, right) = (along(q0, q1), along(q3, q2), along(q0, q3), along(q1, q2));
        let mut net = coons(&bottom, &top, &left, &right);
        fit_inner(&under, &mut net, facing);
        NurbsSurface::new([3, 3], [knots.clone(), knots.clone()], &net, &[1.0; 16]).ok()
    });
    let faces: Vec<NurbsSurface> = faces.into_iter().flatten().collect();
    let scan_to_patches = measure(scan, &faces, settings.tolerance);
    Ok(Wrap { faces, coarse, scan_to_patches })
}

/// Scan points to the patches, each drawn as a 6 × 6 grid of triangles.
fn measure(scan: &Scan, faces: &[NurbsSurface], tolerance: f64) -> Spread {
    const G: usize = 6;
    let mut positions = Vec::with_capacity(faces.len() * (G + 1) * (G + 1));
    let mut triangles = Vec::with_capacity(faces.len() * G * G * 2);
    for f in faces {
        let base = positions.len() as u32;
        for j in 0..=G {
            for i in 0..=G {
                positions.push(f.eval(i as f64 / G as f64, j as f64 / G as f64).unwrap_or(Vec3::ZERO));
            }
        }
        let at = |i: usize, j: usize| base + (j * (G + 1) + i) as u32;
        for j in 0..G {
            for i in 0..G {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }
    let tree = TriangleTree::of(&positions, &triangles);
    let sampled = evenly(scan.positions.len(), SAMPLES);
    let mut d: Vec<f64> = par::map(sampled.len(), |k| tree.nearest(scan.positions[sampled[k]]).map_or(f64::INFINITY, |(d, _)| d));
    spread(&mut d, tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UV sphere of radius `r` with `rings` × `segments` cells, closed, faces outward.
    fn sphere(r: f64, rings: u32, segments: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut p = vec![Vec3::new(0.0, 0.0, r)];
        for i in 1..rings {
            let th = std::f64::consts::PI * i as f64 / rings as f64;
            for j in 0..segments {
                let ph = 2.0 * std::f64::consts::PI * j as f64 / segments as f64;
                p.push(Vec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
            }
        }
        p.push(Vec3::new(0.0, 0.0, -r));
        let bottom = (p.len() - 1) as u32;
        let at = |i: u32, j: u32| 1 + (i - 1) * segments + j % segments;
        let mut t = Vec::new();
        for j in 0..segments {
            t.push([0, at(1, j), at(1, j + 1)]);
            t.push([at(rings - 1, j), bottom, at(rings - 1, j + 1)]);
        }
        for i in 1..rings - 1 {
            for j in 0..segments {
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (p, t)
    }

    #[test]
    fn a_held_reduction_stays_within_its_budget_and_goes_far() {
        let (p, t) = sphere(20.0, 60, 120);
        let scan = Scan::new(&p, &t);
        let c = coarse(&scan, &Settings { tolerance: 0.1, ..Settings::default() }).expect("coarse");
        assert!(c.met, "{c:?}");
        assert!(c.scan_to_coarse.within >= 0.99 && c.coarse_to_scan.within >= 0.99, "{:?} {:?}", c.scan_to_coarse, c.coarse_to_scan);
        // Chord sag on R 20 within 0.1 mm allows edges of about 2·sqrt(2·20·0.1) = 4 mm: a few
        // hundred triangles, far fewer than the 14,160 of the input.
        assert!(c.triangles.len() < 3_000, "{} triangles", c.triangles.len());
    }

    #[test]
    fn a_sphere_wraps_within_its_tolerance_with_far_fewer_patches_than_triangles() {
        let (p, t) = sphere(20.0, 60, 120);
        let scan = Scan::new(&p, &t);
        let w = wrap(&scan, &Settings { tolerance: 0.1, ..Settings::default() }).expect("wrap");
        assert_eq!(w.faces.len(), w.coarse.triangles.len() * 3);
        assert!(w.scan_to_patches.within >= 0.99, "{:?}", w.scan_to_patches);
        assert!(w.faces.len() < 9_000, "{} patches", w.faces.len());
    }

    #[test]
    fn noise_averages_out_in_the_patches() {
        // The sphere with 0.05 mm of noise along the normals, wrapped to 0.2 mm: the patches lie
        // nearer the true sphere than the noisy scan does (least squares, not interpolation).
        let (mut p, t) = sphere(20.0, 60, 120);
        let mut seed = 12345u64;
        for q in p.iter_mut() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.1; // uniform ±0.05
            let len = q.len();
            *q = *q * ((len + n) / len);
        }
        let scan = Scan::new(&p, &t);
        let w = wrap(&scan, &Settings { tolerance: 0.2, ..Settings::default() }).expect("wrap");
        let mut worst = 0.0f64;
        let mut sum = 0.0;
        let mut count = 0.0;
        for f in &w.faces {
            for (u, v) in [(0.5, 0.5), (0.25, 0.75), (0.75, 0.25)] {
                let q = f.eval(u, v).expect("eval");
                let e = (q.len() - 20.0).abs();
                worst = worst.max(e);
                sum += e * e;
                count += 1.0;
            }
        }
        let rms = (sum / count).sqrt();
        // The scan's own rms off the sphere is 0.05 / sqrt(3) ≈ 0.029 mm.
        assert!(rms < 0.029, "patch rms {rms} off the true sphere (worst {worst})");
    }

    #[test]
    fn the_most_patches_bound_the_wrap() {
        let (p, t) = sphere(20.0, 60, 120);
        let scan = Scan::new(&p, &t);
        let w = wrap(&scan, &Settings { tolerance: 0.01, max_patches: 600, ..Settings::default() }).expect("wrap");
        assert!(w.coarse.capped && !w.coarse.met, "{:?}", w.coarse);
        assert!(w.faces.len() <= 600, "{} patches", w.faces.len());
    }

    #[test]
    fn hostile_input_is_refused() {
        let scan = Scan::new(&[], &[]);
        assert!(wrap(&scan, &Settings::default()).is_err());
        let (p, t) = sphere(5.0, 8, 16);
        let scan = Scan::new(&p, &t);
        for tol in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(wrap(&scan, &Settings { tolerance: tol, ..Settings::default() }).is_err());
        }
        assert!(wrap(&scan, &Settings { share: 1.5, ..Settings::default() }).is_err());
    }
}
