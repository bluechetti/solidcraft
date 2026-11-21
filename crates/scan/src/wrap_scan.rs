//! Wrap Scan: one patch network over a whole scan, for parts no exact face describes (cast and
//! organic parts). Design: `~/Projects/solidcraft-handoff/designs/A1-wrap-and-hybrid.md`.
//!
//! The layout and the rounds are adapted from d0pawlus/SurfaceCraft's `surface.wrap` (MIT OR
//! Apache-2.0, `geom/src/reduce.rs`, `fit/src/patch.rs`, `engine/src/cmd/wrap_rounds.rs`); the fit
//! is ours: where its patches pass through about ten points of the scan each, ours are fitted to
//! the scan by least squares, so noise averages out instead of being copied.
//!
//! 1. **Coarse mesh**: the scan reduced by [`crate::reduce::reduce_held`], every collapse held
//!    within a budget of the scan (twice the tolerance to start: the patches are fitted to the scan
//!    itself, so the coarse mesh may stray further than they may). More triangles than the most
//!    patches allow: every budget doubled and the scan reduced again, so the tolerance is given up
//!    evenly.
//! 5. **Rounds** ([`wrap`]): the patches measured against the scan; where a scan point is farther
//!    than the tolerance, the budget of the scan points within two rings of it is halved and steps
//!    1–4 run again from the scan, up to [`ROUNDS`] times, the best kept (d0's rounds, driven by
//!    the patches rather than by the coarse mesh).
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
//! A place's "point under" is the nearest point of the scan, taken along the coarse face's normal
//! only (the place keeps where it is across the face, so no corner or place can pass another and
//! fold a patch); it counts only when that point's triangle faces within about 72° of the coarse
//! triangle (cos 0.3, as d0's), so a thin wall's far side is never taken. Continuity across sides is position only (G0); tangency is a later step.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::{Scan, par, reduce, welded};

/// Rounds of reduction at most (d0's: 7).
pub const ROUNDS: usize = 7;
/// The first budget as a multiple of the tolerance: the patches are fitted to the scan, not to the
/// coarse mesh, so it may stray further than they may (d0 starts at four); the rounds fix the rest.
const START: f64 = 2.0;
/// Times the budgets are doubled to keep within the most patches before an unheld reduction.
const WIDEN: usize = 8;
/// A coarse face below this shape ([`shape`]: about a 12° smallest angle) and longer than [`LONG`]
/// tolerances is a needle to mend: its patches would be too thin for any tessellation to follow (a
/// 24 × 0.7 mm face made a patch that needed more than 20,000 nodes on one edge).
const NEEDLE: f64 = 0.35;
/// A coarse face whose smallest angle is below this is poor: its patches' corners are too sharp
/// for the kernel's check (a 4° corner left edges off their faces); 10°.
const MIN_ANGLE: f64 = 0.1745;
/// A degenerate face is a cap when its angle at its middle point is above this.
const CAP_ANGLE: f64 = 2.6; // ≈ 149°
/// Below this shape a face counts as degenerate: its normal says nothing.
const DEGENERATE: f64 = 0.1;
/// Two faces are flipped across their edge only when they bend by less than this (cos 20°).
const FLAT: f64 = 0.94;
/// Rounds of flips and splits.
const TIDY_ROUNDS: usize = 6;
/// Passes of relaxation at the end, the share of the way to the neighbours' middle each moves, and
/// the bend that makes an edge a crease (its points stay): cos 30°.
const RELAX_ROUNDS: usize = 8;
const RELAX_STEP: f64 = 0.5;
const CREASE: f64 = 0.866;

/// Halvings (on a log scale) between the last two widened budgets, to end near the cap.
const NARROW: usize = 3;
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

/// The coarse mesh the patches were laid on, and how the rounds went.
#[derive(Clone, Debug, PartialEq)]
pub struct Coarse {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub rounds: usize,
    /// The patches met the share within the tolerance, nothing beyond three tolerances.
    pub met: bool,
    /// The most patches bounded the wrap: the budgets were widened (or, at worst, the scan reduced
    /// unheld) to stay within the cap, so the tolerance was given up where it had to be.
    pub capped: bool,
}

impl Coarse {
    /// Coarse triangles (three patches each).
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }
}

/// A wrap: its patches and how far they are from the scan.
#[derive(Clone, Debug)]
pub struct Wrap {
    pub faces: Vec<NurbsSurface>,
    pub coarse: Coarse,
    /// Scan points to the patches (each patch drawn as a 6 × 6 grid of triangles).
    pub scan_to_patches: Spread,
    /// The scan points farthest from the patches, farthest first (at most [`WORST`]).
    pub worst: Vec<(Vec3, f64)>,
}

/// Places reported as the worst.
pub const WORST: usize = 8;

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

/// The scan reduced once, every collapse held within `budget` (per scan point); when that leaves
/// more than `cap` triangles, every budget is doubled and the scan reduced again (the tolerance
/// given up evenly, not where an unheld reduction happens to cut), up to [`WIDEN`] times; only then
/// is it reduced to the cap unheld. The bool says whether the cap bounded it.
fn reduce_once(scan: &Scan, tree: &TriangleTree, budget: &[f64], cap: usize) -> Result<(reduce::Reduced, bool), WrapError> {
    let held = reduce::Settings { target: 4, max_error: 0.0, keep_rim: true };
    let at = |scale: f64| -> Result<reduce::Reduced, WrapError> {
        let scaled: Vec<f64> = budget.iter().map(|b| b * scale).collect();
        reduce::reduce_held(scan, held, &scaled, tree).ok_or(WrapError::Reduce)
    };
    let first = at(1.0)?;
    if first.triangles.len() <= cap {
        return Ok((first, false));
    }
    // Doubled until it fits, then narrowed between the last two scales so the wrap ends near the
    // cap rather than up to half below it.
    let mut too_fine = 1.0;
    let mut fits: Option<(f64, reduce::Reduced)> = None;
    for widen in 1..=WIDEN {
        let scale = f64::from(1u32 << widen);
        let r = at(scale)?;
        if r.triangles.len() <= cap {
            fits = Some((scale, r));
            break;
        }
        too_fine = scale;
    }
    let Some((mut coarse_scale, mut best)) = fits else {
        let to_cap = reduce::Settings { target: cap, max_error: 0.0, keep_rim: false };
        return Ok((reduce::reduce(scan, to_cap).ok_or(WrapError::Reduce)?, true));
    };
    for _ in 0..NARROW {
        let mid = (too_fine * coarse_scale).sqrt();
        let r = at(mid)?;
        if r.triangles.len() <= cap {
            coarse_scale = mid;
            best = r;
        } else {
            too_fine = mid;
        }
    }
    Ok((best, true))
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
    /// Where `p` meets the scan along `facing`: its height taken from the nearest point of the
    /// scan whose triangle faces `facing` (cos ≥ [`FACING`]).
    fn under(&self, p: Vec3, facing: Vec3) -> Option<Vec3> {
        let (_, t) = self.tree.nearest(p)?;
        let [a, b, c] = self.scan.corners(t as usize)?;
        let n = (b - a).cross(c - a);
        let len = n.len();
        if !(len > 0.0) || (n / len).dot(facing) < FACING {
            return None;
        }
        let q = closest_on_triangle(p, a, b, c);
        // Along `facing` only: the place keeps where it is across the coarse face and takes the
        // scan's height. A sideways move as large as the tolerance would reorder the corners and
        // places of a thin face and fold its patches.
        Some(p + facing * (q - p).dot(facing))
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
    if !(p1.is_finite() && p2.is_finite()) || !in_order(a, p1, p2, b) {
        return [a, thirds[0], thirds[1], b];
    }
    [a, p1, p2, b]
}

/// Do the inner control points lie in order along the chord, well between its ends, and not far
/// off it? A side fitted to points that bunch at one end can otherwise turn back on itself (a
/// cusp), which no tessellation of the face can follow.
fn in_order(a: Vec3, p1: Vec3, p2: Vec3, b: Vec3) -> bool {
    let d = b - a;
    let l2 = d.dot(d);
    if !(l2 > 0.0) {
        return false;
    }
    let (s1, s2) = ((p1 - a).dot(d) / l2, (p2 - a).dot(d) / l2);
    let off = |p: Vec3, s: f64| (p - (a + d * s)).len();
    let reach = l2.sqrt();
    0.05 < s1 && s1 < s2 && s2 < 0.95 && off(p1, s1) <= reach && off(p2, s2) <= reach
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

/// Does every row and every column of the net run one way (each step along the row's or column's
/// own direction from end to end)? A net that doubles back makes a folded patch.
fn monotone(net: &[Vec3; 16]) -> bool {
    let at = |i: usize, j: usize| net[j * 4 + i];
    let runs_one_way = |p: [Vec3; 4]| {
        let way = p[3] - p[0];
        way.len() > 0.0 && (0..3).all(|k| (p[k + 1] - p[k]).dot(way) > 0.0)
    };
    (0..4).all(|j| runs_one_way([at(0, j), at(1, j), at(2, j), at(3, j)])) && (0..4).all(|i| runs_one_way([at(i, 0), at(i, 1), at(i, 2), at(i, 3)]))
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

/// The patches laid on a coarse mesh and fitted to the scan, in an order that keeps neighbours
/// together (the coarse triangles walked breadth first, three patches each), so a run of them is
/// one connected piece.
fn patches(under: &Under, positions: &[Vec3], triangles: &[[u32; 3]]) -> Vec<NurbsSurface> {
    let cp = positions;
    // Layout points: the coarse corners, then each edge's midpoint, then each triangle's middle.
    let mut points: Vec<Vec3> = cp.to_vec();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut quads: Vec<([u32; 4], Vec3)> = Vec::with_capacity(triangles.len() * 3);
    for t in walked(triangles) {
        let Some(&[a, b, c]) = triangles.get(t) else { continue };
        if [a, b, c].iter().any(|&v| v as usize >= cp.len()) {
            continue;
        }
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
        points.push(inner_point(pa, pb, pc));
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
        side(under, placed[x as usize], placed[y as usize], sides.get(&(x, y)).copied().unwrap_or(Vec3::Z))
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
        let start = coons(&bottom, &top, &left, &right);
        let mut net = start;
        fit_inner(under, &mut net, facing);
        // A fitted net that doubles back on itself is a folded patch: then the unfitted Coons net
        // of its sides. Never other sides: the sides are shared with the neighbouring patches,
        // and a side of its own would leave the body open there.
        if !monotone(&net) {
            net = start;
        }
        NurbsSurface::new([3, 3], [knots.clone(), knots.clone()], &net, &[1.0; 16]).ok()
    });
    faces.into_iter().flatten().collect()
}

/// A triangle's shape: 4√3 × its area over the sum of its sides' squares, 1 for an equilateral
/// triangle, falling to 0 as it becomes a needle or a sliver.
fn shape(t: [Vec3; 3]) -> f64 {
    let area2 = (t[1] - t[0]).cross(t[2] - t[0]).len();
    let sides = (t[1] - t[0]).dot(t[1] - t[0]) + (t[2] - t[1]).dot(t[2] - t[1]) + (t[0] - t[2]).dot(t[0] - t[2]);
    if sides > 0.0 { 2.0 * 3f64.sqrt() * area2 / sides } else { 0.0 }
}

fn unit_normal(t: [Vec3; 3]) -> Option<Vec3> {
    let n = (t[1] - t[0]).cross(t[2] - t[0]);
    let len = n.len();
    (len > 0.0).then(|| n / len)
}

/// Edges of the mesh: each side (lower point first) with the faces on it.
fn edge_faces(triangles: &[[u32; 3]]) -> HashMap<(u32, u32), Vec<usize>> {
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, tri) in triangles.iter().enumerate() {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    by_edge
}

/// The coarse mesh mended of long needles, which no patch should be laid on: edges flipped where
/// the two faces are nearly flat to each other, the new edge on the scan and the smallest angle
/// grows (a CAD export's fans of long thin faces become fair triangles of the same points); then
/// the longest side of each long needle left split at a point of the scan (a thin strip such as a
/// rounded rim gets points along it). Rounds until nothing changes.
fn tidy(positions: &mut [Vec3], triangles: &mut Vec<[u32; 3]>, under: &Under, tolerance: f64) {
    let on_scan = |p: Vec3| under.tree.nearest(p).is_some_and(|(d, _)| d <= tolerance);
    let corners = |positions: &[Vec3], t: [u32; 3]| t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
    for _ in 0..TIDY_ROUNDS {
        let mut changed = false;
        // Flips, one per face per pass.
        let by_edge = edge_faces(triangles);
        let mut touched = vec![false; triangles.len()];
        let mut keys: Vec<(u32, u32)> = by_edge.keys().copied().collect();
        keys.sort_unstable();
        for (a, b) in keys {
            let Some(faces) = by_edge.get(&(a, b)) else { continue };
            let &[t1, t2] = faces.as_slice() else { continue };
            if touched[t1] || touched[t2] {
                continue;
            }
            let (f1, f2) = (triangles[t1], triangles[t2]);
            // f1 runs x → y with z opposite; f2 runs y → x with w opposite.
            let Some(k1) = (0..3).find(|&k| (f1[k], f1[(k + 1) % 3]) == (a, b) || (f1[k], f1[(k + 1) % 3]) == (b, a)) else { continue };
            let (x, y, z) = (f1[k1], f1[(k1 + 1) % 3], f1[(k1 + 2) % 3]);
            let Some(k2) = (0..3).find(|&k| (f2[k], f2[(k + 1) % 3]) == (y, x)) else { continue };
            let w = f2[(k2 + 2) % 3];
            if z == w || by_edge.contains_key(&(z.min(w), z.max(w))) {
                continue;
            }
            let (c1, c2) = (corners(positions, f1), corners(positions, f2));
            let (q1, q2) = (shape(c1), shape(c2));
            // The way the pair faces: both normals where both faces are fair; only the fair one's
            // where the other is degenerate (three points nearly in a row: its normal is noise,
            // and flipping its long edge is exactly the mend it needs).
            let n = match (unit_normal(c1), unit_normal(c2)) {
                (Some(n1), Some(n2)) if q1 >= DEGENERATE && q2 >= DEGENERATE => {
                    if n1.dot(n2) < FLAT {
                        continue;
                    }
                    n1 + n2
                }
                (Some(n1), _) if q1 >= q2 => n1,
                (_, Some(n2)) => n2,
                (Some(n1), None) => n1,
                (None, None) => continue,
            };
            let (g1, g2) = ([x, w, z], [w, y, z]);
            let (d1, d2) = (corners(positions, g1), corners(positions, g2));
            let (Some(m1), Some(m2)) = (unit_normal(d1), unit_normal(d2)) else { continue };
            if m1.dot(n) < FLAT * n.len() || m2.dot(n) < FLAT * n.len() {
                continue;
            }
            if shape(d1).min(shape(d2)) <= shape(c1).min(shape(c2)) + 1e-9 {
                continue;
            }
            let (pz, pw) = (corners(positions, [z, w, z])[0], corners(positions, [w, z, w])[0]);
            if !on_scan((pz + pw) / 2.0) || !on_scan((d1[0] + d1[1] + d1[2]) / 3.0) || !on_scan((d2[0] + d2[1] + d2[2]) / 3.0) {
                continue;
            }
            // The old edge's middle still covered by the new faces.
            let old_mid = (c1[k1] + c1[(k1 + 1) % 3]) / 2.0;
            let near = |q: [Vec3; 3]| closest_on_triangle(old_mid, q[0], q[1], q[2]).dist(old_mid);
            if near(d1).min(near(d2)) > tolerance {
                continue;
            }
            triangles[t1] = g1;
            triangles[t2] = g2;
            touched[t1] = true;
            touched[t2] = true;
            changed = true;
        }
        // Caps: a face whose middle point lies on its longest side is taken out, and the face
        // across that side split at the point, so nothing moves.
        if remove_caps(positions, triangles, tolerance) {
            changed = true;
        }
        // Collapses: a degenerate face's middle point (the one off its longest side) moved onto the
        // nearer end of that side, where flipping cannot mend it (across a rounded edge the flipped
        // side would cut inside the part).
        if collapse_degenerate(positions, triangles, under, tolerance) {
            changed = true;
        }
        if !changed {
            break;
        }
    }
    // Last, the points relaxed: each moved toward the middle of its neighbours, in its own tangent
    // plane, and back onto the scan, which spreads crowded points and fattens thin faces.
    for _ in 0..RELAX_ROUNDS {
        if !relax(positions, triangles, under, tolerance) {
            break;
        }
    }
}

/// One pass of tangential relaxation (see [`tidy`]); whether any point moved. Points on the open
/// rim or on a crease (two faces bending more than 30°) stay; a move is kept only when no face it
/// touches turns over or grows worse in shape below [`NEEDLE`], every such face's middle lies
/// within the tolerance of the scan, and the old place is still covered.
fn relax(positions: &mut [Vec3], triangles: &[[u32; 3]], under: &Under, tolerance: f64) -> bool {
    let n = positions.len();
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (t, tri) in triangles.iter().enumerate() {
        for &v in tri {
            if let Some(a) = around.get_mut(v as usize) {
                a.push(t);
            }
        }
    }
    let by_edge = edge_faces(triangles);
    let mut fixed = vec![false; n];
    for ((a, b), faces) in &by_edge {
        let creased = match faces.as_slice() {
            [f1, f2] => {
                let c = |t: usize| triangles[t].map(|i| positions[i as usize]);
                match (unit_normal(c(*f1)), unit_normal(c(*f2))) {
                    (Some(n1), Some(n2)) => n1.dot(n2) < CREASE,
                    _ => false,
                }
            }
            _ => true,
        };
        if creased {
            fixed[*a as usize] = true;
            fixed[*b as usize] = true;
        }
    }
    let mut moved = false;
    for v in 0..n {
        if fixed[v] || around[v].is_empty() {
            continue;
        }
        let p = positions[v];
        let faces = &around[v];
        let mut nb: Vec<u32> = faces.iter().flat_map(|&f| triangles[f]).filter(|&x| x as usize != v).collect();
        nb.sort_unstable();
        nb.dedup();
        if nb.is_empty() {
            continue;
        }
        let mid = nb.iter().fold(Vec3::ZERO, |acc, &x| acc + positions[x as usize]) / nb.len() as f64;
        let normal = faces.iter().filter_map(|&f| unit_normal(triangles[f].map(|i| positions[i as usize]))).fold(Vec3::ZERO, |acc, m| acc + m);
        let Some(normal) = (normal.len() > 0.0).then(|| normal / normal.len()) else { continue };
        let step = (mid - p) - normal * (mid - p).dot(normal);
        let target = p + step * RELAX_STEP;
        let Some(on) =
            under.tree.nearest(target).and_then(|(_, s)| under.scan.corners(s as usize)).map(|[a, b, c]| closest_on_triangle(target, a, b, c))
        else {
            continue;
        };
        let mut ok = true;
        let mut fan = Vec::new();
        for &f in faces {
            let before = triangles[f].map(|i| positions[i as usize]);
            let after = triangles[f].map(|i| if i as usize == v { on } else { positions[i as usize] });
            let (qb, qa) = (shape(before), shape(after));
            let turned = match (unit_normal(before), unit_normal(after)) {
                (Some(x), Some(y)) => x.dot(y) < 0.5,
                _ => true,
            };
            let off = under.tree.nearest((after[0] + after[1] + after[2]) / 3.0).is_none_or(|(d, _)| d > tolerance);
            if turned || off || (qa < qb && qa < NEEDLE) {
                ok = false;
                break;
            }
            fan.push(after);
        }
        let covered = fan.iter().any(|t| closest_on_triangle(p, t[0], t[1], t[2]).dist(p) <= tolerance);
        if ok && covered && on.dist(p) > 1e-9 {
            positions[v] = on;
            moved = true;
        }
    }
    moved
}

/// One pass of cap removals (see [`tidy`]); whether any was made. A cap is a degenerate face whose
/// angle at its middle point is above [`CAP_ANGLE`]: the point lies on the face's longest side, so
/// the face is taken out and the face across that side split at the point: the surface does not
/// move, and the two new sides are the cap's own, so the mesh stays manifold.
fn remove_caps(positions: &[Vec3], triangles: &mut Vec<[u32; 3]>, tolerance: f64) -> bool {
    let corners = |t: [u32; 3]| t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
    let by_edge = edge_faces(triangles);
    let mut gone = vec![false; triangles.len()];
    let mut touched = vec![false; triangles.len()];
    let mut added = Vec::new();
    for t in 0..triangles.len() {
        if touched[t] {
            continue;
        }
        let tri = triangles[t];
        let c = corners(tri);
        if shape(c) >= DEGENERATE {
            continue;
        }
        let Some(k) = (0..3).max_by(|&i, &j| c[i].dist(c[(i + 1) % 3]).total_cmp(&c[j].dist(c[(j + 1) % 3]))) else { continue };
        let (a, b, m) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
        let (pa, pb, pm) = (c[k], c[(k + 1) % 3], c[(k + 2) % 3]);
        let (u, v) = (pa - pm, pb - pm);
        let (lu, lv) = (u.len(), v.len());
        if !(lu > 0.0 && lv > 0.0) || u.dot(v) / (lu * lv) > CAP_ANGLE.cos() {
            continue;
        }
        // Only a point that lies on the side to within a quarter of the tolerance: splitting the
        // face across there must not move it (across a corner it would pull a thin rim in).
        let side = pb - pa;
        let off = (pm - pa).cross(side).len() / side.len().max(1e-300);
        if off > 0.25 * tolerance {
            continue;
        }
        let Some(faces) = by_edge.get(&(a.min(b), a.max(b))) else { continue };
        let &[f1, f2] = faces.as_slice() else { continue };
        let other = if f1 == t { f2 } else { f1 };
        if touched[other] {
            continue;
        }
        // The face across runs b → a with d opposite.
        let g = triangles[other];
        let Some(i) = (0..3).find(|&i| g[i] == b && g[(i + 1) % 3] == a) else { continue };
        let d = g[(i + 2) % 3];
        if d == m || by_edge.contains_key(&(m.min(d), m.max(d))) {
            continue;
        }
        triangles[other] = [b, m, d];
        added.push([m, a, d]);
        gone[t] = true;
        touched[t] = true;
        touched[other] = true;
    }
    if added.is_empty() {
        return false;
    }
    let mut k = 0;
    triangles.retain(|_| {
        let keep = !gone[k];
        k += 1;
        keep
    });
    triangles.extend(added);
    true
}

/// The point inside a coarse triangle its three quads meet at: of the middle, the incentre and
/// points drawn toward each corner, the one whose quads' worst corner (furthest from a right angle)
/// is best. The middle of an obtuse triangle gives a quad a corner near 180°, which the kernel's
/// check refuses (its edges part from its faces there).
fn inner_point(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (la, lb, lc) = (b.dist(c), c.dist(a), a.dist(b));
    let perimeter = la + lb + lc;
    let mut candidates = vec![(a + b + c) / 3.0];
    if perimeter > 0.0 {
        candidates.push((a * la + b * lb + c * lc) / perimeter);
    }
    for (w0, w1, w2) in [(0.5, 0.25, 0.25), (0.25, 0.5, 0.25), (0.25, 0.25, 0.5), (0.4, 0.4, 0.2), (0.4, 0.2, 0.4), (0.2, 0.4, 0.4)] {
        candidates.push(a * w0 + b * w1 + c * w2);
    }
    let (mab, mbc, mca) = ((a + b) / 2.0, (b + c) / 2.0, (c + a) / 2.0);
    let angle = |o: Vec3, p: Vec3, q: Vec3| {
        let (u, v) = (p - o, q - o);
        let l = u.len() * v.len();
        if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
    };
    let worst = |g: Vec3| -> f64 {
        let quads = [[a, mab, g, mca], [b, mbc, g, mab], [c, mca, g, mbc]];
        quads
            .iter()
            .flat_map(|q| (0..4).map(move |k| angle(q[k], q[(k + 1) % 4], q[(k + 3) % 4])))
            .map(|x| (x - std::f64::consts::FRAC_PI_2).abs())
            .fold(0.0, f64::max)
    };
    candidates.into_iter().min_by(|x, y| worst(*x).total_cmp(&worst(*y))).unwrap_or((a + b + c) / 3.0)
}

/// A triangle's smallest angle, in radians.
fn smallest_angle(t: [Vec3; 3]) -> f64 {
    (0..3)
        .map(|k| {
            let (u, v) = (t[(k + 1) % 3] - t[k], t[(k + 2) % 3] - t[k]);
            let l = u.len() * v.len();
            if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
        })
        .fold(std::f64::consts::PI, f64::min)
}

/// One pass of collapses of poor faces (see [`tidy`]); whether any was made. A face is poor when
/// its smallest angle is below [`MIN_ANGLE`] (or it is degenerate). A cap (a point near its longest
/// side) has that point moved onto the nearer end of the side; a needle (a short side) has the short
/// side collapsed, either way round. A collapse is made only when the mesh stays manifold (the link
/// condition), no point on the open rim moves, no face is turned over or left poorer than it was,
/// the faces it changes stay on the scan, and the old point is still covered.
fn collapse_degenerate(positions: &[Vec3], triangles: &mut Vec<[u32; 3]>, under: &Under, tolerance: f64) -> bool {
    let corners = |t: [u32; 3]| t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
    let on_scan = |p: Vec3| under.tree.nearest(p).is_some_and(|(d, _)| d <= tolerance);
    let point = |v: u32| positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let mut around: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for (t, tri) in triangles.iter().enumerate() {
        for &v in tri {
            if let Some(a) = around.get_mut(v as usize) {
                a.push(t);
            }
        }
    }
    let mut gone = vec![false; triangles.len()];
    let mut touched = vec![false; triangles.len()];
    let mut any = false;
    for t in 0..triangles.len() {
        if gone[t] || touched[t] {
            continue;
        }
        let tri = triangles[t];
        let c = corners(tri);
        if shape(c) >= DEGENERATE && smallest_angle(c) >= MIN_ANGLE {
            continue;
        }
        let side = |k: usize| c[k].dist(c[(k + 1) % 3]);
        let (Some(long), Some(short)) =
            ((0..3).max_by(|&i, &j| side(i).total_cmp(&side(j))), (0..3).min_by(|&i, &j| side(i).total_cmp(&side(j))))
        else {
            continue;
        };
        // A cap: its middle point onto the nearer end of the longest side. A needle: the short side.
        let m_cap = tri[(long + 2) % 3];
        let (u, v) = (point(m_cap) - c[long], point(m_cap) - c[(long + 1) % 3]);
        let is_cap = u.len() > 0.0 && v.len() > 0.0 && u.dot(v) / (u.len() * v.len()) < CAP_ANGLE.cos();
        let mut pairs: Vec<(u32, u32)> = if is_cap {
            let (a, b) = (tri[long], tri[(long + 1) % 3]);
            vec![(m_cap, a), (m_cap, b)]
        } else {
            let (a, b) = (tri[short], tri[(short + 1) % 3]);
            vec![(a, b), (b, a)]
        };
        pairs.sort_by(|x, y| point(x.0).dist(point(x.1)).total_cmp(&point(y.0).dist(point(y.1))));
        'pairs: for (m, to) in pairs {
            let pm = point(m);
            let fan = around.get(m as usize).cloned().unwrap_or_default();
            if fan.iter().any(|&f| touched[f] || gone[f]) {
                continue;
            }
            // A point on the open rim stays where it is: moving it would move the rim.
            let mut rim_count: HashMap<u32, usize> = HashMap::new();
            for &f in &fan {
                for &x in &triangles[f] {
                    if x != m {
                        *rim_count.entry(x).or_default() += 1;
                    }
                }
            }
            if rim_count.values().any(|&n| n == 1) {
                continue;
            }
            // The link condition: m and `to` share no neighbour but the third points of the faces
            // on their side.
            let neighbours = |v: u32| -> Vec<u32> {
                let mut n: Vec<u32> =
                    around.get(v as usize).into_iter().flatten().filter(|&&f| !gone[f]).flat_map(|&f| triangles[f]).filter(|&x| x != v).collect();
                n.sort_unstable();
                n.dedup();
                n
            };
            let (nm, nt) = (neighbours(m), neighbours(to));
            let shared_faces: Vec<usize> = fan.iter().copied().filter(|&f| triangles[f].contains(&to)).collect();
            let opposite: Vec<u32> = shared_faces.iter().flat_map(|&f| triangles[f]).filter(|&x| x != m && x != to).collect();
            let common = nm.iter().filter(|x| nt.contains(x)).count();
            if common != opposite.len() || shared_faces.is_empty() {
                continue;
            }
            let mut new = Vec::new();
            for &f in &fan {
                if shared_faces.contains(&f) {
                    continue;
                }
                let before = triangles[f];
                let after = before.map(|x| if x == m { to } else { x });
                let (cb, ca) = (corners(before), corners(after));
                let turned = match (unit_normal(cb), unit_normal(ca)) {
                    (Some(nb), Some(na)) => shape(cb) >= DEGENERATE && nb.dot(na) < 0.0,
                    _ => false,
                };
                let poorer = smallest_angle(ca) < MIN_ANGLE && smallest_angle(ca) < smallest_angle(cb);
                if turned || poorer || !on_scan((ca[0] + ca[1] + ca[2]) / 3.0) {
                    continue 'pairs;
                }
                new.push((f, after));
            }
            // What is taken away must still be covered: the old point within the tolerance of the
            // faces it leaves (a thin rim must not be pulled in).
            let covered = new.iter().any(|(_, after)| {
                let q = corners(*after);
                closest_on_triangle(pm, q[0], q[1], q[2]).dist(pm) <= tolerance
            });
            if !covered {
                continue;
            }
            for f in shared_faces {
                gone[f] = true;
            }
            for (f, after) in new {
                triangles[f] = after;
                touched[f] = true;
                if let Some(a) = around.get_mut(to as usize) {
                    a.push(f);
                }
            }
            any = true;
            break;
        }
    }
    if any {
        let mut k = 0;
        triangles.retain(|_| {
            let keep = !gone[k];
            k += 1;
            keep
        });
    }
    any
}

/// The triangles' indices walked breadth first across shared edges, each piece in turn.
fn walked(triangles: &[[u32; 3]]) -> Vec<usize> {
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, tri) in triangles.iter().enumerate() {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut seen = vec![false; triangles.len()];
    let mut order = Vec::with_capacity(triangles.len());
    for start in 0..triangles.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(t) = queue.pop_front() {
            order.push(t);
            let tri = triangles[t];
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                for &u in by_edge.get(&(a.min(b), a.max(b))).map_or(&[][..], Vec::as_slice) {
                    if !seen[u] {
                        seen[u] = true;
                        queue.push_back(u);
                    }
                }
            }
        }
    }
    order
}

/// The whole scan wrapped in patches (see the module): rounds of reduce, lay, fit and measure,
/// detail kept round the scan points the patches miss, until they meet the share or the cap.
pub fn wrap(scan: &Scan, settings: &Settings) -> Result<Wrap, WrapError> {
    let tol = settings.tolerance;
    if !(tol.is_finite() && tol > 0.0) || !(settings.share > 0.0 && settings.share <= 1.0) {
        return Err(WrapError::Input("the tolerance must be above zero and the share in (0, 1]"));
    }
    if scan.triangle_count() == 0 {
        return Err(WrapError::Input("the scan has no triangles"));
    }
    let tree = TriangleTree::of(scan.positions, scan.triangles);
    let under = Under { scan, tree: TriangleTree::of(scan.positions, scan.triangles) };
    let rep = welded(scan.positions);
    let rings = rings(scan, &rep);
    let n = scan.positions.len();
    let mut used = vec![false; n];
    for t in scan.triangles {
        for &v in t {
            if let Some(x) = used.get_mut(v as usize) {
                *x = true;
            }
        }
    }
    let used: Vec<usize> = (0..n).filter(|&v| used[v]).collect();
    let sampled: Vec<usize> = evenly(used.len(), SAMPLES).into_iter().filter_map(|k| used.get(k).copied()).collect();
    let cap = (settings.max_patches / 3).max(4);
    let mut budget = vec![START * tol; n];
    let mut mark = vec![0usize; n];
    let mut best: Option<Wrap> = None;
    for round in 1..=ROUNDS {
        let (mut r, capped) = reduce_once(scan, &tree, &budget, cap)?;
        if r.triangles.is_empty() {
            return Err(WrapError::Reduce);
        }
        tidy(&mut r.positions, &mut r.triangles, &under, tol);
        let faces = patches(&under, &r.positions, &r.triangles);
        let d = distances(scan, &faces, &sampled);
        let mut sorted = d.clone();
        let spread = spread(&mut sorted, tol);
        let mut order: Vec<usize> = (0..d.len()).collect();
        order.sort_by(|&x, &y| d[y].total_cmp(&d[x]));
        let worst: Vec<(Vec3, f64)> =
            order.into_iter().take(WORST).filter_map(|k| sampled.get(k).and_then(|&v| scan.positions.get(v)).map(|&p| (p, d[k]))).collect();
        let met = spread.within >= settings.share && spread.max <= 3.0 * tol;
        let better = best.as_ref().is_none_or(|b| spread.within > b.scan_to_patches.within);
        let coarse = Coarse { positions: r.positions, triangles: r.triangles, rounds: round, met, capped };
        if better {
            best = Some(Wrap { faces, coarse, scan_to_patches: spread, worst });
        } else if let Some(b) = best.as_mut() {
            b.coarse.rounds = round;
        }
        if met || capped || round == ROUNDS {
            break;
        }
        // Keep detail round every scan point the patches miss.
        for (k, &dk) in d.iter().enumerate() {
            if dk > tol
                && let Some(&v) = sampled.get(k)
            {
                tighten(v, &rep, &rings, &mut budget, &mut mark, round);
            }
        }
    }
    best.ok_or(WrapError::Reduce)
}

/// The sampled scan points' distances to the patches, each patch drawn as a 6 × 6 grid of triangles.
fn distances(scan: &Scan, faces: &[NurbsSurface], sampled: &[usize]) -> Vec<f64> {
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
    par::map(sampled.len(), |k| scan.positions.get(sampled[k]).and_then(|&p| tree.nearest(p)).map_or(f64::INFINITY, |(d, _)| d))
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
        let tree = TriangleTree::of(&p, &t);
        let (r, capped) = reduce_once(&scan, &tree, &vec![0.1; p.len()], usize::MAX).expect("reduce");
        assert!(!capped);
        // Every coarse point lies within its budget of the scan, by the guard.
        let worst = r.positions.iter().map(|q| tree.nearest(*q).map_or(f64::INFINITY, |(d, _)| d)).fold(0.0, f64::max);
        assert!(worst <= 0.1 + 1e-9, "a coarse point {worst} mm off the scan");
        // Chord sag on R 20 within 0.1 mm allows edges of about 2·sqrt(2·20·0.1) = 4 mm: far fewer
        // triangles than the 14,160 of the input.
        assert!(r.triangles.len() < 3_000, "{} triangles", r.triangles.len());
    }

    #[test]
    fn a_side_that_would_turn_back_is_kept_straight() {
        let (a, b) = (Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
        assert!(in_order(a, Vec3::new(3.0, 1.0, 0.0), Vec3::new(7.0, 1.0, 0.0), b));
        // Inner points swapped (a loop), past an end (a cusp), or far off the chord: refused.
        assert!(!in_order(a, Vec3::new(7.0, 1.0, 0.0), Vec3::new(3.0, 1.0, 0.0), b));
        assert!(!in_order(a, Vec3::new(-1.0, 1.0, 0.0), Vec3::new(7.0, 1.0, 0.0), b));
        assert!(!in_order(a, Vec3::new(3.0, 30.0, 0.0), Vec3::new(7.0, 1.0, 0.0), b));
        assert!(!in_order(a, a, a, a));
    }

    #[test]
    fn a_face_of_three_points_in_a_row_is_flipped_away() {
        // A square 10 × 10 split by a diagonal into a fair face and, beside it, a face whose third
        // point lies 0.001 off the diagonal: the flip leaves no degenerate face.
        let p = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(5.0, 5.001, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        ];
        let t = vec![[0, 1, 2], [0, 2, 3], [0, 3, 4], [3, 2, 4]];
        let scan = Scan::new(&p, &t);
        let under = Under { scan: &scan, tree: TriangleTree::of(&p, &t) };
        let (mut q, mut f) = (p.clone(), t.clone());
        tidy(&mut q, &mut f, &under, 0.2);
        let worst = f.iter().map(|tri| shape(tri.map(|i| q[i as usize]))).fold(1.0, f64::min);
        assert!(worst >= DEGENERATE, "{f:?}: worst shape {worst}");
    }

    #[test]
    fn a_point_in_a_row_on_a_rounded_edge_is_collapsed() {
        // A strip folded 90° along x = 0 (two faces meeting at the fold), with an extra point on
        // the fold making a face of three points in a row: no flip mends it (the flipped side would
        // cut the corner), a collapse does.
        let p = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(0.0, 5.0005, 0.0),
            Vec3::new(5.0, 5.0, 0.0),
            Vec3::new(0.0, 5.0, 5.0),
        ];
        let t = vec![[0, 3, 1], [0, 1, 2], [0, 2, 4], [2, 1, 4]];
        let scan = Scan::new(&p, &t);
        let under = Under { scan: &scan, tree: TriangleTree::of(&p, &t) };
        let mut f = t.clone();
        assert!(collapse_degenerate(&p, &mut f, &under, 0.2));
        let worst = f.iter().map(|tri| shape(tri.map(|i| p[i as usize]))).fold(1.0, f64::min);
        assert!(worst >= DEGENERATE, "{f:?}: worst shape {worst}");
    }

    #[test]
    fn a_cap_on_a_rounded_edge_is_taken_out_without_moving_anything() {
        // The folded strip of the collapse test: the face with its third point on the fold goes,
        // the face across the fold is split there; same area, no degenerate face, still closed
        // round the point (each side of the new faces used by two faces or on the rim).
        let p = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(0.0, 5.0005, 0.0),
            Vec3::new(5.0, 5.0, 0.0),
            Vec3::new(0.0, 5.0, 5.0),
        ];
        let mut f = vec![[0, 3, 1], [0, 1, 2], [0, 2, 4], [2, 1, 4]];
        let area = |f: &[[u32; 3]]| -> f64 {
            f.iter().map(|t| (p[t[1] as usize] - p[t[0] as usize]).cross(p[t[2] as usize] - p[t[0] as usize]).len() / 2.0).sum()
        };
        let before = area(&f);
        assert!(remove_caps(&p, &mut f, 0.2));
        let worst = f.iter().map(|tri| shape(tri.map(|i| p[i as usize]))).fold(1.0, f64::min);
        assert!(worst >= DEGENERATE, "{f:?}: worst shape {worst}");
        assert!((area(&f) - before).abs() < 1e-3, "area {} against {before}", area(&f));
        assert!(edge_faces(&f).values().all(|v| v.len() <= 2));
    }

    #[test]
    fn a_net_that_doubles_back_is_not_monotone() {
        let flat: [Vec3; 16] = std::array::from_fn(|k| Vec3::new((k % 4) as f64, (k / 4) as f64, 0.0));
        assert!(monotone(&flat));
        let mut folded = flat;
        folded[5] = Vec3::new(2.5, 1.0, 0.0); // the second point of row 1 past the third
        assert!(!monotone(&folded));
    }

    #[test]
    fn an_obtuse_triangle_meets_its_quads_away_from_its_middle() {
        // A 150° triangle: at its middle one quad's corner is near 180°; the chosen point is better.
        let (a, b, c) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(5.0 + 10.0, 2.0, 0.0));
        let g = inner_point(a, b, c);
        let worst = |g: Vec3| {
            let (mab, mbc, mca) = ((a + b) / 2.0, (b + c) / 2.0, (c + a) / 2.0);
            [[a, mab, g, mca], [b, mbc, g, mab], [c, mca, g, mbc]]
                .iter()
                .flat_map(|q| (0..4).map(move |k| {
                    let (u, v) = (q[(k + 1) % 4] - q[k], q[(k + 3) % 4] - q[k]);
                    (u.dot(v) / (u.len() * v.len())).clamp(-1.0, 1.0).acos().to_degrees()
                }))
                .fold(0.0, f64::max)
        };
        assert!(worst(g) < worst((a + b + c) / 3.0), "{} against {}", worst(g), worst((a + b + c) / 3.0));
    }

    #[test]
    fn the_patches_come_piece_by_piece() {
        // Two separate triangles' fans: the walk finishes one piece before it starts the other.
        let t = [[0, 1, 2], [10, 11, 12], [0, 2, 3], [10, 12, 13], [0, 3, 1]];
        let order = walked(&t);
        assert_eq!(order.len(), 5);
        let first_piece: Vec<bool> = order.iter().map(|&k| t[k][0] == 0).collect();
        assert_eq!(first_piece, [true, true, true, false, false]);
    }

    #[test]
    fn a_sphere_wraps_within_its_tolerance_with_far_fewer_patches_than_triangles() {
        let (p, t) = sphere(20.0, 60, 120);
        let scan = Scan::new(&p, &t);
        let w = wrap(&scan, &Settings { tolerance: 0.1, ..Settings::default() }).expect("wrap");
        assert_eq!(w.faces.len(), w.coarse.triangles.len() * 3);
        assert!(w.scan_to_patches.within >= 0.99, "{:?}", w.scan_to_patches);
        // A UV sphere's cells are needles near its poles, which the held reduction will not make
        // worse ([`crate::reduce::reduce_held`]): still far below 3 × the 14,160 triangles.
        assert!(w.faces.len() < 10_000, "{} patches", w.faces.len());
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
