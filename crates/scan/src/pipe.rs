//! Pipe It's reading: a round bar or tube bent along a path (a handrail, a bent pipe, a wire
//! form) is read off the shapes Find All found on it: straight runs are cylinders, bends are tori
//! whose tube is as thick as the cylinders, each run on from the one before where they meet. The
//! path through their axes, lines and arcs in one plane, is what Pipe sweeps a circle along.

use std::collections::BTreeSet;
use std::f64::consts::TAU;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::in_plane;
use crate::reconstruct::Face;

/// Tubes this near alike in radius, as a share of it (and the scan's tolerance), are one tube.
const SAME_RADIUS: f64 = 0.03;
/// Ends this near each other, as a share of the tube's radius, meet: Find All's straight runs and
/// bends run on into each other tangentially, so where one shape stops and the next starts on the
/// scan is loose by about the tube's width.
const MEET: f64 = 2.0;
/// Directions this close are one direction (a cosine: about 2°).
const SAME_DIRECTION: f64 = 0.9994;
/// A path off its plane by more than this share of the tube's radius is no flat path.
const FLAT: f64 = 0.1;

/// A piece of the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line {
        from: Vec3,
        to: Vec3,
    },
    /// About `centre`, counter-clockwise about the plane's normal from `from` to `to`, or the
    /// other way with `clockwise`.
    Arc {
        centre: Vec3,
        from: Vec3,
        to: Vec3,
        clockwise: bool,
    },
}

impl Piece {
    pub fn ends(&self) -> (Vec3, Vec3) {
        match *self {
            Piece::Line { from, to } | Piece::Arc { from, to, .. } => (from, to),
        }
    }

    fn reversed(self) -> Piece {
        match self {
            Piece::Line { from, to } => Piece::Line { from: to, to: from },
            Piece::Arc { centre, from, to, clockwise } => Piece::Arc { centre, from: to, to: from, clockwise: !clockwise },
        }
    }
}

/// A bar bent along a flat path.
#[derive(Clone, Debug, PartialEq)]
pub struct Pipe {
    /// The path's plane: a point on it and its unit normal.
    pub origin: Vec3,
    pub normal: Vec3,
    pub pieces: Vec<Piece>,
    pub radius: f64,
    /// The faces it is made of (the tube's, and the flat ends').
    pub faces: BTreeSet<usize>,
}

/// One tube piece as read: its shape's face, its two ends on the axis, and (a bend's) the centre.
struct Run {
    face: usize,
    piece: Piece,
}

fn unit(v: Vec3) -> Vec3 {
    v.normalized().unwrap_or(Vec3::ZERO)
}

/// The path through the tube faces among `faces` (cylinders `is_hole` does not call holes, tori),
/// when they run on one from another along one flat path with at least one bend. `span` gives how
/// far a face runs along a direction; `angles` how far round an axis (through a point) a face's
/// triangles go, as the angle each starts and ends at, counter-clockwise from `u` about the axis.
pub fn read(
    faces: &[Face],
    is_hole: &dyn Fn(usize) -> bool,
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    angles: &dyn Fn(usize, Vec3, Vec3, Vec3) -> Option<(f64, f64)>,
    tolerance: f64,
) -> Option<Pipe> {
    // The tube's radius: the bends' (a torus's tube is the bar itself).
    let bends: Vec<usize> = (0..faces.len()).filter(|&k| faces[k].area > 0.0 && matches!(faces[k].shape, Shape::Torus { .. })).collect();
    let radius = bends.iter().find_map(|&k| match faces[k].shape {
        Shape::Torus { minor, major, .. } if major > minor => Some(minor),
        _ => None,
    })?;
    let alike = |r: f64| (r - radius).abs() <= SAME_RADIUS * radius + tolerance;
    let mut runs: Vec<Run> = Vec::new();
    for (k, face) in faces.iter().enumerate() {
        if face.area <= 0.0 {
            continue;
        }
        match face.shape {
            Shape::Cylinder { point, axis, radius: r } if alike(r) && !is_hole(k) => {
                let axis = unit(axis);
                let (a, b) = span(k, axis)?;
                let at = |t: f64| point + axis * (t - point.dot(axis));
                runs.push(Run { face: k, piece: Piece::Line { from: at(a), to: at(b) } });
            }
            Shape::Torus { center, axis, major, minor } if alike(minor) && major > minor => {
                let axis = unit(axis);
                let (u, v) = in_plane(axis);
                let (start, end) = angles(k, center, axis, u)?;
                let on = |t: f64| center + (u * t.cos() + v * t.sin()) * major;
                runs.push(Run { face: k, piece: Piece::Arc { centre: center, from: on(start), to: on(end), clockwise: false } });
                // `v` is axis × u: counter-clockwise about `axis`.
                let _ = v;
            }
            _ => {}
        }
    }
    if runs.len() < 2 || !runs.iter().any(|r| matches!(r.piece, Piece::Arc { .. })) {
        return None;
    }
    // The plane: the first bend's.
    let (origin, normal) = runs.iter().find_map(|r| match (r.piece, faces[r.face].shape) {
        (Piece::Arc { centre, .. }, Shape::Torus { axis, .. }) => Some((centre, unit(axis))),
        _ => None,
    })?;
    let flat = |p: Vec3| (p - origin).dot(normal).abs() <= FLAT * radius + tolerance;
    for r in &runs {
        let (a, b) = r.piece.ends();
        if !flat(a) || !flat(b) {
            return None;
        }
        if let (Piece::Arc { .. }, Shape::Torus { axis, .. }) = (r.piece, faces[r.face].shape)
            && unit(axis).dot(normal).abs() < SAME_DIRECTION
        {
            return None;
        }
    }
    // An arc's normal turned to the plane's: counter-clockwise about the other way is clockwise.
    for r in &mut runs {
        if let (Piece::Arc { centre, from, to, .. }, Shape::Torus { axis, .. }) = (r.piece, faces[r.face].shape)
            && unit(axis).dot(normal) < 0.0
        {
            r.piece = Piece::Arc { centre, from, to, clockwise: true };
        }
    }
    // Chain them: from a run with a free end, each next the one whose end meets this one's.
    let meet = MEET * radius + 2.0 * tolerance;
    let free = |i: usize, end: Vec3| {
        !runs.iter().enumerate().any(|(j, r)| j != i && (r.piece.ends().0.dist(end) <= meet || r.piece.ends().1.dist(end) <= meet))
    };
    let first = (0..runs.len()).find(|&i| free(i, runs[i].piece.ends().0) || free(i, runs[i].piece.ends().1))?;
    let mut used = vec![false; runs.len()];
    used[first] = true;
    let mut piece = runs[first].piece;
    if !free(first, piece.ends().0) {
        piece = piece.reversed();
    }
    let mut chain = vec![(runs[first].face, piece)];
    loop {
        let end = chain.last().map(|c| c.1.ends().1)?;
        let next = (0..runs.len()).filter(|&j| !used[j]).find_map(|j| {
            let (a, b) = runs[j].piece.ends();
            if a.dist(end) <= meet {
                Some((j, runs[j].piece))
            } else if b.dist(end) <= meet {
                Some((j, runs[j].piece.reversed()))
            } else {
                None
            }
        });
        let Some((j, piece)) = next else { break };
        used[j] = true;
        chain.push((runs[j].face, piece));
    }
    if used.iter().any(|u| !u) {
        return None;
    }
    // Where two pieces meet they meet at one point: where a line runs onto a bend, the point of
    // the line nearest the bend's centre (where it touches the bend's circle), both moved there.
    let foot = |from: Vec3, to: Vec3, centre: Vec3| {
        let d = unit(to - from);
        from + d * (centre - from).dot(d)
    };
    let mut pieces: Vec<Piece> = chain.iter().map(|c| c.1).collect();
    for i in 0..pieces.len().saturating_sub(1) {
        let (a, b) = (pieces[i], pieces[i + 1]);
        match (a, b) {
            (Piece::Line { from, to }, Piece::Arc { centre, to: end, clockwise, .. }) => {
                let joint = foot(from, to, centre);
                pieces[i] = Piece::Line { from, to: joint };
                pieces[i + 1] = Piece::Arc { centre, from: joint, to: end, clockwise };
            }
            (Piece::Arc { centre, from: start, clockwise, .. }, Piece::Line { from, to }) => {
                let joint = foot(from, to, centre);
                pieces[i] = Piece::Arc { centre, from: start, to: joint, clockwise };
                pieces[i + 1] = Piece::Line { from: joint, to };
            }
            (Piece::Line { from, .. }, Piece::Line { from: joint, .. }) => pieces[i] = Piece::Line { from, to: joint },
            (Piece::Arc { to: joint, .. }, Piece::Arc { centre, to, clockwise, .. }) => {
                pieces[i + 1] = Piece::Arc { centre, from: joint, to, clockwise }
            }
        }
    }
    // Laid onto the plane.
    let onto = |p: Vec3| p - normal * (p - origin).dot(normal);
    let pieces = pieces
        .into_iter()
        .map(|p| match p {
            Piece::Line { from, to } => Piece::Line { from: onto(from), to: onto(to) },
            Piece::Arc { centre, from, to, clockwise } => Piece::Arc { centre: onto(centre), from: onto(from), to: onto(to), clockwise },
        })
        .collect();
    let pieces = tangent(pieces, normal);
    Some(Pipe { origin, normal, pieces, radius, faces: chain.iter().map(|c| c.0).collect() })
}

/// `pieces` made to run on into each other exactly, as Pipe needs its path: each arc's ends at
/// one radius from its centre (their mean), and a line at the end of the path, beside one arc,
/// turned about its far end so that it touches the arc where it meets it.
fn tangent(mut pieces: Vec<Piece>, normal: Vec3) -> Vec<Piece> {
    let n = pieces.len();
    for i in 0..n {
        let Piece::Arc { centre, from, to, clockwise } = pieces[i] else { continue };
        let r = 0.5 * (from.dist(centre) + to.dist(centre));
        let (a, b) = (centre + unit(from - centre) * r, centre + unit(to - centre) * r);
        pieces[i] = Piece::Arc { centre, from: a, to: b, clockwise };
        // The way the path runs at a point of the arc, counter-clockwise about the normal or not.
        let along = |p: Vec3| {
            let d = unit(normal.cross(p - centre));
            if clockwise { -d } else { d }
        };
        if i > 0
            && let Piece::Line { from: far, .. } = pieces[i - 1]
            && (i == 1 || !matches!(pieces[i - 2], Piece::Arc { .. }))
        {
            // The line before ends at `a`, running along the arc's way there.
            let d = along(a);
            let far = a - d * (a - far).dot(d).max(0.0);
            pieces[i - 1] = Piece::Line { from: far, to: a };
        } else if i > 0
            && let Piece::Line { from, .. } = pieces[i - 1]
        {
            pieces[i - 1] = Piece::Line { from, to: a };
        }
        if i + 1 < n
            && let Piece::Line { to: far, .. } = pieces[i + 1]
            && (i + 2 == n || !matches!(pieces[i + 2], Piece::Arc { .. }))
        {
            let d = along(b);
            let far = b + d * (far - b).dot(d).max(0.0);
            pieces[i + 1] = Piece::Line { from: b, to: far };
        } else if i + 1 < n
            && let Piece::Line { to, .. } = pieces[i + 1]
        {
            pieces[i + 1] = Piece::Line { from: b, to };
        }
    }
    pieces
}

/// The angles a set of points round an axis through `centre` (unit `axis`, `u` square to it)
/// starts and ends at, counter-clockwise, across the widest gap between them.
pub fn angular_extent(points: impl Iterator<Item = Vec3>, centre: Vec3, axis: Vec3, u: Vec3) -> Option<(f64, f64)> {
    let v = axis.cross(u);
    let mut angles: Vec<f64> = points.map(|p| p - centre).map(|d| d.dot(v).atan2(d.dot(u))).collect();
    if angles.len() < 2 {
        return None;
    }
    angles.sort_by(f64::total_cmp);
    let n = angles.len();
    let (mut gap, mut after) = (angles[0] + TAU - angles[n - 1], 0);
    for k in 1..n {
        if angles[k] - angles[k - 1] > gap {
            (gap, after) = (angles[k] - angles[k - 1], k);
        }
    }
    let start = angles[after];
    let end = if after == 0 { angles[n - 1] } else { angles[after - 1] + TAU };
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_up_a_bend_and_along_is_one_flat_path() {
        // Up Z from 0 to 50, a quarter bend R 30 about (30, 0, 50), along X from 30 to 90 at z 80.
        let faces = [
            Face { shape: Shape::Cylinder { point: Vec3::new(0.0, 0.0, 25.0), axis: Vec3::Z, radius: 7.99 }, area: 2500.0 },
            Face { shape: Shape::Torus { center: Vec3::new(30.0, 0.0, 50.0), axis: -Vec3::Y, major: 30.0, minor: 7.99 }, area: 2300.0 },
            Face { shape: Shape::Cylinder { point: Vec3::new(60.0, 0.0, 80.0), axis: Vec3::X, radius: 8.0 }, area: 3000.0 },
        ];
        let spans = |k: usize, d: Vec3| -> Option<(f64, f64)> {
            let (a, b) = match k {
                0 => (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 50.0)),
                2 => (Vec3::new(30.0, 0.0, 80.0), Vec3::new(90.0, 0.0, 80.0)),
                _ => return None,
            };
            let (x, y) = (a.dot(d), b.dot(d));
            Some((x.min(y), x.max(y)))
        };
        let angles = |_: usize, centre: Vec3, axis: Vec3, u: Vec3| {
            let arc = (0..=20).map(|i| {
                let t = std::f64::consts::FRAC_PI_2 * i as f64 / 20.0;
                Vec3::new(30.0 - 30.0 * t.cos(), 0.0, 50.0 + 30.0 * t.sin())
            });
            angular_extent(arc, centre, axis, u)
        };
        let pipe = read(&faces, &|_| false, &spans, &angles, 0.1).unwrap();
        assert_eq!(pipe.pieces.len(), 3, "{pipe:?}");
        assert!((pipe.radius - 7.99).abs() < 1e-9);
        let (start, _) = pipe.pieces[0].ends();
        let (_, end) = pipe.pieces[2].ends();
        let free_ends = [start, end];
        assert!(
            free_ends.iter().any(|p| p.dist(Vec3::ZERO) < 1e-6) && free_ends.iter().any(|p| p.dist(Vec3::new(90.0, 0.0, 80.0)) < 1e-6),
            "{pipe:?}"
        );
        for w in pipe.pieces.windows(2) {
            assert!(w[0].ends().1.dist(w[1].ends().0) < 1e-9, "{pipe:?}");
        }
        // A straight bar alone is no pipe: Extrude It builds it.
        assert!(read(&faces[..1], &|_| false, &spans, &angles, 0.1).is_none());
    }
}
