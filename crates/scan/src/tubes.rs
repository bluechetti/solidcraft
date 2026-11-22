//! Tubes It's reading: a part made of round tubes bent through space (an intake or exhaust
//! manifold's runners, a frame of bent tube). Find All reads such a tube as a run of tori (its
//! bends, each about its own axis) and cylinders (its straight stretches), all of one tube
//! radius, each running on from the one before where they meet. Unlike Pipe It's one flat path,
//! the part may hold several tubes, each bending in any direction: each is read as the path
//! through its pieces' axes, points along it a Pipe sweeps a circle along (through a 3D spline).

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::in_plane;
use crate::reconstruct::Face;

/// Pieces whose tube radii are this near, as a share of the radius, are of one tube: Find All
/// read the tori along one runner of a cast manifold at 21.3 to 23.4 mm.
const SAME_TUBE: f64 = 0.08;
/// Ends this near, as a share of the tube's radius, meet: where one shape stops and the next
/// starts on the scan is loose by about the tube's width.
const MEET: f64 = 1.5;
/// Where two pieces meet they run on the same way: their directions there within about 30°.
const RUNS_ON: f64 = 0.85;
/// A bend whose tube is no thinner than this share of its bend radius is no tube (a fillet's
/// torus, a boss's round).
const THIN: f64 = 0.95;
/// A bend going round nearly all the way is a ring (a flange's round), not a bend of a tube.
const MOST_TURN: f64 = 0.9 * TAU;
/// Points along a bend at most this far apart, in radians.
const STEP: f64 = 0.26;
/// A tube shorter than this many times its radius is no tube worth a Pipe.
const SHORTEST: f64 = 4.0;

/// A round tube bent through space.
#[derive(Clone, Debug, PartialEq)]
pub struct Tube {
    pub radius: f64,
    /// Points along its axis, end to end: a smooth curve through them is its path.
    pub path: Vec<Vec3>,
    /// How long that path is.
    pub length: f64,
    /// How many bends and straight stretches it was read from.
    pub bends: usize,
    pub straights: usize,
    /// The faces it is made of.
    pub faces: BTreeSet<usize>,
    /// Its path as a tube is bent: the corners of its centreline from one end to the other, and
    /// the radius its axis bends round at each corner between ([`Tube::path_pieces`]).
    pub corners: Vec<Vec3>,
    pub bend_radii: Vec<f64>,
}

/// A piece of a tube's path: straight, or an arc from its start through a point on it to its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathPiece {
    Line(Vec3, Vec3),
    Arc(Vec3, Vec3, Vec3),
}

/// The least a bend's radius may come down to where its corners crowd it, against the tube's own:
/// a bend tighter than the tube folds through itself and cannot be swept.
const LEAST_BEND: f64 = 1.02;

impl Tube {
    /// The path as lines and arcs: straight between its corners, round each corner on an arc of
    /// its bend radius tangent to both, so the path turns smoothly and never tighter than the bend
    /// (a curve through the points along its axis kinks where two pieces' fits meet, and a sweep
    /// along it folds through itself wherever it turns tighter than the tube). Where two corners
    /// leave too little straight between them for both arcs, both are made smaller; `None` when
    /// one would then bend tighter than [`LEAST_BEND`] of the tube's radius.
    pub fn path_pieces(&self) -> Option<Vec<PathPiece>> {
        let c = &self.corners;
        let n = c.len();
        if n < 2 || self.bend_radii.len() + 2 != n {
            return None;
        }
        // Each corner's turn and how far back along each side its arc starts.
        let turn = |k: usize| {
            let (a, b) = (unit(c[k] - c[k - 1]), unit(c[k + 1] - c[k]));
            a.dot(b).clamp(-1.0, 1.0).acos()
        };
        let back: Vec<f64> = (0..n).map(|k| if k == 0 || k + 1 == n { 0.0 } else { self.bend_radii[k - 1] * (0.5 * turn(k)).tan() }).collect();
        // Each side's share of what its two arcs ask for, and each corner the smaller of its two.
        let share: Vec<f64> = (0..n - 1)
            .map(|k| {
                let asked = back[k] + back[k + 1];
                if asked > 0.0 { (c[k].dist(c[k + 1]) / asked).min(1.0) } else { 1.0 }
            })
            .collect();
        let mut pieces = Vec::new();
        let mut at = c[0];
        for k in 1..n - 1 {
            let scale = share[k - 1].min(share[k]);
            let (radius, t) = (self.bend_radii[k - 1] * scale, back[k] * scale);
            let (a, b) = (unit(c[k] - c[k - 1]), unit(c[k + 1] - c[k]));
            if t <= 1e-9 * radius.max(1.0) {
                continue;
            }
            if !(radius >= LEAST_BEND * self.radius) || !(t.is_finite()) {
                return None;
            }
            let (from, to) = (c[k] - a * t, c[k] + b * t);
            let half = 0.5 * turn(k);
            let centre = c[k] + unit(b - a) * (radius / half.cos());
            let through = centre + unit(c[k] - centre) * radius;
            if at.dist(from) > 1e-9 {
                pieces.push(PathPiece::Line(at, from));
            }
            pieces.push(PathPiece::Arc(from, through, to));
            at = to;
        }
        if at.dist(c[n - 1]) > 1e-9 {
            pieces.push(PathPiece::Line(at, c[n - 1]));
        }
        Some(pieces)
    }
}

/// One piece as read: its face, its tube radius, its area, points along its axis from one end
/// to the other, and the way it runs at each end, pointing out of the piece.
struct Piece {
    face: usize,
    radius: f64,
    area: f64,
    bend: bool,
    points: Vec<Vec3>,
    out: [Vec3; 2],
    /// A bend's axis: its centre, the unit directions its circle is drawn with (`on` at angle `t`
    /// is `centre + (u cos t + v sin t) major`), its radius, and the angles it runs from and to.
    round: Option<Round>,
}

#[derive(Clone, Copy)]
struct Round {
    centre: Vec3,
    u: Vec3,
    v: Vec3,
    major: f64,
    start: f64,
    end: f64,
}

/// The most a bend turns between two corners of its path: a corner is where the lines tangent to
/// the bend's two ends cross, which runs off to far away as the turn nears a half.
const MOST_PER_CORNER: f64 = std::f64::consts::FRAC_PI_2;

impl Piece {
    fn end(&self, k: usize) -> Vec3 {
        if k == 0 { self.points.first().copied().unwrap_or(Vec3::ZERO) } else { self.points.last().copied().unwrap_or(Vec3::ZERO) }
    }
}

fn unit(v: Vec3) -> Vec3 {
    v.normalized().unwrap_or(Vec3::ZERO)
}

/// The tubes among `faces`: tori thin for their bend and cylinders `is_hole` does not call holes,
/// of one tube radius, chained where one's end meets the next's running on the same way; each
/// chain with a bend and at least two pieces, at least [`SHORTEST`] radii long. `span` gives how
/// far a face runs along a direction; `angles` how far round an axis (through a point) a face's
/// triangles go, counter-clockwise from `u` about the axis. Longest first.
pub fn read(
    faces: &[Face],
    is_hole: &dyn Fn(usize) -> bool,
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    angles: &dyn Fn(usize, Vec3, Vec3, Vec3) -> Option<(f64, f64)>,
) -> Vec<Tube> {
    let mut pieces: Vec<Piece> = Vec::new();
    for (k, face) in faces.iter().enumerate() {
        if face.area <= 0.0 {
            continue;
        }
        match face.shape {
            Shape::Torus { center, axis, major, minor } if minor > 0.0 && minor < THIN * major => {
                let axis = unit(axis);
                let (u, v) = in_plane(axis);
                let Some((start, end)) = angles(k, center, axis, u) else { continue };
                let turn = end - start;
                if !(turn > 0.0 && turn < MOST_TURN) {
                    continue;
                }
                let steps = (turn / STEP).ceil().max(1.0) as usize;
                let on = |t: f64| center + (u * t.cos() + v * t.sin()) * major;
                let along = |t: f64| v * t.cos() - u * t.sin();
                let points = (0..=steps).map(|i| on(start + turn * i as f64 / steps as f64)).collect();
                let round = Some(Round { centre: center, u, v, major, start, end });
                pieces.push(Piece { face: k, radius: minor, area: face.area, bend: true, points, out: [-along(start), along(end)], round });
            }
            Shape::Cylinder { point, axis, radius } if radius > 0.0 && !is_hole(k) => {
                let axis = unit(axis);
                let Some((a, b)) = span(k, axis) else { continue };
                if b - a <= 0.0 {
                    continue;
                }
                let at = |t: f64| point + axis * (t - point.dot(axis));
                pieces.push(Piece { face: k, radius, area: face.area, bend: false, points: vec![at(a), at(b)], out: [-axis, axis], round: None });
            }
            _ => {}
        }
    }
    // Each end's partner: the nearest end of another piece of the same tube that meets it running
    // on the same way, taken nearest first, each end once.
    let mut pairs: Vec<(f64, usize, usize, usize, usize)> = Vec::new();
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            let (a, b) = (&pieces[i], &pieces[j]);
            let r = 0.5 * (a.radius + b.radius);
            if (a.radius - b.radius).abs() > SAME_TUBE * r {
                continue;
            }
            for ea in 0..2 {
                for eb in 0..2 {
                    let d = a.end(ea).dist(b.end(eb));
                    // out of one is into the other: they point at each other
                    if d <= MEET * r && a.out[ea].dot(b.out[eb]) <= -RUNS_ON {
                        pairs.push((d, i, ea, j, eb));
                    }
                }
            }
        }
    }
    pairs.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut next: Vec<[Option<(usize, usize)>; 2]> = vec![[None, None]; pieces.len()];
    for &(_, i, ea, j, eb) in &pairs {
        if next[i][ea].is_none() && next[j][eb].is_none() {
            next[i][ea] = Some((j, eb));
            next[j][eb] = Some((i, ea));
        }
    }
    // Walk each chain from a free end.
    let mut used = vec![false; pieces.len()];
    let mut tubes = Vec::new();
    for first in 0..pieces.len() {
        if used[first] {
            continue;
        }
        let Some(from) = (0..2).find(|&e| next[first][e].is_none()) else { continue };
        // (piece, the end it is entered by)
        let mut chain: Vec<(usize, usize)> = vec![(first, from)];
        used[first] = true;
        while let Some(&(i, entered)) = chain.last() {
            match next[i][1 - entered] {
                Some((j, at)) if !used[j] => {
                    used[j] = true;
                    chain.push((j, at));
                }
                _ => break,
            }
        }
        if let Some(tube) = tube_of(&pieces, &chain) {
            tubes.push(tube);
        }
    }
    tubes.sort_by(|a, b| b.length.total_cmp(&a.length));
    tubes
}

/// The tube a chain of pieces makes (each entered by the end given): its path through their
/// points, the two points at each joint made one (halfway between), its radius the pieces' mean
/// by area. `None` without a bend, with one piece, or shorter than [`SHORTEST`] radii.
fn tube_of(pieces: &[Piece], chain: &[(usize, usize)]) -> Option<Tube> {
    let bends = chain.iter().filter(|(i, _)| pieces[*i].bend).count();
    if chain.len() < 2 || bends == 0 {
        return None;
    }
    let area: f64 = chain.iter().map(|(i, _)| pieces[*i].area).sum();
    let radius = chain.iter().map(|(i, _)| pieces[*i].radius * pieces[*i].area).sum::<f64>() / area.max(1e-300);
    let mut path: Vec<Vec3> = Vec::new();
    for &(i, entered) in chain {
        let mut points = pieces[i].points.clone();
        if entered == 1 {
            points.reverse();
        }
        match path.last_mut() {
            // a joint: the end of the piece before and this one's start made one
            Some(last) => {
                let mut rest = points.into_iter();
                if let Some(first) = rest.next() {
                    *last = (*last + first) * 0.5;
                }
                path.extend(rest);
            }
            None => path.extend(points),
        }
    }
    let path = forward(path);
    let length: f64 = path.windows(2).map(|w| w[0].dist(w[1])).sum();
    if length < SHORTEST * radius {
        return None;
    }
    // The corners: each bend's, in parts of at most a quarter turn, where the lines tangent to
    // the part's two ends cross, in the order the chain runs.
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else { return None };
    let (mut corners, mut bend_radii) = (vec![first], Vec::new());
    for &(i, entered) in chain {
        let Some(round) = pieces[i].round else { continue };
        let parts = ((round.end - round.start) / MOST_PER_CORNER).ceil().max(1.0) as usize;
        let step = (round.end - round.start) / parts as f64;
        let mut at: Vec<Vec3> = (0..parts)
            .map(|j| {
                let middle = round.start + step * (j as f64 + 0.5);
                round.centre + (round.u * middle.cos() + round.v * middle.sin()) * (round.major / (0.5 * step).cos())
            })
            .collect();
        if entered == 1 {
            at.reverse();
        }
        for corner in at {
            corners.push(corner);
            bend_radii.push(round.major);
        }
    }
    corners.push(last);
    let faces = chain.iter().map(|(i, _)| pieces[*i].face).collect();
    Some(Tube { radius, path, length, bends, straights: chain.len() - bends, faces, corners, bend_radii })
}

/// `path` with no step back: where neighbouring pieces' ends overlap on the scan (one bend's
/// fitted extent running past where the next starts), the points that turn the path back on
/// itself (more than square from the step before) are dropped, both ends kept.
fn forward(mut path: Vec<Vec3>) -> Vec<Vec3> {
    let mut k = 1;
    while k + 1 < path.len() {
        if (path[k] - path[k - 1]).dot(path[k + 1] - path[k]) < 0.0 {
            path.remove(k);
            k = k.saturating_sub(1).max(1);
        } else {
            k += 1;
        }
    }
    path
}

/// Inside a bend's angles this wide a gap with no triangle says the face is pieces of several
/// tubes on one torus, not one bend (radians, 20°).
const GAP: f64 = 0.35;

/// The angles round an axis through `centre` (unit `axis`, `u` square to it) a bend's `points`
/// start and end at, counter-clockwise across the widest gap between them: as
/// [`crate::pipe::angular_extent`], but `None` where a second gap inside is wider than [`GAP`].
pub fn bend_extent(points: impl Iterator<Item = Vec3>, centre: Vec3, axis: Vec3, u: Vec3) -> Option<(f64, f64)> {
    let v = axis.cross(u);
    let mut angles: Vec<f64> = points.map(|p| p - centre).map(|d| d.dot(v).atan2(d.dot(u))).collect();
    if angles.len() < 2 {
        return None;
    }
    angles.sort_by(f64::total_cmp);
    let n = angles.len();
    let mut gaps: Vec<f64> = (1..n).map(|k| angles[k] - angles[k - 1]).collect();
    gaps.push(angles[0] + TAU - angles[n - 1]);
    gaps.sort_by(|a, b| b.total_cmp(a));
    if gaps.get(1).is_some_and(|&g| g > GAP) {
        return None;
    }
    crate::pipe::angular_extent(angles.iter().map(|&t| centre + u * t.cos() + v * t.sin()), centre, axis, u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::angular_extent;

    /// A tube R 10 along X from (0, 0, 0) to (50, 0, 0), a quarter bend R 30 about (50, 0, 30) up
    /// to (80, 0, 30), then a quarter bend R 40 about (80, 40, 30) over to (80, 40, 70): two
    /// bends in two planes, one tube. A thinner tube apart is another.
    #[test]
    fn a_tube_bent_in_two_planes_is_one_tube_and_a_thinner_one_beside_it_another() {
        let quarter = std::f64::consts::FRAC_PI_2;
        let faces = vec![
            Face { shape: Shape::Cylinder { point: Vec3::new(25.0, 0.0, 0.0), axis: Vec3::X, radius: 10.0 }, area: 3000.0 },
            Face { shape: Shape::Torus { center: Vec3::new(50.0, 0.0, 30.0), axis: Vec3::Y, major: 30.0, minor: 10.2 }, area: 2900.0 },
            Face { shape: Shape::Torus { center: Vec3::new(80.0, 40.0, 30.0), axis: Vec3::X, major: 40.0, minor: 9.9 }, area: 3900.0 },
            // a thinner tube elsewhere: a bend and a straight
            Face { shape: Shape::Torus { center: Vec3::new(0.0, 200.0, 0.0), axis: Vec3::Z, major: 25.0, minor: 4.0 }, area: 900.0 },
            Face { shape: Shape::Cylinder { point: Vec3::new(-30.0, 175.0, 0.0), axis: Vec3::X, radius: 4.0 }, area: 1000.0 },
        ];
        let arc = |centre: Vec3, from: Vec3, to: Vec3, n: usize| -> Vec<Vec3> {
            // a quarter from `from` to `to` about `centre` (both at one radius, square to each other)
            (0..=n)
                .map(|i| {
                    let t = quarter * i as f64 / n as f64;
                    centre + (from - centre) * t.cos() + (to - centre) * t.sin()
                })
                .collect()
        };
        let bend = |k: usize| -> Vec<Vec3> {
            match k {
                1 => arc(Vec3::new(50.0, 0.0, 30.0), Vec3::new(50.0, 0.0, 0.0), Vec3::new(80.0, 0.0, 30.0), 20),
                2 => arc(Vec3::new(80.0, 40.0, 30.0), Vec3::new(80.0, 0.0, 30.0), Vec3::new(80.0, 40.0, 70.0), 20),
                _ => arc(Vec3::new(0.0, 200.0, 0.0), Vec3::new(0.0, 175.0, 0.0), Vec3::new(25.0, 200.0, 0.0), 20),
            }
        };
        let spans = |k: usize, d: Vec3| -> Option<(f64, f64)> {
            let (a, b) = match k {
                0 => (Vec3::new(0.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)),
                // the thin one's straight runs on into its bend at (0, 175, 0)
                4 => (Vec3::new(0.0, 175.0, 0.0), Vec3::new(-60.0, 175.0, 0.0)),
                _ => return None,
            };
            let (x, y) = (a.dot(d), b.dot(d));
            Some((x.min(y), x.max(y)))
        };
        let angles = |k: usize, centre: Vec3, axis: Vec3, u: Vec3| angular_extent(bend(k).into_iter(), centre, axis, u);
        let tubes = read(&faces, &|_| false, &spans, &angles);
        assert_eq!(tubes.len(), 2, "{tubes:?}");
        let big = &tubes[0];
        assert_eq!(big.faces, BTreeSet::from([0, 1, 2]), "{big:?}");
        assert_eq!((big.bends, big.straights), (2, 1));
        assert!((big.radius - 10.0).abs() < 0.2, "{big:?}");
        // From one end of the path to the other: (0,0,0) and (80,40,70), either way round.
        let (a, b) = (big.path[0], *big.path.last().unwrap());
        let ends = [Vec3::ZERO, Vec3::new(80.0, 40.0, 70.0)];
        assert!(ends.iter().any(|e| e.dist(a) < 1e-6) && ends.iter().any(|e| e.dist(b) < 1e-6), "{a:?} {b:?}");
        // 50 straight, then quarters of R 30 and R 40.
        let length = 50.0 + quarter * 30.0 + quarter * 40.0;
        assert!((big.length - length).abs() < 0.01 * length, "{} vs {length}", big.length);
        // As a tube is bent: straight 50, a quarter of R 30, a quarter of R 40, every arc on its
        // bend's radius and the path running on smoothly.
        let pieces = big.path_pieces().expect("a path that sweeps");
        let arcs: Vec<(Vec3, Vec3, Vec3)> =
            pieces.iter().filter_map(|p| if let PathPiece::Arc(a, m, b) = *p { Some((a, m, b)) } else { None }).collect();
        assert_eq!(arcs.len(), 2, "{pieces:?}");
        let radius_of = |(a, m, b): (Vec3, Vec3, Vec3)| {
            // the circle through three points: its radius
            let (x, y) = (a - m, b - m);
            x.len() * y.len() * (x - y).len() / (2.0 * x.cross(y).len())
        };
        let found: Vec<f64> = arcs.iter().map(|&a| radius_of(a)).collect();
        assert!((found[0] - 30.0).abs() < 0.3 && (found[1] - 40.0).abs() < 0.4, "{found:?}");
        let swept: f64 = pieces
            .iter()
            .map(|p| match *p {
                PathPiece::Line(a, b) => a.dist(b),
                PathPiece::Arc(a, m, b) => {
                    let r = radius_of((a, m, b));
                    2.0 * r * (0.5 * a.dist(b) / r).asin()
                }
            })
            .sum();
        assert!((swept - length).abs() < 0.01 * length, "{swept} vs {length}");
        // Corners crowded so that a bend would turn tighter than the tube: no path.
        let crowded = Tube {
            corners: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(20.0, 10.0, 0.0)],
            bend_radii: vec![30.0, 30.0],
            ..big.clone()
        };
        assert!(crowded.path_pieces().is_none());
        let thin = &tubes[1];
        assert_eq!(thin.faces, BTreeSet::from([3, 4]), "{thin:?}");
        // A ring all the way round is no bend of a tube.
        let ring = [Face { shape: Shape::Torus { center: Vec3::ZERO, axis: Vec3::Z, major: 30.0, minor: 5.0 }, area: 6000.0 }];
        let round = |_: usize, centre: Vec3, axis: Vec3, u: Vec3| {
            angular_extent((0..72).map(|i| Vec3::new(30.0 * (i as f64 * 0.0873).cos(), 30.0 * (i as f64 * 0.0873).sin(), 0.0)), centre, axis, u)
        };
        assert!(read(&ring, &|_| false, &spans, &round).is_empty());
        // A path stepping back where two pieces overlap runs forward only.
        let back = vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0), Vec3::new(15.0, 1.0, 0.0), Vec3::new(30.0, 2.0, 0.0)];
        assert_eq!(forward(back).len(), 4);
        // A torus face in two pieces 90° apart (two runners on one fitted torus) is no bend.
        let two = (0..10).map(|i| i as f64 * 0.05).chain((0..10).map(|i| 2.0 + i as f64 * 0.05));
        let points: Vec<Vec3> = two.map(|t| Vec3::new(30.0 * t.cos(), 30.0 * t.sin(), 0.0)).collect();
        let (u, _) = in_plane(Vec3::Z);
        assert!(bend_extent(points.iter().copied(), Vec3::ZERO, Vec3::Z, u).is_none());
        assert!(bend_extent(points[..10].iter().copied(), Vec3::ZERO, Vec3::Z, u).is_some());
    }
}
