//! Patterns of a Free Form and tools for its open edges: copies of faces laid in a line or round
//! an axis that sew to each other where their sides meet, two open sides made to have as many
//! points (knot insertion, the strip of faces across their seams refined with them so that every
//! seam keeps its counts), the whole open edge a point is on, and the row between two points.
//!
//! Everything here is a pure function over faces. A network is one piece (see
//! [`crate::network::connected`]): a pattern whose copies do not touch is refused with
//! [`PatternError::Disconnected`], and `joins` itself accepts disconnected faces (it only reads
//! them); it is the pattern that keeps the Free Form in one piece.

use std::collections::{HashMap, VecDeque};

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{Dir, NurbsSurface};
use solidcraft_geom::prelude::*;

use crate::merge::Side;
use crate::net::MOST;
use crate::network::{Joins, MOST_FACES, NetworkError, connected, joins, network_points, with_network_points};
use crate::surface_edit::{points, with_points};

/// The most copies (the original included) a pattern makes.
pub const MOST_COPIES: usize = 64;

/// Where the edge of a network turns by more than this, degrees, an edge chain ends.
pub const CORNER_DEGREES: f64 = 30.0;

/// Copies in a line: `count` faces' sets in all, each `spacing` further along `direction`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Linear {
    pub direction: Vec3,
    pub spacing: f64,
    pub count: usize,
}

/// Copies round an axis: `count` sets in all, each turned `angle` degrees further.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circular {
    pub axis_point: Vec3,
    pub axis: Vec3,
    pub angle: f64,
    pub count: usize,
}

/// Why a pattern or an edge tool could not be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternError {
    /// No faces to copy.
    NoFaces,
    /// The count is not from 2 to 64.
    BadCount,
    /// The direction or axis is zero or not a number, or the spacing or angle is not.
    BadPlacement,
    /// More than 400 faces would result.
    TooManyFaces,
    /// The copies do not touch the original's faces or each other: a Free Form is one piece.
    Disconnected,
    /// A side asked for is not on the network's edge, or a face is not there.
    NotOpen,
    /// Matching the sides would give a net more than 48 points a side.
    TooMany,
    /// The faces could not be sewn or refined.
    Failed,
    Network(NetworkError),
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatternError::NoFaces => f.write_str("there are no faces to copy"),
            PatternError::BadCount => f.write_str("a pattern has from 2 to 64 copies, the original counted"),
            PatternError::BadPlacement => f.write_str("the pattern's direction or axis must be a line (not zero), and its spacing or angle a number"),
            PatternError::TooManyFaces => f.write_str("the pattern would have more than 400 faces: use fewer copies"),
            PatternError::Disconnected => f.write_str(
                "the copies do not touch each other: a Free Form is one piece, so space them so that their edges meet (a spacing equal to the width, or an angle equal to the shape's), or copy the body instead",
            ),
            PatternError::NotOpen => f.write_str("that side is not on the Free Form's edge: choose the edge of a face with no face beside it"),
            PatternError::TooMany => f.write_str("matching the edges' points would give a net more than 48 points a side"),
            PatternError::Failed => f.write_str("the faces could not be made to match"),
            PatternError::Network(e) => e.fmt(f),
        }
    }
}

impl From<NetworkError> for PatternError {
    fn from(e: NetworkError) -> Self {
        match e {
            NetworkError::TooManyFaces => PatternError::TooManyFaces,
            e => PatternError::Network(e),
        }
    }
}

/// `faces` with every control point put through `place`.
fn placed(faces: &[NurbsSurface], place: impl Fn(Vec3) -> Vec3) -> Result<Vec<NurbsSurface>, PatternError> {
    faces.iter().map(|f| with_points(f, &points(f).into_iter().map(&place).collect::<Vec<_>>()).ok_or(PatternError::Failed)).collect()
}

/// The copies sewn: the faces that meet are one network, the points that meet put exactly
/// together; an error when they are not one piece.
fn sewn(all: Vec<NurbsSurface>) -> Result<Vec<NurbsSurface>, PatternError> {
    if all.len() > MOST_FACES {
        return Err(PatternError::TooManyFaces);
    }
    let j = joins(&all)?;
    if !connected(&all, &j) {
        return Err(PatternError::Disconnected);
    }
    let places = network_points(&all, &j);
    let made = with_network_points(&all, &j, &places).ok_or(PatternError::Failed)?;
    joins(&made)?;
    Ok(made)
}

fn check_count(faces: &[NurbsSurface], count: usize) -> Result<(), PatternError> {
    if faces.is_empty() {
        return Err(PatternError::NoFaces);
    }
    if !(2..=MOST_COPIES).contains(&count) {
        return Err(PatternError::BadCount);
    }
    if faces.len().saturating_mul(count) > MOST_FACES {
        return Err(PatternError::TooManyFaces);
    }
    Ok(())
}

/// The faces followed by copies of them, each `spacing` further along the direction. Where a
/// copy's sides meet the one before's the faces are sewn.
pub fn linear(faces: &[NurbsSurface], p: &Linear) -> Result<Vec<NurbsSurface>, PatternError> {
    check_count(faces, p.count)?;
    let dir = p.direction.normalized().filter(|_| p.direction.is_finite() && p.spacing.is_finite() && p.spacing != 0.0);
    let dir = dir.ok_or(PatternError::BadPlacement)?;
    let mut all = faces.to_vec();
    for k in 1..p.count {
        let offset = dir * (p.spacing * k as f64);
        all.extend(placed(faces, |q| q + offset)?);
    }
    sewn(all)
}

/// `q` turned `angle` radians about the line through `c` along the unit `k`.
fn turned(q: Vec3, c: Vec3, k: Vec3, angle: f64) -> Vec3 {
    let (s, cos) = angle.sin_cos();
    let r = q - c;
    c + r * cos + k.cross(r) * s + k * (k.dot(r) * (1.0 - cos))
}

/// The faces followed by copies of them, each turned `angle` degrees further about the axis. A
/// whole turn (`angle` times `count` is 360) closes on itself, sewn.
pub fn circular(faces: &[NurbsSurface], p: &Circular) -> Result<Vec<NurbsSurface>, PatternError> {
    check_count(faces, p.count)?;
    let axis = p.axis.normalized().filter(|_| p.axis.is_finite() && p.axis_point.is_finite() && p.angle.is_finite() && p.angle != 0.0);
    let axis = axis.ok_or(PatternError::BadPlacement)?;
    let mut all = faces.to_vec();
    for k in 1..p.count {
        let a = (p.angle * k as f64).to_radians();
        all.extend(placed(faces, |q| turned(q, p.axis_point, axis, a))?);
    }
    sewn(all)
}

fn dir_of(d: usize) -> Dir {
    if d == 0 { Dir::S } else { Dir::T }
}

/// The middle of the widest span of `s` along `d`, 0 to 1 along its domain.
fn widest_middle(s: &NurbsSurface, d: usize) -> Option<f64> {
    let (p, n, k) = (*s.degree().get(d)?, *s.count().get(d)?, s.knots().get(d)?);
    let (lo, hi) = (*k.get(p)?, *k.get(n)?);
    if !(hi > lo) {
        return None;
    }
    let mut best: Option<(f64, f64)> = None;
    for i in p..n {
        let (a, b) = (*k.get(i)?, *k.get(i + 1)?);
        if b > a && best.is_none_or(|(w, _)| b - a > w) {
            best = Some((b - a, 0.5 * (a + b)));
        }
    }
    best.map(|(_, m)| (m - lo) / (hi - lo))
}

fn insert_at(s: &NurbsSurface, d: usize, t: f64) -> Option<NurbsSurface> {
    let (p, n, k) = (*s.degree().get(d)?, *s.count().get(d)?, s.knots().get(d)?);
    let (lo, hi) = (*k.get(p)?, *k.get(n)?);
    s.insert_knot(dir_of(d), lo + (hi - lo) * t.clamp(0.0, 1.0)).ok()
}

/// The faces with the two open sides made to have as many points: the one with fewer has knots
/// put in (its shape exactly as it was). Every face across a seam that runs the same way gets the
/// same knots, and so on along the strip, so the seams keep their counts. The knots go in the
/// widest spans of the face with fewer points, which are the same places on the faces beside it.
pub fn match_counts(faces: &[NurbsSurface], a: (usize, Side), b: (usize, Side)) -> Result<Vec<NurbsSurface>, PatternError> {
    let before = joins(faces)?;
    if !before.open.contains(&a) || !before.open.contains(&b) {
        return Err(PatternError::NotOpen);
    }
    let count_of = |(f, s): (usize, Side)| faces.get(f).and_then(|face| face.count().get(s.along()).copied()).ok_or(PatternError::NotOpen);
    let (ca, cb) = (count_of(a)?, count_of(b)?);
    if ca == cb {
        return Ok(faces.to_vec());
    }
    let ((rep, side), extra) = if ca < cb { (a, cb - ca) } else { (b, ca - cb) };
    let d = side.along();
    // The strip: every (face, direction) whose points along it must be as many, and whether its
    // parameter runs the other way from the first's.
    let mut strip: HashMap<(usize, usize), bool> = HashMap::new();
    let mut queue = VecDeque::new();
    strip.insert((rep, d), false);
    queue.push_back((rep, d));
    while let Some((f, dir)) = queue.pop_front() {
        let flip = strip.get(&(f, dir)).copied().unwrap_or(false);
        for seam in &before.seams {
            let mut sides = Vec::new();
            if seam.a == f && seam.a_side.along() == dir {
                sides.push((seam.b, seam.b_side.along()));
            }
            if seam.b == f && seam.b_side.along() == dir {
                sides.push((seam.a, seam.a_side.along()));
            }
            for next in sides {
                if let std::collections::hash_map::Entry::Vacant(e) = strip.entry(next) {
                    e.insert(flip != seam.reversed);
                    queue.push_back(next);
                }
            }
        }
    }
    let mut made = faces.to_vec();
    for &(f, dir) in strip.keys() {
        let n = faces.get(f).and_then(|s| s.count().get(dir).copied()).ok_or(PatternError::Failed)?;
        if n + extra > MOST {
            return Err(PatternError::TooMany);
        }
    }
    let mut members: Vec<((usize, usize), bool)> = strip.into_iter().collect();
    members.sort_unstable();
    for _ in 0..extra {
        let t = made.get(rep).and_then(|s| widest_middle(s, d)).ok_or(PatternError::Failed)?;
        for &((f, dir), flip) in &members {
            let slot = made.get_mut(f).ok_or(PatternError::Failed)?;
            *slot = insert_at(slot, dir, if flip { 1.0 - t } else { t }).ok_or(PatternError::Failed)?;
        }
    }
    let after = joins(&made)?;
    let (na, nb) = (count_of_in(&made, a), count_of_in(&made, b));
    if after.seams.len() != before.seams.len() || na.is_none() || na != nb {
        return Err(PatternError::Failed);
    }
    Ok(made)
}

fn count_of_in(faces: &[NurbsSurface], (f, s): (usize, Side)) -> Option<usize> {
    faces.get(f)?.count().get(s.along()).copied()
}

/// The network points of the network's open edge in order along it, both ways from `start` (a
/// point on the edge) to a corner: where the edge turns by more than 30 degrees, an irregular
/// corner, or where the edge forks. The corners are in the chain. Empty when `start` is not on the
/// edge. A closed edge comes back round to `start`, which is then once in it.
pub fn edge_chain(joins: &Joins, faces: &[NurbsSurface], start: usize) -> Vec<usize> {
    let mut next: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(f, s) in &joins.open {
        for w in joins.side(faces, f, s).windows(2) {
            if let [p, q] = *w {
                if p == q {
                    continue;
                }
                for (x, y) in [(p, q), (q, p)] {
                    let e = next.entry(x).or_default();
                    if !e.contains(&y) {
                        e.push(y);
                    }
                }
            }
        }
    }
    let Some(first) = next.get(&start) else { return Vec::new() };
    let places = network_points(faces, joins);
    let at = |p: usize| places.get(p).copied();
    let irregular: std::collections::HashSet<usize> = joins.irregular().map(|c| c.point).collect();
    let cos_limit = CORNER_DEGREES.to_radians().cos();
    let walk = |toward: usize| -> Vec<usize> {
        let mut chain = vec![start];
        let (mut prev, mut p) = (start, toward);
        loop {
            chain.push(p);
            if p == start || chain.len() > joins.len() + 1 {
                chain.pop();
                break;
            }
            let Some(options) = next.get(&p) else { break };
            let onward: Vec<usize> = options.iter().copied().filter(|q| *q != prev).collect();
            let [q] = onward[..] else { break };
            if irregular.contains(&p) {
                break;
            }
            if let (Some(a), Some(b), Some(c)) = (at(prev), at(p), at(q))
                && let (Some(i), Some(o)) = ((b - a).normalized(), (c - b).normalized())
                && i.dot(o) < cos_limit
            {
                break;
            }
            (prev, p) = (p, q);
        }
        chain.remove(0);
        chain
    };
    let mut back = Vec::new();
    let mut forward = Vec::new();
    if let Some(&x) = first.first() {
        forward = walk(x);
    }
    if let Some(&y) = first.get(1) {
        back = walk(y);
    }
    // A closed edge walked both ways has been all the way round: once is enough.
    if back.contains(&start) || forward.iter().any(|p| back.contains(p)) {
        back.clear();
    }
    back.reverse();
    back.push(start);
    back.extend(forward);
    back
}

/// The network points of the row or column of one face's grid that runs from point `a` to point
/// `b`, in order, when they lie on one; else `None`.
pub fn loop_through(joins: &Joins, faces: &[NurbsSurface], a: usize, b: usize) -> Option<Vec<usize>> {
    if a == b {
        return None;
    }
    for (face, of) in faces.iter().zip(&joins.of) {
        let nu = *face.count().first()?;
        if nu == 0 {
            continue;
        }
        let at = |p: usize| of.iter().enumerate().filter(move |(_, id)| **id == p).map(move |(k, _)| (k % nu, k / nu));
        for (ia, ja) in at(a) {
            for (ib, jb) in at(b) {
                let line: Vec<(usize, usize)> = if ja == jb {
                    if ia <= ib { (ia..=ib).map(|i| (i, ja)).collect() } else { (ib..=ia).rev().map(|i| (i, ja)).collect() }
                } else if ia == ib {
                    if ja <= jb { (ja..=jb).map(|j| (ia, j)).collect() } else { (jb..=ja).rev().map(|j| (ia, j)).collect() }
                } else {
                    continue;
                };
                let ids: Option<Vec<usize>> = line.iter().map(|&(i, j)| of.get(j * nu + i).copied()).collect();
                if ids.is_some() {
                    return ids;
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::free_form::{net_over, net_surface, surface};

    fn square(x: f64, y: f64, size: f64, count: usize) -> NurbsSurface {
        let c = [Vec3::new(x, y, 0.0), Vec3::new(x + size, y, 0.0), Vec3::new(x + size, y + size, 0.0), Vec3::new(x, y + size, 0.0)];
        surface(&net_over(c, count), count).unwrap()
    }

    /// A 90 degree wedge of a ring on a polar net: its last column is its first turned a quarter.
    fn wedge() -> NurbsSurface {
        let mut net = Vec::new();
        for j in 0..4 {
            for i in 0..4 {
                let t = f64::from(i) * 30.0_f64.to_radians();
                let r = 10.0 + 4.0 * f64::from(j);
                net.push(Vec3::new(r * t.cos(), r * t.sin(), 0.0));
            }
        }
        net_surface(&net, [4, 4]).unwrap()
    }

    #[test]
    fn four_wedges_close_into_a_ring() {
        let p = Circular { axis_point: Vec3::ZERO, axis: Vec3::Z, angle: 90.0, count: 4 };
        let ring = circular(&[wedge()], &p).unwrap();
        assert_eq!(ring.len(), 4);
        let j = joins(&ring).unwrap();
        assert_eq!(j.seams.len(), 4, "every wedge sewn to the next, the last to the first");
        assert!(connected(&ring, &j));
        // Three wedges leave a gap but are still one piece.
        let open = circular(&[wedge()], &Circular { count: 3, ..p }).unwrap();
        assert_eq!(joins(&open).unwrap().seams.len(), 2);
        // Wedges 45 degrees apart overlap but do not sew: one piece or an error, never a panic.
        let _ = circular(&[wedge()], &Circular { angle: 45.0, ..p });
        // Apart: not one piece.
        assert_eq!(circular(&[wedge()], &Circular { angle: 120.0, count: 3, ..p }), Err(PatternError::Disconnected));
    }

    #[test]
    fn three_squares_in_a_row() {
        let p = Linear { direction: Vec3::X, spacing: 10.0, count: 3 };
        let row = linear(&[square(0.0, 0.0, 10.0, 4)], &p).unwrap();
        assert_eq!(row.len(), 3);
        let j = joins(&row).unwrap();
        assert_eq!((j.seams.len(), j.open.len()), (2, 8));
        assert_eq!(linear(&[square(0.0, 0.0, 10.0, 4)], &Linear { spacing: 20.0, ..p }), Err(PatternError::Disconnected));
    }

    #[test]
    fn hostile_input_is_an_error() {
        let f = [square(0.0, 0.0, 10.0, 4)];
        let p = Linear { direction: Vec3::X, spacing: 10.0, count: 3 };
        assert_eq!(linear(&f, &Linear { count: 0, ..p }), Err(PatternError::BadCount));
        assert_eq!(linear(&f, &Linear { count: 65, ..p }), Err(PatternError::BadCount));
        assert_eq!(linear(&f, &Linear { direction: Vec3::ZERO, ..p }), Err(PatternError::BadPlacement));
        assert_eq!(linear(&f, &Linear { spacing: f64::NAN, ..p }), Err(PatternError::BadPlacement));
        assert_eq!(linear(&[], &p), Err(PatternError::NoFaces));
        let c = Circular { axis_point: Vec3::ZERO, axis: Vec3::ZERO, angle: 90.0, count: 4 };
        assert_eq!(circular(&f, &c), Err(PatternError::BadPlacement));
        assert_eq!(circular(&f, &Circular { axis: Vec3::Z, angle: f64::NAN, ..c }), Err(PatternError::BadPlacement));
        assert!(!PatternError::Disconnected.to_string().is_empty());
    }

    #[test]
    fn counts_of_two_sides_are_matched_along_the_strip() {
        // 5 points a side beside 7: sides of the left face's right column and the right's left.
        let left = square(0.0, 0.0, 10.0, 5);
        let c = [Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0), Vec3::new(20.0, 10.0, 0.0), Vec3::new(10.0, 10.0, 0.0)];
        let right = surface(&net_over(c, 7), 7).unwrap();
        // Not sewn: the two are open with different counts. Above the left sits a third face that
        // must follow the refinement.
        let above = square(0.0, 10.0, 10.0, 5);
        let faces = [left, right, above];
        let made = match_counts(&faces, (0, Side::LastColumn), (1, Side::FirstColumn)).unwrap();
        assert_eq!(made[0].count()[1], 7);
        assert_eq!(made[1].count()[1], 7);
        assert_eq!(made[2].count()[0], 5, "the face above is joined along a row: its u count is its own");
        let j = joins(&made).unwrap();
        assert_eq!(j.seams.len(), 1, "above to left stays sewn");
        // Unchanged shape.
        let p = faces[0].eval(0.3, 0.6).unwrap();
        assert!(p.dist(made[0].eval(0.3, 0.6).unwrap()) < 1e-9);
        // Equal counts: nothing to do. A side that is not open: an error.
        assert_eq!(match_counts(&made, (0, Side::LastColumn), (1, Side::FirstColumn)).unwrap(), made);
        assert_eq!(match_counts(&made, (0, Side::LastRow), (0, Side::LastColumn)), Err(PatternError::NotOpen));
    }

    #[test]
    fn counts_propagate_through_a_strip() {
        // Two faces one above the other (sewn along a row); refining the upper's open row side
        // (along u) refines the lower's too, so that their seam keeps its points.
        let a = square(0.0, 0.0, 10.0, 4);
        let b = square(0.0, 10.0, 10.0, 4);
        let c = [Vec3::new(20.0, 20.0, 0.0), Vec3::new(30.0, 20.0, 0.0), Vec3::new(30.0, 30.0, 0.0), Vec3::new(20.0, 30.0, 0.0)];
        let far = surface(&net_over(c, 6), 6).unwrap();
        let faces = [a, b, far];
        assert_eq!(joins(&faces).unwrap().seams.len(), 1);
        let made = match_counts(&faces, (1, Side::LastRow), (2, Side::LastRow)).unwrap();
        assert_eq!(made[1].count()[0], 6);
        assert_eq!(made[0].count()[0], 6, "the neighbour along the strip follows");
        assert_eq!(joins(&made).unwrap().seams.len(), 1);
    }

    #[test]
    fn edge_chain_follows_the_edge_to_a_corner() {
        let faces: Vec<NurbsSurface> = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (10.0, 10.0)].iter().map(|&(x, y)| square(x, y, 10.0, 4)).collect();
        let j = joins(&faces).unwrap();
        let places = network_points(&faces, &j);
        let id = |x: f64, y: f64| places.iter().position(|p| p.dist(Vec3::new(x, y, 0.0)) < 1e-9);
        // A point on the bottom edge: the chain runs from the corner (0, 0) to (20, 0).
        let start = id(10.0, 0.0).unwrap();
        let chain = edge_chain(&j, &faces, start);
        let first = chain.first().copied().and_then(|p| places.get(p).copied()).unwrap();
        let last = chain.last().copied().and_then(|p| places.get(p).copied()).unwrap();
        let ends = [first, last];
        assert!(ends.iter().any(|p| p.dist(Vec3::ZERO) < 1e-9) && ends.iter().any(|p| p.dist(Vec3::new(20.0, 0.0, 0.0)) < 1e-9), "{ends:?}");
        assert!(chain.contains(&start));
        assert_eq!(chain.len(), 7, "two faces' 4 points sharing one");
        // An inside point is not on the edge.
        assert!(edge_chain(&j, &faces, id(10.0, 10.0).unwrap()).is_empty());
        assert!(edge_chain(&j, &faces, usize::MAX).is_empty());
    }

    #[test]
    fn loop_through_runs_along_a_row_or_a_column() {
        let faces = [square(0.0, 0.0, 10.0, 4)];
        let j = joins(&faces).unwrap();
        assert_eq!(loop_through(&j, &faces, 1, 3), Some(vec![1, 2, 3]));
        assert_eq!(loop_through(&j, &faces, 3, 1), Some(vec![3, 2, 1]));
        assert_eq!(loop_through(&j, &faces, 1, 13), Some(vec![1, 5, 9, 13]));
        assert_eq!(loop_through(&j, &faces, 0, 5), None, "a diagonal");
        assert_eq!(loop_through(&j, &faces, 2, 2), None);
        assert_eq!(loop_through(&j, &faces, 2, 99), None);
    }
}
