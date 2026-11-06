//! The profiles Reconstruct sketches, read off the faces Find All fitted rather than traced off
//! the scan, after SurfaceCraft's: so each curve of a profile is a face exactly as fitted, and two
//! features sharing a face put it in one place.
//!
//! - **A prism's profile** ([`prism_outlines`]): the scan is sliced across the prism and each loop
//!   of the slice walked. Where it runs on one of the prism's walls, that wall is a curve of the
//!   profile (a plane along the prism a line, a cylinder along it a circle), and the walls follow
//!   each other round as the loop meets them. Where the loop leaves the walls (a feature that runs
//!   another way, a fillet across the prism, noise) the neighbouring walls run on to where they
//!   cross, or, parallel, are joined straight across. A loop on fewer than three walls (nor round
//!   one circle) is another feature's, and is left out.
//! - **A turned part's profile** ([`turned_outline`]): each face about the axis, held to it
//!   ([`held`]), is a curve in the half-section (height along the axis, radius from it): a
//!   cylinder a line along the axis, a plane square to it a line across, a cone a slanted line, a
//!   torus or a ball on the axis a circle. Faces that meet on the scan follow each other round the
//!   profile, the corners where their curves cross nearest where the scan has them meet; a face
//!   that comes down to the axis meets the axis itself, which closes the profile there.

use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::fit::Shape;
use crate::reconstruct::{Axis, Face, Prism, Reading, Revolve};
use crate::segment::Outcome;

/// A piece of an outline, from its corner to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line,
    /// An arc about `center`, counter-clockwise from this corner to the next or (`ccw` false) the
    /// other way.
    Arc {
        center: Vec2,
        ccw: bool,
    },
}

/// A closed outline: piece `k` runs from corner `k` to corner `k + 1`, the last back to the
/// first; `faces[k]` is the face piece `k` lies on, `None` for a piece no face holds (straight
/// across a gap, or a turned part's axis).
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub corners: Vec<Vec2>,
    pub pieces: Vec<Piece>,
    pub faces: Vec<Option<usize>>,
}

fn unit2(v: Vec2) -> Vec2 {
    v.normalized().unwrap_or(Vec2::ZERO)
}

/// The signed angle from `a` to `b`.
fn angle_to(a: Vec2, b: Vec2) -> f64 {
    a.cross(b).atan2(a.dot(b))
}

/// `v` turned `angle` radians counter-clockwise.
fn rotated(v: Vec2, angle: f64) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

// ---------------------------------------------------------------------------------------------
// A prism's profile.

/// How far from square to the prism a plane's normal may be (a sine), or from along it a
/// cylinder's axis, to be a wall along it.
const ALONG: f64 = 0.035;
/// The sine of the least angle between two walls that cross at a corner: nearer parallel, they
/// are joined straight across.
const CROSSING: f64 = 0.035;
/// A line and a circle this near touching, as a share of the radius, touch.
const TOUCHING: f64 = 0.05;

/// A wall of the prism in the slice: a line through `point` along the unit `direction`, or a
/// circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wall {
    Line { point: Vec2, direction: Vec2 },
    Circle { center: Vec2, radius: f64 },
}

impl Wall {
    /// The point of the wall nearest `p`.
    fn foot(&self, p: Vec2) -> Vec2 {
        match *self {
            Wall::Line { point, direction } => point + direction * (p - point).dot(direction),
            Wall::Circle { center, radius } => center + unit2(p - center) * radius,
        }
    }
}

/// The wall `shape` is in the slice `plane` (whose normal runs along the prism), when it runs
/// along the prism: a plane whose normal is square to it, or a cylinder along it.
pub fn wall_of(shape: &Shape, plane: &Plane) -> Option<Wall> {
    let d = plane.normal();
    match *shape {
        Shape::Plane { point, normal } => {
            let n = normal.normalized()?;
            if n.dot(d).abs() > ALONG {
                return None;
            }
            let across = Vec2::new(n.dot(plane.x), n.dot(plane.y));
            let flat = across.normalized()?;
            // The slice's points on the plane: n · (origin + x u + y v − point) = 0.
            let at = n.dot(point - plane.origin) / across.len();
            Some(Wall::Line { point: flat * at, direction: flat.perp() })
        }
        Shape::Cylinder { point, axis, radius } => {
            let a = axis.normalized()?;
            if a.cross(d).len() > ALONG {
                return None;
            }
            let along = (plane.origin - point).dot(d) / a.dot(d);
            Some(Wall::Circle { center: plane.to_local(point + a * along), radius })
        }
        _ => None,
    }
}

/// The outlines the `walls` (by face index, the wall) give from the slice's closed `loops`, each
/// with its points and which wall (its place in `walls`) each point is on: one outline for each
/// loop that runs on three walls or more, or round one circle. A stretch on a wall shorter than
/// `reach` is passed over. An outline that would cross itself is left out, and of two that cross
/// each other the smaller; a loop the other way from the largest is a hole, kept only inside an
/// outline of the prism's (one outside them all is in what another prism makes).
pub fn prism_outlines(walls: &[(usize, Wall)], loops: &[(Vec<Vec2>, Vec<Option<usize>>)], reach: f64) -> Vec<Outline> {
    let mut found: Vec<(Outline, Vec<Vec2>)> = loops
        .iter()
        .filter_map(|(points, on)| outline_of(walls, points, on, reach))
        .map(|o| {
            let r = ring(&o);
            (o, r)
        })
        .filter(|(_, r)| is_simple_ring(r))
        .collect();
    found.sort_by(|a, b| area(&b.1).abs().total_cmp(&area(&a.1).abs()));
    let mut kept: Vec<(Outline, Vec<Vec2>)> = Vec::new();
    for (o, r) in found {
        if kept.iter().all(|(_, k)| !rings_cross(k, &r)) {
            kept.push((o, r));
        }
    }
    let material = kept.first().map_or(1.0, |(_, r)| area(r).signum());
    let outer: Vec<Vec<Vec2>> = kept.iter().filter(|(_, r)| area(r).signum() == material).map(|(_, r)| r.clone()).collect();
    kept.into_iter()
        .filter(|(_, r)| area(r).signum() == material || outer.iter().any(|o| r.iter().all(|p| contains(o, *p))))
        .map(|(o, _)| o)
        .collect()
}

/// Whether `p` is inside the closed ring (even-odd).
pub fn contains(ring: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Twice the signed area a closed ring encloses.
pub fn area(ring: &[Vec2]) -> f64 {
    (0..ring.len()).map(|i| ring[i].cross(ring[(i + 1) % ring.len()])).sum()
}

/// Whether two closed rings cross (or touch) each other.
fn rings_cross(a: &[Vec2], b: &[Vec2]) -> bool {
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    (0..a.len()).any(|i| {
        let (p, q) = (a[i], a[(i + 1) % a.len()]);
        (0..b.len()).any(|j| {
            let (r, t) = (b[j], b[(j + 1) % b.len()]);
            side(p, q, r) * side(p, q, t) <= 0.0 && side(r, t, p) * side(r, t, q) <= 0.0
        })
    })
}

/// The outline as a ring of points, each arc taken as eight chords.
pub fn ring(outline: &Outline) -> Vec<Vec2> {
    let n = outline.corners.len();
    let mut ring: Vec<Vec2> = Vec::new();
    for k in 0..n {
        let (a, b) = (outline.corners[k], outline.corners[(k + 1) % n]);
        ring.push(a);
        if let Some(Piece::Arc { center, ccw }) = outline.pieces.get(k).copied() {
            let mut turn = angle_to(a - center, b - center);
            if ccw && turn <= 0.0 {
                turn += TAU;
            } else if !ccw && turn >= 0.0 {
                turn -= TAU;
            }
            ring.extend((1..8).map(|i| center + rotated(a - center, turn * f64::from(i) / 8.0)));
        }
    }
    ring
}

/// Whether a ring crosses or touches itself nowhere.
fn is_simple_ring(ring: &[Vec2]) -> bool {
    let m = ring.len();
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let crosses = |a: Vec2, b: Vec2, c: Vec2, d: Vec2| side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0;
    m >= 3 && (0..m).all(|i| ((i + 2)..m).all(|j| (i == 0 && j == m - 1) || !crosses(ring[i], ring[(i + 1) % m], ring[j], ring[(j + 1) % m])))
}

/// A stretch of a loop on one wall: the wall (its place in `walls`), where it starts and ends,
/// and how far round the wall's centre it turns (a circle's, counter-clockwise positive).
#[derive(Clone, Copy, Debug)]
struct Stretch {
    wall: usize,
    first: Vec2,
    last: Vec2,
    turn: f64,
}

fn outline_of(walls: &[(usize, Wall)], points: &[Vec2], on: &[Option<usize>], reach: f64) -> Option<Outline> {
    let n = points.len();
    if n < 3 || on.len() != n {
        return None;
    }
    // Start where the wall changes, so no stretch is cut in two by the loop's own start.
    let start = match (0..n).find(|&i| on[i] != on[(i + n - 1) % n]) {
        Some(start) => start,
        None if on[0].is_some() => 0,
        None => return None,
    };
    let centre_of = |wall: usize| match walls.get(wall).map(|w| w.1) {
        Some(Wall::Circle { center, .. }) => Some(center),
        _ => None,
    };
    let mut stretches: Vec<Stretch> = Vec::new();
    let mut i = 0;
    while i < n {
        let k = (start + i) % n;
        let Some(wall) = on[k] else {
            i += 1;
            continue;
        };
        let mut j = i;
        let (mut length, mut turn) = (0.0, 0.0);
        while j + 1 < n && on[(start + j + 1) % n] == Some(wall) {
            let (a, b) = (points[(start + j) % n], points[(start + j + 1) % n]);
            length += a.dist(b);
            if let Some(center) = centre_of(wall) {
                turn += angle_to(a - center, b - center);
            }
            j += 1;
        }
        if length >= reach {
            let stretch = Stretch { wall, first: points[k], last: points[(start + j) % n], turn };
            match stretches.last_mut() {
                // One wall again past a stretch passed over: one stretch.
                Some(previous) if previous.wall == wall => {
                    let gap = centre_of(wall).map_or(0.0, |c| angle_to(previous.last - c, stretch.first - c));
                    previous.turn += stretch.turn + gap;
                    previous.last = stretch.last;
                }
                _ => stretches.push(stretch),
            }
        }
        i = j + 1;
    }
    if stretches.len() > 1 && stretches[0].wall == stretches[stretches.len() - 1].wall {
        let last = stretches.pop()?;
        stretches[0].first = last.first;
        stretches[0].turn += last.turn;
    }
    // Round one circle alone: a hole or a boss, two halves.
    if stretches.len() == 1 {
        let Wall::Circle { center, radius } = walls.get(stretches[0].wall)?.1 else { return None };
        if stretches[0].turn.abs() < PI {
            return None;
        }
        let ccw = stretches[0].turn > 0.0;
        let face = Some(walls[stretches[0].wall].0);
        return Some(Outline {
            corners: vec![center + Vec2::X * radius, center - Vec2::X * radius],
            pieces: vec![Piece::Arc { center, ccw }; 2],
            faces: vec![face; 2],
        });
    }
    if stretches.len() < 3 {
        return None;
    }
    let (mut corners, mut pieces, mut faces) = (Vec::new(), Vec::new(), Vec::new());
    let count = stretches.len();
    let wall = |r: usize| walls[stretches[r % count].wall].1;
    let laid: Vec<Wall> = (0..count).map(|r| laid_tangent(wall(r + count - 1), wall(r), wall(r + 1))).collect();
    for r in 0..count {
        let (here, next) = (stretches[r], stretches[(r + 1) % count]);
        let (a, b) = (laid[r], laid[(r + 1) % count]);
        let near = (here.last + next.first) * 0.5;
        let gap = here.last.dist(next.first);
        let crossing = cross(&a, &b, near).filter(|c| c.dist(near) <= 2.0 * gap + 4.0 * reach);
        let piece = |wall: &Wall, stretch: &Stretch| match *wall {
            Wall::Line { .. } => Piece::Line,
            Wall::Circle { center, .. } => Piece::Arc { center, ccw: stretch.turn > 0.0 },
        };
        // This stretch's piece ends at the corner (or its own end, bridged), the next starts there.
        match crossing {
            Some(c) => {
                corners.push(c);
                pieces.push(piece(&b, &next));
                faces.push(Some(walls[next.wall].0));
            }
            None => {
                corners.push(a.foot(here.last));
                pieces.push(Piece::Line);
                faces.push(None);
                corners.push(b.foot(next.first));
                pieces.push(piece(&b, &next));
                faces.push(Some(walls[next.wall].0));
            }
        }
    }
    Some(Outline { corners, pieces, faces })
}

/// A round between two flat walls that it touches or nearly (as [`cross`] reads touching), laid
/// tangent to both with its radius kept, or between two parallel walls half way across. The flat
/// walls are fitted over far more of the scan than a round, so they stay as fitted; laid
/// otherwise, the sketch's tangent joins would move them by as much as the round is off (0.2 mm
/// on a scanned case). Any other wall as it was.
fn laid_tangent(before: Wall, round: Wall, after: Wall) -> Wall {
    let (Wall::Line { point: p, direction: d }, Wall::Circle { center, radius }, Wall::Line { point: q, direction: e }) = (before, round, after)
    else {
        return round;
    };
    // Each flat wall's side toward the round's centre, and how far the centre is from it.
    let toward = |point: Vec2, direction: Vec2| {
        let n = direction.perp();
        let off = n.dot(center - point);
        (n * off.signum(), off.abs())
    };
    let ((m, a), (k, b)) = (toward(p, d), toward(q, e));
    if (a - radius).abs() > TOUCHING * radius || (b - radius).abs() > TOUCHING * radius {
        return round;
    }
    let s = d.cross(e);
    if s.abs() >= CROSSING {
        // The two walls moved in by the radius cross at the laid round's centre.
        let (p, q) = (p + m * radius, q + k * radius);
        return Wall::Circle { center: p + d * ((q - p).cross(e) / s), radius };
    }
    // Parallel walls either side of the centre (a slot's end): the round as wide as the gap.
    if m.dot(k) >= 0.0 {
        return round;
    }
    let half = 0.5 * m.dot(q - p);
    Wall::Circle { center: center + m * (half - a), radius: half }
}

/// Where walls `a` and `b` cross, the crossing nearest `near`; for a line and a circle that only
/// touch (within a twentieth of the radius), where they touch.
fn cross(a: &Wall, b: &Wall, near: Vec2) -> Option<Vec2> {
    let nearest = |points: [Vec2; 2]| points.into_iter().min_by(|x, y| x.dist(near).total_cmp(&y.dist(near)));
    match (*a, *b) {
        (Wall::Line { point: p, direction: d }, Wall::Line { point: q, direction: e }) => {
            let s = d.cross(e);
            if s.abs() < CROSSING {
                return None;
            }
            Some(p + d * ((q - p).cross(e) / s))
        }
        (Wall::Line { point, direction }, Wall::Circle { center, radius }) | (Wall::Circle { center, radius }, Wall::Line { point, direction }) => {
            let foot = point + direction * (center - point).dot(direction);
            let off = foot.dist(center);
            // Touching or nearly (a fillet's tangent, a little off either way): where they touch.
            if (off - radius).abs() <= TOUCHING * radius {
                return Some(center + unit2(foot - center) * radius);
            }
            if off > radius {
                return None;
            }
            let half = (radius * radius - off * off).sqrt();
            nearest([foot + direction * half, foot - direction * half])
        }
        (Wall::Circle { center: c, radius: r }, Wall::Circle { center: k, radius: s }) => {
            let d = c.dist(k);
            if d < 1e-12 || d > r + s || d < (r - s).abs() {
                return None;
            }
            let a = (r * r - s * s + d * d) / (2.0 * d);
            let h = (r * r - a * a).max(0.0).sqrt();
            let along = (k - c) / d;
            let middle = c + along * a;
            nearest([middle + along.perp() * h, middle - along.perp() * h])
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A turned part's profile.

/// How far from lying along the axis a shape's own axis or normal may be, in radians, to be
/// about it.
const ABOUT: f64 = 1e-3;

/// A face about a turning axis, in the half-section: a cylinder of a radius, a plane at a height
/// along the axis, a cone (its radius `slope` × height + `offset`), or a torus, whose section is
/// the circle of radius `minor` about (`height`, `major`) (a ball on the axis has `major` 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Turned {
    Cylinder { radius: f64 },
    Plane { height: f64 },
    Cone { slope: f64, offset: f64 },
    Torus { major: f64, height: f64, minor: f64 },
}

impl Turned {
    /// The fitted shape this face is about `axis`.
    pub fn shape(&self, axis: &Axis) -> Shape {
        match *self {
            Turned::Cylinder { radius } => Shape::Cylinder { point: axis.point, axis: axis.direction, radius },
            Turned::Plane { height } => Shape::Plane { point: axis.point + axis.direction * height, normal: axis.direction },
            Turned::Cone { slope, offset } => {
                let apex = -offset / slope;
                Shape::Cone {
                    apex: axis.point + axis.direction * apex,
                    axis: if slope > 0.0 { axis.direction } else { -axis.direction },
                    half_angle: slope.abs().atan(),
                }
            }
            Turned::Torus { major, height, minor } => {
                Shape::Torus { center: axis.point + axis.direction * height, axis: axis.direction, major, minor }
            }
        }
    }
}

/// Where `p` lies about `axis`: its height along it and its radius from it.
pub fn polar(p: Vec3, axis: &Axis) -> Vec2 {
    let v = p - axis.point;
    let t = v.dot(axis.direction);
    Vec2::new(t, (v - axis.direction * t).len())
}

/// The face `shape` is about `axis`, as fitted: `None` for a shape about another axis (within
/// `tolerance` of it), or none.
pub fn face_about(shape: &Shape, axis: &Axis, tolerance: f64) -> Option<Turned> {
    let d = axis.direction;
    let along = |v: Vec3| v.normalized().is_some_and(|u| u.dot(d).abs() >= 1.0 - ABOUT * ABOUT / 2.0);
    let height = |p: Vec3| (p - axis.point).dot(d);
    let on_axis = |p: Vec3| {
        let q = p - axis.point;
        (q - d * q.dot(d)).len() <= tolerance
    };
    match *shape {
        Shape::Plane { point, normal } if along(normal) => Some(Turned::Plane { height: height(point) }),
        Shape::Cylinder { point, axis: own, radius } if along(own) && on_axis(point) => Some(Turned::Cylinder { radius }),
        Shape::Cone { apex, axis: own, half_angle } if along(own) && on_axis(apex) => {
            let slope = half_angle.tan() * own.dot(d).signum();
            Some(Turned::Cone { slope, offset: -slope * height(apex) })
        }
        Shape::Torus { center, axis: own, major, minor } if along(own) && on_axis(center) => {
            Some(Turned::Torus { major, height: height(center), minor })
        }
        Shape::Sphere { center, radius } if on_axis(center) => Some(Turned::Torus { major: 0.0, height: height(center), minor: radius }),
        _ => None,
    }
}

/// A face of `kind` held to the axis: fitted again to its points in the half-section (height,
/// radius), each weighted, so that it shares the one axis exactly. A plane's height is their mean,
/// a cylinder's radius their mean, a cone's line their line of least squares, a torus's or ball's
/// circle their circle of least squares. `None` with too few points, or a cone that is no slope.
pub fn held(kind: crate::fit::Kind, points: &[(Vec2, f64)]) -> Option<Turned> {
    use crate::fit::Kind;
    let total: f64 = points.iter().map(|p| p.1).sum();
    if !(total > 0.0) || points.len() < 3 {
        return None;
    }
    let mean = points.iter().fold(Vec2::ZERO, |s, (p, w)| s + *p * *w) / total;
    match kind {
        Kind::Plane => Some(Turned::Plane { height: mean.x }),
        Kind::Cylinder => Some(Turned::Cylinder { radius: mean.y }),
        Kind::Cone => {
            let (mut stt, mut str) = (0.0, 0.0);
            for (p, w) in points {
                let d = *p - mean;
                stt += w * d.x * d.x;
                str += w * d.x * d.y;
            }
            let slope = str / stt;
            (stt > 0.0 && slope.is_finite() && slope.abs() > 1e-6).then_some(Turned::Cone { slope, offset: mean.y - slope * mean.x })
        }
        Kind::Torus | Kind::Sphere => {
            let (c, minor) = crate::fit::kasa(points.iter().map(|(p, w)| ([p.x, p.y], *w)))?;
            // A ball's centre lies on the axis.
            let major = if kind == Kind::Sphere { 0.0 } else { c[1] };
            Some(Turned::Torus { major, height: c[0], minor })
        }
    }
}

/// A face about the axis as the turned profile reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileFace {
    pub face: Turned,
    /// The middle of what the scan has of it, in height and radius: which side of its circle's
    /// centre a round lies.
    pub middle: Vec2,
    /// Where the scan has it nearest the axis, when it comes down to the axis.
    pub at_axis: Option<Vec2>,
}

/// Two faces meeting on the scan, and the middle of where they meet, in height and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meeting {
    pub faces: [usize; 2],
    pub at: Vec2,
}

/// Why there is no turned profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Fewer than two faces about the axis meet.
    TooFew,
    /// A face about the axis meets other than two (its index, how many).
    Meets(usize, usize),
    /// The faces about the axis make more than one outline.
    Apart,
    /// The neighbours' curves cross nowhere near where the scan has them meet.
    Corner(usize, usize),
}

/// A face's curve in the half-section.
#[derive(Clone, Copy, Debug)]
enum Curve {
    Line { point: Vec2, direction: Vec2 },
    Circle { center: Vec2, radius: f64 },
}

fn curve(face: Option<&Turned>) -> Curve {
    match face {
        None => Curve::Line { point: Vec2::ZERO, direction: Vec2::X },
        Some(&Turned::Cylinder { radius }) => Curve::Line { point: Vec2::new(0.0, radius), direction: Vec2::X },
        Some(&Turned::Plane { height }) => Curve::Line { point: Vec2::new(height, 0.0), direction: Vec2::Y },
        Some(&Turned::Cone { slope, offset }) => Curve::Line { point: Vec2::new(0.0, offset), direction: unit2(Vec2::new(1.0, slope)) },
        Some(&Turned::Torus { major, height, minor }) => Curve::Circle { center: Vec2::new(height, major), radius: minor },
    }
}

/// Where a line and a circle cross, or the line's nearest point to the circle when they only
/// come near (a round meeting a face in tangency, a little off).
fn line_circle(point: Vec2, direction: Vec2, center: Vec2, radius: f64) -> Vec<Vec2> {
    let foot = point + direction * (center - point).dot(direction);
    let off = (center - foot).len();
    // Nearly touching, either side: where they would touch, on the circle (a round runs into its
    // face in tangency, and a crossing so near a touch moves far with a little noise).
    if (off - radius).abs() <= TOUCHING * radius {
        return vec![center + (foot - center).normalized().unwrap_or(direction.perp()) * radius];
    }
    if off >= radius {
        return vec![foot];
    }
    let half = (radius * radius - off * off).sqrt();
    vec![foot - direction * half, foot + direction * half]
}

/// Where two curves cross, the crossing nearest `near`; with no crossing worth the name, the
/// point of the first nearest the second.
fn crossing(a: Curve, b: Curve, near: Vec2) -> Vec2 {
    let candidates = match (a, b) {
        (Curve::Line { point: p, direction: u }, Curve::Line { point: q, direction: v }) => {
            let cross = u.cross(v);
            if cross.abs() < 1e-9 { vec![p + u * (near - p).dot(u)] } else { vec![p + u * ((q - p).cross(v) / cross)] }
        }
        (Curve::Line { point, direction }, Curve::Circle { center, radius })
        | (Curve::Circle { center, radius }, Curve::Line { point, direction }) => line_circle(point, direction, center, radius),
        (Curve::Circle { center: c, radius: r }, Curve::Circle { center: e, radius: s }) => {
            let between = e - c;
            let apart = between.len();
            if apart < 1e-12 {
                vec![c + (near - c).normalized().unwrap_or(Vec2::X) * r]
            } else {
                let toward = between / apart;
                let along = (apart * apart + r * r - s * s) / (2.0 * apart);
                let half = (r * r - along * along).max(0.0).sqrt();
                let foot = c + toward * along;
                vec![foot - toward.perp() * half, foot + toward.perp() * half]
            }
        }
    };
    candidates.into_iter().min_by(|x, y| x.dist(near).total_cmp(&y.dist(near))).unwrap_or(near)
}

/// The profile the faces about the axis give, from where they meet on the scan. `reach` is how
/// far a corner may lie from where the scan has its two faces meet.
pub fn turned_outline(faces: &[ProfileFace], meetings: &[Meeting], reach: f64) -> Result<Outline, Refusal> {
    // The axis is one more node, met by every face that comes down to it.
    let axis = faces.len();
    let mut met: BTreeMap<(usize, usize), Vec2> = BTreeMap::new();
    for m in meetings {
        let [a, b] = m.faces;
        if a != b && a < faces.len() && b < faces.len() {
            met.insert((a.min(b), a.max(b)), m.at);
        }
    }
    for (k, face) in faces.iter().enumerate() {
        if let Some(at) = face.at_axis {
            met.insert((k, axis), Vec2::new(at.x, 0.0));
        }
    }
    let mut beside: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(a, b) in met.keys() {
        beside.entry(a).or_default().push(b);
        beside.entry(b).or_default().push(a);
    }
    if beside.keys().filter(|&&k| k != axis).count() < 2 {
        return Err(Refusal::TooFew);
    }
    if let Some((&k, others)) = beside.iter().find(|(_, others)| others.len() != 2) {
        return Err(if k == axis { Refusal::Apart } else { Refusal::Meets(k, others.len()) });
    }
    // Round the one cycle, from the lowest face.
    let start = *beside.keys().next().ok_or(Refusal::TooFew)?;
    let mut order = vec![start];
    let mut previous = start;
    let mut here = beside[&start][0];
    while here != start {
        order.push(here);
        let next = if beside[&here][0] == previous { beside[&here][1] } else { beside[&here][0] };
        previous = here;
        here = next;
        if order.len() > beside.len() {
            return Err(Refusal::Apart);
        }
    }
    if order.len() != beside.len() {
        return Err(Refusal::Apart);
    }
    let face_of = |k: usize| (k != axis).then(|| &faces[k].face);
    let n = order.len();
    let mut corners = Vec::with_capacity(n);
    for k in 0..n {
        let (before, after) = (order[(k + n - 1) % n], order[k]);
        let at = met[&(before.min(after), before.max(after))];
        let corner = crossing(curve(face_of(before)), curve(face_of(after)), at);
        if corner.dist(at) > reach {
            return Err(Refusal::Corner(before.min(after), before.max(after)));
        }
        // Nothing lies beyond the axis.
        corners.push(Vec2::new(corner.x, corner.y.max(0.0)));
    }
    let pieces = (0..n)
        .map(|k| match curve(face_of(order[k])) {
            Curve::Line { .. } => Piece::Line,
            Curve::Circle { center, .. } => {
                let (from, to) = (corners[k] - center, corners[(k + 1) % n] - center);
                let middle = faces[order[k]].middle - center;
                let sweep = |a: Vec2, b: Vec2| angle_to(a, b).rem_euclid(TAU);
                Piece::Arc { center, ccw: sweep(from, middle) <= sweep(from, to) }
            }
        })
        .collect();
    Ok(Outline { corners, pieces, faces: order.iter().map(|&k| (k != axis).then_some(k)).collect() })
}

// ---------------------------------------------------------------------------------------------
// Profiles read off a scan.

/// The slice a prism's profile lies in: halfway along it, square to it; its X and Y two
/// directions square to the prism (X × Y along it).
pub fn prism_plane(prism: &Prism) -> Plane {
    let (x, y) = crate::in_plane(prism.direction);
    Plane { origin: prism.middle(), x, y }
}

/// Prism `prism`'s profile off its walls: the scan sliced halfway along it, each loop that runs on
/// its walls the curves of those walls, exactly as fitted, from crossing to crossing. The plane it
/// lies in and its outlines (`faces` index `reading.faces`); why not, when no loop goes round its
/// walls.
pub fn prism_profile(scan: &Scan, outcome: &Outcome, reading: &Reading, prism: &Prism) -> Result<(Plane, Vec<Outline>), String> {
    prism_profile_from(scan, &crate::welded(scan.positions), outcome, &reading.faces, &reading.same, reading.size, prism)
}

/// [`prism_profile`] from the shapes found (`faces`, each on the surface `same` names) and the
/// part's size, the scan's points already welded (`weld`), so that Reconstruct can ask it of every
/// prism it reads.
pub(crate) fn prism_profile_from(
    scan: &Scan,
    weld: &[u32],
    outcome: &Outcome,
    faces: &[Face],
    same: &[usize],
    size: f64,
    prism: &Prism,
) -> Result<(Plane, Vec<Outline>), String> {
    let plane = prism_plane(prism);
    // Which face's surface each triangle is on.
    let mut owner = vec![u32::MAX; scan.triangle_count()];
    for (k, segment) in outcome.segments.iter().enumerate().take(faces.len()) {
        let surface = same.get(k).copied().unwrap_or(k);
        for &t in &segment.triangles {
            if let Some(slot) = owner.get_mut(t) {
                *slot = surface as u32;
            }
        }
    }
    let walls: Vec<(usize, Wall)> =
        prism.walls.iter().filter_map(|&k| faces.get(k).and_then(|f| wall_of(&f.shape, &plane)).map(|w| (k, w))).collect();
    let place: BTreeMap<usize, usize> = walls.iter().enumerate().map(|(i, (k, _))| (*k, i)).collect();
    let loops: Vec<(Vec<Vec2>, Vec<Option<usize>>)> = crate::section::section_welded(scan, weld, &plane)
        .into_iter()
        .filter(|r| r.closed)
        .map(|r| {
            let on = r.triangles.iter().map(|&t| owner.get(t).and_then(|&k| place.get(&(k as usize)).copied())).collect();
            (r.points, on)
        })
        .collect();
    let outlines = prism_outlines(&walls, &loops, 0.005 * size);
    if outlines.is_empty() {
        return Err("no loop of the scan's slice halfway along the prism runs round its walls".into());
    }
    Ok((plane, outlines))
}

/// The plane a turned part's profile lies in: through the axis, its X along it and its Y square
/// to it, so that a Revolve about its X turns the profile into the part.
pub fn turned_plane(axis: &Axis) -> Plane {
    let (_, y) = crate::in_plane(axis.direction);
    Plane { origin: axis.point, x: axis.direction, y }
}

/// The turned part's profile: each of the revolve's faces held to its axis, the corners where
/// neighbours' curves cross nearest where they meet on the scan. The plane it lies in, the
/// outline (`faces` index `reading.faces`) and each face as held (by face index); why not.
pub fn turned_profile(scan: &Scan, outcome: &Outcome, reading: &Reading) -> Result<(Plane, Outline, BTreeMap<usize, Turned>), String> {
    let revolve = reading.revolve.as_ref().ok_or("the part is not turned: no axis holds most of its faces")?;
    turned_profile_of(scan, outcome, &reading.faces, &reading.same, reading.size, revolve)
}

/// [`turned_profile`] of `revolve` from the shapes `found` and the part's size, so that
/// Reconstruct can ask it before it proposes Revolve It.
pub(crate) fn turned_profile_of(
    scan: &Scan,
    outcome: &Outcome,
    found: &[Face],
    same: &[usize],
    size: f64,
    revolve: &Revolve,
) -> Result<(Plane, Outline, BTreeMap<usize, Turned>), String> {
    let axis = &revolve.axis;
    let polar_of = |p: Vec3| polar(p, axis);
    // Each member held to the axis, and which member each triangle is on.
    let mut owner = vec![usize::MAX; scan.triangle_count()];
    let mut faces = Vec::new();
    let mut numbered = Vec::new();
    let mut held_faces = BTreeMap::new();
    for &k in &revolve.members {
        let Some(face) = found.get(k) else { continue };
        // The member's surface: it and the pieces of it.
        let triangles: Vec<usize> = (0..found.len())
            .filter(|&j| same.get(j).copied().unwrap_or(j) == k)
            .filter_map(|j| outcome.segments.get(j))
            .flat_map(|s| s.triangles.iter().copied())
            .collect();
        let points: Vec<(Vec2, f64)> = triangles.iter().filter_map(|&t| scan.corners(t)).flatten().map(|p| (polar_of(p), 1.0)).collect();
        let Some(turned) = held(face.shape.kind(), &points) else { continue };
        let total = points.len().max(1) as f64;
        let middle = points.iter().fold(Vec2::ZERO, |s, (p, _)| s + *p) / total;
        let nearest = points.iter().map(|(p, _)| *p).min_by(|a, b| a.y.total_cmp(&b.y));
        // A face comes down to the axis when the scan has it within a hundredth of the part.
        let at_axis = nearest.filter(|n| n.y <= 0.01 * size);
        for &t in &triangles {
            if let Some(slot) = owner.get_mut(t) {
                *slot = faces.len();
            }
        }
        faces.push(ProfileFace { face: turned, middle, at_axis });
        numbered.push(k);
        held_faces.insert(k, turned);
    }
    // Where each pair meets: the middle of the edges their triangles share, at least two of them.
    let neighbours = crate::base::edge_neighbours(scan);
    let mut shared: BTreeMap<(usize, usize), (Vec2, usize)> = BTreeMap::new();
    for t in 0..owner.len() {
        let a = owner[t];
        if a == usize::MAX {
            continue;
        }
        let Some(&[p, q, r]) = scan.triangles.get(t) else { continue };
        for n in neighbours.of(scan, t) {
            let b = owner.get(n).copied().unwrap_or(usize::MAX);
            if b == usize::MAX || a >= b {
                continue;
            }
            let Some(other) = scan.triangles.get(n) else { continue };
            let common: Vec<u32> = [p, q, r].into_iter().filter(|v| other.contains(v)).collect();
            if let [x, y] = common[..] {
                let middle = (polar_of(scan.positions[x as usize]) + polar_of(scan.positions[y as usize])) * 0.5;
                let entry = shared.entry((a, b)).or_insert((Vec2::ZERO, 0));
                entry.0 += middle;
                entry.1 += 1;
            }
        }
    }
    let meetings: Vec<Meeting> =
        shared.into_iter().filter(|(_, (_, n))| *n >= 2).map(|((a, b), (sum, n))| Meeting { faces: [a, b], at: sum / n as f64 }).collect();
    let outline = turned_outline(&faces, &meetings, 0.05 * size).map_err(|refusal| match refusal {
        Refusal::TooFew => "fewer than two faces about the turning axis meet on the scan, so there is no profile to turn".to_string(),
        Refusal::Meets(k, n) => format!("face {} meets {n} other faces about the turning axis, where a profile has each meet two: a face between them is missing, or one too many is found", numbered[k] + 1),
        Refusal::Apart => "the faces about the turning axis make more than one outline, so there is no one profile to turn".to_string(),
        Refusal::Corner(a, b) => format!("faces {} and {} meet on the scan where their shapes do not cross: one of them is fitted wrong", numbered[a] + 1, numbered.get(b).map_or(0, |k| k + 1)),
    })?;
    // Numbered by the reading's faces.
    let outline = Outline { faces: outline.faces.iter().map(|f| f.map(|i| numbered[i])).collect(), ..outline };
    Ok((turned_plane(axis), outline, held_faces))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::reconstruct::read;
    use crate::segment::{Options, find_all};
    use crate::test_meshes::{extruded, revolved, rounded_rectangle, steps, turned};

    /// A rounded block 40 × 30 × 10, corners R 5, turned and moved anywhere: one prism, its profile
    /// four lines and four arcs exactly as the walls were fitted.
    #[test]
    fn a_rounded_blocks_profile_is_its_lines_and_arcs() {
        let (positions, triangles) = extruded(&rounded_rectangle(40.0, 30.0, 5.0, 0.5), 0.0, 10.0, 16);
        let axis = Vec3::new(0.6, 0.2, -0.3).normalized().unwrap();
        let moved = turned(&positions, axis, 0.5, Vec3::new(3.0, -8.0, 21.0));
        let scan = Scan::new(&moved, &triangles);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let reading = read(&scan, &outcome);
        assert!(reading.revolve.is_none());
        let prism = reading.prisms.first().expect("a prism");
        assert!((prism.depth() - 10.0).abs() < 0.05, "{prism:?}");
        let (_, outlines) = prism_profile(&scan, &outcome, &reading, prism).unwrap();
        assert_eq!(outlines.len(), 1, "{outlines:?}");
        let o = &outlines[0];
        let lines = o.pieces.iter().filter(|p| **p == Piece::Line).count();
        assert_eq!((lines, o.pieces.len() - lines), (4, 4), "{o:?}");
        // The outline's length: 2 (30 + 20) of sides and a whole turn of R 5.
        let points = ring(o);
        let length: f64 = (0..points.len()).map(|i| points[i].dist(points[(i + 1) % points.len()])).sum();
        let expected = 100.0 + std::f64::consts::TAU * 5.0;
        assert!((length - expected).abs() < 0.1, "{length} against {expected}");
    }

    /// A rounded rectangle 40 × 30, corners R 5, whose rounds were fitted a little off its sides,
    /// as on a noisy scan: each round is laid tangent to its two sides, and the sides stay where
    /// they were fitted, so a sketch holding the joins tangent has nothing to move.
    #[test]
    fn rounds_are_laid_tangent_to_the_sides_as_fitted() {
        use std::f64::consts::FRAC_PI_2;
        let (w, h, r) = (40.0, 30.0, 5.0);
        let walls: Vec<(usize, Wall)> = vec![
            (0, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::X }),
            (1, Wall::Line { point: Vec2::new(w, 0.0), direction: Vec2::Y }),
            (2, Wall::Line { point: Vec2::new(w, h), direction: -Vec2::X }),
            (3, Wall::Line { point: Vec2::new(0.0, h), direction: -Vec2::Y }),
            (4, Wall::Circle { center: Vec2::new(w - r + 0.12, r + 0.1), radius: r + 0.05 }),
            (5, Wall::Circle { center: Vec2::new(w - r - 0.1, h - r + 0.15), radius: r - 0.08 }),
            (6, Wall::Circle { center: Vec2::new(r + 0.2, h - r), radius: r + 0.1 }),
            (7, Wall::Circle { center: Vec2::new(r - 0.05, r - 0.12), radius: r }),
        ];
        // The slice round the true outline, counter-clockwise, each point on its wall.
        let sides = [
            (Vec2::new(r, 0.0), Vec2::new(w - r, 0.0)),
            (Vec2::new(w, r), Vec2::new(w, h - r)),
            (Vec2::new(w - r, h), Vec2::new(r, h)),
            (Vec2::new(0.0, h - r), Vec2::new(0.0, r)),
        ];
        let rounds = [(Vec2::new(w - r, r), -FRAC_PI_2), (Vec2::new(w - r, h - r), 0.0), (Vec2::new(r, h - r), FRAC_PI_2), (Vec2::new(r, r), PI)];
        let (mut points, mut on) = (Vec::new(), Vec::new());
        for (k, ((from, to), (center, start))) in sides.into_iter().zip(rounds).enumerate() {
            for i in 0..20 {
                points.push(from + (to - from) * (f64::from(i) / 20.0));
                on.push(Some(k));
            }
            for i in 0..10 {
                points.push(center + rotated(Vec2::X * r, start + FRAC_PI_2 * f64::from(i) / 10.0));
                on.push(Some(4 + k));
            }
        }
        let outlines = prism_outlines(&walls, &[(points, on)], 0.2);
        assert_eq!(outlines.len(), 1, "{outlines:?}");
        let o = &outlines[0];
        assert_eq!(o.pieces.len(), 8, "{o:?}");
        let n = o.pieces.len();
        for k in 0..n {
            let (from, to) = (o.corners[k], o.corners[(k + 1) % n]);
            match o.pieces[k] {
                Piece::Line => {
                    let Some(Wall::Line { point, direction }) = o.faces[k].map(|f| walls[f].1) else { panic!("{o:?}") };
                    for c in [from, to] {
                        assert!(direction.cross(c - point).abs() < 1e-9, "a corner off its side as fitted: {o:?}");
                    }
                }
                Piece::Arc { center, .. } => {
                    let radius = from.dist(center);
                    assert!((to.dist(center) - radius).abs() < 1e-9, "{o:?}");
                    // Square to the side at each end: tangent.
                    for (corner, side) in [(from, (k + n - 1) % n), (to, (k + 1) % n)] {
                        let Some(Wall::Line { direction, .. }) = o.faces[side].map(|f| walls[f].1) else { panic!("{o:?}") };
                        assert!((corner - center).dot(direction).abs() < 1e-9, "not tangent: {o:?}");
                    }
                }
            }
        }
    }

    /// The turned part: a face at −4 down to the axis, a round R 4 into the shaft R 20, the shaft
    /// to 20, a chamfer to R 15 at 25 and the face there down to the axis.
    #[test]
    fn a_turned_parts_profile_is_its_faces_held_to_the_axis() {
        let mut profile = vec![[-4.0, 0.0]];
        profile.extend(steps([-4.0, 0.5], [-4.0, 15.5], 30));
        profile.extend((0..=24).map(|i| f64::from(i) / 24.0 * std::f64::consts::FRAC_PI_2).map(|a| [-4.0 * a.cos(), 16.0 + 4.0 * a.sin()]));
        profile.extend(steps([0.0, 20.0], [20.0, 20.0], 40).into_iter().skip(1));
        profile.extend(steps([20.0, 20.0], [25.0, 15.0], 14).into_iter().skip(1));
        profile.extend(steps([25.0, 15.0], [25.0, 0.5], 29).into_iter().skip(1));
        profile.push([25.0, 0.0]);
        let (positions, triangles) = revolved(&profile, 180, false);
        let axis = Vec3::new(0.3, 0.5, 0.8).normalized().unwrap();
        let moved = turned(&positions, axis, 1.1, Vec3::new(14.0, -6.0, 30.0));
        let scan = Scan::new(&moved, &triangles);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let reading = read(&scan, &outcome);
        let (plane, outline, held) = turned_profile(&scan, &outcome, &reading).unwrap();
        assert_eq!(outline.pieces.len(), 6, "{outline:?}");
        assert_eq!(held.len(), 5);
        // Its corners in the part's own frame: the axis runs one way or the other along Z.
        let local: Vec<Vec2> = outline
            .corners
            .iter()
            .map(|c| plane.to_world(*c))
            .map(|p| p - Vec3::new(14.0, -6.0, 30.0))
            .map(|p| {
                let z = p.dot(axis * 0.0 + crate::place::Rigid::turn(axis, 1.1).turned(Vec3::Z));
                Vec2::new(z, (p - crate::place::Rigid::turn(axis, 1.1).turned(Vec3::Z) * z).len())
            })
            .collect();
        for corner in [[-4.0, 0.0], [-4.0, 16.0], [0.0, 20.0], [20.0, 20.0], [25.0, 15.0], [25.0, 0.0]] {
            assert!(local.iter().any(|c| c.dist(Vec2::new(corner[0], corner[1])) < 0.05), "{corner:?} in {local:?}");
        }
    }

    /// A stepped shaft about the X of the half-section: R 20 from 0 to 30, R 10 from 30 to 50, the
    /// ends and the step planes, the ends down to the axis.
    #[test]
    fn a_stepped_shafts_faces_go_round_its_profile() {
        let faces = vec![
            ProfileFace { face: Turned::Plane { height: 0.0 }, middle: Vec2::new(0.0, 10.0), at_axis: Some(Vec2::new(0.0, 0.0)) },
            ProfileFace { face: Turned::Cylinder { radius: 20.0 }, middle: Vec2::new(15.0, 20.0), at_axis: None },
            ProfileFace { face: Turned::Plane { height: 30.0 }, middle: Vec2::new(30.0, 15.0), at_axis: None },
            ProfileFace { face: Turned::Cylinder { radius: 10.0 }, middle: Vec2::new(40.0, 10.0), at_axis: None },
            ProfileFace { face: Turned::Plane { height: 50.0 }, middle: Vec2::new(50.0, 5.0), at_axis: Some(Vec2::new(50.0, 0.0)) },
        ];
        let meetings = vec![
            Meeting { faces: [0, 1], at: Vec2::new(0.0, 20.0) },
            Meeting { faces: [1, 2], at: Vec2::new(30.0, 20.0) },
            Meeting { faces: [2, 3], at: Vec2::new(30.0, 10.0) },
            Meeting { faces: [3, 4], at: Vec2::new(50.0, 10.0) },
        ];
        let outline = turned_outline(&faces, &meetings, 1.0).unwrap();
        assert_eq!(outline.corners.len(), 6);
        for corner in
            [Vec2::new(0.0, 0.0), Vec2::new(0.0, 20.0), Vec2::new(30.0, 20.0), Vec2::new(30.0, 10.0), Vec2::new(50.0, 10.0), Vec2::new(50.0, 0.0)]
        {
            assert!(outline.corners.iter().any(|c| c.dist(corner) < 1e-9), "{corner:?} in {:?}", outline.corners);
        }
        assert!(outline.pieces.iter().all(|p| *p == Piece::Line));
        // A face missing between two is refused.
        assert!(matches!(turned_outline(&faces[..3], &meetings[..2], 1.0), Err(Refusal::Apart | Refusal::Meets(..))));
    }

    #[test]
    fn a_held_cone_and_round_are_their_lines_and_circles() {
        let cone: Vec<(Vec2, f64)> = (0..10).map(f64::from).map(|t| (Vec2::new(t, 20.0 - 0.5 * t), 1.0)).collect();
        let Some(Turned::Cone { slope, offset }) = held(crate::fit::Kind::Cone, &cone) else { panic!() };
        assert!((slope + 0.5).abs() < 1e-12 && (offset - 20.0).abs() < 1e-12);
        let round: Vec<(Vec2, f64)> =
            (0..10).map(|i| f64::from(i) * 0.15).map(|a| (Vec2::new(5.0 + 3.0 * a.cos(), 12.0 + 3.0 * a.sin()), 1.0)).collect();
        let Some(Turned::Torus { major, height, minor }) = held(crate::fit::Kind::Torus, &round) else { panic!() };
        assert!((major - 12.0).abs() < 1e-9 && (height - 5.0).abs() < 1e-9 && (minor - 3.0).abs() < 1e-9);
    }

    /// A box slice: four walls, a loop round them with its corners cut by noise, comes out as the
    /// walls' own square.
    #[test]
    fn a_prisms_profile_is_its_walls_crossing() {
        let walls = vec![
            (0, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::X }),
            (1, Wall::Line { point: Vec2::new(30.0, 0.0), direction: Vec2::Y }),
            (2, Wall::Line { point: Vec2::new(0.0, 20.0), direction: Vec2::X }),
            (3, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::Y }),
        ];
        let mut points = Vec::new();
        let mut on = Vec::new();
        let side = |from: Vec2, to: Vec2, wall: usize, points: &mut Vec<Vec2>, on: &mut Vec<Option<usize>>| {
            for i in 0..20 {
                let t = f64::from(i) / 20.0;
                let p = from + (to - from) * t;
                points.push(p);
                // Near the corners, off every wall (a round corner).
                on.push(if t < 0.1 || t > 0.9 { None } else { Some(wall) });
            }
        };
        side(Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), 0, &mut points, &mut on);
        side(Vec2::new(30.0, 0.0), Vec2::new(30.0, 20.0), 1, &mut points, &mut on);
        side(Vec2::new(30.0, 20.0), Vec2::new(0.0, 20.0), 2, &mut points, &mut on);
        side(Vec2::new(0.0, 20.0), Vec2::new(0.0, 0.0), 3, &mut points, &mut on);
        let outlines = prism_outlines(&walls, &[(points, on)], 0.5);
        assert_eq!(outlines.len(), 1);
        let o = &outlines[0];
        assert_eq!(o.corners.len(), 4);
        for corner in [Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), Vec2::new(30.0, 20.0), Vec2::new(0.0, 20.0)] {
            assert!(o.corners.iter().any(|c| c.dist(corner) < 1e-9), "{corner:?} in {:?}", o.corners);
        }
    }
}
