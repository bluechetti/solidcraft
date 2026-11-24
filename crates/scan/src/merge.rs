//! Merge Surfaces (SurfaceCraft's Merge Quad Surfaces): Free Forms whose
//! sides run along each other made to meet exactly, so that they sew into one body. Each side of a
//! net that runs along a side of another net within the gap is matched to it: their knots are made
//! the same (each net refined by the other's knots, its surface exactly as it was), the corners
//! that meet are put at their middle, and the two sides' points are put half way between, so the
//! two sides are one curve. A net's own opposite sides meet too: a net wrapped the whole way round
//! a part, open at its seam, is closed there the same way.

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{Dir, NurbsSurface};
use solidcraft_geom::prelude::*;

use crate::net::MOST;
use crate::surface_edit::{points, with_points};

/// A side of a net: its first or last row (running along u) or column (along v).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    FirstRow,
    LastRow,
    FirstColumn,
    LastColumn,
}

pub const SIDES: [Side; 4] = [Side::FirstRow, Side::LastRow, Side::FirstColumn, Side::LastColumn];

impl Side {
    /// The way the side runs: 0 along u (a row), 1 along v (a column).
    pub fn along(self) -> usize {
        match self {
            Side::FirstRow | Side::LastRow => 0,
            Side::FirstColumn | Side::LastColumn => 1,
        }
    }

    /// The places of its points in a net of `[nu, nv]` (u fastest), in order along it.
    pub(crate) fn indices(self, [nu, nv]: [usize; 2]) -> Vec<usize> {
        match self {
            Side::FirstRow => (0..nu).collect(),
            Side::LastRow => (0..nu).map(|i| (nv - 1) * nu + i).collect(),
            Side::FirstColumn => (0..nv).map(|j| j * nu).collect(),
            Side::LastColumn => (0..nv).map(|j| j * nu + nu - 1).collect(),
        }
    }

    /// The side across the net from it.
    pub(crate) fn opposite(self) -> Side {
        match self {
            Side::FirstRow => Side::LastRow,
            Side::LastRow => Side::FirstRow,
            Side::FirstColumn => Side::LastColumn,
            Side::LastColumn => Side::FirstColumn,
        }
    }

    /// How long the side of `s` is, along it.
    pub fn length(self, s: &NurbsSurface) -> f64 {
        let at: Vec<Vec3> = (0..=SAMPLES * 4).filter_map(|k| self.at(s, k as f64 / (SAMPLES * 4) as f64)).collect();
        at.windows(2).map(|w| w[0].dist(w[1])).sum()
    }

    /// The point of the side of `s` at `t`, 0 to 1 along it.
    pub fn at(self, s: &NurbsSurface, t: f64) -> Option<Vec3> {
        let [[u0, u1], [v0, v1]] = s.domain();
        let (u, v) = match self {
            Side::FirstRow => (u0 + (u1 - u0) * t, v0),
            Side::LastRow => (u0 + (u1 - u0) * t, v1),
            Side::FirstColumn => (u0, v0 + (v1 - v0) * t),
            Side::LastColumn => (u1, v0 + (v1 - v0) * t),
        };
        s.eval(u, v).ok()
    }
}

/// Two sides that meet: net `a`'s and net `b`'s, `reversed` when they run opposite ways, and how
/// far apart they were on average before.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    pub a: usize,
    pub a_side: Side,
    pub b: usize,
    pub b_side: Side,
    pub reversed: bool,
    pub gap: f64,
}

/// The nets made to meet, and where they meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Merged {
    pub surfaces: Vec<NurbsSurface>,
    pub seams: Vec<Seam>,
}

/// Why the nets could not be made to meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeError {
    /// No side of one runs along a side of another within the gap.
    NoSeam,
    /// A net with weights, or of another degree than its neighbour's.
    NotCubic,
    /// Matching the knots would make a net too fine.
    TooMany,
    Failed,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MergeError::NoSeam => "no side of one surface runs along a side of another within the gap: put their corners together, or widen the gap",
            MergeError::NotCubic => "only surfaces Fit Surface, New Free Form and Wrap Surface make can be merged",
            MergeError::TooMany => "matching the surfaces' points would give a net more than 48 points a side",
            MergeError::Failed => "the surfaces could not be made to meet",
        })
    }
}

/// How many places along two sides are compared.
const SAMPLES: usize = 9;

/// Knots closer than this (on 0 to 1) are one.
const SAME_KNOT: f64 = 1e-9;

/// How far apart two sides are on average along them (the second run backwards when `reversed`).
fn apart(a: &NurbsSurface, sa: Side, b: &NurbsSurface, sb: Side, reversed: bool) -> Option<f64> {
    let mut sum = 0.0;
    for k in 0..=SAMPLES {
        let t = k as f64 / SAMPLES as f64;
        sum += sa.at(a, t)?.dist(sb.at(b, if reversed { 1.0 - t } else { t })?);
    }
    Some(sum / (SAMPLES + 1) as f64)
}

/// The sides of `surfaces` that run along each other within `gap`, each side in one seam at most,
/// nearest first: sides of two nets, or a net's opposite sides (its seam).
pub fn seams(surfaces: &[NurbsSurface], gap: f64) -> Vec<Seam> {
    let mut all = Vec::new();
    for a in 0..surfaces.len() {
        for b in a..surfaces.len() {
            for a_side in SIDES {
                for b_side in SIDES {
                    // A net meets itself only across: its first row with its last, or column.
                    if a == b && (b_side != a_side.opposite() || matches!(a_side, Side::LastRow | Side::LastColumn)) {
                        continue;
                    }
                    for reversed in [false, true] {
                        if let Some(d) = apart(&surfaces[a], a_side, &surfaces[b], b_side, reversed).filter(|d| *d <= gap) {
                            all.push(Seam { a, a_side, b, b_side, reversed, gap: d });
                        }
                    }
                }
            }
        }
    }
    all.sort_by(|x, y| x.gap.total_cmp(&y.gap));
    let mut used: Vec<(usize, Side)> = Vec::new();
    all.retain(|s| {
        if used.contains(&(s.a, s.a_side)) || used.contains(&(s.b, s.b_side)) {
            return false;
        }
        used.push((s.a, s.a_side));
        used.push((s.b, s.b_side));
        true
    });
    all
}

/// The two sides of `surfaces` that come nearest running along each other, at any gap: how far
/// apart they are on average, and whether they are one net's own opposite sides (a wrap's seam).
/// `None` for no surfaces.
pub fn nearest_sides(surfaces: &[NurbsSurface]) -> Option<(f64, bool)> {
    seams(surfaces, f64::INFINITY).into_iter().map(|s| (s.gap, s.a == s.b)).min_by(|x, y| x.0.total_cmp(&y.0))
}

fn dir(d: usize) -> Dir {
    if d == 0 { Dir::S } else { Dir::T }
}

/// The knots of `s` along `d` on 0 to 1, turned round when `reversed`.
fn unit_knots(s: &NurbsSurface, d: usize, reversed: bool) -> Vec<f64> {
    let k = &s.knots()[d];
    let (lo, hi) = (k.first().copied().unwrap_or(0.0), k.last().copied().unwrap_or(1.0));
    let span = if hi > lo { hi - lo } else { 1.0 };
    let unit = k.iter().map(|x| (x - lo) / span);
    if reversed { unit.rev().map(|x| 1.0 - x).collect() } else { unit.collect() }
}

/// The inner knots of `want` that `have` lacks, as many times as it lacks them.
fn missing(have: &[f64], want: &[f64]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < want.len() {
        let x = want[k];
        let times = want[k..].iter().take_while(|y| (**y - x).abs() < SAME_KNOT).count();
        k += times;
        if x <= SAME_KNOT || x >= 1.0 - SAME_KNOT {
            continue;
        }
        let had = have.iter().filter(|y| (**y - x).abs() < SAME_KNOT).count();
        out.extend(std::iter::repeat_n(x, times.saturating_sub(had)));
    }
    out
}

/// A knot on 0 to 1 (turned round when `reversed`) on `s`'s own knots along `d`.
fn own_knot(s: &NurbsSurface, d: usize, reversed: bool, x: f64) -> f64 {
    let k = &s.knots()[d];
    let (lo, hi) = (k.first().copied().unwrap_or(0.0), k.last().copied().unwrap_or(1.0));
    lo + if reversed { 1.0 - x } else { x } * (hi - lo)
}

fn cubic(s: &NurbsSurface) -> bool {
    s.degree() == [3, 3] && s.homogeneous().iter().all(|h| (h.w - 1.0).abs() <= 1e-12)
}

/// `surfaces` made to meet where their sides run along each other within `gap`.
pub fn merge(surfaces: &[NurbsSurface], gap: f64) -> Result<Merged, MergeError> {
    if surfaces.iter().any(|s| !cubic(s)) {
        return Err(MergeError::NotCubic);
    }
    let seams = seams(surfaces, gap);
    if seams.is_empty() {
        return Err(MergeError::NoSeam);
    }
    let mut nets = surfaces.to_vec();
    // The knots along each seam made the same, both ways round, until no net gains one (a knot a
    // net gains along one seam reaches its opposite side, and that side's seam too).
    for round in 0.. {
        if round > 2 * seams.len() + 2 {
            return Err(MergeError::Failed);
        }
        let mut changed = false;
        for seam in &seams {
            let (da, db) = (seam.a_side.along(), seam.b_side.along());
            if seam.a == seam.b {
                // A seam of one net: its knots that way made the same both ways round.
                let (k, turned) = (unit_knots(&nets[seam.a], da, false), unit_knots(&nets[seam.a], da, seam.reversed));
                for x in missing(&k, &turned) {
                    let at = own_knot(&nets[seam.a], da, false, x);
                    nets[seam.a] = nets[seam.a].insert_knot(dir(da), at).map_err(|_| MergeError::Failed)?;
                    changed = true;
                }
                if nets[seam.a].count().iter().any(|n| *n > MOST) {
                    return Err(MergeError::TooMany);
                }
                continue;
            }
            let ka = unit_knots(&nets[seam.a], da, false);
            let kb = unit_knots(&nets[seam.b], db, seam.reversed);
            for x in missing(&ka, &kb) {
                let at = own_knot(&nets[seam.a], da, false, x);
                nets[seam.a] = nets[seam.a].insert_knot(dir(da), at).map_err(|_| MergeError::Failed)?;
                changed = true;
            }
            for x in missing(&kb, &ka) {
                let at = own_knot(&nets[seam.b], db, seam.reversed, x);
                nets[seam.b] = nets[seam.b].insert_knot(dir(db), at).map_err(|_| MergeError::Failed)?;
                changed = true;
            }
            if nets[seam.a].count().iter().chain(nets[seam.b].count().iter()).any(|n| *n > MOST) {
                return Err(MergeError::TooMany);
            }
        }
        if !changed {
            break;
        }
    }
    let mut places: Vec<Vec<Vec3>> = nets.iter().map(points).collect();
    let counts: Vec<[usize; 2]> = nets.iter().map(NurbsSurface::count).collect();
    let pairs = |seam: &Seam| -> Result<Vec<(usize, usize)>, MergeError> {
        let a = seam.a_side.indices(counts[seam.a]);
        let mut b = seam.b_side.indices(counts[seam.b]);
        if seam.reversed {
            b.reverse();
        }
        if a.len() != b.len() {
            return Err(MergeError::Failed);
        }
        Ok(a.into_iter().zip(b).collect())
    };
    // The corners that meet put at their middle: every net point at the end of a seam, with all
    // the others it is joined to through seams.
    let mut joined: Vec<Vec<(usize, usize)>> = Vec::new();
    for seam in &seams {
        let pairs = pairs(seam)?;
        for (x, y) in [pairs.first(), pairs.last()].into_iter().flatten() {
            let (p, q) = ((seam.a, *x), (seam.b, *y));
            let found: Vec<usize> = (0..joined.len()).filter(|k| joined[*k].contains(&p) || joined[*k].contains(&q)).collect();
            let mut group: Vec<(usize, usize)> = vec![p, q];
            for k in found.iter().rev() {
                group.extend(joined.remove(*k));
            }
            group.sort_unstable();
            group.dedup();
            joined.push(group);
        }
    }
    for group in &joined {
        let middle = group.iter().fold(Vec3::ZERO, |s, (n, i)| s + places[*n][*i]) / group.len() as f64;
        for (n, i) in group {
            places[*n][*i] = middle;
        }
    }
    // Each seam's points half way between its two sides (the corners are already one point).
    for seam in &seams {
        for (x, y) in pairs(seam)? {
            let middle = (places[seam.a][x] + places[seam.b][y]) * 0.5;
            places[seam.a][x] = middle;
            places[seam.b][y] = middle;
        }
    }
    let surfaces = nets.iter().zip(&places).map(|(s, p)| with_points(s, p).ok_or(MergeError::Failed)).collect::<Result<Vec<_>, _>>()?;
    Ok(Merged { surfaces, seams })
}

/// The net cut in two across the middle of its knots the way `d` runs (0 u, 1 v): a knot put
/// there until the surface passes through a line of the net's points, which both halves keep, so
/// they meet exactly and together are the surface as it was. A net closed round on its own seam
/// is sewn as two such halves, each seam then between two faces.
pub fn halves(s: &NurbsSurface, d: usize) -> Result<(NurbsSurface, NurbsSurface), MergeError> {
    if !cubic(s) || d > 1 {
        return Err(MergeError::NotCubic);
    }
    let p = s.degree()[d];
    let knots = &s.knots()[d];
    let (lo, hi) = (knots.get(p).copied().unwrap_or(0.0), knots.get(knots.len().saturating_sub(p + 1)).copied().unwrap_or(1.0));
    let mid = 0.5 * (lo + hi);
    let had = knots.iter().filter(|k| (**k - mid).abs() <= SAME_KNOT * (hi - lo).abs().max(1.0)).count();
    let mut cut = s.clone();
    for _ in had..p {
        cut = cut.insert_knot(dir(d), mid).map_err(|_| MergeError::Failed)?;
    }
    let k = cut.knots()[d].clone();
    // The surface passes through point `first - 1` that way, where the middle's knots begin.
    let first = k.iter().position(|x| (*x - mid).abs() <= SAME_KNOT * (hi - lo).abs().max(1.0)).ok_or(MergeError::Failed)?;
    let shared = first.checked_sub(1).ok_or(MergeError::Failed)?;
    let [nu, nv] = cut.count();
    let n = if d == 0 { nu } else { nv };
    if shared < p || shared + p >= n {
        return Err(MergeError::Failed);
    }
    let net = points(&cut);
    let part = |range: std::ops::Range<usize>| -> Vec<Vec3> {
        if d == 0 {
            (0..nv).flat_map(|j| range.clone().map(move |i| j * nu + i)).map(|i| net[i]).collect()
        } else {
            range.flat_map(|j| (0..nu).map(move |i| j * nu + i)).map(|i| net[i]).collect()
        }
    };
    let low_knots: Vec<f64> = k[..first].iter().copied().chain(std::iter::repeat_n(mid, p + 1)).collect();
    let high_knots: Vec<f64> = std::iter::repeat_n(mid, p + 1).chain(k[first + p..].iter().copied()).collect();
    let (low, high) = (part(0..shared + 1), part(shared..n));
    let other = cut.knots()[1 - d].clone();
    let make = |own: Vec<f64>, pts: &[Vec3]| {
        let knots = if d == 0 { [own, other.clone()] } else { [other.clone(), own] };
        NurbsSurface::new(cut.degree(), knots, pts, &vec![1.0; pts.len()]).map_err(|_| MergeError::Failed)
    };
    Ok((make(low_knots, &low)?, make(high_knots, &high)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::free_form::{net_over, surface};

    /// The largest distance between the side `sa` of `a` and `sb` of `b` along them.
    fn worst(a: &NurbsSurface, sa: Side, b: &NurbsSurface, sb: Side, reversed: bool) -> f64 {
        (0..=40)
            .map(|k| {
                let t = f64::from(k) / 40.0;
                sa.at(a, t).unwrap().dist(sb.at(b, if reversed { 1.0 - t } else { t }).unwrap())
            })
            .fold(0.0, f64::max)
    }

    /// Two flat nets side by side, 0.4 mm apart, of 5 and 7 points: they come to share their side
    /// exactly, the coarser refined to the finer's points along it, and each still lies flat.
    #[test]
    fn two_nets_side_by_side_meet_exactly() {
        let left =
            surface(&net_over([Vec3::new(0.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0), Vec3::new(20.0, 30.0, 0.0), Vec3::new(0.0, 30.0, 0.0)], 5), 5)
                .unwrap();
        let right =
            surface(&net_over([Vec3::new(20.4, 0.0, 0.0), Vec3::new(45.0, 0.0, 0.0), Vec3::new(45.0, 30.0, 0.0), Vec3::new(20.4, 30.0, 0.0)], 7), 7)
                .unwrap();
        let merged = merge(&[left.clone(), right.clone()], 1.0).unwrap();
        assert_eq!(merged.seams.len(), 1);
        let seam = merged.seams[0];
        assert_eq!((seam.a_side, seam.b_side, seam.reversed), (Side::LastColumn, Side::FirstColumn, false));
        let [a, b] = [&merged.surfaces[0], &merged.surfaces[1]];
        assert!(worst(a, seam.a_side, b, seam.b_side, false) < 1e-9, "one curve");
        assert_eq!(a.count()[1], b.count()[1], "as many points along the side");
        assert!(points(a).iter().chain(points(b).iter()).all(|p| p.z.abs() < 1e-12), "still flat");
        // A side run the other way, and nets too far apart.
        let turned =
            surface(&net_over([Vec3::new(45.0, 30.0, 0.0), Vec3::new(20.4, 30.0, 0.0), Vec3::new(20.4, 0.0, 0.0), Vec3::new(45.0, 0.0, 0.0)], 4), 4)
                .unwrap();
        let merged = merge(&[left.clone(), turned], 1.0).unwrap();
        let seam = merged.seams[0];
        assert!(seam.reversed && worst(&merged.surfaces[0], seam.a_side, &merged.surfaces[1], seam.b_side, true) < 1e-9, "{seam:?}");
        assert_eq!(merge(&[left, right], 0.1), Err(MergeError::NoSeam));
    }

    /// Four nets round a point: the corner they share is one point and every seam one curve.
    #[test]
    fn four_nets_round_a_point_meet_at_it() {
        let quad = |x: f64, y: f64, n: usize, lift: f64| {
            let c = [Vec3::new(x, y, 0.0), Vec3::new(x + 10.0, y, lift), Vec3::new(x + 10.0, y + 10.0, 0.0), Vec3::new(x, y + 10.0, lift)];
            surface(&net_over(c, n), n).unwrap()
        };
        let nets = [quad(0.0, 0.0, 4, 0.0), quad(10.3, 0.0, 5, 0.2), quad(0.0, 10.2, 6, 0.0), quad(10.1, 10.1, 4, 0.1)];
        let merged = merge(&nets, 1.0).unwrap();
        assert_eq!(merged.seams.len(), 4, "{:?}", merged.seams);
        for seam in &merged.seams {
            let w = worst(&merged.surfaces[seam.a], seam.a_side, &merged.surfaces[seam.b], seam.b_side, seam.reversed);
            assert!(w < 1e-9, "{seam:?}: {w}");
        }
    }

    /// A net bent nearly the whole way round a cylinder, open at a narrow seam: its first and last
    /// columns are made one.
    #[test]
    fn a_net_wrapped_round_closes_its_own_seam() {
        let (nu, nv) = (8usize, 5usize);
        let mut net = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                let a = -3.1 + 6.2 * i as f64 / (nu - 1) as f64;
                net.push(Vec3::new(10.0 * a.cos(), 10.0 * a.sin(), 10.0 * j as f64));
            }
        }
        let wrapped = crate::free_form::net_surface(&net, [nu, nv]).unwrap();
        let merged = merge(std::slice::from_ref(&wrapped), 2.0).unwrap();
        let [seam] = merged.seams.as_slice() else { panic!("{:?}", merged.seams) };
        assert_eq!((seam.a, seam.b, seam.a_side, seam.b_side), (0, 0, Side::FirstColumn, Side::LastColumn));
        let closed = &merged.surfaces[0];
        assert!(worst(closed, Side::FirstColumn, closed, Side::LastColumn, false) < 1e-9, "the seam is one curve");
        // Too narrow a gap finds no seam; the nearest sides are its own seam, 0.83 mm apart.
        assert_eq!(merge(std::slice::from_ref(&wrapped), 0.1), Err(MergeError::NoSeam));
        let (apart, own) = nearest_sides(&[wrapped]).unwrap();
        assert!(own && (apart - 10.0 * (std::f64::consts::TAU - 6.2)).abs() < 0.05, "{apart}");
    }

    /// The two halves are the surface: each place on it is on one half, and they share their
    /// middle line exactly.
    #[test]
    fn halves_are_the_surface_cut_in_two() {
        let c = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(30.0, 0.0, 4.0), Vec3::new(30.0, 20.0, 0.0), Vec3::new(0.0, 20.0, 2.0)];
        let mut net = net_over(c, 6);
        net[14].z += 3.0;
        let s = surface(&net, 6).unwrap();
        for d in [0, 1] {
            let (low, high) = halves(&s, d).unwrap();
            let (lo_side, hi_side) = if d == 0 { (Side::LastColumn, Side::FirstColumn) } else { (Side::LastRow, Side::FirstRow) };
            assert!(worst(&low, lo_side, &high, hi_side, false) < 1e-12, "they meet");
            for k in 0..=10 {
                let t = f64::from(k) / 10.0;
                let whole = if d == 0 { s.eval(0.25, t).unwrap() } else { s.eval(t, 0.25).unwrap() };
                let half = if d == 0 { low.eval(0.25, t).unwrap() } else { low.eval(t, 0.25).unwrap() };
                assert!(whole.dist(half) < 1e-9, "{d}: {whole:?} {half:?}");
            }
        }
    }
}
