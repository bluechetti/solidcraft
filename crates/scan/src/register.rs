//! Registration: one scan laid onto another (a second pass onto the first), after SurfaceCraft's.
//! The fit is iterative closest point, point to plane, both ways (the moving scan's points to the
//! reference and the reference's to the moving scan), trimmed of the share that has nothing under
//! it and robustly weighted. It starts from where the scan lies and, searching, also from every way
//! the two principal-axis frames can lie on each other and from the moves that matched surface
//! features agree on (Fast Point Feature Histograms, Rusu, Blodow and Beetz 2009, matched and
//! sampled by consensus) and, for passes that meet only along a seam, from the flat faces they
//! share (`frames`); each start is judged by how much of the scan it lays on the reference,
//! close and facing the same way, so a part that overlaps in part is not laid askew. A second
//! placement nearly as good (a part nearly its own mirror image) is said.

use solidcore_record::Vec3;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::place::Rigid;

mod frames;
use crate::{Scan, par};

/// How a scan is laid onto a reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Search the starts (principal axes, matched features) as well as where the scan lies.
    pub search: bool,
    /// The share of the scan, 0 to 0.6, the fit may leave out because the reference has nothing
    /// near it.
    pub ignored: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { search: true, ignored: 0.2 }
    }
}

/// What laying a scan onto a reference found.
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    /// The move that lays the scan on the reference.
    pub moved: Rigid,
    /// How far the scan lay from the reference before and after (the trimmed root mean square of
    /// the nearest distances), in mm.
    pub rms_before: f64,
    pub rms_after: f64,
    /// The rms of the distances of the share of the scan that lies on the reference (`matched`):
    /// how well what overlaps fits. `rms_after` counts all of the scan that is not ignored, so
    /// for passes that overlap in part it is as large as what has no partner is far off.
    pub rms_matched: f64,
    /// The share of the scan within a few widths of the surfaces' roughness of the reference after.
    pub matched: f64,
    pub rounds: usize,
    pub converged: bool,
    /// Where the winning start came from.
    pub start: &'static str,
    /// Another placement, well apart, that lays nearly as much of the scan on the reference.
    pub rival: Option<String>,
    /// Set when the fit would have left the scan further from the reference than it lay, so it
    /// was left where it was: the trimmed root mean square the fit reached, in mm.
    pub kept: Option<f64>,
    /// The share of the scan the fit was allowed to leave out.
    pub ignored: f64,
}

impl Registration {
    /// What a person should know about this fit besides its numbers, if anything.
    pub fn note(&self) -> Option<String> {
        if let Some(worse) = self.kept {
            return Some(format!(
                "left where it was: fitting it moved it further off ({:.3} mm rms instead of {:.3} mm); lay it nearer by hand (scan.align_pairs), or lower `ignored`",
                worse, self.rms_before
            ));
        }
        if self.matched < WEAK {
            return Some(format!(
                "only {:.0} % of the scan lies on the other: check it by eye (a seam where two passes meet fits this way; two similar flat panels laid one over the other can too); scan.align_pairs places it by points",
                100.0 * self.matched
            ));
        }
        if self.ignored < 0.6 && 1.0 - self.matched > self.ignored + 0.1 {
            return Some(format!(
                "{:.0} % of the scan lies off the other but the fit may leave out only {:.0} % (`ignored`), so its rms counts what has no partner; passes that overlap in part want `ignored` up to 0.6",
                100.0 * (1.0 - self.matched),
                100.0 * self.ignored
            ));
        }
        if !self.converged {
            return Some("the fit was still settling when it stopped".into());
        }
        None
    }
}

/// Why a scan could not be laid onto a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// A scan has too little surface to fit.
    TooLittle,
    /// The fit found too few points of the scan near the reference.
    NoOverlap,
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RegisterError::TooLittle => "a scan has too little surface to lay onto the other",
            RegisterError::NoOverlap => "the scans have too little in common to lay one onto the other",
        })
    }
}

/// Samples the fit works with, and the finer ones it finishes with.
const FEW: usize = 1_500;
const MANY: usize = 8_000;
/// Points each scan's features are described at.
const FEATURE_POINTS: usize = 4_000;
/// The most vertices the last pass and the matched share are read at.
const VERTEX_LIMIT: usize = 200_000;
/// The most of the scan's vertices the fit is taken to its end on ([`polish`]).
const POLISH_POINTS: usize = 40_000;
/// How many widths of the surfaces' roughness two surfaces of one part may lie apart.
const ROUGHNESS_REACH: f64 = 5.0;
/// Two placements are two when one turns more than this from the other (5°), or moves the scan's
/// middle more than this many spacings.
const DISTINCT_TURN: f64 = 0.087;
const DISTINCT_SLIDE: f64 = 3.0;
/// The most placements fitted to the end and judged again, and the share of the best one must
/// reach to be among them; a second at this share of the best is a rival.
/// A start whose fit lays this share of what may be fitted (all but `ignored`) onto the
/// reference makes the search by flat faces needless (`register`).
const ENOUGH_LAID: f64 = 0.75;
const MOST_FINALISTS: usize = 4;
const FINALIST_SHARE: f64 = 0.5;
const RIVAL_SHARE: f64 = 0.9;
/// The cosine two normals must reach to face the same way, either way round.
const FACING: f64 = 0.8;
/// Below this share of the scan lying on the other, a fit is called weak.
const WEAK: f64 = 0.2;
/// The cosine the gap to a nearest point must reach with the surface's normal there for the pair
/// to count in a strict fit: a gap along the surface runs to its rim or an edge, not to the surface.
const ACROSS: f64 = 0.7;
/// The median absolute deviation as a standard deviation, for normal noise.
const MAD_TO_SIGMA: f64 = 1.4826;

impl Rigid {
    /// This move, then `after`.
    pub fn then(&self, after: &Rigid) -> Rigid {
        let rows = |r: &[[f64; 3]; 3], c: &[[f64; 3]; 3]| -> [[f64; 3]; 3] {
            std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r[i][k] * c[k][j]).sum()))
        };
        Rigid { rotation: rows(&after.rotation, &self.rotation), translation: after.apply(self.translation) }
    }

    /// How far this move turns from `other`, in radians.
    pub fn turn_from(&self, other: &Rigid) -> f64 {
        let (a, b) = (self.rotation, other.rotation);
        let trace: f64 = (0..3).map(|i| (0..3).map(|k| a[k][i] * b[k][i]).sum::<f64>()).sum();
        ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos()
    }
}

/// A scan as the fit reads it: its nearest points and its triangles' directions.
pub(crate) struct Surface<'a> {
    scan: Scan<'a>,
    tree: TriangleTree,
    diagonal: f64,
}

/// The nearest point of a surface to a point.
#[derive(Clone, Copy)]
pub(crate) struct Hit {
    pub(crate) point: Vec3,
    pub(crate) distance: f64,
    pub(crate) normal: Option<Vec3>,
}

impl<'a> Surface<'a> {
    pub(crate) fn new(scan: Scan<'a>) -> Self {
        let mesh = solidcraft_geom::Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Default::default() };
        Surface { tree: TriangleTree::new(&mesh), diagonal: scan.extent().max(1e-9), scan }
    }

    pub(crate) fn nearest(&self, p: Vec3) -> Option<Hit> {
        let (distance, t) = self.tree.nearest(p)?;
        let [a, b, c] = self.scan.corners(t as usize)?;
        Some(Hit { point: closest_on_triangle(p, a, b, c), distance, normal: (b - a).cross(c - a).normalized() })
    }
}

/// How pairs are made: the farthest pair kept, and whether the pairs the surfaces cannot agree on
/// are left out. A strict fit takes only pairs that face each other and whose gap runs across the
/// surface, so the open rim of one pass does not draw the other onto it, as a thin overlap's
/// does; a loose one also pairs along a gap at an edge, which draws a far start in.
#[derive(Clone, Copy, Debug)]
struct Rule {
    gate: f64,
    strict: bool,
}

impl Rule {
    fn loose(gate: f64) -> Self {
        Rule { gate, strict: false }
    }

    fn strict(gate: f64) -> Self {
        Rule { gate, strict: true }
    }
}

/// A point of the scan with what it is matched to.
#[derive(Clone, Copy, Debug)]
struct Pair {
    point: Vec3,
    target: Vec3,
    normal: Vec3,
    distance: f64,
    weight: f64,
}

/// Points spread over the scan's surface in proportion to area (a low-discrepancy pattern, so the
/// same scan gives the same points), at most `count`.
pub fn sample(scan: &Scan, count: usize) -> Vec<Vec3> {
    let mut cumulative = Vec::with_capacity(scan.triangle_count());
    let mut total = 0.0;
    for t in 0..scan.triangle_count() {
        let area = scan.corners(t).map_or(0.0, |[a, b, c]| (b - a).cross(c - a).len() / 2.0);
        total += if area.is_finite() { area } else { 0.0 };
        cumulative.push(total);
    }
    if !(total > 0.0) || count == 0 {
        return Vec::new();
    }
    let (g1, g2) = (0.754_877_666_246_692_8, 0.569_840_290_998_053_3);
    (0..count)
        .filter_map(|i| {
            let at = (i as f64 + 0.5) / count as f64 * total;
            let t = cumulative.partition_point(|&c| c < at).min(cumulative.len() - 1);
            let [a, b, c] = scan.corners(t)?;
            let (mut r1, mut r2) = ((0.5 + i as f64 * g1).fract(), (0.5 + i as f64 * g2).fract());
            if r1 + r2 > 1.0 {
                (r1, r2) = (1.0 - r1, 1.0 - r2);
            }
            Some(a + (b - a) * r1 + (c - a) * r2)
        })
        .collect()
}

fn every(points: &[Vec3], limit: usize) -> Vec<Vec3> {
    crate::thinned(points, limit)
}

/// The rms of the nearest distances from `points`, moved, to `reference`, the worst `ignored`
/// share left out.
pub(crate) fn trimmed_rms(points: &[Vec3], moved: &Rigid, reference: &Surface, ignored: f64) -> f64 {
    let mut distances: Vec<f64> =
        par::map(points.len(), |i| reference.nearest(moved.apply(points[i])).map(|h| h.distance)).into_iter().flatten().collect();
    if distances.is_empty() {
        return f64::INFINITY;
    }
    distances.sort_by(f64::total_cmp);
    let keep = ((distances.len() as f64 * (1.0 - ignored)).round() as usize).clamp(1, distances.len());
    (distances[..keep].iter().map(|d| d * d).sum::<f64>() / keep as f64).sqrt()
}

/// Each point, moved, paired with the nearest point of `reference`, measured along the
/// reference's direction there (along the gap itself at an edge or a corner). `own` holds the
/// points' normals, for a strict rule.
fn pairs_to(points: &[Vec3], moved: &Rigid, reference: &Surface, rule: Rule, own: &[Vec3]) -> Vec<Pair> {
    par::map(points.len(), |i| {
        let p = moved.apply(points[i]);
        let hit = reference.nearest(p)?;
        let mut normal = hit.normal?;
        let delta = p - hit.point;
        if rule.strict && !agrees(moved.turned(*own.get(i)?), normal, delta, hit.distance, reference.diagonal) {
            return None;
        }
        if hit.distance > 1e-10 * reference.diagonal && delta.dot(normal).abs() < (1.0 - 1e-6) * hit.distance {
            normal = delta / hit.distance;
        }
        Some(Pair { point: p, target: hit.point, normal, distance: hit.distance, weight: 1.0 })
    })
    .into_iter()
    .flatten()
    .collect()
}

/// Each of the reference's points paired with the nearest point of the moving scan, moved. `own`
/// holds the reference points' normals, for a strict rule.
fn pairs_from(targets: &[Vec3], moved: &Rigid, moving: &Surface, rule: Rule, own: &[Vec3]) -> Vec<Pair> {
    let back = moved.inverse();
    par::map(targets.len(), |i| {
        let target = targets[i];
        let hit = moving.nearest(back.apply(target))?;
        let point = moved.apply(hit.point);
        let mut normal = moved.turned(hit.normal?);
        let delta = point - target;
        if rule.strict && !agrees(*own.get(i)?, normal, delta, hit.distance, moving.diagonal) {
            return None;
        }
        if hit.distance > 1e-10 * moving.diagonal && delta.dot(normal).abs() < (1.0 - 1e-6) * hit.distance {
            normal = delta / delta.len();
        }
        Some(Pair { point, target, normal, distance: hit.distance, weight: 1.0 })
    })
    .into_iter()
    .flatten()
    .collect()
}

/// Whether a point with normal `mine` and the surface point it is paired with, `theirs`, `gap`
/// apart (`distance` long), can be the same surface: facing alike, either way round, and the gap
/// running across the surface, not along it to a rim.
fn agrees(mine: Vec3, theirs: Vec3, gap: Vec3, distance: f64, diagonal: f64) -> bool {
    mine.dot(theirs).abs() >= FACING && (distance <= 1e-6 * diagonal || gap.dot(theirs).abs() >= ACROSS * distance)
}

/// The normal of the surface at each point of `points` (on it), zero where it has none.
fn normals_at(points: &[Vec3], surface: &Surface) -> Vec<Vec3> {
    par::map(points.len(), |i| surface.nearest(points[i]).and_then(|h| h.normal).unwrap_or(Vec3::ZERO))
}

/// The finite pairs, nearest first: the worst `ignored` share left out (of `population` points,
/// when a strict rule has already left the pairs that cannot agree out), those past the median and
/// three robust widths gated out, and each weighted by Cauchy's function of its distance.
fn robust(pairs: Vec<Pair>, ignored: f64, reach: f64, gate: f64, population: Option<usize>) -> Vec<Pair> {
    let mut ordered: Vec<Pair> = pairs.into_iter().filter(|p| p.distance.is_finite() && p.distance <= gate).collect();
    ordered.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    if ordered.len() < 6 {
        return Vec::new();
    }
    let median = ordered[ordered.len() / 2].distance;
    let mut deviations: Vec<f64> = ordered.iter().map(|p| (p.distance - median).abs()).collect();
    deviations.sort_by(f64::total_cmp);
    let scatter = (MAD_TO_SIGMA * deviations[deviations.len() / 2]).max(1e-6 * reach.max(1.0));
    let limit = median + 3.0 * scatter;
    let base = population.unwrap_or(ordered.len());
    let keep = ((base as f64 * (1.0 - ignored.clamp(0.0, 0.6))) as usize).max(6).min(ordered.len());
    let scale = (median + scatter).max(1e-6 * reach.max(1.0));
    ordered.truncate(keep);
    ordered
        .into_iter()
        .filter(|p| p.distance <= limit)
        .map(|mut p| {
            let x = p.distance / scale;
            p.weight = 1.0 / (1.0 + x * x);
            p
        })
        .collect()
}

/// The eigenvalues and eigenvectors (columns, `v[i][k]` the i-th component of the k-th) of a
/// symmetric 6 × 6 matrix, by Jacobi's rotations.
fn eigen6(mut a: [[f64; 6]; 6]) -> ([f64; 6], [[f64; 6]; 6]) {
    let mut v = [[0.0; 6]; 6];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..60 {
        let off: f64 = (0..6).flat_map(|i| (0..6).filter(move |&j| j != i).map(move |j| (i, j))).map(|(i, j)| a[i][j] * a[i][j]).sum();
        if off < 1e-30 {
            break;
        }
        for p in 0..6 {
            for q in p + 1..6 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (row_p, row_q) = (a[p], a[q]);
                for (k, (apk, aqk)) in row_p.into_iter().zip(row_q).enumerate() {
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vp, vq) = (row[p], row[q]);
                    row[p] = c * vp - s * vq;
                    row[q] = s * vp + c * vq;
                }
            }
        }
    }
    (std::array::from_fn(|i| a[i][i]), v)
}

/// What a run of the fit gave.
struct Refined {
    moved: Rigid,
    rounds: usize,
    converged: bool,
}

/// One point-to-plane step for `kept`: the move that takes the pairs' points nearest their
/// targets along the pairs' normals (by their weights), how far it moves (a slide plus a turn at
/// the pairs' spread), and the pairs' weighted rms before it. Directions the pairs do not pin (a
/// plane slides along itself) are not moved. `None` for pairs that pin nothing.
fn step(kept: &[Pair], reach: f64) -> Option<(Rigid, f64, f64)> {
    let centroid = kept.iter().fold(Vec3::ZERO, |s, p| s + p.point) / kept.len() as f64;
    let scale = (kept.iter().map(|p| (p.point - centroid).len2()).sum::<f64>() / kept.len() as f64).sqrt().max(1e-9 * reach);
    let (mut h, mut g, mut sum) = ([[0.0f64; 6]; 6], [0.0f64; 6], 0.0);
    for p in kept {
        let c = (p.point - centroid).cross(p.normal) / scale;
        let row = [c.x, c.y, c.z, p.normal.x, p.normal.y, p.normal.z];
        let residual = (p.point - p.target).dot(p.normal);
        sum += p.weight * residual * residual;
        for i in 0..6 {
            g[i] -= p.weight * row[i] * residual;
            for j in 0..6 {
                h[i][j] += p.weight * row[i] * row[j];
            }
        }
    }
    let (values, vectors) = eigen6(h);
    let largest = values.iter().copied().fold(0.0, f64::max);
    if !(largest > 0.0) {
        return None;
    }
    let mut x = [0.0; 6];
    for k in 0..6 {
        if !(values[k] > 1e-4 * largest) {
            continue;
        }
        let along = (0..6).map(|i| vectors[i][k] * g[i]).sum::<f64>() / values[k];
        for (i, xi) in x.iter_mut().enumerate() {
            *xi += vectors[i][k] * along;
        }
    }
    let turn = Vec3::new(x[0], x[1], x[2]) / scale;
    let slide = Vec3::new(x[3], x[4], x[5]);
    let angle = turn.len();
    let rotation = match turn.normalized() {
        Some(axis) if angle > 1e-15 => Rigid::turn(axis, angle),
        _ => Rigid::shift(Vec3::ZERO),
    };
    let step = Rigid { translation: centroid - rotation.turned(centroid) + slide, ..rotation };
    Some((step, slide.len() + angle * scale, (sum / kept.len() as f64).sqrt()))
}

/// The trimmed fit to its end, by the measure the fit is judged by ([`trimmed_rms`]): from `start`,
/// round after round the nearest `1 - ignored` share of `points` (those `rule` pairs) stepped
/// onto `reference`, all counting alike, and the place of least trimmed rms met on the way. The
/// two-way fit weighs its pairs anew each round and pairs the reference back onto the scan, so it
/// can rest a little off where the scan lies nearest (a scan 0.6 mm proud of its model, open
/// underneath, rested 2 mm up it: 1.078 mm rms where 1.043 was to be had). Never further off than
/// `start` by that measure.
fn polish(points: &[Vec3], start: Rigid, reference: &Surface, ignored: f64, rule: Rule, own: &[Vec3]) -> Rigid {
    const ROUNDS: usize = 80;
    const PATIENCE: usize = 6;
    let reach = reference.diagonal;
    let mut best = (trimmed_rms(points, &start, reference, ignored), start);
    let (mut moved, mut since) = (start, 0);
    for _ in 0..ROUNDS {
        let mut pairs: Vec<Pair> =
            pairs_to(points, &moved, reference, rule, own).into_iter().filter(|p| p.distance.is_finite() && p.distance <= rule.gate).collect();
        pairs.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let keep = ((points.len() as f64 * (1.0 - ignored)).round() as usize).min(pairs.len());
        pairs.truncate(keep);
        if pairs.len() < 6 {
            break;
        }
        let Some((by, far, _)) = step(&pairs, reach) else { break };
        moved = moved.then(&by);
        let rms = trimmed_rms(points, &moved, reference, ignored);
        if rms < best.0 - 1e-9 * reach {
            best = (rms, moved);
            since = 0;
        } else {
            since += 1;
        }
        if far < 1e-7 * reach || since >= PATIENCE {
            break;
        }
    }
    best.1
}

/// Point-to-plane ICP from `start`, both ways, at most `rounds` rounds, pairs `rule` leaves out
/// left out. Directions the pairs do not pin (a plane slides along itself) are not moved.
#[allow(clippy::too_many_arguments)]
fn refine(
    points: &[Vec3],
    start: Rigid,
    reference: &Surface,
    ignored: f64,
    rounds: usize,
    moving: &Surface,
    reverse: &[Vec3],
    rule: Rule,
) -> Refined {
    let gate = rule.gate;
    let (own, reverse_own) = if rule.strict { (normals_at(points, moving), normals_at(reverse, reference)) } else { (Vec::new(), Vec::new()) };
    let mut moved = start;
    let (mut converged, mut used, mut previous) = (false, 0, f64::INFINITY);
    let reach = reference.diagonal;
    for round in 0..rounds {
        used = round + 1;
        let mut forward = robust(pairs_to(points, &moved, reference, rule, &own), ignored, reach, gate, rule.strict.then_some(points.len()));
        let mut backward =
            robust(pairs_from(reverse, &moved, moving, rule, &reverse_own), ignored, reach, gate, rule.strict.then_some(reverse.len()));
        if forward.len() < 6 || backward.len() < 6 {
            break;
        }
        // The two ways count alike.
        let count = forward.len() as f64;
        let (fw, bw) = (forward.iter().map(|p| p.weight).sum::<f64>(), backward.iter().map(|p| p.weight).sum::<f64>());
        for p in &mut forward {
            p.weight *= count / fw.max(1e-12);
        }
        for p in &mut backward {
            p.weight *= count / bw.max(1e-12);
        }
        forward.extend(backward);
        let kept = forward;
        let Some((by, far, rms)) = step(&kept, reach) else { break };
        moved = moved.then(&by);
        if far < 1e-7 * reach || (previous - rms).abs() < 1e-9 * reach {
            converged = true;
            break;
        }
        previous = rms;
    }
    Refined { moved, rounds: used, converged }
}

/// The centroid, the axes longest first as a right-handed frame, and how far the points spread
/// along each.
pub(crate) fn principal(points: &[Vec3]) -> Option<(Vec3, [Vec3; 3], [f64; 3])> {
    if points.is_empty() {
        return None;
    }
    let centroid = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in points {
        let d = *p - centroid;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    if !(values[2] > 0.0) {
        return None;
    }
    let x = axes[2].normalized()?;
    let y = axes[1].normalized()?;
    Some((centroid, [x, y, x.cross(y)], [values[2], values[1], values[0]]))
}

/// The rigid move with rotation columns `f` times `eᵀ` and centres `from` onto `to`.
fn frame_move(e: [Vec3; 3], f: [Vec3; 3], from: Vec3, to: Vec3) -> Rigid {
    let get = |v: Vec3, i: usize| [v.x, v.y, v.z][i];
    let rotation: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| get(f[k], i) * get(e[k], j)).sum()));
    let turn = Rigid { rotation, translation: Vec3::ZERO };
    Rigid { translation: to - turn.turned(from), ..turn }
}

/// The ways of laying the moving points' principal axes on the reference's, centres together:
/// the four turns that keep the axes' order, and with axes of near-equal spread, those that swap.
fn principal_starts(moving: &[Vec3], reference: &[Vec3]) -> Vec<Rigid> {
    let (Some(a), Some(b)) = (principal(moving), principal(reference)) else { return Vec::new() };
    let mut permutations: Vec<[usize; 3]> = vec![[0, 1, 2]];
    if a.2[1] > 0.7 * a.2[0] || b.2[1] > 0.7 * b.2[0] {
        permutations.push([1, 0, 2]);
    }
    if a.2[2] > 0.7 * a.2[1] || b.2[2] > 0.7 * b.2[1] {
        permutations.push([0, 2, 1]);
    }
    if permutations.len() == 3 {
        permutations.extend([[1, 2, 0], [2, 0, 1], [2, 1, 0]]);
    }
    let mut starts = Vec::new();
    for p in &permutations {
        for signs in [[1.0, 1.0, 1.0], [1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, 1.0]] {
            let (f0, f1) = (b.1[p[0]] * signs[0], b.1[p[1]] * signs[1]);
            // Right-handed whatever the swap.
            let f2 = f0.cross(f1);
            starts.push(frame_move(a.1, [f0, f1, f2], a.0, b.0));
        }
    }
    starts
}

/// The least-squares rigid move laying `sources` onto `targets` (in step): Kabsch's, from the
/// eigenvectors of AᵀA. `None` for fewer than three pairs or points along a line.
fn fit_pairs(sources: &[Vec3], targets: &[Vec3]) -> Option<Rigid> {
    let n = sources.len().min(targets.len());
    if n < 3 {
        return None;
    }
    let cs = sources[..n].iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f64;
    let ct = targets[..n].iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f64;
    let mut a = [[0.0f64; 3]; 3];
    for i in 0..n {
        let (s, t) = (sources[i] - cs, targets[i] - ct);
        let (s, t) = ([s.x, s.y, s.z], [t.x, t.y, t.z]);
        for (r, ti) in a.iter_mut().zip(t) {
            for (cell, sj) in r.iter_mut().zip(s) {
                *cell += ti * sj;
            }
        }
    }
    let ata: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[k][i] * a[k][j]).sum()));
    let (values, axes) = solidcraft_geom::inertia::principal(&ata, 0.0)?;
    let sigma = [values[2].max(0.0).sqrt(), values[1].max(0.0).sqrt()];
    if !(sigma[0] > 1e-12 && sigma[1] > 1e-7 * sigma[0]) {
        return None;
    }
    let (v1, v2) = (axes[2].normalized()?, axes[1].normalized()?);
    let times = |v: Vec3| {
        Vec3::new(
            a[0][0] * v.x + a[0][1] * v.y + a[0][2] * v.z,
            a[1][0] * v.x + a[1][1] * v.y + a[1][2] * v.z,
            a[2][0] * v.x + a[2][1] * v.y + a[2][2] * v.z,
        )
    };
    let (u1, u2) = (times(v1) / sigma[0], times(v2) / sigma[1]);
    Some(frame_move([v1, v2, v1.cross(v2)], [u1, u2, u1.cross(u2)], cs, ct))
}

/// A grid of points by cells, for the points near a point.
struct Grid<'a> {
    points: &'a [Vec3],
    cell: f64,
    cells: std::collections::HashMap<[i64; 3], Vec<u32>>,
}

impl<'a> Grid<'a> {
    fn new(points: &'a [Vec3], cell: f64) -> Self {
        let mut cells: std::collections::HashMap<[i64; 3], Vec<u32>> = std::collections::HashMap::new();
        for (i, p) in points.iter().enumerate() {
            cells.entry(Self::key(*p, cell)).or_default().push(i as u32);
        }
        Grid { points, cell, cells }
    }

    fn key(p: Vec3, cell: f64) -> [i64; 3] {
        [(p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64]
    }

    /// Each point within `reach` of `p`, with its distance.
    fn within(&self, p: Vec3, reach: f64, mut found: impl FnMut(u32, f64)) {
        let [x, y, z] = Self::key(p, self.cell);
        let r = (reach / self.cell).ceil() as i64;
        for i in x - r..=x + r {
            for j in y - r..=y + r {
                for k in z - r..=z + r {
                    for &q in self.cells.get(&[i, j, k]).into_iter().flatten() {
                        let d = self.points[q as usize].dist(p);
                        if d <= reach {
                            found(q, d);
                        }
                    }
                }
            }
        }
    }
}

/// Bins of each of a histogram's three angles.
const BINS: usize = 11;
const LENGTH: usize = 3 * BINS;

/// How two oriented points lie to each other (the pair feature): the turn of the second normal
/// about the first's frame and two cosines. `None` for points together or a frame with no turn.
fn pair_feature(p1: Vec3, n1: Vec3, p2: Vec3, n2: Vec3) -> Option<[f64; 3]> {
    let mut d = p2 - p1;
    let length = d.len();
    if !(length > 0.0) {
        return None;
    }
    let (a1, a2) = (n1.dot(d) / length, n2.dot(d) / length);
    let (source, target, third) = if a1.abs().acos() > a2.abs().acos() {
        d = -d;
        (n2, n1, -a2)
    } else {
        (n1, n2, a1)
    };
    let v = d.cross(source).normalized()?;
    let w = source.cross(v);
    Some([w.dot(target).atan2(source.dot(target)), v.dot(target), third])
}

fn bin(value: f64) -> usize {
    ((BINS as f64 * (value + 1.0) * 0.5).floor().max(0.0) as usize).min(BINS - 1)
}

/// Each point's normal (the way its neighbours within `reach` spread least, turned to agree with
/// the surface's) and its Fast Point Feature Histogram over `5 × spacing`.
fn describe(points: &[Vec3], surface: &Surface, spacing: f64) -> Vec<[f32; LENGTH]> {
    let grid = Grid::new(points, spacing);
    let normals: Vec<Vec3> = points
        .iter()
        .map(|&p| {
            let own = surface.nearest(p).and_then(|h| h.normal).unwrap_or(Vec3::Z);
            let mut near = Vec::new();
            grid.within(p, 2.0 * spacing, |q, _| near.push(points[q as usize]));
            principal(&near).map(|(_, axes, _)| axes[2]).filter(|_| near.len() >= 5).map_or(own, |n| if n.dot(own) < 0.0 { -n } else { n })
        })
        .collect();
    let neighbours: Vec<Vec<(u32, f64)>> = points
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let mut near = Vec::new();
            grid.within(p, 5.0 * spacing, |q, d| {
                if q as usize != i && d > 0.0 {
                    near.push((q, d));
                }
            });
            near
        })
        .collect();
    let simple: Vec<[f64; LENGTH]> = (0..points.len())
        .map(|i| {
            let mut h = [0.0; LENGTH];
            let mut counted = 0usize;
            for &(q, _) in &neighbours[i] {
                let Some([f1, f2, f3]) = pair_feature(points[i], normals[i], points[q as usize], normals[q as usize]) else { continue };
                h[bin(f1 / std::f64::consts::PI)] += 1.0;
                h[BINS + bin(f2)] += 1.0;
                h[2 * BINS + bin(f3)] += 1.0;
                counted += 1;
            }
            if counted > 0 {
                for v in &mut h {
                    *v *= 100.0 / counted as f64;
                }
            }
            h
        })
        .collect();
    (0..points.len())
        .map(|i| {
            let mut sum = [0.0; LENGTH];
            for &(q, distance) in &neighbours[i] {
                for (s, v) in sum.iter_mut().zip(&simple[q as usize]) {
                    *s += v / distance;
                }
            }
            let mut out = [0.0f32; LENGTH];
            for third in 0..3 {
                let range = third * BINS..(third + 1) * BINS;
                let total: f64 = sum[range.clone()].iter().sum();
                for k in range {
                    out[k] = ((if total > 0.0 { 100.0 * sum[k] / total } else { 0.0 }) + simple[i][k]) as f32;
                }
            }
            out
        })
        .collect()
}

/// For each of `from`, the one of `to` most like it.
fn most_alike(from: &[[f32; LENGTH]], to: &[[f32; LENGTH]]) -> Vec<usize> {
    par::map(from.len(), |i| {
        to.iter()
            .enumerate()
            .map(|(j, t)| (from[i].iter().zip(t).map(|(x, y)| (x - y) * (x - y)).sum::<f32>(), j))
            .fold((f32::INFINITY, 0), |best, c| if c.0 < best.0 { c } else { best })
            .1
    })
}

/// A small fast generator, seeded, so the same scans give the same starts.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Moves that lay the moving points onto the reference's where their shapes agree, best first:
/// each moving point matched to the reference point whose histogram is most like it, and the
/// moves three matches agree on found by random sample consensus. At most eight, each different
/// from the others.
fn feature_starts(moving: &[Vec3], moving_surface: &Surface, reference: &[Vec3], reference_surface: &Surface, spacing: f64) -> Vec<Rigid> {
    if moving.len() < 12 || reference.len() < 12 || !(spacing > 0.0) {
        return Vec::new();
    }
    let (a, b) = (describe(moving, moving_surface, spacing), describe(reference, reference_surface, spacing));
    let forward = most_alike(&a, &b);
    let backward = most_alike(&b, &a);
    let mutual: Vec<(usize, usize)> = forward.iter().enumerate().filter(|&(i, &j)| backward[j] == i).map(|(i, &j)| (i, j)).collect();
    let matches = if mutual.len() >= 30 { mutual } else { forward.iter().copied().enumerate().collect() };
    if matches.len() < 3 {
        return Vec::new();
    }
    let (from, to): (Vec<Vec3>, Vec<Vec3>) = matches.iter().map(|&(i, j)| (moving[i], reference[j])).unzip();
    let agree = 1.5 * spacing;
    let shortest = 2.0 * spacing;
    let agreeing = |t: &Rigid| from.iter().zip(&to).filter(|(p, q)| t.apply(**p).dist(**q) <= agree).count();
    let mut random = SplitMix64(0xC0A2_5E00);
    let mut found: Vec<(usize, usize, Rigid)> = Vec::new();
    for trial in 0..20_000usize {
        let mut pick = || (random.next() % from.len() as u64) as usize;
        let k = [pick(), pick(), pick()];
        if k[0] == k[1] || k[1] == k[2] || k[0] == k[2] {
            continue;
        }
        let alike = [(0, 1), (1, 2), (0, 2)].iter().all(|&(x, y)| {
            let (m, r) = (from[k[x]].dist(from[k[y]]), to[k[x]].dist(to[k[y]]));
            m >= shortest && r >= shortest && m.min(r) >= 0.9 * m.max(r)
        });
        if !alike {
            continue;
        }
        let Some(solved) = fit_pairs(&k.map(|i| from[i]), &k.map(|i| to[i])) else { continue };
        let count = agreeing(&solved);
        if count >= 3 {
            found.push((count, trial, solved));
        }
    }
    found.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
    let middle = moving.iter().fold(Vec3::ZERO, |s, p| s + *p) / moving.len() as f64;
    let mut kept: Vec<Rigid> = Vec::new();
    for (_, _, t) in found {
        if kept.len() >= 8 {
            break;
        }
        // Fitted again to every match that agrees with it.
        let (p, q): (Vec<Vec3>, Vec<Vec3>) = from.iter().zip(&to).filter(|(p, q)| t.apply(**p).dist(**q) <= agree).map(|(p, q)| (*p, *q)).unzip();
        let t = fit_pairs(&p, &q).unwrap_or(t);
        if kept.iter().all(|k| k.turn_from(&t) > 0.17 || k.apply(middle).dist(t.apply(middle)) > 3.0 * spacing) {
            kept.push(t);
        }
    }
    kept
}

/// How far a scan's vertices stray from the plane of the patch round them: the median over patches
/// spread across it, so edges and corners do not count.
fn roughness(scan: &Scan) -> Option<f64> {
    let n = scan.positions.len();
    let mut starts = vec![0u32; n + 1];
    for t in scan.triangles {
        for &v in t {
            if let Some(s) = starts.get_mut(v as usize + 1) {
                *s += 2;
            }
        }
    }
    for k in 1..starts.len() {
        starts[k] += starts[k - 1];
    }
    let mut filled = starts.clone();
    let mut neighbours = vec![0u32; starts[n] as usize];
    for t in scan.triangles {
        for k in 0..3 {
            let v = t[k] as usize;
            for other in [t[(k + 1) % 3], t[(k + 2) % 3]] {
                if let Some(slot) = filled.get_mut(v)
                    && let Some(cell) = neighbours.get_mut(*slot as usize)
                {
                    *cell = other;
                    *slot += 1;
                }
            }
        }
    }
    let ring = |v: u32| -> &[u32] {
        let (a, b) = (starts.get(v as usize).copied().unwrap_or(0) as usize, starts.get(v as usize + 1).copied().unwrap_or(0) as usize);
        neighbours.get(a..b).unwrap_or_default()
    };
    let seeds: Vec<usize> = (0..n).step_by((n / 3000).max(1)).collect();
    let mut values: Vec<f64> = seeds
        .iter()
        .filter_map(|&seed| {
            let mut patch = vec![seed as u32];
            for _ in 0..3 {
                let mut grown = patch.clone();
                for &v in &patch {
                    grown.extend_from_slice(ring(v));
                }
                grown.sort_unstable();
                grown.dedup();
                patch = grown;
            }
            if patch.len() < 8 {
                return None;
            }
            let points: Vec<Vec3> = patch.iter().filter_map(|&v| scan.positions.get(v as usize).copied()).collect();
            let (centre, _, _) = principal(&points)?;
            let mut spread = [[0.0f64; 3]; 3];
            for p in &points {
                let d = *p - centre;
                let d = [d.x, d.y, d.z];
                for (row, di) in spread.iter_mut().zip(d) {
                    for (cell, dj) in row.iter_mut().zip(d) {
                        *cell += di * dj;
                    }
                }
            }
            let (least, _) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
            Some((least[0].max(0.0) / points.len() as f64).sqrt())
        })
        .collect();
    if values.len() * 2 < seeds.len() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// How many of `points`, moved, lie within `reach` of `reference` facing as it does there (the
/// points' `normals` turned with them), either way round.
fn consensus(points: &[Vec3], normals: &[Vec3], moved: &Rigid, reference: &Surface, reach: f64) -> f64 {
    points
        .iter()
        .zip(normals)
        .filter(|(p, n)| {
            reference
                .nearest(moved.apply(**p))
                .is_some_and(|h| h.distance <= reach && h.normal.is_some_and(|m| moved.turned(**n).dot(m).abs() >= FACING))
        })
        .count() as f64
}

/// The area of a scan's surface.
fn area(scan: &Scan) -> f64 {
    (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).map(|[a, b, c]| (b - a).cross(c - a).len() / 2.0).filter(|a| a.is_finite()).sum()
}

/// Lays `moving` onto `reference`: the move, and how well it fits.
pub fn register(moving: &Scan, reference: &Scan, settings: &Settings) -> Result<Registration, RegisterError> {
    let ignored = settings.ignored.clamp(0.0, 0.6);
    let samples = sample(moving, MANY);
    let reference_samples = sample(reference, MANY);
    if samples.len() < 12 || reference_samples.len() < 12 {
        return Err(RegisterError::TooLittle);
    }
    let (mine, theirs) = (Surface::new(*moving), Surface::new(*reference));
    let few = every(&samples, FEW);
    let reverse_few = every(&reference_samples, FEW);
    let vertices = every(&moving.points().collect::<Vec<_>>(), VERTEX_LIMIT);
    let identity = Rigid::shift(Vec3::ZERO);
    let rms_before = trimmed_rms(&vertices, &identity, &theirs, ignored);
    let rough = [roughness(moving), roughness(reference)].into_iter().flatten().fold(0.0, f64::max);

    // Each start with the gate of a seam start (one found from the flat faces, fitted strictly).
    let mut starts: Vec<(&'static str, Rigid, Option<f64>)> = vec![("where it was", identity, None)];
    let mut spacing = None;
    if settings.search {
        starts.extend(principal_starts(&samples, &reference_samples).into_iter().map(|t| ("its principal axes", t, None)));
        let apart = ((area(moving).max(area(reference))) / FEATURE_POINTS as f64).sqrt();
        let count = |a: f64| ((a / (apart * apart)).round() as usize).clamp(12, FEATURE_POINTS);
        let (m, r) = (sample(moving, count(area(moving))), sample(reference, count(area(reference))));
        starts.extend(feature_starts(&m, &mine, &r, &theirs, apart).into_iter().map(|t| ("shapes that match", t, None)));
        spacing = Some(apart);
    }
    let mut best = (starts[0].0, starts[0].1);
    let mut gated: Option<(Rigid, f64)> = None;
    let mut rival = None;
    // The placements that lay nearly as much of the scan on the reference as the best (a part
    // nearly alike turned half round): each is fitted to its end and the nearest taken.
    let mut rivals: Vec<(&'static str, Rigid, Option<f64>)> = Vec::new();
    if starts.len() > 1 {
        let reach = (ROUGHNESS_REACH * rough).max(2e-3 * theirs.diagonal);
        let normals: Vec<Vec3> = few.iter().map(|p| mine.nearest(*p).and_then(|h| h.normal).unwrap_or(Vec3::ZERO)).collect();
        let gate = spacing.map_or(f64::INFINITY, |s| 2.0 * s);
        let mut scored: Vec<(f64, Rigid, Rule, usize)> = Vec::new();
        let score = |starts: &[(&'static str, Rigid, Option<f64>)], from: usize, scored: &mut Vec<(f64, Rigid, Rule, usize)>| {
            for (index, (_, start, seam)) in starts.iter().enumerate().skip(from) {
                crate::progress::stage("trying starting places", index, starts.len());
                if let Some(seam_gate) = *seam {
                    let rule = Rule::strict(seam_gate);
                    let held = refine(&few, *start, &theirs, ignored, 12, &mine, &reverse_few, rule).moved;
                    scored.push((consensus(&few, &normals, &held, &theirs, reach), held, rule, index));
                    continue;
                }
                let free = refine(&few, *start, &theirs, ignored, 8, &mine, &reverse_few, Rule::loose(f64::INFINITY)).moved;
                scored.push((consensus(&few, &normals, &free, &theirs, reach), free, Rule::loose(f64::INFINITY), index));
                if gate.is_finite() {
                    let held = refine(&few, *start, &theirs, ignored, 8, &mine, &reverse_few, Rule::loose(gate)).moved;
                    scored.push((consensus(&few, &normals, &held, &theirs, reach), held, Rule::loose(gate), index));
                }
            }
        };
        score(&starts, 0, &mut scored);
        // The search by flat faces (frames.rs) is for passes that overlap in part (a van's wall and
        // its roof), and the slow part of the search (6.6 of 8 s on the holes plate against its
        // model). Only when no other start lays most of what may be fitted onto the reference.
        let enough = ENOUGH_LAID * (1.0 - ignored) * few.len() as f64;
        if settings.search && scored.iter().all(|s| s.0 < enough) {
            crate::progress::stage("matching flat faces", 0, 1);
            let (seams, cell) = frames::starts(&mine, &theirs);
            let from = starts.len();
            starts.extend(seams.into_iter().map(|t| ("its flat faces", t, Some(2.0 * cell))));
            score(&starts, from, &mut scored);
        }
        let best_score = scored.iter().map(|s| s.0).fold(0.0, f64::max);
        let middle = few.iter().fold(Vec3::ZERO, |s, p| s + *p) / few.len().max(1) as f64;
        let slide = spacing.map_or(reach, |s| DISTINCT_SLIDE * s);
        let distinct = |a: &Rigid, b: &Rigid| a.turn_from(b) > DISTINCT_TURN || a.apply(middle).dist(b.apply(middle)) > slide;
        let mut order: Vec<usize> = (0..scored.len()).collect();
        order.sort_by(|&x, &y| scored[y].0.total_cmp(&scored[x].0));
        let mut finalists: Vec<usize> = Vec::new();
        for k in order {
            if finalists.len() >= MOST_FINALISTS || scored[k].0 < FINALIST_SHARE * best_score {
                break;
            }
            if finalists.iter().all(|&f| distinct(&scored[f].1, &scored[k].1)) {
                finalists.push(k);
            }
        }
        // Fitted to the end and judged within the surfaces' own roughness, which a placement a
        // little off does not reach.
        let tight = (ROUGHNESS_REACH * rough).max(2e-4 * theirs.diagonal);
        let judged: Vec<(f64, Rigid, Rule, usize)> = finalists
            .iter()
            .map(|&k| {
                let (_, from, rule, start) = scored[k];
                let fitted = refine(&few, from, &theirs, ignored, 40, &mine, &reverse_few, rule).moved;
                (consensus(&few, &normals, &fitted, &theirs, tight), fitted, rule, start)
            })
            .collect();
        if let Some(&(score, fitted, rule, start)) = judged.iter().max_by(|a, b| a.0.total_cmp(&b.0)) {
            best = (starts[start].0, fitted);
            if rule.gate.is_finite() {
                gated = Some((fitted, rule.gate));
            }
            rivals = judged
                .iter()
                .filter(|j| distinct(&fitted, &j.1) && score > 0.0 && j.0 >= RIVAL_SHARE * score)
                .map(|j| (starts[j.3].0, j.1, j.2.gate.is_finite().then_some(j.2.gate)))
                .collect();
            let other = judged.iter().filter(|j| distinct(&fitted, &j.1)).max_by(|a, b| a.0.total_cmp(&b.0));
            if let Some(&(theirs_score, at, _, _)) = other.filter(|j| score > 0.0 && j.0 >= RIVAL_SHARE * score) {
                let away = fitted.apply(middle).dist(at.apply(middle));
                let how = if away > slide {
                    format!("{away:.0} mm away")
                } else {
                    format!("turned {:.0}° from this one", fitted.turn_from(&at).to_degrees())
                };
                rival = Some(format!("another placement {how} lays {:.0}% as much of the scan on the reference", 100.0 * theirs_score / score));
            }
        }
    }
    let reference_vertices = every(&reference.points().collect::<Vec<_>>(), VERTEX_LIMIT);
    let measure = |moved: &Rigid, rms: f64| -> (f64, f64) {
        let least = 1e-3 * theirs.diagonal;
        let near = if 3.0 * rms > least {
            let r = if rough > 0.0 { (3.0 * rms).min(ROUGHNESS_REACH * rough) } else { 3.0 * rms };
            r.max(least)
        } else {
            least
        };
        // The share of the scan within that reach of the reference, and the rms of that share's
        // distances: how well what overlaps fits, whatever else of the scan there is.
        let on: Vec<f64> = vertices.iter().filter_map(|p| theirs.nearest(moved.apply(*p)).map(|h| h.distance)).filter(|d| *d <= near).collect();
        let rms = if on.is_empty() { 0.0 } else { (on.iter().map(|d| d * d).sum::<f64>() / on.len() as f64).sqrt() };
        (on.len() as f64 / vertices.len().max(1) as f64, rms)
    };
    let polish_points = every(&vertices, POLISH_POINTS);
    // From a placement to the end: where it comes to rest, its trimmed rms, whether it settled,
    // the rounds it took, and whether the scan lies whole on the reference there (all of it that
    // is not ignored within the surfaces' own roughness of it, as `matched` counts). Only then is its trimmed rms a measure of the placement: a pass that
    // overlaps the other in part lies "nearer" by it wherever more of it happens to be near.
    let settle = |from: Rigid, gate: Option<f64>| -> Result<(Rigid, f64, bool, usize, bool), RegisterError> {
        let (rough_gate, fine_gate) = gate.map_or((f64::INFINITY, f64::INFINITY), |g| (g / 2.0, g / 4.0));
        // The fit from `from` to the end: strict, so that a thin overlap is not drawn off by the rim of
        // one pass, and loose where the surfaces leave too few pairs that agree.
        let finish = |strict: bool| -> Result<(Refined, Refined, Refined), RegisterError> {
            let rule = |gate: f64| if strict { Rule::strict(gate) } else { Rule::loose(gate) };
            let (own, reverse_own) =
                if strict { (normals_at(&vertices, &mine), normals_at(&reference_vertices, &theirs)) } else { (Vec::new(), Vec::new()) };
            let first = refine(&few, from, &theirs, ignored, 40, &mine, &reverse_few, rule(rough_gate));
            let fine = refine(&samples, first.moved, &theirs, ignored, 20, &mine, &reference_samples, rule(fine_gate));
            // The last pass on the scan's own vertices that lie on the reference.
            let inliers: Vec<Vec3> = robust(
                pairs_to(&vertices, &fine.moved, &theirs, rule(fine_gate), &own),
                ignored,
                theirs.diagonal,
                fine_gate,
                strict.then_some(vertices.len()),
            )
            .iter()
            .map(|p| fine.moved.inverse().apply(p.point))
            .collect();
            let reverse: Vec<Vec3> = robust(
                pairs_from(&reference_vertices, &fine.moved, &mine, rule(fine_gate), &reverse_own),
                ignored,
                theirs.diagonal,
                fine_gate,
                strict.then_some(reference_vertices.len()),
            )
            .iter()
            .map(|p| p.target)
            .collect();
            if inliers.len() < 12 || reverse.len() < 12 {
                return Err(RegisterError::NoOverlap);
            }
            let last =
                refine(&every(&inliers, VERTEX_LIMIT / 4), fine.moved, &theirs, 0.0, 40, &mine, &every(&reverse, VERTEX_LIMIT / 4), rule(fine_gate));
            Ok((first, fine, last))
        };
        let (first, fine, last) = match finish(true) {
            Ok(done) => done,
            Err(_) => finish(false)?,
        };
        let fitted_rms = trimmed_rms(&vertices, &last.moved, &theirs, ignored);
        let rounds = first.rounds + fine.rounds + last.rounds;
        let whole = measure(&last.moved, fitted_rms).0 >= 1.0 - ignored;
        if !whole {
            return Ok((last.moved, fitted_rms, last.converged, rounds, false));
        }
        // Then to the end by the measure it is judged by, kept when the whole scan lies nearer for it.
        let polished = polish(&polish_points, last.moved, &theirs, ignored, Rule::loose(f64::INFINITY), &[]);
        let polished_rms = trimmed_rms(&vertices, &polished, &theirs, ignored);
        Ok(if polished_rms < fitted_rms {
            (polished, polished_rms, true, rounds, true)
        } else {
            (last.moved, fitted_rms, last.converged, rounds, true)
        })
    };
    let (mut placed, mut rms_after, mut settled, mut rounds, whole) = settle(best.1, gated.map(|g| g.1))?;
    // A rival is taken when the scan lies whole on the reference both ways and clearly nearer its way.
    for (name, from, gate) in rivals.into_iter().filter(|_| whole) {
        if let Some(other) = settle(from, gate).ok().filter(|other| other.4 && other.1 + 1e-6 * theirs.diagonal < rms_after) {
            (placed, rms_after, settled, rounds, _) = other;
            best.0 = name;
        }
    }
    // A fit that leaves the scan further from the reference than it lay is not taken: the scan
    // stays where it was (a good placement by hand, a thin overlap the fit could not hold).
    if rms_after > rms_before + 1e-6 * theirs.diagonal {
        return Ok(Registration {
            moved: identity,
            rms_before,
            rms_after: rms_before,
            matched: measure(&identity, rms_before).0,
            rms_matched: measure(&identity, rms_before).1,
            rounds,
            converged: false,
            start: "where it was",
            rival,
            kept: Some(rms_after),
            ignored,
        });
    }
    Ok(Registration {
        moved: placed,
        rms_before,
        rms_after,
        matched: measure(&placed, rms_after).0,
        rms_matched: measure(&placed, rms_after).1,
        rounds,
        converged: settled,
        start: best.0,
        rival,
        kept: None,
        ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, turned};

    /// A block with a boss at one end, so no turn of it is itself: a scan of it.
    fn part() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 40.0, 20.0), 16);
        let (boss, more) = fine_cuboid(Vec3::new(5.0, 5.0, 20.0), Vec3::new(20.0, 15.0, 32.0), 8);
        let base = positions.len() as u32;
        positions.extend(boss);
        triangles.extend(more.into_iter().map(|t| t.map(|v| v + base)));
        (positions, triangles)
    }

    #[test]
    fn a_pass_turned_a_little_is_laid_back_from_where_it_lies() {
        let (positions, triangles) = part();
        let axis = Vec3::new(0.3, 0.8, 0.5).normalized().unwrap();
        let moved = turned(&positions, axis, 0.12, Vec3::new(2.0, -1.5, 1.0));
        let (reference, pass) = (Scan::new(&positions, &triangles), Scan::new(&moved, &triangles));
        let found = register(&pass, &reference, &Settings { search: false, ignored: 0.2 }).unwrap();
        let back = moved.iter().map(|p| found.moved.apply(*p)).zip(&positions).map(|(a, b)| a.dist(*b)).fold(0.0, f64::max);
        assert!(back < 1e-3, "every point back within a micron: {back} ({found:?})");
        assert!(found.rms_after < 1e-4 && found.rms_before > 1.0, "{found:?}");
        assert!(found.matched > 0.99, "{found:?}");
    }

    /// Turned half round and moved far off, the search finds it: the principal axes or the matched
    /// features start it near.
    #[test]
    fn a_pass_lying_any_way_is_found_by_the_search() {
        let (positions, triangles) = part();
        let axis = Vec3::new(-0.4, 0.2, 0.9).normalized().unwrap();
        let moved = turned(&positions, axis, 2.6, Vec3::new(140.0, -60.0, 35.0));
        let (reference, pass) = (Scan::new(&positions, &triangles), Scan::new(&moved, &triangles));
        let found = register(&pass, &reference, &Settings::default()).unwrap();
        let back = moved.iter().map(|p| found.moved.apply(*p)).zip(&positions).map(|(a, b)| a.dist(*b)).fold(0.0, f64::max);
        assert!(back < 1e-2, "{back} from {} ({found:?})", found.start);
        assert!(found.matched > 0.99, "{found:?}");
    }

    /// A scan of the whole part that is not the part exactly, as a fused scan is against its CAD
    /// model: a little proud of it, lumpy, and open underneath. Wherever it starts (where it lies, a
    /// little off, or turned and far off) it comes to rest at one place, and nowhere further from
    /// the model than where it lay. (T-LESS 09's scan rested 2 mm up its model, 1.078 mm rms, where
    /// 1.008 was to be had; a second fit from there said it could do no better.)
    #[test]
    fn a_scan_proud_of_its_model_comes_to_rest_at_one_place_from_any_start() {
        let (positions, triangles) = part();
        let middle = Vec3::new(30.0, 20.0, 12.0);
        let lumpy: Vec<Vec3> = positions
            .iter()
            .map(|p| {
                let out = *p - middle;
                // Proud by about 0.3 mm, and rough from point to point as a scan is.
                let lump = 0.5 * ((0.37 * p.x).sin() * (0.53 * p.y).cos() + (0.41 * p.z).sin());
                let grain = (12.9898 * p.x + 78.233 * p.y + 37.719 * p.z).sin();
                *p + out * (0.012 + 0.006 * lump + 0.008 * grain)
            })
            .collect();
        let open: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| t.iter().any(|&v| positions[v as usize].z > 1e-9)).collect();
        let reference = Scan::new(&positions, &triangles);
        let rest = |start: &[Vec3], search: bool| {
            let found = register(&Scan::new(start, &open), &reference, &Settings { search, ignored: 0.2 }).unwrap();
            (start.iter().map(|p| found.moved.apply(*p)).collect::<Vec<Vec3>>(), found)
        };
        let (here, lay) = rest(&lumpy, false);
        assert!(lay.rms_after <= lay.rms_before, "{lay:?}");
        let near = turned(&lumpy, Vec3::new(0.2, -0.5, 0.8).normalized().unwrap(), 0.05, Vec3::new(0.8, -0.6, 1.5));
        let far = turned(&lumpy, Vec3::new(0.0, 0.1, 1.0).normalized().unwrap(), 0.35, Vec3::new(10.0, -8.0, 6.0));
        for (name, start, search) in [("near", &near, false), ("far", &far, true)] {
            let (there, found) = rest(start, search);
            let apart = here.iter().zip(&there).map(|(a, b)| a.dist(*b)).fold(0.0, f64::max);
            assert!(apart < 0.05, "from {name}: {apart} mm from where it rests from its own place ({found:?})");
            assert!((found.rms_after - lay.rms_after).abs() < 1e-3, "from {name}: {} against {}", found.rms_after, lay.rms_after);
        }
        // A second fit from where it rests leaves it there.
        let again = register(&Scan::new(&here, &open), &reference, &Settings { search: false, ignored: 0.2 }).unwrap();
        let stays = here.iter().map(|p| again.moved.apply(*p).dist(*p)).fold(0.0, f64::max);
        assert!(stays < 0.01, "{stays} ({again:?})");
    }

    /// A pass that covers only part of the part (its top and two sides cut away) is laid onto the
    /// whole.
    #[test]
    fn a_pass_of_part_of_the_part_lies_on_the_whole() {
        let (positions, triangles) = part();
        let kept: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| t.iter().all(|&v| positions[v as usize].y < 30.0)).collect();
        let axis = Vec3::new(0.1, -0.6, 0.8).normalized().unwrap();
        let moved = turned(&positions, axis, 0.4, Vec3::new(-12.0, 8.0, 5.0));
        let (reference, pass) = (Scan::new(&positions, &triangles), Scan::new(&moved, &kept));
        let found = register(&pass, &reference, &Settings::default()).unwrap();
        let back = kept.iter().flatten().map(|&v| found.moved.apply(moved[v as usize]).dist(positions[v as usize])).fold(0.0, f64::max);
        assert!(back < 1e-2, "{back} ({found:?})");
        // What lies on the whole fits it; the figure for that is the matched share's own.
        assert!(found.rms_matched < 1e-3 && found.matched > 0.5, "{found:?}");
    }

    /// A closed box scanned in two passes that share only a thin band round its middle, as a van's
    /// walls and its ceiling do: the lower pass, the upper pass, and the band's vertices, with a few
    /// bosses in the band so that nothing can slide.
    fn seam() -> (Vec<Vec3>, Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<[u32; 3]>) {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(400.0, 180.0, 200.0), 40);
        for (lo, hi) in [
            (Vec3::new(-15.0, 40.0, 125.0), Vec3::new(0.0, 80.0, 145.0)),
            (Vec3::new(400.0, 100.0, 128.0), Vec3::new(420.0, 125.0, 142.0)),
            (Vec3::new(120.0, 180.0, 122.0), Vec3::new(170.0, 200.0, 138.0)),
            (Vec3::new(260.0, -12.0, 130.0), Vec3::new(290.0, 0.0, 146.0)),
        ] {
            let (more, tri) = fine_cuboid(lo, hi, 3);
            let base = positions.len() as u32;
            positions.extend(more);
            triangles.extend(tri.into_iter().map(|t| t.map(|v| v + base)));
        }
        // Ribs and bosses all over the walls, as sheet metal has, in a fixed irregular pattern:
        // a placement that lays flat wall on flat wall still has to lay the ribs on each other.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move |lo: f64, hi: f64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            lo + (hi - lo) * ((state >> 11) as f64 / (1u64 << 53) as f64)
        };
        for wall in 0..4 {
            for _ in 0..14 {
                let (along, up) = (next(10.0, if wall < 2 { 340.0 } else { 140.0 }), next(10.0, 175.0));
                let (wide, high, out) = (next(15.0, 55.0), next(8.0, 30.0), next(5.0, 12.0));
                let (lo, hi) = match wall {
                    0 => (Vec3::new(-out, along, up), Vec3::new(0.0, along + wide, up + high)),
                    1 => (Vec3::new(400.0, along, up), Vec3::new(400.0 + out, along + wide, up + high)),
                    2 => (Vec3::new(along, -out, up), Vec3::new(along + wide, 0.0, up + high)),
                    _ => (Vec3::new(along, 180.0, up), Vec3::new(along + wide, 180.0 + out, up + high)),
                };
                let (more, tri) = fine_cuboid(lo, hi, 2);
                let base = positions.len() as u32;
                positions.extend(more);
                triangles.extend(tri.into_iter().map(|t| t.map(|v| v + base)));
            }
        }
        let within = |t: &[u32; 3], keep: &dyn Fn(f64) -> bool| t.iter().all(|&v| keep(positions[v as usize].z));
        let lower: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| within(t, &|z| z < 150.0)).collect();
        let upper: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| within(t, &|z| z > 120.0)).collect();
        let band: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| within(t, &|z| z > 120.0 && z < 150.0)).collect();
        (positions, lower, upper, band)
    }

    /// The most a pass's band vertices lie from where they were, after `found` is applied to `moved`.
    fn off_by(moved: &[Vec3], positions: &[Vec3], band: &[[u32; 3]], found: &Rigid) -> f64 {
        band.iter().flatten().map(|&v| found.apply(moved[v as usize]).dist(positions[v as usize])).fold(0.0, f64::max)
    }

    #[test]
    fn a_pass_sharing_only_a_seam_is_laid_back_from_near() {
        let (positions, lower, upper, band) = seam();
        let axis = Vec3::new(0.3, 0.8, 0.5).normalized().unwrap();
        let moved = turned(&positions, axis, 0.03, Vec3::new(4.0, -3.0, 2.0));
        let (reference, pass) = (Scan::new(&positions, &lower), Scan::new(&moved, &upper));
        let found = register(&pass, &reference, &Settings { search: false, ignored: 0.6 }).unwrap();
        let back = off_by(&moved, &positions, &band, &found.moved);
        assert!(back < 0.5, "the seam back within half a millimetre: {back} ({found:?})");
        assert!(found.rms_after < found.rms_before, "{found:?}");
    }

    /// The upper pass turned half round and moved far off has, among the starts the flat faces
    /// give, one a few cells from where it belongs. (Whether the search then picks it depends on
    /// the surfaces not being flat to a hair all over: a box that is, lays flat wall on flat wall
    /// as well.)
    #[test]
    fn the_flat_faces_of_a_seam_give_a_start_near_it() {
        let (positions, lower, upper, band) = seam();
        let axis = Vec3::new(0.2, -0.5, 0.8).normalized().unwrap();
        let moved = turned(&positions, axis, 2.2, Vec3::new(260.0, -180.0, 90.0));
        let (reference, pass) = (Scan::new(&positions, &lower), Scan::new(&moved, &upper));
        let (starts, cell) = frames::starts(&Surface::new(pass), &Surface::new(reference));
        assert!(cell > 0.0 && !starts.is_empty());
        let nearest = starts.iter().map(|t| off_by(&moved, &positions, &band, t)).fold(f64::INFINITY, f64::min);
        assert!(nearest < 0.5 * cell, "nearest start {nearest} mm off, a cell is {cell} mm");
    }

    #[test]
    fn a_pass_already_in_place_is_never_made_worse() {
        let (positions, lower, upper, band) = seam();
        let (reference, pass) = (Scan::new(&positions, &lower), Scan::new(&positions, &upper));
        for ignored in [0.2, 0.6] {
            let found = register(&pass, &reference, &Settings { search: false, ignored }).unwrap();
            assert!(found.rms_after <= found.rms_before + 1e-9, "{found:?}");
            assert!(off_by(&positions, &positions, &band, &found.moved) < 0.1, "stays put at {ignored}: {found:?}");
        }
    }

    /// A fit as the van's walls and ceiling gave: 38 % of the scan lies on the other.
    fn thin(ignored: f64, matched: f64) -> Registration {
        Registration {
            moved: Rigid::shift(Vec3::ZERO),
            rms_before: 3.0,
            rms_after: 3.0,
            matched,
            rms_matched: 0.0,
            rounds: 3,
            converged: true,
            start: "where it was",
            rival: None,
            kept: None,
            ignored,
        }
    }

    #[test]
    fn a_fit_says_when_the_scan_has_more_off_the_other_than_it_may_leave_out() {
        let note = thin(0.2, 0.38).note().expect("a note");
        assert!(note.contains("`ignored`") && note.contains("62 %"), "{note}");
        assert!(thin(0.6, 0.38).note().is_none(), "nothing more to raise");
        assert!(thin(0.2, 0.95).note().is_none(), "a scan that lies on the other");
    }

    #[test]
    fn a_fit_with_almost_none_of_the_scan_on_the_other_says_so() {
        let note = thin(0.6, 0.16).note().expect("a note");
        assert!(note.contains("16 %") && note.contains("scan.align_pairs"), "{note}");
    }

    #[test]
    fn a_fit_taken_back_says_so() {
        let found = Registration { kept: Some(30.0), converged: false, ..thin(0.6, 0.4) };
        assert!(found.note().is_some_and(|n| n.contains("left where it was")));
    }

    #[test]
    fn a_scan_with_no_surface_is_refused() {
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let (good, triangles) = part();
        assert_eq!(register(&Scan::new(&positions, &[]), &Scan::new(&good, &triangles), &Settings::default()), Err(RegisterError::TooLittle));
    }
}
