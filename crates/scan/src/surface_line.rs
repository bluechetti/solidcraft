//! A line measured on the scan: dots on it, and between each two the way across the scan the cut
//! of a plane through both takes, so the length read is the length along the surface, not
//! straight through the air. The plane through two dots also holds the direction the line is
//! looked at from (the view it was drawn in), so the stretch is what was drawn on the screen; with
//! no view, the way the scan faces at the two dots, so on a flat face the stretch is straight and
//! round a cylinder it follows the round.
//!
//! The cut of a closed piece goes two ways from one dot to the other: the stretch is the way that
//! stays between the two dots seen along the line joining them (the other runs out past an end,
//! round the back), then the shorter. Where no cut joins the two dots (a hole between them, two
//! pieces of the scan), the stretch is draped instead: the straight line between them sampled and
//! each sample taken to the scan, and it is counted as not followed, so the reading says where it
//! did not follow the surface.
//!
//! The reading: the length along the surface; the length straight between the dots (what the dots
//! alone would read) and how much they under-read by; how far the surface strays from the straight
//! lines between the dots (the worst and the root mean square); the straight distance between the
//! ends and its parts; and, where the line is an arc (a fillet, a bore read across), its radius
//! and how far it goes round.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::curve::Projector;

/// The most dots a line takes.
pub const MOST_DOTS: usize = 1000;
/// The most samples a draped stretch takes.
const MOST_SAMPLES: usize = 10_000;

/// Why a line could not be measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineError {
    /// Fewer than two dots, or more than [`MOST_DOTS`].
    Count,
    /// A dot is nowhere near the scan.
    OffTheScan { dot: usize },
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineError::Count => write!(f, "a line has 2 to {MOST_DOTS} dots on the scan"),
            LineError::OffTheScan { dot } => write!(f, "dot {} is not on the scan", dot + 1),
        }
    }
}

/// The way across the scan between two neighbouring dots.
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    /// From the one dot to the other, both included.
    pub path: Vec<Vec3>,
    /// It follows the scan's cut; false: draped across a hole or between pieces.
    pub followed: bool,
}

impl Stretch {
    pub fn length(&self) -> f64 {
        self.path.windows(2).map(|w| w[0].dist(w[1])).sum()
    }
}

/// The line as an arc: the circle it lies on and how far round it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Round {
    pub radius: f64,
    pub centre: Vec3,
    /// How far round the circle the line goes, in degrees.
    pub turned: f64,
    /// The most the line is off that circle, mm.
    pub within: f64,
}

/// A line measured on the scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The dots, each on the scan.
    pub dots: Vec<Vec3>,
    pub stretches: Vec<Stretch>,
    /// Along the surface, from the first dot to the last.
    pub along: f64,
    /// Straight from dot to dot.
    pub straight: f64,
    /// The most and the root mean square the surface is off the straight lines between the dots.
    pub worst_gap: f64,
    pub rms_gap: f64,
    /// From the first dot to the last, straight, and its parts.
    pub ends: f64,
    pub delta: Vec3,
    /// Stretches that could not follow the surface (draped).
    pub not_followed: usize,
    /// The circle the line goes round, when it is an arc.
    pub round: Option<Round>,
}

/// The line through `dots` measured on the scan: each dot taken to the scan, each stretch the cut
/// of the plane through its two dots and `view` (the direction looked from), or the way the scan
/// faces there when no view is given.
pub fn measure(scan: &Scan, on: &Projector, dots: &[Vec3], view: Option<Vec3>) -> Result<Line, LineError> {
    if dots.len() < 2 || dots.len() > MOST_DOTS {
        return Err(LineError::Count);
    }
    let reach = 3.0 * on.edge.max(1e-9);
    let mut placed = Vec::with_capacity(dots.len());
    for (i, d) in dots.iter().enumerate() {
        match on.nearest(*d) {
            Some((q, off)) if d.is_finite() && off <= reach.max(1e-6 * (1.0 + d.len())) => placed.push(q),
            _ => return Err(LineError::OffTheScan { dot: i }),
        }
    }
    let weld = crate::welded(scan.positions);
    let view = view.and_then(|v| v.normalized());
    let stretches: Vec<Stretch> = placed.windows(2).map(|w| stretch(scan, &weld, on, w[0], w[1], view, reach)).collect();
    Ok(reading(placed, stretches))
}

/// The numbers of a line from its dots and stretches.
fn reading(dots: Vec<Vec3>, stretches: Vec<Stretch>) -> Line {
    let along = stretches.iter().map(Stretch::length).sum();
    let straight = dots.windows(2).map(|w| w[0].dist(w[1])).sum();
    let (mut worst, mut squares, mut count) = (0.0f64, 0.0f64, 0usize);
    for (s, w) in stretches.iter().zip(dots.windows(2)) {
        for p in &s.path {
            let gap = off_segment(*p, w[0], w[1]);
            worst = worst.max(gap);
            squares += gap * gap;
            count += 1;
        }
    }
    let (first, last) = (dots[0], dots[dots.len() - 1]);
    let path: Vec<Vec3> = stretches.iter().flat_map(|s| s.path.iter().copied()).collect();
    Line {
        round: round(&path),
        not_followed: stretches.iter().filter(|s| !s.followed).count(),
        along,
        straight,
        worst_gap: worst,
        rms_gap: if count > 0 { (squares / count as f64).sqrt() } else { 0.0 },
        ends: first.dist(last),
        delta: last - first,
        stretches,
        dots,
    }
}

/// How far `p` is from the segment `a`–`b`.
fn off_segment(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// The plane's `up`: the view, else the way the scan faces at the two dots, else any direction
/// square to the chord.
fn up_of(on: &Projector, a: Vec3, b: Vec3, view: Option<Vec3>) -> Vec3 {
    let chord = (b - a).normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let square = |v: Vec3| (v - chord * v.dot(chord)).normalized();
    let faces = || {
        let (na, nb) = (on.normal(a)?, on.normal(b)?);
        // The two summed with the sign that leaves more of them square to the chord: faces facing
        // apart (either side of a thin wall) still give a plane through both.
        let part = |v: Vec3| (v - chord * v.dot(chord)).len();
        square(if part(na + nb) >= part(na - nb) { na + nb } else { na - nb })
    };
    view.and_then(square)
        .or_else(faces)
        .or_else(|| square(Vec3::new(0.0, 0.0, 1.0)))
        .or_else(|| square(Vec3::new(0.0, 1.0, 0.0)))
        .unwrap_or(Vec3::new(0.0, 1.0, 0.0))
}

/// The stretch from `a` to `b`: the cut, else draped.
fn stretch(scan: &Scan, weld: &[u32], on: &Projector, a: Vec3, b: Vec3, view: Option<Vec3>, reach: f64) -> Stretch {
    let length = a.dist(b);
    if !(length > 0.0) {
        return Stretch { path: vec![a, b], followed: true };
    }
    let up = up_of(on, a, b, view);
    let followed = Plane::new(a, (b - a) / length, up).and_then(|plane| cut_between(scan, weld, &plane, length, reach));
    match followed {
        Some(mut path) => {
            // The ends exactly on the dots.
            if let Some(p) = path.first_mut() {
                *p = a;
            }
            if let Some(p) = path.last_mut() {
                *p = b;
            }
            Stretch { path, followed: true }
        }
        None => Stretch { path: draped(on, a, b, up, reach), followed: false },
    }
}

/// The straight line `a`–`b` sampled about every half `reach` and each sample taken to the scan
/// along `up` (else to its nearest place).
fn draped(on: &Projector, a: Vec3, b: Vec3, up: Vec3, reach: f64) -> Vec<Vec3> {
    let n = ((a.dist(b) / (0.5 * reach)).ceil() as usize).clamp(1, MOST_SAMPLES);
    (0..=n)
        .map(|i| {
            let s = a + (b - a) * (i as f64 / n as f64);
            if i == 0 || i == n {
                return s;
            }
            on.along(s, up).or_else(|| on.nearest(s)).map_or(s, |(q, _)| q)
        })
        .collect()
}

/// Where along a run (as the index of a segment plus the share of it) the run comes nearest `q`,
/// and how near.
fn nearest_on(points: &[Vec2], closed: bool, q: Vec2) -> Option<(f64, f64)> {
    let n = points.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    let mut best: Option<(f64, f64)> = None;
    for i in 0..segments {
        let (p, r) = (points[i], points[(i + 1) % n]);
        let d = r - p;
        let l2 = d.x * d.x + d.y * d.y;
        let t = if l2 > 0.0 { (((q.x - p.x) * d.x + (q.y - p.y) * d.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let at = Vec2::new(p.x + d.x * t, p.y + d.y * t);
        let off = ((q.x - at.x).powi(2) + (q.y - at.y).powi(2)).sqrt();
        if best.is_none_or(|(o, _)| off < o) {
            best = Some((off, i as f64 + t));
        }
    }
    if n == 1 {
        let off = ((q.x - points[0].x).powi(2) + (q.y - points[0].y).powi(2)).sqrt();
        return Some((off, 0.0));
    }
    // The end of a closed run's last segment is its first point.
    best.map(|(off, s)| (off, if closed && s >= n as f64 { s - n as f64 } else { s }))
}

/// The point of a run at position `s` (a segment's index plus the share of it).
fn point_at(points: &[Vec2], s: f64) -> Vec2 {
    let n = points.len();
    let i = (s.floor() as usize).min(n.saturating_sub(1));
    let t = s - i as f64;
    let (p, r) = (points[i], points[(i + 1) % n]);
    Vec2::new(p.x + (r.x - p.x) * t, p.y + (r.y - p.y) * t)
}

/// The run's points from position `from` to `to`, going forward (`step` 1) or back (`step` -1),
/// round the end of a closed run: the two ends and every point of the run between.
fn walk(points: &[Vec2], from: f64, to: f64, forward: bool) -> Vec<Vec2> {
    let n = points.len();
    let mut out = vec![point_at(points, from)];
    if forward {
        // Points with index after `from` up to and including the one before `to`.
        let (start, end) = (from.floor() as usize + 1, to.floor() as usize);
        let count = if to >= from { end + 1 - start.min(end + 1) } else { (n - start % n) % n + end + 1 };
        for k in 0..count.min(n) {
            out.push(points[(start + k) % n]);
        }
    } else {
        // Points with index at or before `from`, back to the one after `to`.
        let (start, end) = (from.floor() as usize, to.floor() as usize + 1);
        let count = if from >= to { (start + 1).saturating_sub(end) } else { start + 1 + n - end.min(n) };
        for k in 0..count.min(n) {
            out.push(points[(start + n - k) % n]);
        }
    }
    out.push(point_at(points, to));
    out
}

/// The way along the plane's cut from its origin (the first dot) to `(length, 0)` (the second),
/// in the world: `None` when no run of the cut passes within `reach` of both.
fn cut_between(scan: &Scan, weld: &[u32], plane: &Plane, length: f64, reach: f64) -> Option<Vec<Vec3>> {
    let (a, b) = (Vec2::new(0.0, 0.0), Vec2::new(length, 0.0));
    let runs = crate::section::section_welded(scan, weld, plane);
    let (run, sa, sb) = runs
        .iter()
        .filter(|r| r.points.len() >= 2)
        .filter_map(|r| {
            let (da, sa) = nearest_on(&r.points, r.closed, a)?;
            let (db, sb) = nearest_on(&r.points, r.closed, b)?;
            (da <= reach && db <= reach).then_some((r, sa, sb, da.max(db)))
        })
        .min_by(|x, y| x.3.total_cmp(&y.3))
        .map(|(r, sa, sb, _)| (r, sa, sb))?;
    let points = &run.points;
    let between = |way: &[Vec2]| way.iter().filter(|p| p.x >= -reach && p.x <= length + reach).count() as f64 / way.len().max(1) as f64;
    let long = |way: &[Vec2]| way.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt()).sum::<f64>();
    let way = if run.closed {
        let (fwd, back) = (walk(points, sa, sb, true), walk(points, sa, sb, false));
        let (bf, bb) = (between(&fwd), between(&back));
        if bf > bb + 1e-9 || ((bf - bb).abs() <= 1e-9 && long(&fwd) <= long(&back)) { fwd } else { back }
    } else {
        walk(points, sa, sb, sb >= sa)
    };
    Some(way.into_iter().map(|p| plane.to_world(p)).collect())
}

/// The circle the path goes round, when it is an arc: within 1.5 % of its radius of it, within 2 %
/// of its radius of the plane through it, and round at least 30°.
fn round(path: &[Vec3]) -> Option<Round> {
    if path.len() < 5 {
        return None;
    }
    let centroid = path.iter().fold(Vec3::ZERO, |s, p| s + *p) / path.len() as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in path {
        let d = *p - centroid;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    // Smallest spread first: the plane's normal, then its two directions.
    let (_, v) = crate::constrain::eigen3(spread);
    let axes = [Vec3::new(v[2][0], v[2][1], v[2][2]), Vec3::new(v[1][0], v[1][1], v[1][2]), Vec3::new(v[0][0], v[0][1], v[0][2])];
    if !axes.iter().all(|a| a.is_finite()) {
        return None;
    }
    let flat = path.iter().map(|p| (*p - centroid).dot(axes[2]).abs()).fold(0.0, f64::max);
    let local: Vec<[f64; 2]> = path.iter().map(|p| [(*p - centroid).dot(axes[0]), (*p - centroid).dot(axes[1])]).collect();
    let (c, radius) = crate::fit::kasa(local.iter().map(|q| (*q, 1.0)))?;
    let within = local.iter().map(|q| (((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2)).sqrt() - radius).abs()).fold(0.0, f64::max);
    // How far round, the angle summed step by step (never more than once round for one step).
    let angle = |q: &[f64; 2]| (q[1] - c[1]).atan2(q[0] - c[0]);
    let mut turned = 0.0f64;
    for w in local.windows(2) {
        let mut d = angle(&w[1]) - angle(&w[0]);
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        turned += d;
    }
    let turned = turned.abs().to_degrees();
    let centre = centroid + axes[0] * c[0] + axes[1] * c[1];
    (within <= 0.015 * radius && flat <= 0.02 * radius && turned >= 30.0).then_some(Round { radius, centre, turned, within })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of `nu` × `nv` cells, `at(i, j)` its points, wrapped round in `u` when `round_u`;
    /// the cell (`skip`) left out.
    fn grid(nu: usize, nv: usize, round_u: bool, at: impl Fn(usize, usize) -> Vec3, skip: Option<(usize, usize)>) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let cols = if round_u { nu } else { nu + 1 };
        let mut p = Vec::new();
        for j in 0..=nv {
            for i in 0..cols {
                p.push(at(i, j));
            }
        }
        let idx = |i: usize, j: usize| (j * cols + i % cols) as u32;
        let mut t = Vec::new();
        for j in 0..nv {
            for i in 0..nu {
                if skip == Some((i, j)) {
                    continue;
                }
                t.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
                t.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
            }
        }
        (p, t)
    }

    /// On a flat sheet the line is the straight one, followed, reading what the dots read.
    #[test]
    fn on_a_flat_sheet_the_length_is_the_straight_one() {
        let (p, t) = grid(20, 20, false, |i, j| Vec3::new(i as f64, j as f64, 0.0), None);
        let scan = Scan::new(&p, &t);
        let on = Projector::new(&scan);
        let line = measure(&scan, &on, &[Vec3::new(2.3, 3.1, 0.0), Vec3::new(17.2, 11.9, 0.0), Vec3::new(5.0, 18.0, 0.0)], None).unwrap();
        assert_eq!(line.not_followed, 0);
        assert!((line.along - line.straight).abs() < 1e-9, "{} {}", line.along, line.straight);
        assert!(line.worst_gap < 1e-9 && line.round.is_none());
        assert!((line.ends - Vec3::new(2.3, 3.1, 0.0).dist(Vec3::new(5.0, 18.0, 0.0))).abs() < 1e-9);
    }

    /// Round a cylinder of radius 10 the quarter turn is the arc (the mesh's polygon of 128 sides
    /// is 0.01 % short of the true arc), read as a round of radius 10 turning 90°.
    #[test]
    fn round_a_cylinder_the_line_is_the_arc() {
        let n = 128;
        let at = |i: usize, j: usize| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            Vec3::new(10.0 * a.cos(), 10.0 * a.sin(), j as f64)
        };
        let (p, t) = grid(n, 20, true, at, None);
        let scan = Scan::new(&p, &t);
        let on = Projector::new(&scan);
        let line = measure(&scan, &on, &[Vec3::new(10.0, 0.0, 10.0), Vec3::new(0.0, 10.0, 10.0)], None).unwrap();
        let arc = 10.0 * std::f64::consts::FRAC_PI_2;
        assert_eq!(line.not_followed, 0);
        assert!((line.along - arc).abs() < 0.002 * arc, "{} against {arc}", line.along);
        assert!((line.straight - 200.0f64.sqrt()).abs() < 1e-9);
        let round = line.round.expect("an arc");
        assert!((round.radius - 10.0).abs() < 0.05 && (round.turned - 90.0).abs() < 1.0, "{round:?}");
        // Looked at from the side, square to the axis, the same two dots give the same arc: the
        // short way, not round the back (three quarters of the circle).
        let dots = [Vec3::new(10.0, 0.0, 10.0), Vec3::new(0.0, 10.0, 10.0)];
        let seen = measure(&scan, &on, &dots, Some(Vec3::new(-1.0, -1.0, 0.0))).unwrap();
        assert!((seen.along - line.along).abs() < 1e-9, "{}", seen.along);
        // Looked at down the axis, the line drawn between them on the screen crosses the inside,
        // where there is no surface: draped, and said so.
        let down = measure(&scan, &on, &dots, Some(Vec3::new(0.0, 0.0, -1.0))).unwrap();
        assert_eq!(down.not_followed, 1);
    }

    /// Over the edge of a box the line is the two straight runs to the edge and down from it, the
    /// way between the dots, not round the far side.
    #[test]
    fn over_an_edge_the_line_goes_by_the_edge() {
        // The top (z = 20) and the side x = 20 of a box 20 mm across, as one folded sheet.
        let n = 20;
        let at = |i: usize, j: usize| {
            let (u, y) = (i as f64 * 2.0, j as f64);
            if u <= 20.0 { Vec3::new(u, y, 20.0) } else { Vec3::new(20.0, y, 40.0 - u) }
        };
        let (p, t) = grid(n, 20, false, at, None);
        let scan = Scan::new(&p, &t);
        let on = Projector::new(&scan);
        let line = measure(&scan, &on, &[Vec3::new(5.0, 10.0, 20.0), Vec3::new(20.0, 10.0, 5.0)], None).unwrap();
        assert_eq!(line.not_followed, 0);
        assert!((line.along - 30.0).abs() < 1e-9, "{}", line.along);
        assert!((line.worst_gap - 15.0 / 2.0f64.sqrt()).abs() < 1e-9, "the edge is the farthest off the straight line: {}", line.worst_gap);
    }

    /// Across a hole there is no way along the surface: the stretch is draped and said so.
    #[test]
    fn across_a_hole_the_stretch_is_draped_and_counted() {
        let (p, t) = grid(20, 20, false, |i, j| Vec3::new(i as f64, j as f64, 0.0), Some((10, 10)));
        let scan = Scan::new(&p, &t);
        let on = Projector::new(&scan);
        let line = measure(&scan, &on, &[Vec3::new(10.5, 2.0, 0.0), Vec3::new(10.5, 18.0, 0.0)], None).unwrap();
        assert_eq!(line.not_followed, 1);
        assert!((line.along - 16.0).abs() < 1e-6, "draped flat: {}", line.along);
    }

    #[test]
    fn too_few_dots_or_one_off_the_scan_is_refused() {
        let (p, t) = grid(4, 4, false, |i, j| Vec3::new(i as f64, j as f64, 0.0), None);
        let scan = Scan::new(&p, &t);
        let on = Projector::new(&scan);
        assert_eq!(measure(&scan, &on, &[Vec3::new(1.0, 1.0, 0.0)], None), Err(LineError::Count));
        assert_eq!(measure(&scan, &on, &[Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 50.0)], None), Err(LineError::OffTheScan { dot: 1 }));
    }
}
