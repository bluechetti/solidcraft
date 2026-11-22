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
/// Reductions tried to fit within the most patches, and how near the cap a fit may stop.
const FIT_TRIES: usize = 5;
const NEAR_CAP: f64 = 0.85;
/// A coarse face below this shape ([`shape`]: about a 12° smallest angle) and longer than [`LONG`]
/// tolerances is a needle to mend: its patches would be too thin for any tessellation to follow (a
/// 24 × 0.7 mm face made a patch that needed more than 20,000 nodes on one edge).
const NEEDLE: f64 = 0.35;
/// Two triangles pair into a quad only when they bend by less than this (30°), and every corner
/// of the quad lies between this angle (15°) and 180° less it.
const PAIR_BEND: f64 = std::f64::consts::FRAC_PI_6;
const PAIR_CORNER: f64 = 0.2618;
/// And every corner of the four quads a pair is cut into lies between this (10°) and 180° less it.
const SUB_CORNER: f64 = 0.1745;
/// How much looser than the tolerance the last tidy pass, of poor faces only, may be.
const POOR_SLACK: f64 = 3.0;
/// Two fitted sides leaving a patch corner closer than this (8°) are made straight there.
const SHARP_CORNER: f64 = 0.1396;
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
    /// The patches its round's budgets would have laid with no cap (more than the cap when
    /// `capped`): about what the tolerance asks for.
    pub wanted: usize,
    /// Coarse triangles whose patches still have a corner outside the fair corners ([`unfair`])
    /// after [`fair_corners`]: none where it could mend them all within the tolerance.
    pub unfair: usize,
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

fn spread(d: &[f64], weight: &[f64], tolerance: f64) -> Spread {
    let n = d.len();
    let w = |k: usize| weight.get(k).copied().filter(|x| x.is_finite() && *x > 0.0).unwrap_or(0.0);
    let total: f64 = (0..n).map(w).sum();
    if n == 0 || !(total > 0.0) {
        return Spread { samples: n, within: 1.0, p99: 0.0, max: 0.0 };
    }
    let within = (0..n).filter(|&k| d[k] <= tolerance).map(w).sum::<f64>() / total;
    let max = d.iter().copied().fold(0.0, f64::max);
    // The distance below which 99 % of the area lies.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&x, &y| d[x].total_cmp(&d[y]));
    let mut acc = 0.0;
    let p99 = order.iter().find(|&&k| {
        acc += w(k);
        acc >= 0.99 * total
    });
    Spread { samples: n, within, p99: p99.map_or(max, |&k| d[k]), max }
}

/// Every `step`-th index below `n`, so at most `most` of them.
fn evenly(n: usize, most: usize) -> Vec<usize> {
    let step = n.div_ceil(most.max(1)).max(1);
    (0..n).step_by(step).collect()
}

/// The scan reduced once, every collapse held within `budget` (per scan point); when that leaves
/// more than `cap` patches, every budget is scaled up by the factor the first count says (the
/// tolerance given up evenly, not where an unheld reduction happens to cut) and corrected up to
/// [`FIT_TRIES`] times; only then is it reduced to the cap unheld. The bool says whether the cap
/// bounded it, the first number by how much the budgets were scaled, the second how many patches
/// the budgets as given would have laid.
fn reduce_once(scan: &Scan, tree: &TriangleTree, budget: &[f64], cap: usize) -> Result<(reduce::Reduced, bool, f64, usize), WrapError> {
    let held = reduce::Settings { target: 4, max_error: 0.0, keep_rim: true };
    let at = |scale: f64| -> Result<reduce::Reduced, WrapError> {
        let scaled: Vec<f64> = budget.iter().map(|b| b * scale).collect();
        reduce::reduce_held(scan, held, &scaled, tree).ok_or(WrapError::Reduce)
    };
    // Patches it would lay: three to a triangle, four to each pair laid as one quad (a little
    // margin, as the tidy pass may pair a few triangles differently).
    let count = |r: &reduce::Reduced| {
        let paired = pairs(&r.positions, &r.triangles).iter().filter(|p| p.is_some()).count();
        (3 * r.triangles.len()).saturating_sub(paired) + r.triangles.len() / 32
    };
    let first = at(1.0)?;
    let wanted = count(&first);
    if wanted <= cap {
        return Ok((first, false, 1.0, wanted));
    }
    // Triangles fall about as 1 / scale (a held edge grows as √budget across a curved face): start
    // at the scale that estimate gives, then correct by the same law, keeping the largest result
    // that fits. Two or three reductions where doubling took up to a dozen (10M triangles: each
    // reduction is minutes).
    let mut scale = (count(&first) as f64 / cap as f64) * 1.05;
    let mut best: Option<(f64, reduce::Reduced)> = None;
    for _ in 0..FIT_TRIES {
        let r = at(scale)?;
        let n = count(&r);
        if n <= cap {
            let better = best.as_ref().is_none_or(|(_, b)| r.triangles.len() > b.triangles.len());
            if better {
                best = Some((scale, r));
            }
            if n as f64 >= NEAR_CAP * cap as f64 {
                break;
            }
        }
        // Next scale by the same law, bounded so one wrong guess cannot run away.
        let ratio = (n.max(1) as f64 / (NEAR_CAP * 0.5 + 0.5) / cap as f64).clamp(0.25, 4.0);
        scale *= ratio;
    }
    match best {
        Some((k, r)) => Ok((r, true, k, wanted)),
        None => {
            let to_cap = reduce::Settings { target: (cap / 3).max(4), max_error: 0.0, keep_rim: false };
            Ok((reduce::reduce(scan, to_cap).ok_or(WrapError::Reduce)?, true, scale, wanted))
        }
    }
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
    let partner = pairs(cp, triangles);
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
        let mut m = |x: u32, y: u32| -> u32 {
            *mid.entry((x.min(y), x.max(y))).or_insert_with(|| {
                points.push((cp[x as usize] + cp[y as usize]) / 2.0);
                (points.len() - 1) as u32
            })
        };
        match partner.get(t).copied().flatten() {
            // The second of a pair: laid with the first.
            Some((u, _)) if u < t => {}
            // A pair: the quad a d b c round their shared side a b, cut in four at its middle.
            Some((u, [qa, qd, qb, qc])) => {
                let Some(other) = triangles.get(u) else { continue };
                let n2 = (cp[other[1] as usize] - cp[other[0] as usize]).cross(cp[other[2] as usize] - cp[other[0] as usize]);
                let sum = n / len + n2 / n2.len().max(1e-300);
                let facing = sum / sum.len().max(1e-300);
                let corners = [qa, qd, qb, qc];
                let (m0, m1, m2, m3) = (m(qa, qd), m(qd, qb), m(qb, qc), m(qc, qa));
                points.push(corners.iter().fold(Vec3::ZERO, |acc, &v| acc + cp[v as usize]) / 4.0);
                let centre = (points.len() - 1) as u32;
                quads.push(([qa, m0, centre, m3], facing));
                quads.push(([qd, m1, centre, m0], facing));
                quads.push(([qb, m2, centre, m1], facing));
                quads.push(([qc, m3, centre, m2], facing));
            }
            None => {
                let facing = n / len;
                let (mab, mbc, mca) = (m(a, b), m(b, c), m(c, a));
                points.push(inner_point(pa, pb, pc));
                let ct = (points.len() - 1) as u32;
                quads.push(([a, mab, ct, mca], facing));
                quads.push(([b, mbc, ct, mab], facing));
                quads.push(([c, mca, ct, mbc], facing));
            }
        }
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
    let mut placed: Vec<Vec3> = par::map(points.len(), |k| under.under(points[k], facing_of[k]).unwrap_or(points[k]));
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
    let mut curve: HashMap<(u32, u32), [Vec3; 4]> = keys.into_iter().zip(curves).collect();
    straighten_sharp_corners(&mut curve, &quads, &placed);
    // Every patch regular: its normal never vanishes, so it does not fold over itself (a folded
    // patch is no surface a face can lie on, and the tessellator refines one without end). As
    // the kernel certifies it, from the cheapest mend to the dearest:
    // - a fitted net that folds is laid as the Coons net of its sides (its own inner points);
    // - a Coons net that folds has its curved sides made straight (sides are shared: both patches
    //   on one see the same, and both are laid again);
    // - a patch that still folds lies on a layout quad that is not convex, its inner point (its
    //   triangle's or pair's alone) placed past a side: that point goes back to where the
    //   layout put it, inside the triangle, and its patches are laid again.
    let mut nets: Vec<[Vec3; 16]> = vec![[Vec3::ZERO; 16]; quads.len()];
    let mut todo: Vec<usize> = (0..quads.len()).collect();
    for round in 0..=MEND_ROUNDS {
        let laid: Vec<([Vec3; 16], [Vec3; 16])> = par::map(todo.len(), |i| lay(under, &quads[todo[i]], &curve, &placed));
        let fitted: Vec<[Vec3; 16]> = laid.iter().map(|(fitted, _)| *fitted).collect();
        let fitted_ok = regular_nets(&fitted);
        let coons_wanted: Vec<usize> = (0..todo.len()).filter(|&i| !fitted_ok[i]).collect();
        let coons_nets: Vec<[Vec3; 16]> = coons_wanted.iter().map(|&i| laid[i].1).collect();
        let coons_ok = regular_nets(&coons_nets);
        let mut folded: Vec<usize> = Vec::new();
        for (i, (fitted, _)) in laid.iter().enumerate() {
            nets[todo[i]] = *fitted;
        }
        for (k, &i) in coons_wanted.iter().enumerate() {
            nets[todo[i]] = laid[i].1;
            if !coons_ok[k] {
                folded.push(todo[i]);
            }
        }
        if folded.is_empty() || round == MEND_ROUNDS {
            break;
        }
        // Sides first; once a folded patch's sides are all straight, its inner point.
        let mut moved: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut changed: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for &k in &folded {
            let q = quads[k].0;
            let mut curved = false;
            for s in 0..4 {
                let key = (q[s].min(q[(s + 1) % 4]), q[s].max(q[(s + 1) % 4]));
                let (a, b) = (placed[key.0 as usize], placed[key.1 as usize]);
                let line = [a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b];
                if curve.get(&key).is_some_and(|c| *c != line) {
                    curve.insert(key, line);
                    changed.insert(key);
                    curved = true;
                }
            }
            if !curved && placed.get(q[2] as usize).is_some_and(|p| Some(p) != points.get(q[2] as usize)) {
                moved.insert(q[2]);
            }
        }
        for &v in &moved {
            if let (Some(p), Some(&at)) = (placed.get_mut(v as usize), points.get(v as usize)) {
                *p = at;
            }
            // Its patches' sides to it run from points that have not moved: laid straight again.
            for (q, _) in &quads {
                if q[2] == v {
                    for key in [(q[1].min(q[2]), q[1].max(q[2])), (q[2].min(q[3]), q[2].max(q[3]))] {
                        let (a, b) = (placed[key.0 as usize], placed[key.1 as usize]);
                        curve.insert(key, [a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b]);
                        changed.insert(key);
                    }
                }
            }
        }
        if changed.is_empty() {
            break;
        }
        todo = (0..quads.len())
            .filter(|&k| {
                let q = quads[k].0;
                (0..4).any(|s| changed.contains(&(q[s].min(q[(s + 1) % 4]), q[s].max(q[(s + 1) % 4]))))
            })
            .collect();
    }
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    nets.iter().filter_map(|net| NurbsSurface::new([3, 3], [knots.clone(), knots.clone()], net, &[1.0; 16]).ok()).collect()
}

/// Rounds of mending folded patches: their sides, then their inner points, with room to spare.
const MEND_ROUNDS: usize = 4;

/// The quad `q` laid as a patch: its fitted net, and the Coons net of its sides alone.
fn lay(under: &Under, q: &([u32; 4], Vec3), curve: &HashMap<(u32, u32), [Vec3; 4]>, placed: &[Vec3]) -> ([Vec3; 16], [Vec3; 16]) {
    let at = |v: u32| placed.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let along = |x: u32, y: u32| -> [Vec3; 4] {
        match curve.get(&(x.min(y), x.max(y))) {
            Some(c) if x < y => *c,
            Some(c) => [c[3], c[2], c[1], c[0]],
            None => {
                let (a, b) = (at(x), at(y));
                [a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b]
            }
        }
    };
    let ([q0, q1, q2, q3], facing) = *q;
    let (bottom, top, left, right) = (along(q0, q1), along(q3, q2), along(q0, q3), along(q1, q2));
    let start = coons(&bottom, &top, &left, &right);
    let mut net = start;
    fit_inner(under, &mut net, facing);
    (net, start)
}

/// The most subdivided pieces the kernel examines to certify one patch regular here: past it
/// the patch is mended as one that folds (its normal comes that near vanishing). A patch that
/// folds took the kernel's whole budget, 20,000, about a second each.
const REGULAR_BUDGET: usize = 1_000;

/// Whether each net (u fastest) is a regular patch, as the kernel certifies it within
/// [`REGULAR_BUDGET`] (`solidcore_ops::surface::regular_within`), in batches across the
/// threads. Where the kernel cannot say, a net counts as regular: sewing asks again with the
/// kernel's whole budget, and refuses a folded patch by name.
fn regular_nets(nets: &[[Vec3; 16]]) -> Vec<bool> {
    const BATCH: usize = 256;
    let batches = nets.len().div_ceil(BATCH);
    let answers: Vec<Vec<bool>> = par::map(batches, |b| {
        let part = nets.get(b * BATCH..((b + 1) * BATCH).min(nets.len())).unwrap_or_default();
        let surfaces: Vec<solidcore_record::brep::Surface> = part.iter().map(bicubic_record).collect();
        solidcore_ops::surface::regular_within(&surfaces, REGULAR_BUDGET).unwrap_or_else(|_| vec![true; part.len()])
    });
    answers.into_iter().flatten().collect()
}

/// A bicubic net (u fastest) as the record's surface: one Bézier patch on [0, 1]².
fn bicubic_record(net: &[Vec3; 16]) -> solidcore_record::brep::Surface {
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let points: Vec<Vec<[f64; 3]>> = (0..4).map(|i| (0..4).map(|j| net[j * 4 + i]).map(|p| [p.x, p.y, p.z]).collect()).collect();
    solidcore_record::brep::Surface::Spline { u_knots: knots.clone(), v_knots: knots, points }
}

/// Where two fitted sides leave a patch corner at under [`SHARP_CORNER`] and under half the angle
/// of their straight chords (each side's end tangent swung toward the other, as where a side
/// crosses a crease), both sides are made straight: the corner keeps its chords' angle, which the
/// mesher can fill. The sides are shared, so both patches on each side see the same curve.
fn straighten_sharp_corners(curve: &mut HashMap<(u32, u32), [Vec3; 4]>, quads: &[([u32; 4], Vec3)], placed: &[Vec3]) {
    let at = |v: u32| placed.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let angle = |u: Vec3, v: Vec3| {
        let l = u.len() * v.len();
        if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
    };
    // The side from `x` to `y` as stored, its first leg from `x`.
    let leg = |curve: &HashMap<(u32, u32), [Vec3; 4]>, x: u32, y: u32| {
        curve.get(&(x.min(y), x.max(y))).map(|c| if x < y { c[1] - c[0] } else { c[2] - c[3] })
    };
    let mut straight: Vec<(u32, u32)> = Vec::new();
    for (q, _) in quads {
        for s in 0..4 {
            let (o, next, prev) = (q[s], q[(s + 1) % 4], q[(s + 3) % 4]);
            let (Some(t1), Some(t2)) = (leg(curve, o, next), leg(curve, o, prev)) else { continue };
            let tangent = angle(t1, t2);
            let chord = angle(at(next) - at(o), at(prev) - at(o));
            if tangent < SHARP_CORNER && tangent < 0.5 * chord {
                straight.push((o.min(next), o.max(next)));
                straight.push((o.min(prev), o.max(prev)));
            }
        }
    }
    for key in straight {
        let (a, b) = (at(key.0), at(key.1));
        curve.insert(key, [a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b]);
    }
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
fn tidy(positions: &mut [Vec3], triangles: &mut Vec<[u32; 3]>, under: &Under, tolerance: f64, poor_only: bool) {
    let poor = |c: [Vec3; 3]| shape(c) < DEGENERATE || smallest_angle(c) < MIN_ANGLE;
    let on_scan = |p: Vec3| under.tree.nearest(p).is_some_and(|(d, _)| d <= tolerance);
    let corners = |positions: &[Vec3], t: [u32; 3]| t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
    // The pass of poor faces only is cheap and a fan of needles (on a flat face) mends a flip at a
    // time: more rounds.
    let rounds = if poor_only { 4 * TIDY_ROUNDS } else { TIDY_ROUNDS };
    for _ in 0..rounds {
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
            if poor_only && !poor(c1) && !poor(c2) {
                continue;
            }
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

/// Coarse triangles paired across a shared side into quads (each its partner and the quad's
/// corners `a d b c`, counter-clockwise, `a b` the side they share): worst-shaped triangles first,
/// each with the neighbour across its longest side, else across another, when the two bend by
/// less than [`PAIR_BEND`]
/// and the quad's corners all lie between [`PAIR_CORNER`] and 180° less it (so it is convex). Two long thin
/// triangles along a rounded edge make a long rectangle, which four patches follow well, where
/// three to a triangle would meet at near-zero and near-straight corners.
fn pairs(cp: &[Vec3], triangles: &[[u32; 3]]) -> Vec<Option<(usize, [u32; 4])>> {
    let at = |v: u32| cp.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let by_edge = edge_faces(triangles);
    let mut order: Vec<usize> = (0..triangles.len()).collect();
    order.sort_by(|&x, &y| shape(triangles[x].map(at)).total_cmp(&shape(triangles[y].map(at))));
    let mut partner: Vec<Option<(usize, [u32; 4])>> = vec![None; triangles.len()];
    let angle = |o: Vec3, p: Vec3, q: Vec3| {
        let (u, v) = (p - o, q - o);
        let l = u.len() * v.len();
        if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
    };
    for t in order {
        if partner[t].is_some() {
            continue;
        }
        let tri = triangles[t];
        let c3 = tri.map(at);
        // Its sides, longest first: the longest makes a rectangle of a strip; the others, next
        // to a sharp corner, give that corner the neighbour's angle too.
        let mut sides = [0usize, 1, 2];
        sides.sort_by(|&i, &j| c3[(j + 1) % 3].dist(c3[j]).total_cmp(&c3[(i + 1) % 3].dist(c3[i])));
        let found = sides.iter().find_map(|&k| {
            let (a, b, c) = (tri[k], tri[(k + 1) % 3], tri[(k + 2) % 3]);
            let faces = by_edge.get(&(a.min(b), a.max(b)))?;
            let &[f1, f2] = faces.as_slice() else { return None };
            let u = if f1 == t { f2 } else { f1 };
            if partner[u].is_some() {
                return None;
            }
            let g = triangles[u];
            let i = (0..3).find(|&i| g[i] == b && g[(i + 1) % 3] == a)?;
            let d = g[(i + 2) % 3];
            let (n1, n2) = (unit_normal(c3)?, unit_normal(g.map(at))?);
            if n1.dot(n2) < PAIR_BEND.cos() {
                return None;
            }
            let q = [at(a), at(d), at(b), at(c)];
            // The quad's corners: at d and c the triangles' own; at a and b, the ends of the side
            // they share, the sum of the two triangles' angles there. (The angle between the quad's
            // two sides there reads a reflex corner, 210°, as 150°: a concave quad, whose patches
            // fold over themselves.)
            let corners = [
                angle(q[0], q[3], q[2]) + angle(q[0], q[2], q[1]),
                angle(q[1], q[2], q[0]),
                angle(q[2], q[0], q[3]) + angle(q[2], q[1], q[0]),
                angle(q[3], q[0], q[2]),
            ];
            let fair = corners.iter().all(|x| (PAIR_CORNER..std::f64::consts::PI - PAIR_CORNER).contains(x));
            // The four quads it is cut into (round its middle) must have fair corners too: a
            // lopsided quad gives sharp ones at its sides' middles.
            let centre = (q[0] + q[1] + q[2] + q[3]) / 4.0;
            let mid = |k: usize| (q[k] + q[(k + 1) % 4]) / 2.0;
            let quarters = (0..4).all(|k| {
                let sub = [q[k], mid(k), centre, mid((k + 3) % 4)];
                (0..4).all(|j| {
                    let x = angle(sub[j], sub[(j + 1) % 4], sub[(j + 3) % 4]);
                    (SUB_CORNER..std::f64::consts::PI - SUB_CORNER).contains(&x)
                })
            });
            (fair && quarters).then_some((u, [a, d, b, c]))
        });
        let Some((u, [a, d, b, c])) = found else { continue };
        partner[t] = Some((u, [a, d, b, c]));
        partner[u] = Some((t, [a, d, b, c]));
    }
    partner
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

/// The corners every patch of the layout keeps: each between this (10°) and 180° less it. A
/// patch with a sharper or flatter corner is one no tessellation follows well (its neighbour's
/// sides are split beside it without end) and no surface an engineer wants.
const FAIR: f64 = 0.1745;
/// Rounds of the fair-corner phase ([`fair_corners`]).
const FAIR_ROUNDS: usize = 8;
/// Steps of a longest-side propagation path ([`fair_corners`]'s split).
const LONGEST_STEPS: usize = 16;

/// The angle at `o` between the directions to `p` and `q` (radians; 0 where one is empty).
fn corner_angle(o: Vec3, p: Vec3, q: Vec3) -> f64 {
    let (u, v) = (p - o, q - o);
    let l = u.len() * v.len();
    if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
}

/// The inner corner at `o` of a polygon running `prev`, `o`, `next` counter-clockwise about `n`
/// (radians, in (0, 2π)): past half a turn where the polygon is concave there. An unsigned angle
/// between the two sides reads a reflex 200° corner as 160°, and the patches that do not mesh are
/// exactly such concave quads (a car rim's slowest patches meshed alone, research H).
fn inner_corner(prev: Vec3, o: Vec3, next: Vec3, n: Vec3) -> f64 {
    let (into, out) = (o - prev, next - o);
    std::f64::consts::PI - into.cross(out).dot(n).atan2(into.dot(out))
}

/// How far quads (each counter-clockwise about `n`) fall outside the fair corners: 0 when every
/// inner corner lies within [`FAIR`] and 180° less it, else the largest excess (radians).
fn quads_unfair(quads: &[[Vec3; 4]], n: Vec3) -> f64 {
    quads
        .iter()
        .flat_map(|q| (0..4).map(move |k| inner_corner(q[(k + 3) % 4], q[k], q[(k + 1) % 4], n)))
        .map(|x| (FAIR - x).max(x - (std::f64::consts::PI - FAIR)).max(0.0))
        .fold(0.0, f64::max)
}

/// How far the three patches a triangle is laid as when it is not paired fall outside the fair
/// corners ([`quads_unfair`]), judged as they will be laid: its corners, the middles of its sides
/// and its [`inner_point`] each placed onto the scan along its normal ([`Under::under`]), as
/// [`patches`] places them. Placing moves a short side's ends by as much as the coarse mesh
/// strays from the scan, which can make a fair layout quad concave. A triangle's own angles are
/// three of its patches' corners, so a needle or a cap is never fair alone; paired into a fair
/// quad it may be ([`pair_unfair`]). A degenerate triangle is unfair by [`FAIR`].
fn unfair(under: &Under, c: [Vec3; 3]) -> f64 {
    let [a, b, d] = c;
    let Some(n) = unit_normal(c) else { return FAIR };
    let put = |p: Vec3| under.under(p, n).unwrap_or(p);
    let g = put(inner_point(a, b, d));
    let (mab, mbd, mda) = (put((a + b) / 2.0), put((b + d) / 2.0), put((d + a) / 2.0));
    let (a, b, d) = (put(a), put(b), put(d));
    quads_unfair(&[[a, mab, g, mda], [b, mbd, g, mab], [d, mda, g, mbd]], n)
}

/// The same for a pair laid as one quad `q` (counter-clockwise, `a d b c` as [`pairs`] gives it)
/// cut in four round its middle, facing `n`.
fn pair_unfair(under: &Under, q: [Vec3; 4], n: Vec3) -> f64 {
    let put = |p: Vec3| under.under(p, n).unwrap_or(p);
    let centre = put((q[0] + q[1] + q[2] + q[3]) / 4.0);
    let mid = |k: usize| put((q[k] + q[(k + 1) % 4]) / 2.0);
    let corner = q.map(put);
    let quarters: Vec<[Vec3; 4]> = (0..4).map(|k| [corner[k], mid(k), centre, mid((k + 3) % 4)]).collect();
    quads_unfair(&quarters, n)
}

/// Each coarse triangle's unfairness as the layout will lay it: alone ([`unfair`]) or with its
/// pair ([`pair_unfair`], the same for both).
fn layout_unfair(under: &Under, positions: &[Vec3], triangles: &[[u32; 3]], partner: &[Option<(usize, [u32; 4])>]) -> Vec<f64> {
    let at = |v: u32| positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    par::map(triangles.len(), |f| {
        let c = triangles[f].map(at);
        match partner.get(f).copied().flatten() {
            Some((u, quad)) => {
                let other = triangles.get(u).map(|t| t.map(at));
                let n = unit_normal(c).zip(other.and_then(unit_normal)).map(|(n1, n2)| n1 + n2).filter(|n| n.len() > 0.0).map(|n| n / n.len());
                n.map_or(FAIR, |n| pair_unfair(under, quad.map(at), n))
            }
            None => unfair(under, c),
        }
    })
}

/// The coarse mesh mended until every patch the layout lays on it has fair corners ([`FAIR`]),
/// held within `tolerance` of the scan: Hu, Yan, Bommes, Alliez and Benes's angle improvement
/// ("Error-bounded and feature preserving surface remeshing with minimal angle improvement", TVCG
/// 2017, §3.2), with the patches' corners in place of the triangles' angles. The unfair triangles
/// (not paired, see [`unfair`]), worst first, each by the first of these that works:
/// - the side opposite its smallest angle collapsed, either way round (a needle's short side, a
///   cap's point onto an end of its long side);
/// - the side opposite its largest angle flipped (a cap's long side: Botsch and Kobbelt's mend,
///   which subdividing cannot make);
/// - one of its points moved toward the middle of its neighbours and back onto the scan;
/// - the longest side along its longest-side path (Rivara) split at the scan's point nearest its
///   middle, which may mend nothing at once but gives the next round points to work with.
///
/// Each is made only when the mesh stays manifold, no point on the open rim moves, no face turns
/// over, every face it makes lies on the scan within `tolerance` and covers what it replaces, and
/// the worst patch corner of the faces it changes, as laid ([`unfair`]), becomes fairer than it
/// was (for the split: no worse): no new corner is worse than the one being mended. Rounds until none is unfair or
/// [`FAIR_ROUNDS`]; how many triangles are still unfair.
fn fair_corners(positions: &mut Vec<Vec3>, triangles: &mut Vec<[u32; 3]>, under: &Under, tolerance: f64) -> usize {
    let on_scan = |p: Vec3| under.tree.nearest(p).is_some_and(|(d, _)| d <= tolerance);
    let onto_scan =
        |p: Vec3| under.tree.nearest(p).and_then(|(_, s)| under.scan.corners(s as usize)).map(|[a, b, c]| closest_on_triangle(p, a, b, c));
    for _ in 0..FAIR_ROUNDS {
        let partner = pairs(positions, triangles);
        let corners_of = |positions: &[Vec3], t: [u32; 3]| t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
        // A face's unfairness as laid now: none when it is paired.
        let status = layout_unfair(under, positions, triangles, &partner);
        let mut bad: Vec<usize> = (0..triangles.len()).filter(|&f| status[f] > 0.0).collect();
        if bad.is_empty() {
            break;
        }
        bad.sort_by(|&x, &y| status[y].total_cmp(&status[x]).then(x.cmp(&y)));
        let by_edge = edge_faces(triangles);
        let mut around: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
        for (f, tri) in triangles.iter().enumerate() {
            for &v in tri {
                if let Some(a) = around.get_mut(v as usize) {
                    a.push(f);
                }
            }
        }
        // A point on the open rim, or on a crease (two faces bending more than 30°), stays.
        let mut fixed = vec![false; positions.len()];
        for ((a, b), faces) in &by_edge {
            let keep = match faces.as_slice() {
                [f1, f2] => match (unit_normal(corners_of(positions, triangles[*f1])), unit_normal(corners_of(positions, triangles[*f2]))) {
                    (Some(n1), Some(n2)) => n1.dot(n2) < CREASE,
                    _ => false,
                },
                _ => true,
            };
            if keep {
                fixed[*a as usize] = true;
                fixed[*b as usize] = true;
            }
        }
        let mut touched = vec![false; triangles.len()];
        let mut gone = vec![false; triangles.len()];
        let mut added: Vec<[u32; 3]> = Vec::new();
        let mut changed = false;
        for t in bad {
            if touched[t] || gone[t] {
                continue;
            }
            let tri = triangles[t];
            let c = corners_of(positions, tri);
            let angles = [0, 1, 2].map(|k| corner_angle(c[k], c[(k + 1) % 3], c[(k + 2) % 3]));
            let smallest = (0..3).min_by(|&i, &j| angles[i].total_cmp(&angles[j])).unwrap_or(0);
            let largest = (0..3).max_by(|&i, &j| angles[i].total_cmp(&angles[j])).unwrap_or(0);
            // The faces an operation changes, each with the face it becomes (`None`: taken out),
            // and the faces it adds; it is made only if `accept` says so.
            let mut done: Option<(Vec<(usize, Option<[u32; 3]>)>, Vec<[u32; 3]>, Option<(u32, Vec3)>)> = None;
            // 1. Collapse the side opposite the smallest angle, either way round.
            let (x, y) = (tri[(smallest + 1) % 3], tri[(smallest + 2) % 3]);
            for (m, to) in [(x, y), (y, x)] {
                if done.is_some() || fixed[m as usize] {
                    continue;
                }
                let fan = &around[m as usize];
                if fan.iter().any(|&f| touched[f] || gone[f]) {
                    continue;
                }
                let neighbours = |v: u32| -> Vec<u32> {
                    let mut n: Vec<u32> = around[v as usize].iter().flat_map(|&f| triangles[f]).filter(|&w| w != v).collect();
                    n.sort_unstable();
                    n.dedup();
                    n
                };
                let shared: Vec<usize> = fan.iter().copied().filter(|&f| triangles[f].contains(&to)).collect();
                let opposite = shared.iter().flat_map(|&f| triangles[f]).filter(|&w| w != m && w != to).count();
                let (nm, nt) = (neighbours(m), neighbours(to));
                if shared.is_empty() || nm.iter().filter(|w| nt.contains(w)).count() != opposite {
                    continue;
                }
                let pm = positions[m as usize];
                let mut ok = true;
                let mut ops: Vec<(usize, Option<[u32; 3]>)> = shared.iter().map(|&f| (f, None)).collect();
                for &f in fan.iter().filter(|f| !shared.contains(f)) {
                    let after = triangles[f].map(|w| if w == m { to } else { w });
                    let (cb, ca) = (corners_of(positions, triangles[f]), corners_of(positions, after));
                    let turned = match (unit_normal(cb), unit_normal(ca)) {
                        (Some(nb), Some(na)) => nb.dot(na) < 0.0,
                        (_, None) => true,
                        (None, Some(_)) => false,
                    };
                    if turned || !on_scan((ca[0] + ca[1] + ca[2]) / 3.0) {
                        ok = false;
                        break;
                    }
                    ops.push((f, Some(after)));
                }
                let covered = ops.iter().filter_map(|(_, a)| *a).any(|a| {
                    let q = corners_of(positions, a);
                    closest_on_triangle(pm, q[0], q[1], q[2]).dist(pm) <= tolerance
                });
                if ok && covered {
                    done = Some((ops, Vec::new(), None));
                }
            }
            // 2. Flip the side opposite the largest angle.
            if done.is_none() {
                let (a, b) = (tri[(largest + 1) % 3], tri[(largest + 2) % 3]);
                let z = tri[largest];
                if let Some(&[f1, f2]) = by_edge.get(&(a.min(b), a.max(b))).map(Vec::as_slice) {
                    let u = if f1 == t { f2 } else { f1 };
                    let g = triangles[u];
                    // t runs a → b with z opposite; u runs b → a with w opposite.
                    let w = (0..3).find(|&i| g[i] == b && g[(i + 1) % 3] == a).map(|i| g[(i + 2) % 3]);
                    if let Some(w) = w
                        && !touched[u]
                        && w != z
                        && !by_edge.contains_key(&(z.min(w), z.max(w)))
                    {
                        let (g1, g2) = ([a, w, z], [w, b, z]);
                        let (d1, d2) = (corners_of(positions, g1), corners_of(positions, g2));
                        let (cu, ct) = (corners_of(positions, g), c);
                        // The way the pair faces: by the fairer of the two (a cap's own normal is noise).
                        let n = if shape(ct) >= shape(cu) { unit_normal(ct) } else { unit_normal(cu) };
                        let old_mid = (positions[a as usize] + positions[b as usize]) / 2.0;
                        let near = |q: [Vec3; 3]| closest_on_triangle(old_mid, q[0], q[1], q[2]).dist(old_mid);
                        let faces_ok = match (n, unit_normal(d1), unit_normal(d2)) {
                            (Some(n), Some(m1), Some(m2)) => m1.dot(n) >= FLAT && m2.dot(n) >= FLAT,
                            _ => false,
                        };
                        if faces_ok
                            && on_scan((d1[0] + d1[1] + d1[2]) / 3.0)
                            && on_scan((d2[0] + d2[1] + d2[2]) / 3.0)
                            && near(d1).min(near(d2)) <= tolerance
                        {
                            done = Some((vec![(t, Some(g1)), (u, Some(g2))], Vec::new(), None));
                        }
                    }
                }
            }
            // 3. Move one of its points toward the middle of its neighbours, onto the scan.
            if done.is_none() {
                for &v in &tri {
                    if done.is_some() || fixed[v as usize] {
                        continue;
                    }
                    let fan = &around[v as usize];
                    if fan.iter().any(|&f| touched[f] || gone[f]) {
                        continue;
                    }
                    let p = positions[v as usize];
                    let mut nb: Vec<u32> = fan.iter().flat_map(|&f| triangles[f]).filter(|&w| w != v).collect();
                    nb.sort_unstable();
                    nb.dedup();
                    if nb.is_empty() {
                        continue;
                    }
                    let mid = nb.iter().fold(Vec3::ZERO, |acc, &w| acc + positions[w as usize]) / nb.len() as f64;
                    let normal = fan.iter().filter_map(|&f| unit_normal(corners_of(positions, triangles[f]))).fold(Vec3::ZERO, |acc, m| acc + m);
                    let Some(normal) = (normal.len() > 0.0).then(|| normal / normal.len()) else { continue };
                    for step in [1.0, 0.5] {
                        let target = p + ((mid - p) - normal * (mid - p).dot(normal)) * step;
                        let Some(on) = onto_scan(target) else { continue };
                        let moved = |w: u32| if w == v { on } else { positions[w as usize] };
                        let mut ok = true;
                        let mut after_faces = Vec::new();
                        for &f in fan {
                            let (before, after) = (corners_of(positions, triangles[f]), triangles[f].map(moved));
                            let turned = match (unit_normal(before), unit_normal(after)) {
                                (Some(x), Some(y)) => x.dot(y) < 0.5,
                                (_, None) => true,
                                (None, Some(_)) => false,
                            };
                            if turned || !on_scan((after[0] + after[1] + after[2]) / 3.0) {
                                ok = false;
                                break;
                            }
                            after_faces.push(after);
                        }
                        let covered = after_faces.iter().any(|q| closest_on_triangle(p, q[0], q[1], q[2]).dist(p) <= tolerance);
                        let worse_before = fan.iter().map(|&f| status[f]).fold(0.0, f64::max);
                        let worse_after = after_faces.iter().map(|&q| unfair(under, q)).fold(0.0, f64::max);
                        if ok && covered && worse_after < worse_before {
                            done = Some((fan.iter().map(|&f| (f, Some(triangles[f]))).collect(), Vec::new(), Some((v, on))));
                            break;
                        }
                    }
                }
            }
            // Collapses and flips: only when fairer than before (the move checked its own).
            if let Some((ops, _, None)) = &done {
                let before = ops.iter().map(|(f, _)| status[*f]).fold(0.0, f64::max);
                let after = ops.iter().filter_map(|(_, a)| *a).map(|a| unfair(under, corners_of(positions, a))).fold(0.0, f64::max);
                if !(after < before) {
                    done = None;
                }
            }
            // 4. Split the longest side along the longest-side path.
            if done.is_none() {
                let longest = |f: usize| {
                    let q = corners_of(positions, triangles[f]);
                    (0..3).max_by(|&i, &j| q[(i + 1) % 3].dist(q[i]).total_cmp(&q[(j + 1) % 3].dist(q[j]))).map(|k| {
                        let (x, y) = (triangles[f][k], triangles[f][(k + 1) % 3]);
                        ((x, y), q[k].dist(q[(k + 1) % 3]))
                    })
                };
                let mut f = t;
                for _ in 0..LONGEST_STEPS {
                    let Some(((x, y), len)) = longest(f) else { break };
                    let across = by_edge.get(&(x.min(y), x.max(y))).and_then(|fs| fs.iter().copied().find(|&g| g != f));
                    match across.and_then(|g| longest(g).map(|l| (g, l))) {
                        Some((g, (_, l))) if l > len * (1.0 + 1e-9) => f = g,
                        _ => break,
                    }
                }
                if let Some(((x, y), _)) = longest(f) {
                    let faces: Vec<usize> = by_edge.get(&(x.min(y), x.max(y))).cloned().unwrap_or_default();
                    let mid = (positions[x as usize] + positions[y as usize]) / 2.0;
                    if faces.iter().all(|&g| !touched[g] && !gone[g])
                        && let Some(q) = onto_scan(mid)
                        && let Ok(n) = u32::try_from(positions.len())
                    {
                        let mut ops = Vec::new();
                        let mut new = Vec::new();
                        let mut ok = true;
                        for &g in &faces {
                            let tri_g = triangles[g];
                            // g runs i → j along the side with o opposite.
                            let Some(k) = (0..3).find(|&k| {
                                let (i, j) = (tri_g[k], tri_g[(k + 1) % 3]);
                                (i == x && j == y) || (i == y && j == x)
                            }) else {
                                ok = false;
                                break;
                            };
                            let (i, j, o) = (tri_g[k], tri_g[(k + 1) % 3], tri_g[(k + 2) % 3]);
                            let old = corners_of(positions, tri_g);
                            for half in [[i, n, o], [n, j, o]] {
                                let h = half.map(|w| if w == n { q } else { positions[w as usize] });
                                let flat = match (unit_normal(old), unit_normal(h)) {
                                    (Some(a), Some(b)) => a.dot(b) > 0.0,
                                    _ => false,
                                };
                                if !flat || !on_scan((h[0] + h[1] + h[2]) / 3.0) {
                                    ok = false;
                                }
                            }
                            ops.push((g, Some([i, n, o])));
                            new.push([n, j, o]);
                        }
                        // No face it makes less fair, as laid, than the worst it replaces.
                        let placed = |f: [u32; 3]| unfair(under, f.map(|w| if w == n { q } else { positions[w as usize] }));
                        let before = faces.iter().map(|&g| status[g]).fold(0.0, f64::max);
                        let after = ops.iter().filter_map(|(_, a)| *a).chain(new.iter().copied()).map(placed).fold(0.0, f64::max);
                        if ok && !faces.is_empty() && after <= before {
                            done = Some((ops, new, Some((n, q))));
                        }
                    }
                }
            }
            let Some((ops, new, point)) = done else { continue };
            // Every face round a point of a face it changed is taken as changed this round (their
            // fans are no longer what `around` says).
            if let Some((v, p)) = point {
                if v as usize == positions.len() {
                    positions.push(p);
                    fixed.push(false);
                    around.push(Vec::new());
                } else if let Some(slot) = positions.get_mut(v as usize) {
                    *slot = p;
                }
            }
            let mut ring: Vec<u32> = ops.iter().flat_map(|(f, _)| triangles[*f]).collect();
            ring.extend(new.iter().flatten());
            for (f, after) in &ops {
                match after {
                    Some(a) => triangles[*f] = *a,
                    None => gone[*f] = true,
                }
            }
            for &v in &ring {
                for &f in around.get(v as usize).map_or(&[][..], Vec::as_slice) {
                    if let Some(x) = touched.get_mut(f) {
                        *x = true;
                    }
                }
            }
            added.extend(new);
            changed = true;
        }
        if !changed {
            break;
        }
        let mut k = 0;
        triangles.retain(|_| {
            let keep = !gone[k];
            k += 1;
            keep
        });
        triangles.extend(added);
    }
    // How many are unfair as the mesh is left.
    let partner = pairs(positions, triangles);
    layout_unfair(under, positions, triangles, &partner).iter().filter(|&&x| x > 0.0).count()
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
        let (Some(long), Some(short)) = ((0..3).max_by(|&i, &j| side(i).total_cmp(&side(j))), (0..3).min_by(|&i, &j| side(i).total_cmp(&side(j))))
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
    // Each sampled point stands for a third of its triangles' area, so the share within is a
    // share of the surface (as a reader measures it), not of the points: a scan's points are
    // densest where it is detailed, and counting them hid misses on its broad faces.
    let mut area = vec![0.0; n];
    for t in scan.triangles {
        let c = t.map(|v| scan.positions.get(v as usize).copied().unwrap_or(Vec3::ZERO));
        let third = (c[1] - c[0]).cross(c[2] - c[0]).len() / 6.0;
        for &v in t {
            if let Some(a) = area.get_mut(v as usize) {
                *a += third;
            }
        }
    }
    let weight: Vec<f64> = sampled.iter().map(|&v| area.get(v).copied().unwrap_or(0.0)).collect();
    let cap = settings.max_patches.max(12);
    let mut budget = vec![START * tol; n];
    let mut mark = vec![0usize; n];
    let mut best: Option<Wrap> = None;
    for round in 1..=ROUNDS {
        let (mut r, capped, scale, wanted) = reduce_once(scan, &tree, &budget, cap)?;
        if r.triangles.is_empty() {
            return Err(WrapError::Reduce);
        }
        // A capped mesh lies off the scan by the scaled budgets: its tidying is held to them too,
        // or every repair of a sliver would look like leaving the scan and none would be made.
        tidy(&mut r.positions, &mut r.triangles, &under, tol * scale.max(1.0), false);
        // Slivers it could not mend within that (a needle whose collapse or a cap whose flip
        // misses the scan by a little) are mended within three times it: they cover next to no
        // area, and left in, the mesher splits their long sides without end.
        tidy(&mut r.positions, &mut r.triangles, &under, POOR_SLACK * tol * scale.max(1.0), true);
        // Last, every patch the layout will lay given fair corners, held to the scaled budgets.
        let unfair = fair_corners(&mut r.positions, &mut r.triangles, &under, tol * scale.max(1.0));
        let faces = patches(&under, &r.positions, &r.triangles);
        let d = distances(scan, &faces, &sampled);
        let spread = spread(&d, &weight, tol);
        let mut order: Vec<usize> = (0..d.len()).collect();
        order.sort_by(|&x, &y| d[y].total_cmp(&d[x]));
        let worst: Vec<(Vec3, f64)> =
            order.into_iter().take(WORST).filter_map(|k| sampled.get(k).and_then(|&v| scan.positions.get(v)).map(|&p| (p, d[k]))).collect();
        let met = spread.within >= settings.share && spread.max <= 3.0 * tol;
        let better = best.as_ref().is_none_or(|b| spread.within > b.scan_to_patches.within);
        let coarse = Coarse { positions: r.positions, triangles: r.triangles, rounds: round, met, capped, wanted, unfair };
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
        let (r, capped, _, _) = reduce_once(&scan, &tree, &vec![0.1; p.len()], usize::MAX).expect("reduce");
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
    fn a_cap_on_the_rim_is_mended_until_every_patch_corner_is_fair() {
        // The scan: the plane z = 0 over 40 × 40 mm, 1 mm cells.
        let n = 41u32;
        let p: Vec<Vec3> = (0..n * n).map(|k| Vec3::new(f64::from(k % n), f64::from(k / n), 0.0)).collect();
        let mut t = Vec::new();
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let at = |i: u32, j: u32| j * n + i;
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let scan = Scan::new(&p, &t);
        let under = Under { scan: &scan, tree: TriangleTree::of(&p, &t) };
        // The coarse mesh: the square fanned round its middle E, with F 0.4 mm inside its lower
        // side, so A B F is a cap of 178° on the rim (its patches' corners 1.1° and 177.7°).
        let (a, b, c, d, e, f) = (0u32, 1, 2, 3, 4, 5);
        let mut q = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(40.0, 0.0, 0.0),
            Vec3::new(40.0, 40.0, 0.0),
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(20.0, 20.0, 0.0),
            Vec3::new(20.0, 0.4, 0.0),
        ];
        let mut tris = vec![[a, b, f], [a, f, e], [f, b, e], [b, c, e], [c, d, e], [d, a, e]];
        assert!(unfair(&under, tris[0].map(|i| q[i as usize])) > 0.0, "the cap starts unfair");
        let left = fair_corners(&mut q, &mut tris, &under, 0.1);
        assert_eq!(left, 0, "{tris:?}");
        for tri in &tris {
            let c = tri.map(|i| q[i as usize]);
            assert!(unfair(&under, c) == 0.0, "{tri:?}: corners {}", unfair(&under, c).to_degrees());
            assert!(unit_normal(c).is_some_and(|m| m.z > 0.99), "{tri:?} turned over");
        }
        // The rim stays: the square's corners where they were, its area all covered.
        assert_eq!(&q[..4], &[Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0), Vec3::new(40.0, 40.0, 0.0), Vec3::new(0.0, 40.0, 0.0)]);
        let area: f64 =
            tris.iter().map(|tri| 0.5 * (q[tri[1] as usize] - q[tri[0] as usize]).cross(q[tri[2] as usize] - q[tri[0] as usize]).len()).sum();
        assert!((area - 1600.0).abs() < 1e-9, "area {area}");
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
        tidy(&mut q, &mut f, &under, 0.2, false);
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

    /// A flat net on the unit grid is regular, and still is with the second point of row 1 moved
    /// past the third (x 2.5 where the third is at 2): at any v the surface's row is the rows'
    /// Bernstein blend, whose second point's x is 1 + 1.5 · 3v(1 − v)² ≤ 1.67 < 2, so S_u keeps
    /// running +x and S_u × S_v keeps +z (the old row-and-column test refused this patch). The
    /// bicubic of the bilinear map over the dart (0, 0), (10, 0), (2, 2), (0, 10) folds: S_u × S_v
    /// is 10 × 10 = +100 along z at (0, 0) and (2, −8) × (−8, 2) = −60 at (1, 1).
    #[test]
    fn a_net_regular_where_its_normal_never_vanishes() {
        let flat: [Vec3; 16] = std::array::from_fn(|k| Vec3::new((k % 4) as f64, (k / 4) as f64, 0.0));
        let mut doubled = flat;
        doubled[5] = Vec3::new(2.5, 1.0, 0.0);
        let [p00, p10, p11, p01] = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.0), Vec3::new(0.0, 10.0, 0.0)];
        let dart: [Vec3; 16] = std::array::from_fn(|k| {
            let (u, v) = ((k % 4) as f64 / 3.0, (k / 4) as f64 / 3.0);
            p00 * ((1.0 - u) * (1.0 - v)) + p10 * (u * (1.0 - v)) + p11 * (u * v) + p01 * ((1.0 - u) * v)
        });
        assert_eq!(regular_nets(&[flat, doubled, dart]), [true, true, false]);
    }

    /// Two triangles a (0, 0), b (10, 0), c (-6, -12) and b, a, d (0, 1), sharing a b, make a
    /// concave quad a d b c: its corner at a is the sum of their angles there, 116.6° + 90° =
    /// 206.6° (the angle between its sides there reads 153.4°), so they are not paired. With c
    /// at (5, -6) and d at (5, 6) the quad is a rhombus (corners 100.4° and 79.6°) and they are.
    #[test]
    fn two_triangles_making_a_concave_quad_are_not_paired() {
        let at = |x: f64, y: f64| Vec3::new(x, y, 0.0);
        let concave = [at(0.0, 0.0), at(10.0, 0.0), at(-6.0, -12.0), at(0.0, 1.0)];
        let tris = [[0u32, 1, 2], [1, 0, 3]];
        assert_eq!(pairs(&concave, &tris), [None, None]);
        let rhombus = [at(0.0, 0.0), at(10.0, 0.0), at(5.0, -6.0), at(5.0, 6.0)];
        assert!(pairs(&rhombus, &tris).iter().all(Option::is_some));
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
                .flat_map(|q| {
                    (0..4).map(move |k| {
                        let (u, v) = (q[(k + 1) % 4] - q[k], q[(k + 3) % 4] - q[k]);
                        (u.dot(v) / (u.len() * v.len())).clamp(-1.0, 1.0).acos().to_degrees()
                    })
                })
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
        // Three patches to a triangle, four to a pair of them.
        let n = w.coarse.triangles.len();
        assert!((2 * n..=3 * n).contains(&w.faces.len()), "{} patches on {n} triangles", w.faces.len());
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
        // At 0.01 mm a sphere of R 20 asks for far more than 600 patches.
        assert!(w.coarse.wanted > 600, "wants {}", w.coarse.wanted);
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
