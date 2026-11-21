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

use std::collections::{BTreeMap, BTreeSet};
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
/// cylinder's or cone's axis, to be a wall along it: a drafted wall leans up to about 10°.
const ALONG: f64 = 0.174;
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
        // A drafted round: the circle it makes where its axis crosses the plane.
        Shape::Cone { apex, axis, half_angle } => {
            let a = axis.normalized()?;
            if a.cross(d).len() > ALONG {
                return None;
            }
            let along = (plane.origin - apex).dot(d) / a.dot(d);
            let radius = (along * half_angle.tan()).abs();
            (radius > 0.0).then(|| Wall::Circle { center: plane.to_local(apex + a * along), radius })
        }
        _ => None,
    }
}

/// The slice's and the sketch's planes (`plane`, `sketch`, along the same direction) turned about
/// it so that their X runs along the largest flat wall among `walls`: a part's sides then lie
/// along the sketch's axes, as it was drawn, rather than a few thousandths of a degree off them
/// wherever the plane's axes happened to fall.
fn squared_to_walls(plane: Plane, sketch: Plane, faces: &[Face], walls: &[usize]) -> (Plane, Plane) {
    let d = plane.normal();
    let largest = walls
        .iter()
        .filter_map(|&k| faces.get(k))
        .filter_map(|f| match f.shape {
            Shape::Plane { normal, .. } => Some((f.area, normal)),
            _ => None,
        })
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let Some(x) = largest.and_then(|(_, n)| d.cross(n).normalized()) else { return (plane, sketch) };
    let Some(y) = d.cross(x).normalized() else { return (plane, sketch) };
    (Plane { x, y, ..plane }, Plane { x, y, ..sketch })
}

/// Lines within this of square or parallel to the largest wall are made so (radians: about 0.5°).
const SNAP_SQUARE: f64 = 0.0087;
/// Rounds whose radii are within this share of each other are given one radius.
pub const SAME_ROUND: f64 = 0.015;

/// The walls as they were designed, as far as the scan leaves no doubt: each line within
/// [`SNAP_SQUARE`] of parallel or square to the line of the largest face turned to be exactly so,
/// about the middle of its stretch in the slice's `loops` (where it is measured best); the
/// circles whose radii lie within [`SAME_ROUND`] of each other's given their mean by area. The
/// scan's few hundredths of a degree and of a millimetre otherwise reach the model (and its
/// kernel, which can refuse to shell a tapered body whose rounds and sides are all slightly off).
/// `areas` are the walls' faces' areas.
pub fn as_designed(walls: &[(usize, Wall)], areas: &[f64], loops: &[(Vec<Vec2>, Vec<Option<usize>>)]) -> Vec<(usize, Wall)> {
    let mut out = walls.to_vec();
    let area = |i: usize| areas.get(i).copied().unwrap_or(0.0);
    // Each wall's middle in the slice: the mean of the slice's points on it.
    let mut sums = vec![(Vec2::ZERO, 0usize); walls.len()];
    for (points, on) in loops {
        for (p, w) in points.iter().zip(on) {
            if let Some(sum) = w.and_then(|w| sums.get_mut(w)) {
                sum.0 += *p;
                sum.1 += 1;
            }
        }
    }
    let largest = (0..walls.len()).filter(|&i| matches!(walls[i].1, Wall::Line { .. })).max_by(|&a, &b| area(a).total_cmp(&area(b)));
    if let Some(Wall::Line { direction: reference, .. }) = largest.map(|i| walls[i].1) {
        // Along the sketch's own axes when it is that near them (it is once the planes are
        // squared to the walls): exactly along them.
        let reference = [Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, -1.0)]
            .into_iter()
            .find(|a| angle_to(reference, *a).abs() <= SNAP_SQUARE)
            .unwrap_or(reference);
        for (i, (_, wall)) in out.iter_mut().enumerate() {
            let Wall::Line { point, direction } = *wall else { continue };
            let candidates = [reference, reference.perp(), -reference, -reference.perp()];
            let Some(&snapped) = candidates.iter().max_by(|a, b| a.dot(direction).total_cmp(&b.dot(direction))) else { continue };
            let turn = angle_to(direction, snapped).abs();
            if turn == 0.0 || turn > SNAP_SQUARE {
                continue;
            }
            let pivot = match sums[i] {
                (sum, n) if n > 0 => {
                    let m = sum * (1.0 / n as f64);
                    point + direction * (m - point).dot(direction)
                }
                _ => point,
            };
            *wall = Wall::Line { point: pivot, direction: snapped };
        }
    }
    // Rounds of one radius: each with the others near its radius, largest group first.
    let circles: Vec<usize> = (0..out.len()).filter(|&i| matches!(out[i].1, Wall::Circle { .. })).collect();
    let radii: Vec<f64> = out
        .iter()
        .map(|(_, w)| match *w {
            Wall::Circle { radius, .. } => radius,
            Wall::Line { .. } => 0.0,
        })
        .collect();
    let radius = |i: usize| radii[i];
    let mut done = vec![false; out.len()];
    for &i in &circles {
        if done[i] {
            continue;
        }
        let group: Vec<usize> = circles.iter().copied().filter(|&j| !done[j] && (radius(j) - radius(i)).abs() <= SAME_ROUND * radius(i)).collect();
        let weight: f64 = group.iter().map(|&j| area(j).max(1e-9)).sum();
        let mean = group.iter().map(|&j| radius(j) * area(j).max(1e-9)).sum::<f64>() / weight;
        for &j in &group {
            done[j] = true;
            if let Wall::Circle { center, .. } = out[j].1 {
                out[j].1 = Wall::Circle { center, radius: mean };
            }
        }
    }
    out
}

/// The outlines the `walls` (by face index, the wall) give from the slice's closed `loops`, each
/// with its points and which wall (its place in `walls`) each point is on: one outline for each
/// loop that runs on three walls or more, or round one circle. A stretch on a wall shorter than
/// `reach` is passed over. An outline that would cross itself is left out, and of two that cross
/// each other the smaller; a loop the other way from the largest is a hole, kept only inside an
/// outline of the prism's (one outside them all is in what another prism makes). With `fill`, a
/// gap between two walls where the scan mostly runs along the prism is drawn as the slice runs there
/// ([`Fill`]); without it, the walls either side run on to where they cross.
pub fn prism_outlines(walls: &[(usize, Wall)], loops: &[(Vec<Vec2>, Vec<Option<usize>>)], reach: f64, fill: Option<&Fill<'_>>) -> Vec<Outline> {
    prism_outlines_sided(walls, loops, reach, fill, None)
}

/// [`prism_outlines`], with each loop's `inside` read off the scan where it is known: whether the
/// part's material lies inside the loop (its outline) or outside it (a hole). A loop the scan says
/// is a hole is kept only inside an outline, and a slice with no outline gives none: read off the
/// largest loop instead, a slice that missed the part's rim made its holes' circles the part's
/// outline (a flange at 0.3 mm of noise: plugs extruded where its bolt holes are).
fn prism_outlines_sided(
    walls: &[(usize, Wall)],
    loops: &[(Vec<Vec2>, Vec<Option<usize>>)],
    reach: f64,
    fill: Option<&Fill<'_>>,
    inside: Option<&[bool]>,
) -> Vec<Outline> {
    let mut found: Vec<(Outline, Vec<Vec2>, Option<bool>)> = loops
        .iter()
        .enumerate()
        .filter_map(|(k, (points, on))| {
            let fill = fill.map(|f| (f.tolerance, f.along.get(k).map(Vec::as_slice)));
            let outline = outline_of(walls, points, on, reach, fill)?;
            Some((
                match fill {
                    Some((tolerance, _)) => rounded_as_the_rest(outline, points, tolerance),
                    None => outline,
                },
                inside.and_then(|i| i.get(k).copied()),
            ))
        })
        .map(|(o, side)| {
            let r = ring(&o);
            (o, r, side)
        })
        .filter(|(_, r, _)| is_simple_ring(r))
        .collect();
    found.sort_by(|a, b| area(&b.1).abs().total_cmp(&area(&a.1).abs()));
    let mut kept: Vec<(Outline, Vec<Vec2>, Option<bool>)> = Vec::new();
    for (o, r, side) in found {
        if kept.iter().all(|(_, k, _)| !rings_cross(k, &r)) {
            kept.push((o, r, side));
        }
    }
    if inside.is_some() {
        let outer: Vec<Vec<Vec2>> = kept.iter().filter(|(_, _, side)| *side == Some(true)).map(|(_, r, _)| r.clone()).collect();
        return kept
            .into_iter()
            .filter(|(_, r, side)| *side == Some(true) || outer.iter().any(|o| r.iter().all(|p| contains(o, *p))))
            .map(|(o, _, _)| o)
            .collect();
    }
    let material = kept.first().map_or(1.0, |(_, r, _)| area(r).signum());
    let outer: Vec<Vec<Vec2>> = kept.iter().filter(|(_, r, _)| area(r).signum() == material).map(|(_, r, _)| r.clone()).collect();
    kept.into_iter()
        .filter(|(_, r, _)| area(r).signum() == material || outer.iter().any(|o| r.iter().all(|p| contains(o, *p))))
        .map(|(o, _, _)| o)
        .collect()
}

/// Whether the part's material lies inside the loop `points` (in `plane`'s coordinates), read off
/// the scan's triangles it runs through (`triangles`, one for each point): the scan is wound
/// outward, so each triangle's normal, laid in the plane, points out of the material, to the
/// right of the way the loop runs where the material is on its left. `None` where the triangles
/// say nothing (lying in the plane, or as many each way).
fn material_inside(scan: &Scan, plane: &Plane, points: &[Vec2], triangles: &[usize]) -> Option<bool> {
    let n = points.len();
    if n < 3 || triangles.len() != n {
        return None;
    }
    let (x, y) = (plane.x, plane.y);
    let mut right = 0.0;
    for i in 0..n {
        let Some([a, b, c]) = triangles.get(i).and_then(|&t| scan.corners(t)) else { continue };
        let normal = (b - a).cross(c - a);
        let flat = Vec2::new(normal.dot(x), normal.dot(y));
        let along = points[(i + 1) % n] - points[i];
        right += flat.dot(Vec2::new(along.y, -along.x));
    }
    if right == 0.0 {
        return None;
    }
    // Material on the left of the way it runs: inside a counter-clockwise loop.
    Some((right > 0.0) == (area(points) > 0.0))
}

/// Rounds of one radius are this near alike (a share of the radius).
const ROUNDS_ALIKE: f64 = 0.02;

/// `outline` with a sharp corner between two walls rounded as its other corners are, where the
/// scanner missed that round: two or more of its rounds between straight walls have one radius
/// and turn the same way, and no point of the slice (`points`) lies nearer the corner's tip than
/// half way to where that round would pass. A sharp corner the scanner saw has its slice run into
/// the tip; a round it saw only a little of, too little for Find All to fit, leaves its few points
/// on the round and none at the tip, and a corner it never saw leaves none at all. Rounded so, the
/// corner is what the part's other corners say it is and the scan says nothing against. Where the
/// round would lie too near the tip to tell (within four `tolerance`s) the corner stays sharp.
fn rounded_as_the_rest(outline: Outline, points: &[Vec2], tolerance: f64) -> Outline {
    let n = outline.pieces.len();
    if n < 5 {
        return outline;
    }
    let line = |k: usize| matches!(outline.pieces[k % n], Piece::Line);
    let corner = |k: usize| outline.corners[k % n];
    // The rounds between two straight walls: radius, and which way they turn.
    let rounds: Vec<(f64, f64)> = (0..n)
        .filter_map(|k| {
            let Piece::Arc { center, .. } = outline.pieces[k] else { return None };
            if !line(k + n - 1) || !line(k + 1) || outline.faces[k].is_none() {
                return None;
            }
            let (before, after) = (unit2(corner(k) - corner(k + n - 1)), unit2(corner(k + 2) - corner(k + 1)));
            Some((center.dist(corner(k)), before.cross(after).signum()))
        })
        .collect();
    let Some(&(first, turn)) = rounds.first() else { return outline };
    let radius = rounds.iter().map(|r| r.0).sum::<f64>() / rounds.len() as f64;
    if rounds.len() < 2 || rounds.iter().any(|&(r, t)| (r - radius).abs() > ROUNDS_ALIKE * radius || t != turn) || !(first > 0.0) {
        return outline;
    }
    let mut out = Outline { corners: Vec::new(), pieces: Vec::new(), faces: Vec::new() };
    for k in 0..n {
        let tip = corner(k);
        // A corner between two walls' straight pieces, turning as the rounds do.
        let sharp = line(k + n - 1) && line(k) && outline.faces[(k + n - 1) % n].is_some() && outline.faces[k].is_some();
        let (before, after) = (unit2(tip - corner(k + n - 1)), unit2(corner(k + 1) - tip));
        let bend = before.cross(after);
        let rounded = (sharp && bend.signum() == turn && bend.abs() >= CROSSING)
            .then(|| {
                // The turn's half angle, how far back along each wall the round starts, and how far
                // its middle passes from the tip.
                let half = 0.5 * before.dot(after).clamp(-1.0, 1.0).acos();
                let back = radius * half.tan();
                let clear = radius / half.cos() - radius;
                let room = back < 0.9 * tip.dist(corner(k + n - 1)).min(tip.dist(corner(k + 1)));
                let at_tip = points.iter().any(|p| p.dist(tip) < 0.5 * clear);
                (room && clear > 4.0 * tolerance && !at_tip).then(|| {
                    let (from, to) = (tip - before * back, tip + after * back);
                    let inward = Vec2::new(-before.y, before.x) * turn;
                    (from, to, from + inward * radius)
                })
            })
            .flatten();
        match rounded {
            Some((from, to, center)) => {
                out.corners.extend([from, to]);
                out.pieces.extend([Piece::Arc { center, ccw: turn > 0.0 }, outline.pieces[k]]);
                out.faces.extend([None, outline.faces[k]]);
            }
            None => {
                out.corners.push(tip);
                out.pieces.push(outline.pieces[k]);
                out.faces.push(outline.faces[k]);
            }
        }
    }
    out
}

/// Where a prism's slice may be drawn as it runs between two walls: a gap that strays from where
/// the walls would meet by more than `tolerance` allows is drawn as the slice runs, when every
/// point of it is on scan that runs along the prism (`along`, for each loop each point). A gear's
/// tip and root lands, which Find All leaves as freeform between the flanks, are; a hole or a slot
/// across the prism, met by the slice, is not, and must not be extruded along it.
pub struct Fill<'a> {
    pub tolerance: f64,
    pub along: &'a [Vec<bool>],
}

/// A wall of a prism faces square to it within this (radians), over and above its draft.
const RUNS_ALONG: f64 = 0.1745;
/// The share of a gap's points that must be on scan running along the prism for the gap to be
/// drawn as the slice runs.
const MOSTLY_ALONG: f64 = 0.8;
/// The slices a prism's gaps are checked against lie this share of its depth either side of the
/// middle one.
const OTHER_LEVELS: f64 = 0.25;

/// For each point of each loop of a prism's middle slice, whether the scan there runs along the
/// prism: its triangle (`triangles`, as [`crate::section::Run::triangles`]) faces square to the
/// prism's `direction` (within [`RUNS_ALONG`] and the `draft`), and the slices at `others` (other
/// levels along the prism, each its runs' points) pass within `reach` of it, so the same outline
/// is there too. A prism too short for other levels to tell (`others` empty) goes by facing alone.
fn runs_along(scan: &Scan, loops: &[(&[Vec2], &[usize])], direction: Vec3, draft: f64, others: &[Vec<Vec<Vec2>>], reach: f64) -> Vec<Vec<bool>> {
    let lean = (RUNS_ALONG + draft.abs()).sin();
    // The other levels' segments in a grid of cells `reach` wide or wider, so each point looks
    // only at the segments near it.
    let segments: Vec<Vec<(Vec2, Vec2)>> =
        others.iter().map(|runs| runs.iter().flat_map(|r| (0..r.len()).map(move |i| (r[i], r[(i + 1) % r.len()]))).collect()).collect();
    let all = segments.iter().flatten();
    let (low, high) =
        all.clone().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(l, h), (a, b)| {
            (Vec2::new(l.x.min(a.x).min(b.x), l.y.min(a.y).min(b.y)), Vec2::new(h.x.max(a.x).max(b.x), h.y.max(a.y).max(b.y)))
        });
    let cell = reach.max((high.x - low.x).max(high.y - low.y) / 512.0).max(1e-9);
    // A level with no loop at all (a slice over a patch the scanner missed) leaves `low` without
    // a value: its cells are then all one, and nothing is near, instead of a cell number past
    // the end of the numbers.
    let cell_of = |v: f64| if v.is_finite() { v.floor().clamp(-1e15, 1e15) as i64 } else { 0 };
    let key = |p: Vec2| (cell_of((p.x - low.x) / cell), cell_of((p.y - low.y) / cell));
    let grids: Vec<std::collections::HashMap<(i64, i64), Vec<u32>>> = segments
        .iter()
        .map(|level| {
            let mut grid: std::collections::HashMap<(i64, i64), Vec<u32>> = std::collections::HashMap::new();
            for (k, (a, b)) in level.iter().enumerate() {
                let ((ax, ay), (bx, by)) = (key(*a), key(*b));
                for x in ax.min(bx)..=ax.max(bx) {
                    for y in ay.min(by)..=ay.max(by) {
                        grid.entry((x, y)).or_default().push(k as u32);
                    }
                }
            }
            grid
        })
        .collect();
    let near = |p: Vec2, level: usize| {
        let (x, y) = key(p);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                grids[level].get(&(x + dx, y + dy)).is_some_and(|cell| {
                    cell.iter().any(|&k| segments[level].get(k as usize).is_some_and(|(a, b)| segment_distance(p, *a, *b) <= reach))
                })
            })
        })
    };
    loops
        .iter()
        .map(|(points, triangles)| {
            points
                .iter()
                .zip(triangles.iter())
                .map(|(&p, &t)| {
                    let square =
                        scan.corners(t).and_then(|[a, b, c]| (b - a).cross(c - a).normalized()).is_some_and(|n| n.dot(direction).abs() <= lean);
                    square && (0..segments.len()).all(|level| near(p, level))
                })
                .collect()
        })
        .collect()
}

/// How an outline repeats round its middle: a gear's teeth, a knob's lobes, a spline's keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rotational {
    /// How many times it repeats in one turn (at least 3).
    pub order: usize,
    /// The middle it repeats about, and its largest and least distance from it (a gear's tip and
    /// root radii).
    pub centre: Vec2,
    pub tip: f64,
    pub root: f64,
}

/// The directions round the middle an outline's distance is read in.
const SIGNATURE: usize = 2880;
/// The most times round an outline is tried to repeat.
const MOST_ORDER: usize = 360;
/// Turned by one repeat, the outline lies on itself within this share of how far it rises and
/// falls (or within the tolerance, when that is more).
const REPEATS_WITHIN: f64 = 0.1;

/// How the closed `ring` repeats round its middle, when it does at least 3 times: the least turn
/// that lays the ring on itself again, read off its distance from its middle in [`SIGNATURE`]
/// directions (the mean difference between the distances and the same turned, within `tolerance`
/// or [`REPEATS_WITHIN`] of the rise and fall). `None` for a ring that
/// does not rise and fall by more than the tolerance (a circle repeats any number of times) or
/// repeats fewer than 3 times.
pub fn rotational(ring: &[Vec2], tolerance: f64) -> Option<Rotational> {
    let n = ring.len();
    if n < 3 {
        return None;
    }
    // The middle of the area the ring holds.
    let (mut twice, mut sum) = (0.0, Vec2::ZERO);
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let cross = a.cross(b);
        twice += cross;
        sum += (a + b) * cross;
    }
    if !(twice.abs() > 1e-12) {
        return None;
    }
    let centre = sum / (3.0 * twice);
    // The ring's distance from the middle in each direction: the farthest place the ray meets it.
    let mut reach = vec![0.0f64; SIGNATURE];
    for i in 0..n {
        let (a, b) = (ring[i] - centre, ring[(i + 1) % n] - centre);
        let (from, to) = (a.y.atan2(a.x), b.y.atan2(b.x));
        let mut sweep = to - from;
        if sweep > PI {
            sweep -= TAU;
        } else if sweep < -PI {
            sweep += TAU;
        }
        let (low, high) = if sweep >= 0.0 { (from, from + sweep) } else { (from + sweep, from) };
        let step = TAU / SIGNATURE as f64;
        let (first, last) = ((low / step).ceil() as i64, (high / step).floor() as i64);
        for k in first..=last {
            let angle = k as f64 * step;
            let ray = Vec2::new(angle.cos(), angle.sin());
            // Where the ray from the middle crosses the line through a and b.
            let across = ray.cross(b - a);
            if across.abs() < 1e-15 {
                continue;
            }
            let distance = a.cross(b - a) / across;
            let slot = k.rem_euclid(SIGNATURE as i64) as usize;
            if distance.is_finite() && distance > reach[slot] {
                reach[slot] = distance;
            }
        }
    }
    if reach.iter().any(|r| !(*r > 0.0)) {
        return None;
    }
    let (tip, root) = (reach.iter().copied().fold(0.0, f64::max), reach.iter().copied().fold(f64::INFINITY, f64::min));
    let mean = reach.iter().sum::<f64>() / SIGNATURE as f64;
    let rise = reach.iter().map(|r| (r - mean).abs()).sum::<f64>() / SIGNATURE as f64;
    if !(rise > tolerance) {
        return None;
    }
    let within = tolerance.max(REPEATS_WITHIN * rise);
    // The distance in a direction between two of those read, taken in a line between them.
    let at = |turn: f64| {
        let place = turn.rem_euclid(SIGNATURE as f64);
        let (low, part) = (place.floor() as usize % SIGNATURE, place.fract());
        reach[low] * (1.0 - part) + reach[(low + 1) % SIGNATURE] * part
    };
    // How far the ring is from itself turned by a share of a turn: nothing unturned, rising as it
    // is turned, and back to nothing when it has turned by one repeat. The first turn at which it
    // comes back, once it has left, is one repeat (a smooth outline is near itself for any small
    // turn, which is no repeat).
    let off = |shift: f64| (0..SIGNATURE).map(|j| (reach[j] - at(j as f64 + shift)).abs()).sum::<f64>() / SIGNATURE as f64;
    let turned: Vec<f64> = (0..=SIGNATURE / 3).map(|j| off(j as f64)).collect();
    let most = turned.iter().copied().fold(0.0, f64::max);
    let left = turned.iter().position(|e| *e > (3.0 * within).max(0.5 * most))?;
    let back = (left..turned.len()).find(|&j| turned[j] <= within && turned.get(j + 1).is_none_or(|next| *next >= turned[j]))?;
    let order = (SIGNATURE as f64 / back as f64).round() as usize;
    if !((3..=MOST_ORDER).contains(&order) && off(SIGNATURE as f64 / order as f64) <= within) {
        return None;
    }
    Some(Rotational { order, centre, tip, root })
}

/// How far outside the closed `outline` the point `p` is: its distance from the nearest piece
/// where it is on the side of that piece away from the inside, 0 inside. `around` is 1 for an
/// outline that goes round counter-clockwise, -1 for the other way.
fn outside_of(outline: &Outline, around: f64, p: Vec2) -> f64 {
    let n = outline.corners.len();
    let mut best = (f64::INFINITY, 0.0);
    for k in 0..n {
        let (a, b) = (outline.corners[k], outline.corners[(k + 1) % n]);
        let (distance, out) = match outline.pieces[k] {
            Piece::Line => (segment_distance(p, a, b), -around * (b - a).cross(p - a)),
            Piece::Arc { center, ccw } => {
                let radius = a.dist(center);
                let turn = |to: Vec2| {
                    let t = angle_to(a - center, to - center);
                    if ccw && t < 0.0 {
                        t + TAU
                    } else if !ccw && t > 0.0 {
                        t - TAU
                    } else {
                        t
                    }
                };
                let (sweep, at) = (turn(b), turn(p));
                let sweep = if sweep == 0.0 { if ccw { TAU } else { -TAU } } else { sweep };
                let off = p.dist(center) - radius;
                let distance = if at.abs() <= sweep.abs() { off.abs() } else { p.dist(a).min(p.dist(b)) };
                (distance, around * if ccw { off } else { -off })
            }
        };
        if distance < best.0 {
            best = (distance, out);
        }
    }
    if best.1 > 0.0 { best.0 } else { 0.0 }
}

/// How far the scan stands proud of a prism: of its `outline` (in `plane`, its normal along the
/// prism) between the levels `low` and `high` along the plane's normal from its origin, and of
/// the end it is sat on (`seat`: the level of that end, and 1 when the prism lies above it, -1
/// below). The greater of: the greatest distance outside the outline of a corner of a scan
/// triangle that runs along the prism (its normal within 30 degrees of the plane) between the
/// levels; and the greatest distance past the seat of a corner of a triangle that faces along the
/// prism, over the outline. A pocket offset from the outline by a clearance, and that much deeper
/// than the prism, is this much tighter there, so a nest adds it. A corner further out than
/// `reach` is another part of the thing, not this prism astray, and is not counted. 0 for an
/// outline with no pieces.
pub fn proud(scan: &Scan, plane: &Plane, outline: &Outline, low: f64, high: f64, seat: Option<(f64, f64)>, reach: f64) -> f64 {
    let n = outline.corners.len();
    if n < 2 || outline.pieces.len() != n || !(reach > 0.0) {
        return 0.0;
    }
    let around = if area(&ring(outline)) >= 0.0 { 1.0 } else { -1.0 };
    let (lo, hi) = outline.corners.iter().fold((outline.corners[0], outline.corners[0]), |(lo, hi), c| {
        (Vec2::new(lo.x.min(c.x), lo.y.min(c.y)), Vec2::new(hi.x.max(c.x), hi.y.max(c.y)))
    });
    // An arc's circle may bulge past its ends' box by up to its diameter.
    let bulge = (0..n).fold(0.0f64, |m, k| match outline.pieces[k] {
        Piece::Arc { center, .. } => m.max(2.0 * outline.corners[k].dist(center)),
        Piece::Line => m,
    });
    let pad = bulge + reach;
    let normal = plane.normal();
    let mut worst = 0.0f64;
    for t in 0..scan.triangle_count() {
        let Some(corners) = scan.corners(t) else { continue };
        let Some(facing) = (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalized() else { continue };
        let along = facing.dot(normal).abs() <= 0.5;
        for c in corners {
            let d = c - plane.origin;
            let level = d.dot(normal);
            let p = Vec2::new(d.dot(plane.x), d.dot(plane.y));
            if p.x < lo.x - pad || p.x > hi.x + pad || p.y < lo.y - pad || p.y > hi.y + pad {
                continue;
            }
            let out = if along {
                if level < low || level > high {
                    continue;
                }
                outside_of(outline, around, p)
            } else {
                let Some((end, above)) = seat else { continue };
                let past = (end - level) * above;
                if !(past > 0.0 && past <= reach) || outside_of(outline, around, p) > 0.0 {
                    continue;
                }
                past
            };
            if out <= reach {
                worst = worst.max(out);
            }
        }
    }
    worst
}

/// `outline` without the pieces an offset outward by `reach` would close to nothing, their
/// neighbours run on to where they meet: the outline a pocket round the part is offset from. A
/// piece between two inside corners gets shorter as it is offset outward (each end by the reach
/// times the tangent of half the corner's turn), and an offset that has to keep every piece
/// cannot be made once one is gone: a scanned gear's roots are short pieces a few tenths long. Cut
/// square there, the outline lies inside the part's by less than 0.42 of the dropped corner's
/// round, which is under the reach. Counter-clockwise or clockwise alike; an outline with fewer
/// than four pieces, or whose neighbours would not meet near a piece, is left as it is there.
pub fn roomy(outline: &Outline, reach: f64) -> Outline {
    let mut out = outline.clone();
    if !(reach > 0.0) {
        return out;
    }
    let ring = ring(outline);
    let area: f64 = (0..ring.len()).map(|i| ring[i].cross(ring[(i + 1) % ring.len()])).sum();
    let around = if area >= 0.0 { 1.0 } else { -1.0 };
    loop {
        let n = out.corners.len();
        if n < 4 || out.pieces.len() != n {
            return out;
        }
        // A piece's direction as it leaves its first corner and as it reaches its second.
        let tangents = |k: usize| -> Option<(Vec2, Vec2)> {
            let (a, b) = (out.corners[k], out.corners[(k + 1) % n]);
            match out.pieces[k] {
                Piece::Line => (b - a).normalized().map(|d| (d, d)),
                Piece::Arc { center, ccw } => {
                    let turn = if ccw { 1.0 } else { -1.0 };
                    Some(((a - center).perp().normalized()? * turn, (b - center).perp().normalized()? * turn))
                }
            }
        };
        // How long each piece is once offset: its own length (an arc's on its new radius), less at
        // an inside corner and more at an outside one.
        let after = |k: usize| -> Option<f64> {
            let (a, b) = (out.corners[k], out.corners[(k + 1) % n]);
            let (leaves, arrives) = tangents(k)?;
            let length = match out.pieces[k] {
                Piece::Line => a.dist(b),
                Piece::Arc { center, ccw } => {
                    let radius = a.dist(center);
                    let mut sweep = angle_to(a - center, b - center);
                    if ccw && sweep <= 0.0 {
                        sweep += TAU;
                    } else if !ccw && sweep >= 0.0 {
                        sweep -= TAU;
                    }
                    // An arc bulging out of the outline grows with the offset, one bulging in shrinks.
                    let grown = radius + reach * around * if ccw { 1.0 } else { -1.0 };
                    grown.max(0.0) * sweep.abs()
                }
            };
            let before = tangents((k + n - 1) % n)?.1;
            let next = tangents((k + 1) % n)?.0;
            // A corner's turn, positive the way the outline goes round (an outside corner).
            let corner = |from: Vec2, to: Vec2| reach * (around * angle_to(from, to) / 2.0).clamp(-1.5, 1.5).tan();
            Some(length + corner(before, leaves) + corner(arrives, next))
        };
        let gone = (0..n).filter_map(|k| Some((after(k)?, k))).filter(|(left, _)| *left <= 0.0).min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, k)) = gone else { return out };
        // Its neighbours' own line or circle, run on to where they meet near it.
        let carrier = |j: usize| {
            let (a, b) = (out.corners[j], out.corners[(j + 1) % n]);
            match out.pieces[j] {
                Piece::Line => Wall::Line { point: a, direction: unit2(b - a) },
                Piece::Arc { center, .. } => Wall::Circle { center, radius: a.dist(center) },
            }
        };
        let (before, next) = ((k + n - 1) % n, (k + 1) % n);
        let (a, b) = (out.corners[k], out.corners[next]);
        let middle = (a + b) * 0.5;
        let Some(meet) = cross(&carrier(before), &carrier(next), middle).filter(|c| c.dist(middle) <= a.dist(b) + 4.0 * reach) else { return out };
        out.corners[next] = meet;
        out.corners.remove(k);
        out.pieces.remove(k);
        out.faces.remove(k);
    }
}

/// How an outline is its own mirror image in a line along its plane's X or Y.
#[derive(Clone, Debug, PartialEq)]
pub struct Mirrored {
    /// The line: `across` true for one along Y (at X = `at`, left and right alike), false for one
    /// along X (at Y = `at`, top and bottom alike).
    pub across: bool,
    pub at: f64,
    /// The corners that are each other's image, by their places in the outline (the lower first).
    pub pairs: Vec<(usize, usize)>,
}

/// The lines along X or Y through the middle of `outline`'s box in which it is its own mirror
/// image within `tolerance`: every corner has its image among the corners (or lies on the line),
/// and every piece its image among the pieces (a line a line, an arc an arc about the image of
/// its centre). A part drawn symmetric and scanned comes out so to within the scan's rounding; a
/// sketch that holds the images to each other then needs one size where it had two.
pub fn mirrored(outline: &Outline, tolerance: f64) -> Vec<Mirrored> {
    let n = outline.corners.len();
    if n < 3 || outline.pieces.len() != n {
        return Vec::new();
    }
    let ring = ring(outline);
    let (lo, hi) =
        ring.iter().fold((ring[0], ring[0]), |(lo, hi), p| (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y))));
    let middle = (lo + hi) * 0.5;
    let mut found = Vec::new();
    for across in [true, false] {
        let at = if across { middle.x } else { middle.y };
        let image = |p: Vec2| if across { Vec2::new(2.0 * at - p.x, p.y) } else { Vec2::new(p.x, 2.0 * at - p.y) };
        // Each corner's image among the corners.
        let partner: Option<Vec<usize>> = (0..n).map(|k| (0..n).find(|&j| outline.corners[j].dist(image(outline.corners[k])) <= tolerance)).collect();
        let Some(partner) = partner else { continue };
        if (0..n).any(|k| partner[partner[k]] != k) {
            continue;
        }
        // Each piece's image runs between its ends' images, the other way round.
        let alike = (0..n).all(|k| {
            let (a, b) = (partner[k], partner[(k + 1) % n]);
            let j = if (b + 1) % n == a { b } else { return false };
            match (outline.pieces[k], outline.pieces[j]) {
                (Piece::Line, Piece::Line) => true,
                (Piece::Arc { center, ccw }, Piece::Arc { center: other, ccw: turns }) => other.dist(image(center)) <= tolerance && ccw == turns,
                _ => false,
            }
        });
        if !alike {
            continue;
        }
        let pairs: Vec<(usize, usize)> = (0..n).filter(|&k| k < partner[k]).map(|k| (k, partner[k])).collect();
        if !pairs.is_empty() {
            found.push(Mirrored { across, at, pairs });
        }
    }
    found
}

/// An outline that repeats round its middle, as a disc with one gap cut out of it and repeated:
/// a gear as its tip circle and the space between two teeth.
#[derive(Clone, Debug, PartialEq)]
pub struct Repeated {
    /// How many gaps there are round it.
    pub order: usize,
    /// The middle, and the radius of the circle the outline's outermost stretches lie on.
    pub centre: Vec2,
    pub tip: f64,
    /// One gap, closed outside the circle: the outline from where it leaves the circle to where
    /// it comes back to it, then out, round and back in.
    pub gap: Outline,
}

/// `outline` as a circle with one gap repeated round it, when it [`rotational`]ly repeats and has
/// as many gaps as it repeats: a gap is a run of pieces that dip more than twice `tolerance`
/// below the outermost radius, between stretches that stay up there (a gear's tip lands). The gap whose width is the middle one of them all is taken; its
/// first and last pieces run on to the circle where they come within four tolerances of it. The
/// circle's radius is the middle one of what stays up, as `read` reads it (a size read as
/// designed). `None` for an outline that does not repeat, or whose gaps are not one for each
/// repeat.
pub fn repeated(outline: &Outline, tolerance: f64, read: impl Fn(f64) -> f64) -> Option<Repeated> {
    let n = outline.corners.len();
    if n < 4 || outline.pieces.len() != n {
        return None;
    }
    let found = rotational(&ring(outline), tolerance)?;
    let centre = found.centre;
    let radius = |p: Vec2| p.dist(centre);
    // A piece's own points: its ends, and for an arc seven between.
    let points_of = |k: usize| -> Vec<Vec2> {
        let (a, b) = (outline.corners[k], outline.corners[(k + 1) % n]);
        let mut points = vec![a];
        if let Piece::Arc { center, ccw } = outline.pieces[k] {
            let mut turn = angle_to(a - center, b - center);
            if ccw && turn <= 0.0 {
                turn += TAU;
            } else if !ccw && turn >= 0.0 {
                turn -= TAU;
            }
            points.extend((1..8).map(|i| center + rotated(a - center, turn * f64::from(i) / 8.0)));
        }
        points.push(b);
        points
    };
    let margin = 2.0 * tolerance;
    let up = |p: Vec2| radius(p) >= found.tip - margin;
    let land: Vec<bool> = (0..n).map(|k| points_of(k).into_iter().all(up)).collect();
    // The circle is the part's own only where stretches of the outline lie on it, one between each
    // two gaps (a square's corners touch a circle too, and a square is not a disc cut four times).
    let lands = (0..n).filter(|&k| land[k] && !land[(k + n - 1) % n]).count();
    if lands != found.order {
        return None;
    }
    // Its radius: the middle one of the lands' points.
    let mut high: Vec<f64> = (0..n).filter(|&k| land[k]).flat_map(points_of).map(radius).collect();
    high.sort_by(f64::total_cmp);
    let tip = read(*high.get(high.len() / 2)?);
    // The gaps: runs of pieces that are not land, each from a corner up at the circle to the next.
    let first = (0..n).find(|&k| !land[k] && (land[(k + n - 1) % n] || up(outline.corners[k])))?;
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < n {
        let from = (first + k) % n;
        if land[from] {
            k += 1;
            continue;
        }
        let mut count = 1;
        while k + count < n && !land[(first + k + count) % n] && !up(outline.corners[(first + k + count) % n]) {
            count += 1;
        }
        gaps.push((from, count));
        k += count;
    }
    if gaps.len() != found.order || gaps.iter().any(|&(from, count)| !up(outline.corners[from]) || !up(outline.corners[(from + count) % n])) {
        return None;
    }
    let width = |&(from, count): &(usize, usize)| outline.corners[from].dist(outline.corners[(from + count) % n]);
    let mut by_width = gaps.clone();
    by_width.sort_by(|a, b| width(a).total_cmp(&width(b)));
    let (from, count) = by_width[by_width.len() / 2];
    let circle = Wall::Circle { center: centre, radius: tip };
    // A piece's own line or circle, run on to the tip circle near `end`.
    let run_on = |k: usize, end: Vec2| {
        let (a, b) = (outline.corners[k], outline.corners[(k + 1) % n]);
        let own = match outline.pieces[k] {
            Piece::Line => Wall::Line { point: a, direction: unit2(b - a) },
            Piece::Arc { center, .. } => Wall::Circle { center, radius: a.dist(center) },
        };
        cross(&own, &circle, end).filter(|c| c.dist(end) <= 4.0 * tolerance).unwrap_or(end)
    };
    let (start, end) = (run_on(from, outline.corners[from]), run_on((from + count - 1) % n, outline.corners[(from + count) % n]));
    let mut corners = vec![start];
    corners.extend((1..count).map(|i| outline.corners[(from + i) % n]));
    corners.push(end);
    let mut pieces: Vec<Piece> = (0..count).map(|i| outline.pieces[(from + i) % n]).collect();
    // Out past the circle, back round to above the start, and in.
    let outside = tip + (0.1 * tip).max(8.0 * tolerance);
    let out = |p: Vec2| centre + unit2(p - centre) * outside;
    let forward = angle_to(start - centre, end - centre) > 0.0;
    corners.extend([out(end), out(start)]);
    pieces.extend([Piece::Line, Piece::Arc { center: centre, ccw: !forward }, Piece::Line]);
    let faces = vec![None; pieces.len()];
    Some(Repeated { order: found.order, centre, tip, gap: Outline { corners, pieces, faces } })
}

/// A round of an outline taken out for a sharp corner ([`sharpened`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharpCorner {
    /// Where the two lines either side of the round cross.
    pub at: Vec2,
    /// Each line's unit normal toward the material (the side the outline's walls lean to when the
    /// prism is drafted).
    pub inward: [Vec2; 2],
    /// The round's radius.
    pub radius: f64,
}

impl SharpCorner {
    /// The corner where the two lines have each moved `by` toward the material: a drafted prism's
    /// corner that far up its walls' lean.
    pub fn moved(&self, by: f64) -> Vec2 {
        let [a, b] = self.inward;
        let meet = 1.0 + a.dot(b);
        if meet.abs() < 1e-9 { self.at } else { self.at + (a + b) * (by / meet) }
    }
}

/// `outlines` with each round between two straight lines taken out, the lines run on to where they
/// cross, for the rounds `constant` gives a radius (by the face the round lies on: a round the
/// scan shows with one radius all the way along, where a drafted prism's tapered walls would
/// narrow it). The corners made, for a fillet of each one's radius to round again. `outlines[0]`
/// is the material's outer loop: which side of each line the material is on is read off it.
pub fn sharpened(outlines: &[Outline], constant: impl Fn(usize) -> Option<f64>) -> (Vec<Outline>, Vec<SharpCorner>) {
    let material = outlines.first().map_or(1.0, |o| area(&ring(o)).signum());
    let mut corners = Vec::new();
    let sharp = outlines
        .iter()
        .map(|o| {
            let n = o.pieces.len();
            let line = |k: usize| matches!(o.pieces[k % n], Piece::Line);
            // The corner each round is taken out for, by the round's place.
            let made: Vec<Option<Vec2>> = (0..n)
                .map(|k| {
                    let radius = match (o.pieces[k], o.faces.get(k).copied().flatten()) {
                        (Piece::Arc { .. }, Some(face)) => constant(face)?,
                        _ => return None,
                    };
                    if n < 5 || !line(k + n - 1) || !line(k + 1) {
                        return None;
                    }
                    let (before, after) = (unit2(o.corners[k] - o.corners[(k + n - 1) % n]), unit2(o.corners[(k + 2) % n] - o.corners[(k + 1) % n]));
                    let turn = before.cross(after);
                    if turn.abs() < CROSSING {
                        return None;
                    }
                    let at = o.corners[k] + before * ((o.corners[(k + 1) % n] - o.corners[k]).cross(after) / turn);
                    let inward = |d: Vec2| Vec2::new(-d.y, d.x) * material;
                    corners.push(SharpCorner { at, inward: [inward(before), inward(after)], radius });
                    Some(at)
                })
                .collect();
            let mut out = Outline { corners: Vec::new(), pieces: Vec::new(), faces: Vec::new() };
            for k in 0..n {
                if made[k].is_some() {
                    continue;
                }
                out.corners.push(made[(k + n - 1) % n].unwrap_or(o.corners[k]));
                out.pieces.push(o.pieces[k]);
                out.faces.push(o.faces.get(k).copied().flatten());
            }
            out
        })
        .collect();
    (sharp, corners)
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

/// A stretch of a loop on one wall: the wall (its place in `walls`), where it starts and ends
/// (and how far along the loop from where the walk began, in points), and how far round the
/// wall's centre it turns (a circle's, counter-clockwise positive).
#[derive(Clone, Copy, Debug)]
struct Stretch {
    wall: usize,
    first: Vec2,
    last: Vec2,
    first_at: usize,
    last_at: usize,
    turn: f64,
}

/// The loop's outline on the walls (see [`prism_outlines`]). With `fill` (a tolerance), a gap
/// between two walls whose points stray further from where the walls would meet than the
/// tolerance allows (faces Find All did not find) is drawn as the loop runs there instead, in
/// lines and arcs fitted to it (SurfaceCraft's chain fitter), no face holding them; with the
/// fill's flags (one for each point), only a gap whose points are all flagged ([`Fill`]).
fn outline_of(walls: &[(usize, Wall)], points: &[Vec2], on: &[Option<usize>], reach: f64, fill: Option<(f64, Option<&[bool]>)>) -> Option<Outline> {
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
            let stretch = Stretch { wall, first: points[k], last: points[(start + j) % n], first_at: i, last_at: j, turn };
            match stretches.last_mut() {
                // One wall again past a stretch passed over: one stretch.
                Some(previous) if previous.wall == wall => {
                    let gap = centre_of(wall).map_or(0.0, |c| angle_to(previous.last - c, stretch.first - c));
                    previous.turn += stretch.turn + gap;
                    previous.last = stretch.last;
                    previous.last_at = stretch.last_at;
                }
                _ => stretches.push(stretch),
            }
        }
        i = j + 1;
    }
    if stretches.len() > 1 && stretches[0].wall == stretches[stretches.len() - 1].wall {
        let last = stretches.pop()?;
        stretches[0].first = last.first;
        stretches[0].first_at = last.first_at;
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
    // Each stretch's wall as the loop runs on it. A round wall is the face's cylinder, fitted over
    // all of the face; where the loop's own points on it stray from that by more than the
    // tolerance allows (a gear's flank is no arc of a circle, and the cylinder fitted to one sat
    // 0.1 mm off its middle), the circle is fitted to those points instead, when they do lie on
    // one within the tolerance.
    let mut own: Vec<Wall> = stretches.iter().map(|s| walls[s.wall].1).collect();
    if let Some((tolerance, _)) = fill {
        for (stretch, wall) in stretches.iter_mut().zip(&mut own) {
            if !matches!(wall, Wall::Circle { .. }) {
                continue;
            }
            let end = if stretch.last_at >= stretch.first_at { stretch.last_at } else { stretch.last_at + n };
            let run: Vec<Vec2> =
                (stretch.first_at..=end).map(|i| (start + i) % n).filter(|&i| on[i] == Some(stretch.wall)).map(|i| points[i]).collect();
            let strays = |wall: &Wall| {
                let mut off: Vec<f64> = run.iter().map(|q| wall.foot(*q).dist(*q)).collect();
                off.sort_by(f64::total_cmp);
                off.get(off.len() * 9 / 10).copied().unwrap_or(0.0)
            };
            if run.len() < REFIT_POINTS || strays(wall) <= tolerance {
                continue;
            }
            let Some((center, radius)) = circle_through(&run) else { continue };
            let fitted = Wall::Circle { center, radius };
            let turn: f64 = run.windows(2).map(|w| angle_to(w[0] - center, w[1] - center)).sum();
            if strays(&fitted) <= tolerance && turn * stretch.turn > 0.0 {
                *wall = fitted;
                stretch.turn = turn;
            }
        }
    }
    let wall = |r: usize| own[r % count];
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
        // The loop's own points between the two stretches, when they stray from where the walls
        // would meet: drawn as the loop runs.
        let (from, to) = (a.foot(here.last), b.foot(next.first));
        let filled = fill.and_then(|(tolerance, flags)| {
            let end = if next.first_at > here.last_at { next.first_at } else { next.first_at + n };
            let between: Vec<Vec2> = (here.last_at..=end).map(|i| points[(start + i) % n]).collect();
            // Most of what the gap runs on must run along the prism: a scan's noise tilts single
            // small triangles tens of degrees, a hole's wall across the prism nearly all of them.
            if let Some(flags) = flags {
                let off_walls: Vec<usize> = (here.last_at + 1..end).map(|i| (start + i) % n).filter(|&i| on[i].is_none()).collect();
                let along = off_walls.iter().filter(|&&i| flags.get(i) == Some(&true)).count();
                if (along as f64) < MOSTLY_ALONG * off_walls.len() as f64 {
                    return None;
                }
            }
            let path: Vec<Vec2> = match crossing {
                Some(c) => vec![from, c, to],
                None => vec![from, to],
            };
            let off =
                between.iter().map(|&p| path.windows(2).map(|w| segment_distance(p, w[0], w[1])).fold(f64::INFINITY, f64::min)).fold(0.0, f64::max);
            if between.len() < 4 || off <= (4.0 * tolerance).max(reach) {
                return None;
            }
            match crate::chain::chain(&between, false, tolerance)? {
                crate::chain::Outline::Chain(chain) if !chain.is_empty() => Some((chain, tolerance)),
                _ => None,
            }
        });
        if let Some((mut chain, tolerance)) = filled {
            let kind = |p: &crate::chain::Piece| match *p {
                crate::chain::Piece::Line { .. } => Piece::Line,
                crate::chain::Piece::Arc { center, ccw, .. } => Piece::Arc { center, ccw },
            };
            let straight = |p: &crate::chain::Piece| matches!(p, crate::chain::Piece::Line { .. });
            // A straight piece of the chain that still runs on the wall it leaves, or already on
            // the one it comes to (the faces found stop a little short of their edges), is that
            // wall's: the wall runs on to its end.
            let on_wall =
                |wall: &Wall, p: &crate::chain::Piece| straight(p) && [p.from(), p.to()].iter().all(|q| wall.foot(*q).dist(*q) <= tolerance);
            let (mut from, mut to) = (from, to);
            while chain.len() > 1 && chain.first().is_some_and(|p| on_wall(&a, p)) {
                from = a.foot(chain.remove(0).to());
            }
            while chain.len() > 1 && chain.last().is_some_and(|p| on_wall(&b, p)) {
                to = b.foot(chain.pop().map_or(to, |p| p.from()));
            }
            // From the wall's end to the chain's start, along the chain, and on to the next wall. A
            // straight piece starts or ends on the wall when it comes within the tolerance of it;
            // an arc keeps its own ends (they are on its circle), joined by a line.
            let near = |p: Vec2, q: Vec2| p.dist(q) <= 2.0 * tolerance;
            let mut at = from;
            for p in &chain {
                if at.dist(p.from()) > 1e-9 && !(straight(p) && near(at, p.from())) {
                    corners.push(at);
                    pieces.push(Piece::Line);
                    faces.push(None);
                    at = p.from();
                }
                corners.push(at);
                pieces.push(kind(p));
                faces.push(None);
                at = p.to();
            }
            if at.dist(to) > 1e-9 && !(chain.last().is_some_and(straight) && near(at, to)) {
                corners.push(at);
                pieces.push(Piece::Line);
                faces.push(None);
            }
            corners.push(to);
            pieces.push(piece(&b, &next));
            faces.push(Some(walls[next.wall].0));
            continue;
        }
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

/// The fewest points of a loop on a round wall for the wall to be fitted to them ([`outline_of`]).
const REFIT_POINTS: usize = 8;

/// The circle nearest `points` (least squares of the distances to it): its centre and radius.
/// `None` for fewer than three points or points in a line.
fn circle_through(points: &[Vec2]) -> Option<(Vec2, f64)> {
    let n = points.len() as f64;
    if points.len() < 3 {
        return None;
    }
    let middle = points.iter().fold(Vec2::ZERO, |s, p| s + *p) / n;
    // First by the squares of the distances (a linear fit), about the points' middle.
    let (mut xx, mut xy, mut yy, mut xz, mut yz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for p in points {
        let d = *p - middle;
        let z = d.x * d.x + d.y * d.y;
        xx += d.x * d.x;
        xy += d.x * d.y;
        yy += d.y * d.y;
        xz += d.x * z;
        yz += d.y * z;
    }
    let det = xx * yy - xy * xy;
    if !(det.abs() > 1e-12 * (xx + yy).powi(2)) {
        return None;
    }
    let mut center = middle + Vec2::new((xz * yy - yz * xy) / det, (xx * yz - xy * xz) / det) * 0.5;
    let mean_radius = |c: Vec2| points.iter().map(|p| p.dist(c)).sum::<f64>() / n;
    // Then by the distances themselves: a few steps of Gauss and Newton on the centre.
    for _ in 0..8 {
        let radius = mean_radius(center);
        let (mut a, mut b, mut c, mut gx, mut gy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let mean = points.iter().fold(Vec2::ZERO, |s, p| s + unit2(*p - center)) / n;
        for p in points {
            let u = unit2(*p - center);
            let (jx, jy) = (mean.x - u.x, mean.y - u.y);
            let residual = p.dist(center) - radius;
            a += jx * jx;
            b += jx * jy;
            c += jy * jy;
            gx += jx * residual;
            gy += jy * residual;
        }
        let det = a * c - b * b;
        if !(det.abs() > 1e-18) {
            break;
        }
        let step = Vec2::new(-(gx * c - gy * b) / det, -(a * gy - b * gx) / det);
        if !(step.x.is_finite() && step.y.is_finite()) {
            break;
        }
        center += step;
        if step.dist(Vec2::ZERO) < 1e-12 * radius.max(1.0) {
            break;
        }
    }
    let radius = mean_radius(center);
    (radius.is_finite() && radius > 0.0 && center.x.is_finite() && center.y.is_finite()).then_some((center, radius))
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

/// How far `p` is from the segment from `a` to `b`.
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let t = if d.len2() > 0.0 { ((p - a).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
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
    /// The two ends of what the scan has of it (its two points farthest apart), in height and
    /// radius: where a face the scan has only one neighbour of stops.
    pub extent: [Vec2; 2],
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
    // One run of faces open at both ends (a scan that missed the faces between them: the hidden
    // side of a mounted disc) is closed by a line across from end to end, one more node.
    let closing = faces.len() + 1;
    let ends: Vec<usize> = beside.iter().filter(|(k, others)| **k != axis && others.len() == 1).map(|(k, _)| *k).collect();
    if let [a, b] = ends[..]
        && beside.iter().all(|(k, others)| others.len() == 2 || *k == a || *k == b)
        && connected(&beside, a, b)
    {
        for end in [a, b] {
            let meeting = beside[&end][0];
            let at = met[&(end.min(meeting), end.max(meeting))];
            let [p, q] = faces[end].extent;
            met.insert((end, closing), if p.dist(at) >= q.dist(at) { p } else { q });
        }
        beside.entry(a).or_default().push(closing);
        beside.entry(b).or_default().push(closing);
        beside.insert(closing, vec![a, b]);
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
    // The closing line last: a sketch is dimensioned line by line in order, so by its turn its ends
    // are already held, and the bridge across what the scan never saw follows the faces' sizes
    // rather than holding one of its own (which made a brake disc's profile refuse a longer face).
    if let Some(i) = order.iter().position(|&k| k == closing) {
        order.rotate_left(i + 1);
    }
    let face_of = |k: usize| faces.get(k).map(|f| &f.face);
    let n = order.len();
    let mut corners = Vec::with_capacity(n);
    for k in 0..n {
        let (before, after) = (order[(k + n - 1) % n], order[k]);
        let at = met[&(before.min(after), before.max(after))];
        // Where the closing line meets a face: the end of the face, on its curve.
        let corner = if before == closing || after == closing {
            onto(curve(face_of(if before == closing { after } else { before })), at)
        } else {
            crossing(curve(face_of(before)), curve(face_of(after)), at)
        };
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
    Ok(Outline { corners, pieces, faces: order.iter().map(|&k| (k < faces.len()).then_some(k)).collect() })
}

/// Whether `a` and `b` are joined through `beside`.
fn connected(beside: &BTreeMap<usize, Vec<usize>>, a: usize, b: usize) -> bool {
    let mut seen = BTreeSet::from([a]);
    let mut next = vec![a];
    while let Some(k) = next.pop() {
        for &j in beside.get(&k).map_or(&[][..], Vec::as_slice) {
            if seen.insert(j) {
                next.push(j);
            }
        }
    }
    seen.contains(&b) && seen.len() == beside.len()
}

/// The point of `curve` nearest `p`.
fn onto(curve: Curve, p: Vec2) -> Vec2 {
    match curve {
        Curve::Line { point, direction } => point + direction * (p - point).dot(direction),
        Curve::Circle { center, radius } => center + (p - center).normalized().unwrap_or(Vec2::X) * radius,
    }
}

// ---------------------------------------------------------------------------------------------
// Profiles read off a scan.

/// The slice a prism's profile lies in: halfway along it, square to it; its X and Y two
/// directions square to the prism (X × Y along it).
pub fn prism_plane(prism: &Prism) -> Plane {
    let (x, y) = crate::in_plane(prism.direction);
    Plane { origin: prism.middle(), x, y }
}

/// The plane a prism's profile is sketched in: halfway along it, extruded both ways; for a
/// drafted prism, at its `low` end, extruded along its direction with the walls leaning in.
pub fn prism_sketch_plane(prism: &Prism) -> Plane {
    let plane = prism_plane(prism);
    if prism.draft == 0.0 {
        return plane;
    }
    Plane { origin: prism.origin + prism.direction * prism.low, ..plane }
}

/// The levels a square prism is sliced at when the slice halfway along misses one of its walls,
/// as shares of its depth from halfway, nearest first.
const TRY_LEVELS: [f64; 6] = [0.125, -0.125, 0.25, -0.25, 0.375, -0.375];
/// A run of the cut is a loop when its ends are no further apart than this share of its length.
const NEARLY_CLOSED: f64 = 0.2;
/// The most of a loop's gaps the walls may bridge, as a share of what the scan holds of it.
const BRIDGED_MOST: f64 = 1.0;

/// Whether an open run of a cut all but comes back to where it began.
fn nearly_closed(points: &[Vec2]) -> bool {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return false };
    let length: f64 = points.windows(2).map(|w| w[0].dist(w[1])).sum();
    points.len() >= 3 && first.dist(*last) <= NEARLY_CLOSED * length
}

/// The runs of a cut that are loops: the closed ones, those whose ends nearly meet, and the open
/// pieces of one loop that patches the scanner missed broke apart, joined end to end in the order
/// they go round (each to the nearest end still free, until the nearest is the first piece's own
/// start) where the gaps between them are together no more than [`NEARLY_CLOSED`] of their length.
/// A gap the walls carry the outline across is not counted: the outline is drawn from the walls,
/// not the cut, so the walls run on across it. `wall` names the wall a triangle of the scan is on
/// (its face, and its curve in the slice); a gap is carried when its two ends are on one wall, or
/// on two that meet within the gap (the corner a missing patch hid), and such gaps may be up to
/// [`BRIDGED_MOST`] of the length the scan holds (a scan with a third of it missing). Two walls
/// that do not meet there (a side and a hole near it) are no neighbours in any outline: a piece
/// never goes on from the one to the other, however near. Where an end is on no wall, the gap is
/// an ordinary one. Each piece goes on to the end that costs least, a gap the walls carry weighing
/// as much less as it is allowed to be longer.
fn loops_of(runs: Vec<crate::section::Run>, wall: &dyn Fn(usize) -> Option<(usize, Wall)>) -> Vec<crate::section::Run> {
    let (mut loops, mut open): (Vec<_>, Vec<_>) = runs.into_iter().partition(|r| r.closed || nearly_closed(&r.points));
    let length = |points: &[Vec2]| points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    // Whether the walls carry the outline across the gap from `a` (on the triangle `s`) to `b`
    // (on `t`): `Some(true)` when they do, `Some(false)` when both ends are on walls that do not
    // meet there, `None` when an end is on no wall.
    let carried = |s: Option<&usize>, a: Vec2, t: Option<&usize>, b: Vec2| {
        let (Some((j, one)), Some((k, other))) = (s.and_then(|&s| wall(s)), t.and_then(|&t| wall(t))) else { return None };
        if j == k {
            return Some(true);
        }
        let gap = a.dist(b);
        // A corner as sharp as 30° lies up to about twice the gap from either end.
        Some(cross(&one, &other, (a + b) * 0.5).is_some_and(|c| c.dist(a) <= 2.0 * gap + 1e-9 && c.dist(b) <= 2.0 * gap + 1e-9))
    };
    let weight = NEARLY_CLOSED / BRIDGED_MOST;
    // The longest pieces first: each starts a loop the shorter ones may belong to.
    open.retain(|r| r.points.len() >= 2);
    open.sort_by(|a, b| length(&b.points).total_cmp(&length(&a.points)));
    while !open.is_empty() {
        let mut chain = open.remove(0);
        let Some(&start) = chain.points.first() else { continue };
        let (mut gaps, mut bridged, mut on_scan) = (0.0, 0.0, length(&chain.points));
        while let Some(&end) = chain.points.last() {
            let last = chain.triangles.last();
            let cost = |to: Vec2, on: Option<&usize>| {
                let gap = end.dist(to);
                match carried(last, end, on, to) {
                    Some(true) => (weight * gap, gap, true),
                    Some(false) => (f64::INFINITY, gap, false),
                    None => (gap, gap, false),
                }
            };
            // The end of a piece still free that costs least, and whether that is the piece's last
            // point; against closing the chain on its own start.
            let best = open
                .iter()
                .enumerate()
                .filter_map(|(k, r)| Some((k, (*r.points.first()?, r.triangles.first()), (*r.points.last()?, r.triangles.last()))))
                .flat_map(|(k, first, last)| [(k, false, cost(first.0, first.1)), (k, true, cost(last.0, last.1))])
                .min_by(|a, b| a.2.0.total_cmp(&b.2.0));
            let closing = cost(start, chain.triangles.first());
            match best {
                Some((k, backwards, (price, gap, carry))) if price < closing.0 => {
                    let mut next = open.remove(k);
                    if backwards {
                        next.points.reverse();
                        next.triangles.reverse();
                    }
                    if carry {
                        bridged += gap;
                    } else {
                        gaps += gap;
                    }
                    on_scan += length(&next.points);
                    chain.points.append(&mut next.points);
                    chain.triangles.append(&mut next.triangles);
                }
                _ => {
                    if closing.2 {
                        bridged += closing.1;
                    } else {
                        gaps += closing.1;
                    }
                    break;
                }
            }
        }
        if chain.points.len() >= 3 && gaps <= NEARLY_CLOSED * on_scan && bridged <= BRIDGED_MOST * on_scan {
            loops.push(chain);
        }
    }
    loops
}

/// Prism `prism`'s profile off its walls: the scan sliced halfway along it, each loop that runs on
/// its walls the curves of those walls, exactly as fitted, from crossing to crossing. The plane it
/// lies in and its outlines (`faces` index `reading.faces`); why not, when no loop goes round its
/// walls.
pub fn prism_profile(scan: &Scan, outcome: &Outcome, reading: &Reading, prism: &Prism) -> Result<(Plane, Vec<Outline>), String> {
    let (plane, outlines) = prism_profile_found(scan, outcome, reading, prism)?;
    let least = SLIVER_SHARE * reading.size;
    Ok((plane, outlines.into_iter().map(|o| without_slivers(o, least, false)).collect()))
}

/// [`prism_profile`] as the faces' meetings give it, slivers and all.
fn prism_profile_found(scan: &Scan, outcome: &Outcome, reading: &Reading, prism: &Prism) -> Result<(Plane, Vec<Outline>), String> {
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
    // The scan is sliced halfway along; the walls are where they cross the sketch's plane (the
    // same plane for a square prism, its low end for a drafted one: their in-plane axes are the
    // same, so the slice's loops say which wall is which there too).
    let (plane, sketch) = squared_to_walls(prism_plane(prism), prism_sketch_plane(prism), faces, &prism.walls);
    // A square prism's walls are read where they cross one level along its direction, the same
    // for every prism along it, not where its own sketch lies: a wall a few millionths of a
    // radian off the direction then gives prisms stacked along it the same line, where reading it
    // at each one's own level would leave their sides a few thousandths of a millimetre apart,
    // which the kernel's join cannot merge. (Same in-plane axes: the profile's coordinates do not
    // change.)
    let walls_at = if prism.draft == 0.0 {
        let d = sketch.normal();
        Plane { origin: sketch.origin - d * sketch.origin.dot(d), ..sketch }
    } else {
        sketch
    };
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
        prism.walls.iter().filter_map(|&k| faces.get(k).and_then(|f| wall_of(&f.shape, &walls_at)).map(|w| (k, w))).collect();
    let place: BTreeMap<usize, usize> = walls.iter().enumerate().map(|(i, (k, _))| (*k, i)).collect();
    let areas: Vec<f64> = walls.iter().map(|(k, _)| faces.get(*k).map_or(0.0, |f| f.area)).collect();
    let reach = 0.005 * size;
    // Between two walls the slice may run on scan Find All left as freeform (a gear's tip and
    // root lands): drawn as it runs, where that scan runs along the prism as its walls do.
    let tolerance = outcome.tolerance;
    let step = OTHER_LEVELS * prism.depth();
    // A drafted prism's outline at another level is this much in or out of the middle one's.
    let level_reach = (4.0 * tolerance).max(reach) + step * prism.draft.abs().tan();
    // The outlines the slice at `level` (along the prism from halfway) gives, and how many of the
    // prism's walls they run on. A run of the cut whose ends nearly meet is a loop too: the
    // outline is drawn from the walls' own crossings, so it needs the order the cut meets the
    // walls in, not every point of it, and a cut that passes a hair from a row of the scan's
    // points, or over a patch the scanner missed, comes out as a run just short of closed.
    let sliced = |level: f64| -> (Vec<Outline>, usize) {
        let at = |by: f64| Plane { origin: plane.origin + prism.direction * (level + by), ..plane };
        let on_wall = |t: usize| owner.get(t).and_then(|&k| place.get(&(k as usize))).and_then(|&i| walls.get(i)).copied();
        let runs: Vec<crate::section::Run> = loops_of(crate::section::section_welded(scan, weld, &at(0.0)), &on_wall);
        let loops: Vec<(Vec<Vec2>, Vec<Option<usize>>)> = runs
            .iter()
            .map(|r| {
                let on = r.triangles.iter().map(|&t| owner.get(t).and_then(|&k| place.get(&(k as usize)).copied())).collect();
                (r.points.clone(), on)
            })
            .collect();
        let designed = as_designed(&walls, &areas, &loops);
        let others: Vec<Vec<Vec<Vec2>>> = if step > 4.0 * tolerance {
            [-step, step]
                .into_iter()
                .map(|by| crate::section::section_welded(scan, weld, &at(by)).into_iter().filter(|r| r.closed).map(|r| r.points).collect())
                .collect()
        } else {
            Vec::new()
        };
        let cut: Vec<(&[Vec2], &[usize])> = runs.iter().map(|r| (r.points.as_slice(), r.triangles.as_slice())).collect();
        let along = runs_along(scan, &cut, prism.direction, prism.draft, &others, level_reach);
        // A drafted prism is sliced halfway up but drawn at its low end, where its walls stand
        // further out than the slice by the draft: the slice does not say how a gap runs there.
        let fill = (prism.draft == 0.0).then_some(Fill { tolerance, along: &along });
        // Which side of each loop the material is on, from the scan's own winding.
        let slice = at(0.0);
        let sides: Option<Vec<bool>> = runs.iter().map(|r| material_inside(scan, &slice, &r.points, &r.triangles)).collect();
        let outlines = prism_outlines_sided(&designed, &loops, reach, fill.as_ref(), sides.as_deref());
        let on: BTreeSet<usize> = outlines.iter().flat_map(|o| o.faces.iter().flatten().copied()).collect();
        (outlines, prism.walls.iter().filter(|k| on.contains(k)).count())
    };
    // Halfway along, as the profile has always been read. When that slice misses a wall (a hole
    // whose cut broke up, a side under a missing patch), a square prism's is the same at any
    // level, so the levels on either side are tried and the one that runs on most walls kept:
    // the nearest halfway among equals.
    let (mut outlines, mut most) = sliced(0.0);
    if most < prism.walls.len() && prism.draft == 0.0 {
        let depth = prism.high - prism.low;
        for share in TRY_LEVELS {
            let (found, count) = sliced(share * depth);
            if count > most {
                (outlines, most) = (found, count);
            }
            if most == prism.walls.len() {
                break;
            }
        }
    }
    if outlines.is_empty() {
        return Err("no loop of the scan's slice halfway along the prism runs round its walls".into());
    }
    Ok((sketch, outlines))
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
    turned_profile_open(scan, outcome, reading).map(|(plane, outline, held, _)| (plane, outline, held))
}

/// [`turned_profile`], and which of the outline's pieces (if any) is the line that closes a run
/// of faces open at both ends: what the scan never saw (the inside of a skin scanned from one
/// side), across which there is no face.
#[allow(clippy::type_complexity)]
pub fn turned_profile_open(
    scan: &Scan,
    outcome: &Outcome,
    reading: &Reading,
) -> Result<(Plane, Outline, BTreeMap<usize, Turned>, Option<usize>), String> {
    let revolve = reading.revolve.as_ref().ok_or("the part is not turned: no axis holds most of its faces")?;
    let between = reading.spokes.as_ref().map(|sp| between_spokes(&sp.angles)).unwrap_or_default();
    let (plane, outline, held, closing) = turned_profile_of(scan, outcome, &reading.faces, &reading.same, reading.size, revolve, &between)?;
    let sliver = SLIVER_SHARE * reading.size;
    let outline = without_slivers(outline, sliver, true);
    let n = outline.corners.len();
    let closing = closing.and_then(|[from, to]| {
        (0..n).find(|&k| {
            outline.pieces[k] == Piece::Line
                && outline.faces[k].is_none()
                && outline.corners[k].dist(from) <= sliver
                && outline.corners[(k + 1) % n].dist(to) <= sliver
        })
    });
    Ok((plane, outline, held, closing))
}

/// A line of a profile shorter than this share of the part is a sliver between two faces that
/// meet (a corner the faces' fits leave a hair apart), not a face of the part: a kernel builds a
/// degenerate face from it and refuses the result.
pub const SLIVER_SHARE: f64 = 0.002;

/// `outline` with each line shorter than `least` taken out, its two ends joined at its middle,
/// while three pieces or more are left. With `axis` (a turned profile, the axis along x), corners
/// within `least` of the axis or across it are put on it, so the profile never crosses it.
pub fn without_slivers(mut outline: Outline, least: f64, axis: bool) -> Outline {
    // An arc whose ends all but meet is a sliver too, and read the long way round it would be
    // nearly a whole circle (crossing the axis, on a wheel's hub): a line, to go with the rest.
    let n = outline.corners.len();
    if outline.pieces.len() == n {
        for k in 0..n {
            if matches!(outline.pieces[k], Piece::Arc { .. }) && outline.corners[k].dist(outline.corners[(k + 1) % n]) < least {
                outline.pieces[k] = Piece::Line;
            }
        }
    }
    if axis {
        for c in &mut outline.corners {
            if c.y < least {
                c.y = 0.0;
            }
        }
    }
    loop {
        let n = outline.pieces.len();
        if n <= 3 || outline.corners.len() != n || outline.faces.len() != n {
            return outline;
        }
        let shortest = (0..n)
            .filter(|&k| matches!(outline.pieces[k], Piece::Line))
            .map(|k| (k, outline.corners[k].dist(outline.corners[(k + 1) % n])))
            .filter(|(_, length)| *length < least)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((k, _)) = shortest else { return outline };
        let next = (k + 1) % n;
        let mut middle = (outline.corners[k] + outline.corners[next]) * 0.5;
        if axis && (outline.corners[k].y == 0.0 || outline.corners[next].y == 0.0) {
            middle.y = 0.0;
        }
        outline.corners[k] = middle;
        outline.corners.remove(next);
        outline.pieces.remove(k);
        outline.faces.remove(k);
    }
}

/// A face about the axis with no more than this share of their area, meeting only one other, is
/// left out of the profile as a misfit sliver ([`turned_profile_of`]) ...
const SMALL_FACE_SHARE: f64 = 0.03;
/// ... at most this many of them.
const MOST_DROPPED: usize = 3;

/// [`turned_profile`] of `revolve` from the shapes `found` and the part's size, so that
/// Reconstruct can ask it before it proposes Revolve It.
pub(crate) fn turned_profile_of(
    scan: &Scan,
    outcome: &Outcome,
    found: &[Face],
    same: &[usize],
    size: f64,
    revolve: &Revolve,
    between: &[f64],
) -> Result<(Plane, Outline, BTreeMap<usize, Turned>, Option<[Vec2; 2]>), String> {
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
        let far = |from: Vec2| points.iter().map(|(p, _)| *p).max_by(|a, b| a.dist(from).total_cmp(&b.dist(from))).unwrap_or(from);
        let one = far(middle);
        let extent = [one, far(one)];
        for &t in &triangles {
            if let Some(slot) = owner.get_mut(t) {
                *slot = faces.len();
            }
        }
        faces.push(ProfileFace { face: turned, middle, at_axis, extent });
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
    let mut meetings: Vec<Meeting> =
        shared.into_iter().filter(|(_, (_, n))| *n >= 2).map(|((a, b), (sum, n))| Meeting { faces: [a, b], at: sum / n as f64 }).collect();
    // A small face hanging off the profile by one meeting (a sliver of an edge fitted as a ball or
    // a torus, 1 % of a brake disc) is one face too many, not a missing one: left out, the rest may
    // go round. Its triangles stay unowned, as freeform.
    let area_of = |i: usize| numbered.get(i).and_then(|&k| found.get(k)).map_or(0.0, |f| f.area);
    let total: f64 = (0..faces.len()).map(area_of).sum();
    let mut dropped: BTreeSet<usize> = BTreeSet::new();
    while dropped.len() < MOST_DROPPED {
        let meets = |k: usize| meetings.iter().filter(|m| m.faces.contains(&k)).count() + usize::from(faces[k].at_axis.is_some());
        let Some(k) = (0..faces.len()).find(|&k| !dropped.contains(&k) && meets(k) == 1 && area_of(k) <= SMALL_FACE_SHARE * total) else { break };
        dropped.insert(k);
        meetings.retain(|m| !m.faces.contains(&k));
        if let Some(face) = faces.get_mut(k) {
            face.at_axis = None;
        }
    }
    let read = turned_outline(&faces, &meetings, 0.05 * size);
    // Where the faces go round, the median section too, and the one that explains more of the scan
    // is the profile: a chain can go round by closing a straight line across what it skipped
    // (T-LESS 02's grip lobes between its turned faces).
    let read = match read {
        Ok(chain) => {
            let median = turned_sliced(scan, axis, &faces, &numbered, &owner, 0.005 * size, outcome.tolerance, between);
            // Triangles' middles, each weighed by its area: a scan's points crowd where it curves
            // (a CAD mesh's most of all), so counted by points a profile that only follows the
            // rounds reads as explaining most of it.
            let points: Vec<(Vec2, f64)> = {
                let count = scan.triangle_count();
                let step = (count / EXPLAINED_SAMPLE).max(1);
                (0..count)
                    .step_by(step)
                    .filter_map(|t| scan.corners(t))
                    .map(|[a, b, c]| (polar_of((a + b + c) / 3.0), 0.5 * (b - a).cross(c - a).len()))
                    .collect()
            };
            let explains = |o: &Outline| explained_share(&points, &ring(o), outcome.tolerance.max(0.005 * size));
            let chain_numbered =
                Outline { faces: chain.faces.iter().map(|f| f.map(|i| numbered.get(i).copied().unwrap_or(usize::MAX))).collect(), ..chain.clone() };
            match median {
                Some((m, closing)) if explains(&m) > explains(&chain_numbered) + EXPLAINS_MORE => Err(Ok((m, closing))),
                _ => Ok(chain),
            }
        }
        Err(refusal) => Err(Err(refusal)),
    };
    let (outline, closing) = match read {
        // Numbered by the reading's faces. A run open at both ends was closed by a line of no
        // face, put last, off the axis.
        Ok(outline) => {
            let n = outline.corners.len();
            let closing = (n >= 3 && outline.faces.last() == Some(&None) && (outline.corners[n - 1].y.abs() > 1e-9 || outline.corners[0].y.abs() > 1e-9))
                .then(|| [outline.corners[n - 1], outline.corners[0]]);
            (Outline { faces: outline.faces.iter().map(|f| f.map(|i| numbered[i])).collect(), ..outline }, closing)
        }
        // The median section explained more of the scan than the faces' chain.
        Err(Ok(median)) => median,
        // Where the faces' meetings go round no one profile, a slice through the axis may.
        Err(Err(refusal)) => turned_sliced(scan, axis, &faces, &numbered, &owner, 0.005 * size, outcome.tolerance, between).ok_or_else(|| match refusal {
            Refusal::TooFew => "fewer than two faces about the turning axis meet on the scan, so there is no profile to turn".to_string(),
            Refusal::Meets(k, n) => format!("face {} meets {n} other faces about the turning axis, where a profile has each meet two: a face between them is missing, or one too many is found", numbered[k] + 1),
            Refusal::Apart => "the faces about the turning axis make more than one outline, so there is no one profile to turn".to_string(),
            Refusal::Corner(a, b) => format!("faces {} and {} meet on the scan where their shapes do not cross: one of them is fitted wrong", numbered[a] + 1, numbered.get(b).map_or(0, |k| k + 1)),
        })?,
    };
    Ok((turned_plane(axis), outline, held_faces, closing))
}

/// How many of the scan's points [`turned_profile_of`] scores two profiles on, at most.
const EXPLAINED_SAMPLE: usize = 20_000;
/// The median section is taken over the faces' chain when it explains more of the scan by this
/// share or more.
const EXPLAINS_MORE: f64 = 0.02;

/// The share of `points` (in the half-plane: along the axis, radius; each with its weight, the
/// area it stands for) within `tolerance` of the closed `ring`: how much of the scan a revolve of
/// the profile explains.
fn explained_share(points: &[(Vec2, f64)], ring: &[Vec2], tolerance: f64) -> f64 {
    let m = ring.len();
    let total: f64 = points.iter().map(|p| p.1).sum();
    if !(total > 0.0) || m < 2 {
        return 0.0;
    }
    let near = |p: Vec2| {
        (0..m).any(|k| {
            let (a, b) = (ring[k], ring[(k + 1) % m]);
            let d = b - a;
            let len2 = d.dot(d);
            let t = if len2 > 0.0 { ((p - a).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
            p.dist(a + d * t) <= tolerance
        })
    };
    points.iter().filter(|p| near(p.0)).map(|p| p.1).sum::<f64>() / total
}

/// The angles halfway between neighbouring spokes at `angles` (low to high, radians): where a
/// slice through a wheel's axis meets no spoke.
pub fn between_spokes(angles: &[f64]) -> Vec<f64> {
    let n = angles.len();
    (0..n)
        .map(|k| {
            let (a, b) = (angles[k], if k + 1 < n { angles[k + 1] } else { angles[0] + TAU });
            (0.5 * (a + b)).rem_euclid(TAU)
        })
        .collect()
}

/// How many planes through the axis [`turned_sliced`] cuts the scan with, evenly round half a turn;
/// each gives two half-sections.
const TURNED_SLICES: usize = 8;

/// A face about the axis as a wall of the half-section (height along the axis, radius from it): a
/// cylinder a line along the axis, a plane a line across it, a cone a slanted line, a torus or a
/// ball on the axis a circle.
fn turned_wall(face: &Turned) -> Option<Wall> {
    Some(match *face {
        Turned::Cylinder { radius } => Wall::Line { point: Vec2::new(0.0, radius), direction: Vec2::X },
        Turned::Plane { height } => Wall::Line { point: Vec2::new(height, 0.0), direction: Vec2::Y },
        Turned::Cone { slope, offset } => Wall::Line { point: Vec2::new(0.0, offset), direction: Vec2::new(1.0, slope).normalized()? },
        Turned::Torus { major, height, minor } => Wall::Circle { center: Vec2::new(height, major), radius: minor },
    })
}

/// How many planes through the axis [`median_section`] cuts the scan with, evenly round half a
/// turn: each gives two half-sections.
const MEDIAN_SLICES: usize = 36;

/// The cell of [`median_section`]'s half-plane grid: this share of the tolerance, at least the
/// part's size over [`MEDIAN_MOST_CELLS`] along each side.
const MEDIAN_CELL_SHARE: f64 = 0.5;
/// ... the most cells along a side of the grid.
const MEDIAN_MOST_CELLS: f64 = 1500.0;

/// An arc on no face in a median section whose radius is more than this many times its chord is
/// drawn as a line.
const MEDIAN_STRAIGHT: f64 = 5.0;

/// How far (in cells, each way) [`median_section`] averages the share of the turn a cell is inside.
const MEDIAN_SMOOTH: usize = 2;

/// Each cell of the `w` × `h` grid `v` (row by row) replaced by the mean of the cells within `r`
/// of it along a row.
fn box_mean(v: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let mut out = vec![0.0; v.len()];
    for j in 0..h {
        for i in 0..w {
            let (a, b) = (i.saturating_sub(r), (i + r).min(w - 1));
            out[j * w + i] = (a..=b).map(|k| v[j * w + k]).sum::<f64>() / (b - a + 1) as f64;
        }
    }
    out
}

/// [`box_mean`] along the grid's columns.
fn box_mean_t(v: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let mut out = vec![0.0; v.len()];
    for i in 0..w {
        for j in 0..h {
            let (a, b) = (j.saturating_sub(r), (j + r).min(h - 1));
            out[j * w + i] = (a..=b).map(|k| v[k * w + i]).sum::<f64>() / (b - a + 1) as f64;
        }
    }
    out
}

/// A turned part's median half-section: the scan cut by [`MEDIAN_SLICES`] planes through `axis`,
/// each half-section filled on a grid of the half-plane (height along the axis, radius from it),
/// and the cells inside at half the half-sections or more kept: the body the turn makes, whatever
/// crosses only part of the turn (a thread, grip lobes, a slot, a boss) left out or in by how much
/// of the turn it covers. Its outer boundary as a closed run of points (and the grid's cell), each on the wall of
/// `walls` (numbered as in the caller's list, `axis` the axis's own) within the tolerance (or a
/// cell and a half) of it, else on none. Pieces on no wall are drawn as the run goes
/// ([`outline_of`]'s fill). Nothing when the scan gives no closed half-section.
fn median_section(
    scan: &Scan,
    weld: &[u32],
    axis: &Axis,
    walls: &[(usize, Wall)],
    axis_wall: usize,
    reach: f64,
    tolerance: f64,
) -> Option<(Vec<Vec2>, Vec<Option<usize>>, f64)> {
    let (u, v) = crate::in_plane(axis.direction);
    let mut halves: Vec<Vec<Vec2>> = Vec::new();
    for i in 0..MEDIAN_SLICES {
        let angle = PI * (i as f64 + 0.5) / MEDIAN_SLICES as f64;
        let plane = Plane { origin: axis.point, x: axis.direction, y: u * angle.cos() + v * angle.sin() };
        for run in crate::section::section_welded(scan, weld, &plane) {
            if !run.closed {
                continue;
            }
            let none = vec![None; run.points.len()];
            for side in [1.0, -1.0] {
                let flipped: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p.x, side * p.y)).collect();
                if let Some((points, _)) = above_axis(&flipped, &none, axis_wall) {
                    halves.push(points);
                }
            }
        }
    }
    let count_halves = 2 * MEDIAN_SLICES;
    if halves.is_empty() {
        return None;
    }
    let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut far = 0.0_f64;
    for p in halves.iter().flatten() {
        low = low.min(p.x);
        high = high.max(p.x);
        far = far.max(p.y);
    }
    let size = (high - low).max(far);
    if !(size > 0.0 && size.is_finite()) {
        return None;
    }
    let cell = (MEDIAN_CELL_SHARE * tolerance).max(size / MEDIAN_MOST_CELLS).max(1e-6);
    // A cell of margin at each end and above; the axis is the grid's bottom edge.
    let x0 = low - cell;
    let w = ((high - low) / cell).ceil() as usize + 3;
    let h = (far / cell).ceil() as usize + 2;
    let mut count = vec![0u16; w * h];
    for points in &halves {
        let n = points.len();
        for j in 0..h {
            let y = (j as f64 + 0.5) * cell;
            let mut xs: Vec<f64> = (0..n)
                .filter_map(|k| {
                    let (a, b) = (points[k], points[(k + 1) % n]);
                    ((a.y <= y) != (b.y <= y)).then(|| a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x))
                })
                .collect();
            xs.sort_by(f64::total_cmp);
            for &[enter, leave] in xs.as_chunks::<2>().0 {
                let first = (((enter - x0) / cell) - 0.5).ceil().max(0.0) as usize;
                let last = (((leave - x0) / cell) - 0.5).floor();
                if last < 0.0 {
                    continue;
                }
                for i in first..=(last as usize).min(w - 1) {
                    count[j * w + i] = count[j * w + i].saturating_add(1);
                }
            }
        }
    }
    // The share of the turn each cell is inside, averaged over the cells within
    // [`MEDIAN_SMOOTH`] of it: a thread's band sits near a half everywhere, and the slices'
    // angles beat against its helix, so read cell by cell its edge is a comb.
    let share: Vec<f64> = count.iter().map(|&c| f64::from(c) / count_halves as f64).collect();
    let smooth = box_mean(&box_mean_t(&share, w, h, MEDIAN_SMOOTH), w, h, MEDIAN_SMOOTH);
    let inside: Vec<bool> = smooth.iter().map(|&c| c >= 0.5).collect();
    // The body: its largest piece, with any hole closed inside it (one the axis or an end does not
    // open: a void the threshold left), the threshold's specks beside it dropped.
    let inside = crate::wall_outline::largest(&crate::wall_outline::filled(&inside, w, h), w, h);
    let rings = crate::wall_outline::boundaries(&inside, w, h);
    let area = |r: &[(usize, usize)]| {
        let m = r.len();
        (0..m)
            .map(|k| {
                let (a, b) = (r[k], r[(k + 1) % m]);
                a.0 as f64 * b.1 as f64 - b.0 as f64 * a.1 as f64
            })
            .sum::<f64>()
            .abs()
    };
    let outer = rings.into_iter().max_by(|a, b| area(a).total_cmp(&area(b)))?;
    let stairs: Vec<Vec2> = outer.iter().map(|&(i, j)| Vec2::new(x0 + i as f64 * cell, j as f64 * cell)).collect();
    // The cells' staircase taken off (within three quarters of a cell), then points every two
    // cells along what is left, so each stretch has points to be read on its wall.
    let corners = crate::wall_outline::simplify_ring(&stairs, 0.75 * cell);
    let m = corners.len();
    let mut points = Vec::new();
    for k in 0..m {
        let (a, b) = (corners[k], corners[(k + 1) % m]);
        let steps = ((a.dist(b) / (2.0 * cell)).ceil() as usize).max(1);
        for t in 0..steps {
            points.push(a + (b - a) * (t as f64 / steps as f64));
        }
    }
    let near = tolerance.max(1.5 * cell).max(reach.min(tolerance * 2.0));
    let on: Vec<Option<usize>> = points
        .iter()
        .map(|&p| {
            if p.y <= 0.5 * cell {
                return Some(axis_wall);
            }
            walls
                .iter()
                .enumerate()
                .map(|(k, (_, wall))| (k, wall.foot(p).dist(p)))
                .filter(|&(_, d)| d <= near)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        })
        .collect();
    Some((points, on, cell))
}

/// The part of the closed loop `points` (each on the wall `on` names) with radius at least 0: where
/// it dips below the axis, the axis (wall `axis`) runs from where it leaves to where it comes back.
/// `None` when none of it is above the axis.
fn above_axis(points: &[Vec2], on: &[Option<usize>], axis: usize) -> Option<(Vec<Vec2>, Vec<Option<usize>>)> {
    let n = points.len();
    let (mut kept, mut walls) = (Vec::new(), Vec::new());
    for i in 0..n {
        let (p, q) = (points[i], points[(i + 1) % n]);
        if p.y >= 0.0 {
            kept.push(p);
            walls.push(on[i]);
        }
        if (p.y >= 0.0) != (q.y >= 0.0) {
            let t = p.y / (p.y - q.y);
            kept.push(Vec2::new(p.x + (q.x - p.x) * t, 0.0));
            walls.push(Some(axis));
        }
    }
    (kept.len() >= 3).then_some((kept, walls))
}

/// How many steps the line closing an open run straight across is walked in.
const CLOSING_POINTS: usize = 16;

/// The parts of the open run `points` (each point on the wall `on` names; its ends where the scan,
/// or the part of it selected, stops) with radius at least 0, each closed as a profile is: a part
/// that comes down to the axis at one end runs on `overlap` past its other end, the way the run
/// was going there, and along the axis from that height, so what was cut off there (a boss
/// selected without the block it stands on) stands a little into it: joined to it, the two meet in
/// material, not in two faces a hair apart, which the kernel's boolean refuses. A part with neither
/// end on the axis (a rim's skin, the scan stopping at both lips) is closed straight across, from
/// where it ends to where it began: the closing line is a wall of its own (returned; `closing` is
/// its place among the walls, and the points along it are on it), so the faces at the run's two
/// ends are never taken for one stretch running on across what the scan did not see.
fn above_axis_open(
    points: &[Vec2],
    on: &[Option<usize>],
    axis: usize,
    closing: usize,
    overlap: f64,
) -> Vec<(Vec<Vec2>, Vec<Option<usize>>, Option<Wall>)> {
    let finish = |mut kept: Vec<Vec2>, mut walls: Vec<Option<usize>>| -> Option<(Vec<Vec2>, Vec<Option<usize>>, Option<Wall>)> {
        if kept.len() < 3 {
            return None;
        }
        let (first, last) = (walls.first().copied().flatten() == Some(axis), walls.last().copied().flatten() == Some(axis));
        let n = kept.len();
        // The end cut off, carried on past where the run stopped.
        let onward = |end: Vec2, before: Vec2| end + (end - before).normalized().unwrap_or(Vec2::ZERO) * overlap;
        match (first, last) {
            (false, true) => {
                let start = onward(kept[0], kept[1]);
                let wall = walls[0];
                kept.insert(0, start);
                walls.insert(0, wall);
                kept.push(Vec2::new(start.x, 0.0));
                walls.push(Some(axis));
            }
            (true, false) => {
                let end = onward(kept[n - 1], kept[n - 2]);
                kept.push(end);
                walls.push(walls[n - 1]);
                kept.push(Vec2::new(end.x, 0.0));
                walls.push(Some(axis));
            }
            (false, false) => {
                let (from, to) = (kept[n - 1], kept[0]);
                let Some(direction) = (to - from).normalized() else { return Some((kept, walls, None)) };
                // The run's own two ends are the closing line's ends: what the scan shows up to
                // each (a lip no face holds) is then drawn right up to it.
                (walls[0], walls[n - 1]) = (Some(closing), Some(closing));
                for i in 1..CLOSING_POINTS {
                    kept.push(from + (to - from) * (i as f64 / CLOSING_POINTS as f64));
                    walls.push(Some(closing));
                }
                return Some((kept, walls, Some(Wall::Line { point: from, direction })));
            }
            (true, true) => {}
        }
        Some((kept, walls, None))
    };
    let mut parts = Vec::new();
    let (mut kept, mut walls) = (Vec::new(), Vec::new());
    for (i, &p) in points.iter().enumerate() {
        if p.y >= 0.0 {
            kept.push(p);
            walls.push(on.get(i).copied().flatten());
        }
        let Some(&q) = points.get(i + 1) else { continue };
        if (p.y >= 0.0) != (q.y >= 0.0) {
            let t = p.y / (p.y - q.y);
            kept.push(Vec2::new(p.x + (q.x - p.x) * t, 0.0));
            walls.push(Some(axis));
            // Going below the axis: this part ends at it.
            if p.y >= 0.0 {
                parts.extend(finish(std::mem::take(&mut kept), std::mem::take(&mut walls)));
            }
        }
    }
    parts.extend(finish(kept, walls));
    parts
}

/// A turned part's profile off a slice through its axis, for when the faces' meetings go round no
/// one profile (a face Find All missed between two, a slot or a boss across them): the scan cut by
/// planes through the axis at even turns, each half-section walked as a prism's slice is
/// ([`outline_of`]), every face about the axis (`faces`, numbered in the reading by `numbered`,
/// triangle `t` on face `owner[t]`) its curve held to the axis, and the axis closing the profile
/// where the section comes down to it. Between two faces the walls run on to where they meet, or
/// are bridged straight across; where the slice strays from that by more than the scan's
/// `tolerance` (faces Find All did not find), it is drawn as the slice runs, in lines and arcs. Of
/// the half-sections, the one whose outline runs furthest on the faces, so a slot or a hole across
/// some of them is passed by. Pieces on the axis have no face; neither do the bridges and the
/// pieces drawn off the slice.
fn turned_sliced(
    scan: &Scan,
    axis: &Axis,
    faces: &[ProfileFace],
    numbered: &[usize],
    owner: &[usize],
    reach: f64,
    tolerance: f64,
    between: &[f64],
) -> Option<(Outline, Option<[Vec2; 2]>)> {
    let mut walls: Vec<(usize, Wall)> = faces.iter().zip(numbered).filter_map(|(f, &k)| turned_wall(&f.face).map(|w| (k, w))).collect();
    // Each face's place among the walls (a face with no wall has none).
    let place: Vec<Option<usize>> = {
        let mut next = 0;
        faces.iter().map(|f| turned_wall(&f.face).map(|_| (next, next += 1).0)).collect()
    };
    let axis_wall = walls.len();
    walls.push((usize::MAX, Wall::Line { point: Vec2::ZERO, direction: Vec2::X }));
    let weld = crate::welded(scan.positions);
    let (u, v) = crate::in_plane(axis.direction);
    let mut best: Option<(f64, Outline)> = None;
    // A wheel is cut only between its spokes, each half-section on its own side of the axis: a
    // slice through a spoke draws the spoke into the turned profile.
    let slices: Vec<(f64, &[f64])> = if between.is_empty() {
        (0..TURNED_SLICES).map(|i| (PI * i as f64 / TURNED_SLICES as f64, &[1.0, -1.0][..])).collect()
    } else {
        between.iter().map(|&a| (a, &[1.0][..])).collect()
    };
    for (angle, sides) in slices {
        let plane = Plane { origin: axis.point, x: axis.direction, y: u * angle.cos() + v * angle.sin() };
        for run in crate::section::section_welded(scan, &weld, &plane) {
            let on: Vec<Option<usize>> = run.triangles.iter().map(|&t| owner.get(t).and_then(|&f| place.get(f).copied().flatten())).collect();
            for &side in sides {
                let flipped: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p.x, side * p.y)).collect();
                // A closed loop's half above the axis; an open run's (the scan's edge, or where the
                // part selected stops), closed down to the axis.
                let halves = if run.closed {
                    above_axis(&flipped, &on, axis_wall).map(|(points, on)| (points, on, None)).into_iter().collect()
                } else {
                    above_axis_open(&flipped, &on, axis_wall, axis_wall + 1, 2.0 * reach)
                };
                for (points, on, closing) in halves {
                    // The line closing a run open at both ends is a wall for this run alone.
                    let closed: Vec<(usize, Wall)>;
                    let walls = match closing {
                        Some(line) => {
                            closed = walls.iter().copied().chain([(usize::MAX, line)]).collect();
                            &closed
                        }
                        None => &walls,
                    };
                    let Some(outline) = outline_of(walls, &points, &on, reach, Some((tolerance, None))) else { continue };
                    let ring = ring(&outline);
                    if !is_simple_ring(&ring) || ring.iter().any(|p| p.y < -reach) {
                        continue;
                    }
                    // How far the slice itself runs on the faces: where the scan has them, not
                    // how far the outline draws their walls on (closing an open slice down the
                    // axis draws a face on to the axis, which the scan may have only a stretch of).
                    let on_faces: f64 = (1..points.len())
                        .filter(|&i| on[i].is_some_and(|w| w != axis_wall) && on[i] == on[i - 1])
                        .map(|i| points[i].dist(points[i - 1]))
                        .sum();
                    if best.as_ref().is_none_or(|(b, _)| on_faces > *b) {
                        best = Some((on_faces, outline));
                    }
                }
            }
        }
    }
    // The median section where the faces do not go round: a thread, grip lobes or a slot cross
    // every single half-section, which then draws them in (a zigzag of arcs, or the inner run
    // closed straight across: T-LESS 01 came out r 10.7 of 17.5). The half-plane region inside
    // the part at half the turn or more is the turned body itself, the thread its mean cylinder.
    let median = if between.is_empty() {
        median_section(scan, &weld, axis, &walls[..axis_wall], axis_wall, reach, tolerance).and_then(|(points, on, cell)| {
            let outline = outline_of(&walls, &points, &on, reach, Some((tolerance.max(2.0 * cell), None)));
            let mut outline = outline?;
            // A piece drawn off the section on no face, an arc nearly straight (its radius many
            // times its chord), is a line: read the other way round it is nearly a whole circle,
            // under the axis (T-LESS 03).
            let n = outline.corners.len();
            for k in 0..n {
                if let (Piece::Arc { center, .. }, None) = (outline.pieces[k], outline.faces[k]) {
                    let (a, b) = (outline.corners[k], outline.corners[(k + 1) % n]);
                    if center.dist(a) > MEDIAN_STRAIGHT * a.dist(b) {
                        outline.pieces[k] = Piece::Line;
                    }
                }
            }
            // Pieces whose ends meet (an arc from a point to itself reads as a whole circle) out,
            // as the caller takes them out of every turned profile (`reach` is 0.005 of the
            // part, the slivers' 0.002 of it).
            let outline = without_slivers(outline, SLIVER_SHARE / 0.005 * reach, true);
            let ring = ring(&outline);
            (is_simple_ring(&ring) && ring.iter().all(|p| p.y >= -reach)).then_some(outline)
        })
    } else {
        None
    };
    let outline = match median {
        Some(outline) => outline,
        None => best?.1,
    };
    let corners: Vec<Vec2> = outline.corners.iter().map(|c| Vec2::new(c.x, c.y.max(0.0))).collect();
    // The line that closed a run open at both ends, by its ends.
    let closing = outline.faces.iter().position(|f| *f == Some(usize::MAX)).map(|k| [corners[k], corners[(k + 1) % corners.len()]]);
    let faces = outline.faces.iter().map(|f| f.filter(|&k| k != usize::MAX)).collect();
    Some((Outline { corners, faces, ..outline }, closing))
}

#[cfg(test)]
mod tests {
    /// A hair-long line between two faces is joined away at its middle; a real side stays; a turned
    /// profile's corners a hair off the axis, or across it, go onto it.
    #[test]
    fn slivers_are_joined_away_and_corners_put_on_the_axis() {
        let square =
            |corners: Vec<Vec2>| Outline { pieces: vec![Piece::Line; corners.len()], faces: (0..corners.len()).map(Some).collect(), corners };
        let o = square(vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 5.0), Vec2::new(10.01, 5.0), Vec2::new(0.0, 5.0)]);
        let t = without_slivers(o, 0.1, false);
        assert_eq!(t.pieces.len(), 4, "{t:?}");
        assert!((t.corners[2] - Vec2::new(10.005, 5.0)).len() < 1e-12, "{t:?}");
        assert_eq!(t.faces, [Some(0), Some(1), Some(3), Some(4)]);
        let kept = without_slivers(square(vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)]), 5.0, false);
        assert_eq!(kept.pieces.len(), 3, "a triangle keeps its three sides");
        let turned =
            without_slivers(square(vec![Vec2::new(0.0, -0.01), Vec2::new(10.0, 0.02), Vec2::new(10.0, 5.0), Vec2::new(0.0, 5.0)]), 0.1, true);
        assert!(turned.corners.iter().all(|c| c.y >= 0.0) && turned.corners[0].y == 0.0 && turned.corners[1].y == 0.0, "{turned:?}");
    }

    use super::*;

    /// A square 40 on a side cut where three patches are missing comes as three open pieces, in no
    /// order and one of them backwards. They are one loop, joined in the order they go round; the
    /// same pieces with half a side missing between two of them are not (the gaps are over a fifth
    /// of what the scan holds), and a closed run stays as it is.
    #[test]
    fn the_pieces_of_a_cut_that_missing_patches_broke_apart_are_one_loop() {
        // The square's outline from `from` to `to` along it (0 to 160 round), a point each 1 mm.
        let at = |d: f64| match d.rem_euclid(160.0) {
            d if d < 40.0 => Vec2::new(d, 0.0),
            d if d < 80.0 => Vec2::new(40.0, d - 40.0),
            d if d < 120.0 => Vec2::new(120.0 - d, 40.0),
            d => Vec2::new(0.0, 160.0 - d),
        };
        let piece = |from: u32, to: u32, backwards: bool| {
            let mut points: Vec<Vec2> = (from..=to).map(|d| at(f64::from(d))).collect();
            if backwards {
                points.reverse();
            }
            let triangles = vec![0; points.len()];
            crate::section::Run { points, triangles, closed: false }
        };
        let side = Wall::Line { point: Vec2::ZERO, direction: Vec2::X };
        let on_side = |t: usize| (t == 0).then_some((0, side));
        let joined = loops_of(vec![piece(100, 150, false), piece(5, 50, true), piece(55, 95, false)], &|_| None);
        assert_eq!(joined.len(), 1, "one loop");
        let points = &joined[0].points;
        assert_eq!(points.len(), 51 + 46 + 41);
        // Round it once, one way: the turn about the square's middle adds up to a whole turn.
        let middle = Vec2::new(20.0, 20.0);
        let turn: f64 = (0..points.len())
            .map(|k| {
                let (a, b) = (points[k] - middle, points[(k + 1) % points.len()] - middle);
                (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
            })
            .sum();
        assert!((turn.abs() - std::f64::consts::TAU).abs() < 1e-6, "{turn}");
        // Half a side missing twice over: 40 of gaps on 120 of scan.
        let broken = || vec![piece(100, 150, false), piece(5, 50, true), piece(70, 85, false)];
        assert!(loops_of(broken(), &|_| None).is_empty());
        // ... unless the gaps' ends are on the prism's walls, which run on across them.
        assert_eq!(loops_of(broken(), &on_side).len(), 1);
        // Two pieces of a side only, the rest of the square missing, are no loop even so.
        assert!(loops_of(vec![piece(5, 30, false), piece(45, 70, false)], &on_side).is_empty());
        let closed = crate::section::Run { points: (0..160).map(|d| at(f64::from(d))).collect(), triangles: vec![0; 160], closed: true };
        assert_eq!(loops_of(vec![closed.clone()], &|_| None), vec![closed]);
    }

    /// A plate's outline missing at a corner, and a hole beside that corner missing a little of
    /// its round, the hole's open end nearer the side's than the far side of the corner is. The
    /// sides meet across the corner and carry the outline over it; a side and the hole do not
    /// meet, so they carry nothing: two loops, the square and the hole. Joined to the nearest end
    /// whatever the walls, the hole was taken into the plate's outline and the corner cut off.
    #[test]
    fn a_gap_is_bridged_only_by_walls_that_meet_across_it() {
        let at = |d: f64| match d.rem_euclid(160.0) {
            d if d < 40.0 => Vec2::new(d, 0.0),
            d if d < 80.0 => Vec2::new(40.0, d - 40.0),
            d if d < 120.0 => Vec2::new(120.0 - d, 40.0),
            d => Vec2::new(0.0, 160.0 - d),
        };
        // The square from 20 round to 140 (its corner at the origin missing, too much for a run
        // nearly closed), each point on the side it lies on (triangles 0 to 3).
        let points: Vec<Vec2> = (20..=140).map(|d| at(f64::from(d))).collect();
        let triangles = (20..=140).map(|d| ((d % 160) / 40) as usize).collect();
        let square = crate::section::Run { points, triangles, closed: false };
        // The hole about (5, 25), radius 4, from 200° round to 430°: its gap, too wide for a run
        // nearly closed, faces the corner, its open end 3.8 from the square's (the corner's gap
        // is 28.3).
        let (center, radius) = (Vec2::new(5.0, 25.0), 4.0);
        let points: Vec<Vec2> =
            (0..=46).map(|i| (200.0 + 5.0 * f64::from(i)).to_radians()).map(|a| center + Vec2::new(a.cos(), a.sin()) * radius).collect();
        let hole = crate::section::Run { triangles: vec![4; points.len()], points, closed: false };
        let walls = [
            Wall::Line { point: Vec2::ZERO, direction: Vec2::X },
            Wall::Line { point: Vec2::new(40.0, 0.0), direction: Vec2::Y },
            Wall::Line { point: Vec2::new(0.0, 40.0), direction: Vec2::X },
            Wall::Line { point: Vec2::ZERO, direction: Vec2::Y },
            Wall::Circle { center, radius },
        ];
        let wall = |t: usize| walls.get(t).map(|w| (t, *w));
        let loops = loops_of(vec![square, hole], &wall);
        assert_eq!(loops.len(), 2, "the square and the hole");
        let sizes: Vec<usize> = loops.iter().map(|l| l.points.len()).collect();
        assert!(sizes.contains(&121) && sizes.contains(&47), "{sizes:?}");
    }

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

    /// Which side of a slice the material is on, read off the scan's winding: a block's outline
    /// has it inside; the same walls wound the other way (as a hole's wall faces) have it outside,
    /// whichever way round the loop runs.
    #[test]
    fn a_slices_material_side_is_read_off_the_scans_winding() {
        let (positions, triangles) = extruded(&rounded_rectangle(40.0, 30.0, 5.0, 0.5), 0.0, 10.0, 4);
        let flipped: Vec<[u32; 3]> = triangles.iter().map(|&[a, b, c]| [a, c, b]).collect();
        let plane = Plane { origin: Vec3::new(0.0, 0.0, 5.0), x: Vec3::X, y: Vec3::Y };
        for (tris, inside) in [(&triangles, true), (&flipped, false)] {
            let scan = Scan::new(&positions, tris);
            let runs = crate::section::section_welded(&scan, &crate::welded(&positions), &plane);
            let run = runs.iter().find(|r| r.closed).expect("a loop");
            assert_eq!(material_inside(&scan, &plane, &run.points, &run.triangles), Some(inside));
            let mut points = run.points.clone();
            let mut on = run.triangles.clone();
            points.reverse();
            on.reverse();
            // Reversed, each point keeps the triangle it lies in; the way round does not matter.
            on.rotate_left(1);
            assert_eq!(material_inside(&scan, &plane, &points, &on), Some(inside));
        }
    }

    /// The same block with the lower 6 of one round never scanned: the round's wall ends along the
    /// edge of the scan, not at a face, so it runs on to the floor. One prism still, four arcs in its
    /// profile; before, the round's end split the block into two prisms, the lower one square at
    /// that corner.
    #[test]
    fn a_round_half_missed_still_runs_the_blocks_whole_depth() {
        let (positions, triangles) = extruded(&rounded_rectangle(40.0, 30.0, 5.0, 0.5), 0.0, 10.0, 16);
        let missed = |t: &[u32; 3]| {
            let m = t.iter().fold(Vec3::ZERO, |s, &v| s + positions[v as usize]) / 3.0;
            m.x > 35.0 && m.y < 5.0 && m.z < 6.0 && (Vec2::new(m.x, m.y).dist(Vec2::new(35.0, 5.0)) - 5.0).abs() < 0.2
        };
        let kept: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| !missed(t)).collect();
        assert!(kept.len() < triangles.len(), "some of the round taken away");
        let scan = Scan::new(&positions, &kept);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let reading = read(&scan, &outcome);
        assert_eq!(reading.prisms.len(), 1, "{:?}", reading.prisms);
        let prism = &reading.prisms[0];
        assert!((prism.depth() - 10.0).abs() < 0.05, "{prism:?}");
        let (_, outlines) = prism_profile(&scan, &outcome, &reading, prism).unwrap();
        let o = &outlines[0];
        let lines = o.pieces.iter().filter(|p| **p == Piece::Line).count();
        assert_eq!((lines, o.pieces.len() - lines), (4, 4), "{o:?}");
    }

    /// The block with one round never scanned at all, top to bottom: the corner there is rounded
    /// as the other three are, R 5, since no point of the slice lies at its tip. The same block
    /// made with that corner sharp, and scanned, keeps it sharp: the slice runs into the tip.
    #[test]
    fn a_corner_the_scanner_missed_is_rounded_as_the_others_and_a_sharp_one_it_saw_stays_sharp() {
        let profile_of = |positions: &[Vec3], triangles: &[[u32; 3]]| {
            let scan = Scan::new(positions, triangles);
            let outcome = find_all(&scan, &Options::default(), &|| false);
            let reading = read(&scan, &outcome);
            let prism = reading.prisms.first().cloned().expect("a prism");
            let (plane, outlines) = prism_profile(&scan, &outcome, &reading, &prism).unwrap();
            // The corner's tip, (40, 0), in the sketch's own frame.
            let tip = plane.to_local(Vec3::new(40.0, 0.0, 5.0));
            (outlines.into_iter().next().expect("an outline"), tip)
        };
        let arcs = |o: &Outline| o.pieces.iter().filter(|p| matches!(p, Piece::Arc { .. })).count();
        let (positions, triangles) = extruded(&rounded_rectangle(40.0, 30.0, 5.0, 0.5), 0.0, 10.0, 16);
        let missed = |t: &[u32; 3]| {
            let m = t.iter().fold(Vec3::ZERO, |s, &v| s + positions[v as usize]) / 3.0;
            m.x > 35.0 && m.y < 5.0 && (Vec2::new(m.x, m.y).dist(Vec2::new(35.0, 5.0)) - 5.0).abs() < 0.2
        };
        let kept: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| !missed(t)).collect();
        let (o, tip) = profile_of(&positions, &kept);
        assert_eq!(arcs(&o), 4, "{o:?}");
        // The rounded corner where the round was: its middle 5 (√2 - 1) in from the tip.
        let nearest = ring(&o).iter().map(|p| p.dist(tip)).fold(f64::INFINITY, f64::min);
        assert!((nearest - 5.0 * (2f64.sqrt() - 1.0)).abs() < 0.1, "the corner is rounded R 5: {nearest}");

        // Three corners rounded and the one at (40, 0) sharp, scanned whole.
        let mut outline: Vec<[f64; 2]> = rounded_rectangle(40.0, 30.0, 5.0, 0.5);
        let first = outline.iter().position(|p| p[0] > 35.0 + 1e-9 && p[1] < 5.0 - 1e-9).expect("the round's points");
        outline.retain(|p| !(p[0] > 35.0 + 1e-9 && p[1] < 5.0 - 1e-9));
        outline.insert(first, [40.0, 0.0]);
        let (positions, triangles) = extruded(&outline, 0.0, 10.0, 16);
        let (o, tip) = profile_of(&positions, &triangles);
        assert_eq!(arcs(&o), 3, "{o:?}");
        assert!(o.corners.iter().any(|c| c.dist(tip) < 0.05), "the sharp corner kept: {o:?}");
    }

    /// Walls a scan leaves slightly off: a side 0.2° off square to the largest, turned about the
    /// middle of its stretch to be square; a side 2° off left as it is; rounds of 5.99 and 6.03
    /// given one radius, a round of 3 its own.
    #[test]
    fn walls_are_squared_and_rounds_made_one_radius_as_designed() {
        let off = 0.2f64.to_radians();
        let walls = vec![
            (0, Wall::Line { point: Vec2::new(0.0, -10.0), direction: Vec2::new(1.0, 0.0) }),
            (1, Wall::Line { point: Vec2::new(20.0, 0.0), direction: Vec2::new(off.sin(), off.cos()) }),
            (2, Wall::Line { point: Vec2::new(0.0, 10.0), direction: Vec2::new(2f64.to_radians().cos(), 2f64.to_radians().sin()) }),
            (3, Wall::Circle { center: Vec2::new(14.0, -4.0), radius: 5.99 }),
            (4, Wall::Circle { center: Vec2::new(-14.0, -4.0), radius: 6.03 }),
            (5, Wall::Circle { center: Vec2::new(0.0, 0.0), radius: 3.0 }),
        ];
        let areas = [500.0, 100.0, 100.0, 10.0, 10.0, 10.0];
        // The slice's points on the second side, round its middle at y = 5.
        let points: Vec<Vec2> = (0..5).map(|k| Vec2::new(20.0 + (3.0 + f64::from(k)) * off.sin(), (3.0 + f64::from(k)) * off.cos())).collect();
        let loops = vec![(points, vec![Some(1); 5])];
        let out = as_designed(&walls, &areas, &loops);
        let Wall::Line { point, direction } = out[1].1 else { panic!("a line") };
        assert!((direction.x).abs() < 1e-12 && (direction.y - 1.0).abs() < 1e-12, "square: {direction:?}");
        assert!((point.x - (20.0 + 5.0 * off.sin())).abs() < 1e-9, "turned about its middle: {point:?}");
        assert_eq!(out[2].1, walls[2].1, "2° off is a side of its own");
        let radius = |i: usize| match out[i].1 {
            Wall::Circle { radius, .. } => radius,
            Wall::Line { .. } => f64::NAN,
        };
        assert!((radius(3) - 6.01).abs() < 1e-9 && (radius(4) - 6.01).abs() < 1e-9, "{} {}", radius(3), radius(4));
        assert_eq!(radius(5), 3.0);
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
        let outlines = prism_outlines(&walls, &[(points, on)], 0.2, None);
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

    /// A shaft R 20 × 20 stepping down to R 10 × 20, whose step Find All did not find (taken out
    /// of its shapes, as on a scan whose step was not read): the faces' meetings go round no
    /// profile, so it is read off a slice through the axis, the step drawn as the slice runs there.
    /// The profile is the shaft's half-section, 600 mm².
    #[test]
    fn a_turned_part_with_a_face_unfound_is_read_off_a_slice() {
        let mut profile = vec![[0.0, 0.0]];
        profile.extend(steps([0.0, 0.5], [0.0, 19.5], 19));
        profile.extend(steps([0.0, 20.0], [20.0, 20.0], 40));
        profile.extend(steps([20.0, 20.0], [20.0, 10.0], 20).into_iter().skip(1));
        profile.extend(steps([20.0, 10.0], [40.0, 10.0], 40).into_iter().skip(1));
        profile.extend(steps([40.0, 10.0], [40.0, 0.5], 19).into_iter().skip(1));
        profile.push([40.0, 0.0]);
        let (positions, triangles) = revolved(&profile, 120, false);
        let scan = Scan::new(&positions, &triangles);
        let found = find_all(&scan, &Options::default(), &|| false);
        let step = |s: &crate::segment::Segment| matches!(s.shape, Some(Shape::Plane { point, normal }) if normal.z.abs() > 0.99 && (point.z - 20.0).abs() < 0.5);
        assert!(found.segments.iter().any(step), "Find All found the step");
        // The step left unfound: after the shapes, a region with no shape.
        let mut segments: Vec<crate::segment::Segment> = found.segments.iter().filter(|s| s.shape.is_some() && !step(s)).cloned().collect();
        segments.extend(found.segments.iter().filter(|s| step(s)).map(|s| crate::segment::Segment { shape: None, ..s.clone() }));
        segments.extend(found.segments.iter().filter(|s| s.shape.is_none()).cloned());
        let outcome = Outcome { segments, ..found };
        let reading = read(&scan, &outcome);
        assert!(reading.revolve.is_some() && reading.turned_unread.is_none(), "{:?}", reading.turned_unread);
        let (_, outline, _) = turned_profile(&scan, &outcome, &reading).unwrap();
        assert!(outline.faces.iter().any(Option::is_none), "{outline:?}");
        let half = area(&ring(&outline)).abs() / 2.0;
        assert!((half - 600.0).abs() < 0.02 * 600.0, "{half} mm² against 600: {outline:?}");
    }

    /// A shaft R 15 from z 0 to 20 and 40 to 60, a thread between them (mean R 12, 1 mm deep each
    /// way, 3 mm pitch) and flat steps where they meet: the thread is no face about the axis, so
    /// the faces go round no profile (T-LESS 01, its grip and thread). A single slice through the
    /// axis cuts the thread's helix and draws it as a zigzag of arcs; the median section, inside
    /// the part at half the turn or more, reads the band as its mean cylinder, R 12, a few pieces.
    #[test]
    fn a_threaded_band_is_read_as_its_mean_cylinder() {
        let n = 180usize;
        let mut rings: Vec<(f64, Box<dyn Fn(f64) -> f64>)> = Vec::new();
        let band = |z: f64| -> Box<dyn Fn(f64) -> f64> { Box::new(move |t: f64| 12.0 + (TAU * (z / 3.0 - t / TAU)).sin()) };
        for k in 0..=80 {
            let z = 20.0 * k as f64 / 80.0;
            rings.push((z, Box::new(|_| 15.0)));
        }
        for k in 0..=80 {
            let z = 20.0 + 20.0 * k as f64 / 80.0;
            rings.push((z, band(z)));
        }
        for k in 0..=80 {
            let z = 40.0 + 20.0 * k as f64 / 80.0;
            rings.push((z, Box::new(|_| 15.0)));
        }
        let mut positions: Vec<Vec3> = Vec::new();
        for (z, r) in &rings {
            for j in 0..n {
                let t = TAU * j as f64 / n as f64;
                let r = r(t);
                positions.push(Vec3::new(r * t.cos(), r * t.sin(), *z));
            }
        }
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for i in 0..rings.len() - 1 {
            for j in 0..n {
                let (a, b) = ((i * n + j) as u32, (i * n + (j + 1) % n) as u32);
                let (c, d) = (((i + 1) * n + j) as u32, ((i + 1) * n + (j + 1) % n) as u32);
                triangles.push([a, b, d]);
                triangles.push([a, d, c]);
            }
        }
        let bottom = positions.len() as u32;
        positions.push(Vec3::new(0.0, 0.0, 0.0));
        let top = positions.len() as u32;
        positions.push(Vec3::new(0.0, 0.0, 60.0));
        let last = (rings.len() - 1) * n;
        for j in 0..n {
            triangles.push([bottom, ((j + 1) % n) as u32, j as u32]);
            triangles.push([top, (last + j) as u32, (last + (j + 1) % n) as u32]);
        }
        let scan = Scan::new(&positions, &triangles);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let reading = read(&scan, &outcome);
        assert!(reading.revolve.is_some(), "turned: {:?}", reading.turned_unread);
        let (_, outline, _) = turned_profile(&scan, &outcome, &reading).unwrap();
        assert!(outline.corners.len() <= 14, "a few pieces, not the thread's zigzag: {outline:?}");
        let points = ring(&outline);
        // Along the axis from the profile's own low end: the band is its middle third.
        let low = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        // The profile's radius where it crosses the band's middle, off the axis.
        let middle = low + 30.0;
        let m = points.len();
        let in_band: Vec<f64> = (0..m)
            .filter_map(|k| {
                let (a, b) = (points[k], points[(k + 1) % m]);
                ((a.x - middle) * (b.x - middle) < 0.0).then(|| a.y + (b.y - a.y) * (middle - a.x) / (b.x - a.x))
            })
            .filter(|&y| y > 1.0)
            .collect();
        assert!(!in_band.is_empty() && in_band.iter().all(|r| (r - 12.0).abs() < 0.6), "the band at its mean R 12: {in_band:?} {outline:?}");
    }

    /// A block 40 × 40 × 10 with a turned boss on it, R 8 to 18 high and R 5 to 25: the whole is
    /// no turned part, but the boss selected alone reads as one, its slice through the axis open
    /// where the selection stops and closed down to the axis there. Its profile is the boss's
    /// half-section, 64 + 35 mm².
    #[test]
    fn a_turned_boss_selected_on_a_block_is_read_alone() {
        let (mut positions, mut triangles) = crate::test_meshes::fine_cuboid(Vec3::new(-20.0, -20.0, 0.0), Vec3::new(20.0, 20.0, 10.0), 10);
        let mut profile = vec![[25.0, 0.0]];
        profile.extend(steps([25.0, 0.5], [25.0, 5.0], 9));
        profile.extend(steps([25.0, 5.0], [18.0, 5.0], 14).into_iter().skip(1));
        profile.extend(steps([18.0, 5.0], [18.0, 8.0], 6).into_iter().skip(1));
        profile.extend(steps([18.0, 8.0], [10.0, 8.0], 16).into_iter().skip(1));
        let (boss, faces) = revolved(&profile, 96, false);
        let (base, from) = (positions.len() as u32, triangles.len());
        positions.extend(boss);
        triangles.extend(faces.iter().map(|t| t.map(|i| i + base)));
        let scan = Scan::new(&positions, &triangles);
        let found = find_all(&scan, &Options::default(), &|| false);
        assert!(read(&scan, &found).revolve.is_none(), "the block and the boss are no turned part");
        let selected: Vec<bool> = (0..triangles.len()).map(|t| t >= from).collect();
        let region = crate::reconstruct::read_region(&scan, &found, &selected).expect("the boss is read");
        assert_eq!(region.triangles.len(), triangles.len() - from);
        let reading = &region.reading;
        assert!(reading.revolve.is_some() && reading.turned_unread.is_none(), "{:?} {:?}", reading.revolve, reading.turned_unread);
        let (_, outline, _) = turned_profile(&region.scan(&scan), &region.outcome, reading).unwrap();
        // Closed down the axis 2 × 0.005 of the part's size into the block, so a join meets in material.
        let into = 2.0 * 0.005 * scan.extent();
        let expected = 99.0 + 8.0 * into;
        let half = area(&ring(&outline)).abs() / 2.0;
        assert!((half - expected).abs() < 0.03 * expected, "{half} mm² against {expected}: {outline:?}");
        // Along the axis, the boss's 15 and the stretch into the block.
        let along = |f: fn(f64, f64) -> f64, from: f64| outline.corners.iter().map(|c| c.x).fold(from, f);
        let span = along(f64::max, f64::NEG_INFINITY) - along(f64::min, f64::INFINITY);
        assert!((span - (15.0 + into)).abs() < 0.2, "{span} along the axis: {outline:?}");
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
            ProfileFace {
                face: Turned::Plane { height: 0.0 },
                middle: Vec2::new(0.0, 10.0),
                at_axis: Some(Vec2::new(0.0, 0.0)),
                extent: [Vec2::new(0.0, 10.0); 2],
            },
            ProfileFace { face: Turned::Cylinder { radius: 20.0 }, middle: Vec2::new(15.0, 20.0), at_axis: None, extent: [Vec2::new(15.0, 20.0); 2] },
            ProfileFace { face: Turned::Plane { height: 30.0 }, middle: Vec2::new(30.0, 15.0), at_axis: None, extent: [Vec2::new(30.0, 15.0); 2] },
            ProfileFace { face: Turned::Cylinder { radius: 10.0 }, middle: Vec2::new(40.0, 10.0), at_axis: None, extent: [Vec2::new(40.0, 10.0); 2] },
            ProfileFace {
                face: Turned::Plane { height: 50.0 },
                middle: Vec2::new(50.0, 5.0),
                at_axis: Some(Vec2::new(50.0, 0.0)),
                extent: [Vec2::new(50.0, 5.0); 2],
            },
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

    /// A ring scanned from one side (a brake disc on its hub, its bore never seen): its front face,
    /// rim and back face meet in a run open at both ends. The run is closed by a line from the
    /// front face's inner end (R 30) to the back face's (R 40), across where the scan has nothing.
    #[test]
    fn a_run_of_faces_open_at_both_ends_is_closed_across() {
        let faces = vec![
            ProfileFace {
                face: Turned::Plane { height: 0.0 },
                middle: Vec2::new(0.0, 45.0),
                at_axis: None,
                extent: [Vec2::new(0.0, 30.0), Vec2::new(0.0, 60.0)],
            },
            ProfileFace {
                face: Turned::Cylinder { radius: 60.0 },
                middle: Vec2::new(5.0, 60.0),
                at_axis: None,
                extent: [Vec2::new(0.0, 60.0), Vec2::new(10.0, 60.0)],
            },
            ProfileFace {
                face: Turned::Plane { height: 10.0 },
                middle: Vec2::new(10.0, 50.0),
                at_axis: None,
                extent: [Vec2::new(10.0, 60.0), Vec2::new(10.0, 40.0)],
            },
        ];
        let meetings = vec![Meeting { faces: [0, 1], at: Vec2::new(0.0, 60.0) }, Meeting { faces: [1, 2], at: Vec2::new(10.0, 60.0) }];
        let outline = turned_outline(&faces, &meetings, 1.0).unwrap();
        assert_eq!(outline.corners.len(), 4, "{outline:?}");
        for corner in [Vec2::new(0.0, 30.0), Vec2::new(0.0, 60.0), Vec2::new(10.0, 60.0), Vec2::new(10.0, 40.0)] {
            assert!(outline.corners.iter().any(|c| c.dist(corner) < 1e-9), "{corner:?} in {:?}", outline.corners);
        }
        assert_eq!(outline.faces.iter().filter(|f| f.is_none()).count(), 1, "one closing line: {outline:?}");
        assert_eq!(outline.faces.last(), Some(&None), "the closing line last: {outline:?}");
    }

    /// Halfway between neighbouring spokes, round past a full turn too: five spokes at 18°, 90°,
    /// 162°, 234° and 306° are sliced at 54°, 126°, 198°, 270° and 342°.
    #[test]
    fn slices_between_spokes_fall_halfway() {
        let spokes: Vec<f64> = [18.0_f64, 90.0, 162.0, 234.0, 306.0].iter().map(|a| a.to_radians()).collect();
        let between: Vec<f64> = between_spokes(&spokes).iter().map(|a| a.to_degrees()).collect();
        for (got, want) in between.iter().zip([54.0, 126.0, 198.0, 270.0, 342.0]) {
            assert!((got - want).abs() < 1e-9, "{between:?}");
        }
    }

    /// An arc whose ends all but meet (a round 2 mm across, cut where it touches the axis) is a
    /// line among the slivers, not a circle the long way round.
    #[test]
    fn an_arc_whose_ends_meet_is_a_sliver() {
        let outline = Outline {
            corners: vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(5.0, 10.0), Vec2::new(5.01, 10.0)],
            pieces: vec![Piece::Line, Piece::Line, Piece::Line, Piece::Arc { center: Vec2::new(5.005, 8.0), ccw: false }, Piece::Line],
            faces: vec![None; 5],
        };
        let kept = without_slivers(outline, 0.1, false);
        assert_eq!(kept.pieces.len(), 4, "{kept:?}");
        assert!(kept.pieces.iter().all(|p| *p == Piece::Line), "{kept:?}");
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

    /// A block 30 × 20 whose top right corner is cut by a land no wall holds (Find All left it as
    /// freeform, as it leaves a gear's tip lands): where the scan there runs along the prism, the
    /// land is drawn as the slice runs; where it does not (a hole or a slot across the prism), or
    /// with no fill, the walls run on to their corner.
    #[test]
    fn a_gap_between_walls_is_drawn_as_the_slice_runs_only_where_the_scan_runs_along_the_prism() {
        let walls = vec![
            (0, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::X }),
            (1, Wall::Line { point: Vec2::new(30.0, 0.0), direction: Vec2::Y }),
            (2, Wall::Line { point: Vec2::new(0.0, 20.0), direction: Vec2::X }),
            (3, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::Y }),
        ];
        let (mut points, mut on) = (Vec::new(), Vec::new());
        let mut side = |from: Vec2, to: Vec2, wall: Option<usize>| {
            for i in 0..20 {
                points.push(from + (to - from) * (f64::from(i) / 20.0));
                on.push(wall);
            }
        };
        side(Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), Some(0));
        side(Vec2::new(30.0, 0.0), Vec2::new(30.0, 15.0), Some(1));
        side(Vec2::new(30.0, 15.0), Vec2::new(25.0, 20.0), None);
        side(Vec2::new(25.0, 20.0), Vec2::new(0.0, 20.0), Some(2));
        side(Vec2::new(0.0, 20.0), Vec2::new(0.0, 0.0), Some(3));
        let land: Vec<bool> = on.iter().map(Option::is_none).collect();
        let loops = [(points, on)];
        let corner = Vec2::new(30.0, 20.0);
        let has = |o: &Outline, p: Vec2| o.corners.iter().any(|c| c.dist(p) < 0.05);
        // The land's scan runs along the prism: drawn, one line no face holds.
        let along = vec![vec![true; land.len()]];
        let filled = prism_outlines(&walls, &loops, 0.2, Some(&Fill { tolerance: 0.05, along: &along }));
        assert_eq!(filled.len(), 1, "{filled:?}");
        let o = &filled[0];
        assert!(!has(o, corner) && has(o, Vec2::new(30.0, 15.0)) && has(o, Vec2::new(25.0, 20.0)), "{o:?}");
        assert_eq!((o.pieces.len(), o.faces.iter().filter(|f| f.is_none()).count()), (5, 1), "{o:?}");
        // It runs across the prism there: not drawn along it.
        let across = vec![land.iter().map(|l| !l).collect::<Vec<bool>>()];
        for fill in [Some(Fill { tolerance: 0.05, along: &across }), None] {
            let sharp = prism_outlines(&walls, &loops, 0.2, fill.as_ref());
            assert!(sharp.len() == 1 && sharp[0].corners.len() == 4 && has(&sharp[0], corner), "{sharp:?}");
        }
    }

    /// A block 40 × 30 with a notch in its top whose two inside corners are each cut by a short
    /// piece 0.4 long (as a scan's fit leaves a tight inside round): offset outward by 0.5 those
    /// pieces would close to nothing, so the outline a pocket is offset from has the corners square
    /// instead; offset by 0.1 they stay; an outside corner's short piece stays either way.
    #[test]
    fn pieces_an_offset_would_close_are_cut_square() {
        let t = 0.4 / std::f64::consts::SQRT_2;
        let corners = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(40.0, 0.0),
            Vec2::new(40.0, 30.0 - t),
            Vec2::new(40.0 - t, 30.0),
            Vec2::new(28.0, 30.0),
            Vec2::new(28.0, 20.0 + t),
            Vec2::new(28.0 - t, 20.0),
            Vec2::new(12.0 + t, 20.0),
            Vec2::new(12.0, 20.0 + t),
            Vec2::new(12.0, 30.0),
            Vec2::new(0.0, 30.0),
        ];
        let outline = Outline { pieces: vec![Piece::Line; corners.len()], faces: vec![None; corners.len()], corners };
        let wide = roomy(&outline, 0.5);
        assert_eq!(wide.corners.len(), 9, "the notch's two inside corners are square: {wide:?}");
        let has = |o: &Outline, p: Vec2| o.corners.iter().any(|c| c.dist(p) < 1e-9);
        assert!(has(&wide, Vec2::new(28.0, 20.0)) && has(&wide, Vec2::new(12.0, 20.0)), "{wide:?}");
        assert!(has(&wide, Vec2::new(40.0, 30.0 - t)) && has(&wide, Vec2::new(40.0 - t, 30.0)), "the outside corner keeps its chamfer: {wide:?}");
        assert_eq!(roomy(&outline, 0.1), outline);
        // The other way round, the same.
        let back = Outline { corners: outline.corners.iter().rev().copied().collect(), ..outline.clone() };
        assert_eq!(roomy(&back, 0.5).corners.len(), 9);
    }

    /// A slot 40 × 20 with round ends, 10 tall, its outline drawn 0.1 small all round (R 9.9, the
    /// flats 19.8 apart): the scan stands 0.1 proud of it, on the flats and on the rounds alike.
    /// Drawn true it stands proud by nothing; a flange 5 further out is not the slot's wall and is
    /// not counted, nor are the slot's ends, which face along it: those count against the end the
    /// slot is sat on.
    #[test]
    fn a_scan_outside_its_outline_is_measured_proud_of_it() {
        let slot = |r: f64| Outline {
            corners: vec![Vec2::new(10.0, 10.0 - r), Vec2::new(30.0, 10.0 - r), Vec2::new(30.0, 10.0 + r), Vec2::new(10.0, 10.0 + r)],
            pieces: vec![
                Piece::Line,
                Piece::Arc { center: Vec2::new(30.0, 10.0), ccw: true },
                Piece::Line,
                Piece::Arc { center: Vec2::new(10.0, 10.0), ccw: true },
            ],
            faces: vec![None; 4],
        };
        // The slot's wall as strips of triangles from z = -5 to 5, and its two ends as fans.
        let mut rim: Vec<Vec2> = (0..=20).map(|i| Vec2::new(10.0 + f64::from(i), 0.0)).collect();
        rim.extend((1..48).map(|i| Vec2::new(30.0, 10.0) + rotated(Vec2::new(0.0, -10.0), PI * f64::from(i) / 48.0)));
        rim.extend((0..=20).map(|i| Vec2::new(30.0 - f64::from(i), 20.0)));
        rim.extend((1..48).map(|i| Vec2::new(10.0, 10.0) + rotated(Vec2::new(0.0, 10.0), PI * f64::from(i) / 48.0)));
        let wall = |rim: &[Vec2], positions: &mut Vec<Vec3>, triangles: &mut Vec<[u32; 3]>| {
            let (base, n) = (positions.len() as u32, rim.len() as u32);
            for q in rim {
                positions.push(Vec3::new(q.x, q.y, -5.0));
                positions.push(Vec3::new(q.x, q.y, 5.0));
            }
            for i in 0..n {
                let (a, b) = (base + 2 * i, base + 2 * ((i + 1) % n));
                triangles.push([a, b, a + 1]);
                triangles.push([b, b + 1, a + 1]);
            }
        };
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        wall(&rim, &mut positions, &mut triangles);
        for z in [-5.0, 5.0] {
            let centre = positions.len() as u32;
            positions.push(Vec3::new(20.0, 10.0, z));
            let first = positions.len() as u32;
            // An end's rim reaches 3 past the wall: a face along the prism, not counted.
            positions.extend(rim.iter().map(|q| Vec3::new(20.0 + (q.x - 20.0) * 1.3, 10.0 + (q.y - 10.0) * 1.3, z)));
            let n = rim.len() as u32;
            triangles.extend((0..n).map(|i| [centre, first + i, first + (i + 1) % n]));
        }
        let plane = Plane { origin: Vec3::ZERO, x: Vec3::new(1.0, 0.0, 0.0), y: Vec3::new(0.0, 1.0, 0.0) };
        let scan = Scan::new(&positions, &triangles);
        let small = proud(&scan, &plane, &slot(9.9), -5.0, 5.0, None, 1.0);
        assert!((small - 0.1).abs() < 1e-9, "{small}");
        assert!(proud(&scan, &plane, &slot(10.0), -5.0, 5.0, None, 1.0) < 1e-9);
        // The other way round, the same.
        let back = |o: Outline| Outline {
            corners: vec![o.corners[0], o.corners[3], o.corners[2], o.corners[1]],
            pieces: vec![
                Piece::Arc { center: Vec2::new(10.0, 10.0), ccw: false },
                Piece::Line,
                Piece::Arc { center: Vec2::new(30.0, 10.0), ccw: false },
                Piece::Line,
            ],
            ..o
        };
        let small = proud(&scan, &plane, &back(slot(9.9)), -5.0, 5.0, None, 1.0);
        assert!((small - 0.1).abs() < 1e-9, "{small}");
        // A flange round it, 5 out: beyond the reach.
        let flange: Vec<Vec2> = rim.iter().map(|q| Vec2::new(20.0 + (q.x - 20.0) * 1.5, 10.0 + (q.y - 10.0) * 1.5)).collect();
        wall(&flange, &mut positions, &mut triangles);
        let scan = Scan::new(&positions, &triangles);
        let small = proud(&scan, &plane, &slot(9.9), -5.0, 5.0, None, 1.0);
        assert!((small - 0.1).abs() < 1e-9, "{small}");
        // Above the levels asked, nothing.
        assert!(proud(&scan, &plane, &slot(9.9), 6.0, 8.0, None, 1.0) < 1e-9);
        // Sat on its lower end, taken to be at z = -4.8: the end's scan, at -5, is 0.2 past it,
        // more than the wall is proud of the outline. On its upper end, taken at 4.7, 0.3; an
        // end the scan does not pass, nothing more than the wall.
        let sat = |seat| proud(&scan, &plane, &slot(9.9), -5.0, 5.0, Some(seat), 1.0);
        assert!((sat((-4.8, 1.0)) - 0.2).abs() < 1e-9 && (sat((4.7, -1.0)) - 0.3).abs() < 1e-9, "{} {}", sat((-4.8, 1.0)), sat((4.7, -1.0)));
        assert!((sat((-5.0, 1.0)) - 0.1).abs() < 1e-9 && (sat((-5.5, 1.0)) - 0.1).abs() < 1e-9);
    }

    /// A slot 40 × 20 with round ends, a hair off true as a scan leaves it, is its own image left
    /// to right and top to bottom; with one corner cut off it is neither; an L is neither.
    #[test]
    fn an_outline_drawn_symmetric_is_read_as_its_own_mirror_image() {
        let slot = |nudge: f64| Outline {
            corners: vec![Vec2::new(10.0, 0.0), Vec2::new(30.0 + nudge, 0.0), Vec2::new(30.0, 20.0), Vec2::new(10.0, 20.0 - nudge)],
            pieces: vec![
                Piece::Line,
                Piece::Arc { center: Vec2::new(30.0, 10.0), ccw: true },
                Piece::Line,
                Piece::Arc { center: Vec2::new(10.0, 10.0 + nudge), ccw: true },
            ],
            faces: vec![None; 4],
        };
        let found = mirrored(&slot(0.004), 0.02);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].across && (found[0].at - 20.0).abs() < 0.01 && found[0].pairs == [(0, 1), (2, 3)], "{found:?}");
        assert!(!found[1].across && (found[1].at - 10.0).abs() < 0.01 && found[1].pairs == [(0, 3), (1, 2)], "{found:?}");
        assert!(mirrored(&slot(0.2), 0.02).is_empty(), "off by more than the tolerance");
        let ell = Outline {
            corners: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 8.0),
                Vec2::new(8.0, 8.0),
                Vec2::new(8.0, 20.0),
                Vec2::new(0.0, 20.0),
            ],
            pieces: vec![Piece::Line; 6],
            faces: vec![None; 6],
        };
        assert!(mirrored(&ell, 0.02).is_empty());
    }

    /// A gear outline in lines and arcs (12 teeth: a tip land on the circle of radius 20, straight
    /// flanks, a root land at 16), anywhere on the plane: read as that circle with 12 gaps, the gap
    /// two flanks and a root closed outside the circle. A plain circle does not repeat.
    #[test]
    fn a_gears_outline_is_a_circle_and_one_gap_repeated() {
        let (teeth, centre) = (12usize, Vec2::new(7.0, -3.0));
        let (mut corners, mut pieces) = (Vec::new(), Vec::new());
        for tooth in 0..teeth {
            let at = |part: f64, radius: f64| {
                let angle = TAU * (tooth as f64 + part) / teeth as f64;
                centre + Vec2::new(angle.cos(), angle.sin()) * radius
            };
            // Tip land, flank down, root land, flank up.
            corners.extend([at(0.0, 20.0), at(0.3, 20.0), at(0.45, 16.0), at(0.85, 16.0)]);
            pieces.extend([Piece::Arc { center: centre, ccw: true }, Piece::Line, Piece::Arc { center: centre, ccw: true }, Piece::Line]);
        }
        let faces = vec![None; pieces.len()];
        let found = repeated(&Outline { corners, pieces, faces }, 0.05, |r| r).expect("a gear");
        assert_eq!(found.order, 12);
        assert!(found.centre.dist(centre) < 0.02 && (found.tip - 20.0).abs() < 1e-6, "{found:?}");
        let gap = &found.gap;
        assert_eq!(gap.pieces.len(), 6, "{gap:?}");
        assert_eq!(gap.pieces[..3], [Piece::Line, Piece::Arc { center: centre, ccw: true }, Piece::Line]);
        assert!(matches!(gap.pieces[4], Piece::Arc { ccw: false, .. }), "back round the outside: {gap:?}");
        let radii: Vec<f64> = gap.corners.iter().map(|c| c.dist(found.centre)).collect();
        assert!((radii[0] - 20.0).abs() < 0.02 && (radii[3] - 20.0).abs() < 0.02 && radii[4] > 21.0 && radii[5] > 21.0, "{radii:?}");
        assert!(is_simple_ring(&ring(gap)), "{gap:?}");
        let circle = Outline {
            corners: vec![Vec2::new(5.0, 0.0), Vec2::new(-5.0, 0.0)],
            pieces: vec![Piece::Arc { center: Vec2::ZERO, ccw: true }; 2],
            faces: vec![None; 2],
        };
        assert_eq!(repeated(&circle, 0.05, |r| r), None);
        // A square's corners lie on a circle, but none of its outline does.
        let square = Outline {
            corners: vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)],
            pieces: vec![Piece::Line; 4],
            faces: vec![None; 4],
        };
        assert_eq!(repeated(&square, 0.05, |r| r), None);
    }

    /// A slot shape 30 long with a round end of radius 10 about (30, 10), whose face was fitted as
    /// a cylinder a little off it (about (29.6, 10), radius 10.2: as a gear's flank is fitted over
    /// more than the slice sees). The slice's own points on the round lie on one circle within the
    /// tolerance, so the outline's arc is that circle; with no tolerance to judge by, the face's.
    #[test]
    fn a_round_wall_the_slice_strays_from_is_fitted_to_the_slice() {
        let walls = vec![
            (0, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::X }),
            (1, Wall::Circle { center: Vec2::new(29.6, 10.0), radius: 10.2 }),
            (2, Wall::Line { point: Vec2::new(0.0, 20.0), direction: Vec2::X }),
            (3, Wall::Line { point: Vec2::new(0.0, 0.0), direction: Vec2::Y }),
        ];
        let (mut points, mut on) = (Vec::new(), Vec::new());
        for i in 0..30 {
            points.push(Vec2::new(f64::from(i), 0.0));
            on.push(Some(0));
        }
        for i in 0..40 {
            // A hair of noise on the round, well inside the tolerance.
            let a: f64 = -PI / 2.0 + PI * f64::from(i) / 40.0;
            let r = 10.0 + 0.01 * (7.0 * a).sin();
            points.push(Vec2::new(30.0 + r * a.cos(), 10.0 + r * a.sin()));
            on.push(Some(1));
        }
        for i in 0..30 {
            points.push(Vec2::new(30.0 - f64::from(i), 20.0));
            on.push(Some(2));
        }
        for i in 0..20 {
            points.push(Vec2::new(0.0, 20.0 - f64::from(i)));
            on.push(Some(3));
        }
        let along = vec![vec![true; points.len()]];
        let loops = [(points, on)];
        let centre = |fill: Option<&Fill<'_>>| {
            let found = prism_outlines(&walls, &loops, 0.2, fill);
            assert_eq!(found.len(), 1, "{found:?}");
            let arcs: Vec<Vec2> =
                found[0].pieces.iter().filter_map(|p| if let Piece::Arc { center, .. } = p { Some(*center) } else { None }).collect();
            assert_eq!((found[0].pieces.len(), arcs.len()), (4, 1), "{found:?}");
            arcs[0]
        };
        let fitted = centre(Some(&Fill { tolerance: 0.05, along: &along }));
        assert!(fitted.dist(Vec2::new(30.0, 10.0)) < 0.02, "{fitted:?}");
        let as_found = centre(None);
        assert!(as_found.dist(Vec2::new(30.0, 10.0)) > 0.2, "{as_found:?}");
        // A round the slice lies on is left as its face has it.
        let exact = circle_through(&[Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), Vec2::new(0.6, -0.8)]).unwrap();
        assert!(exact.0.dist(Vec2::ZERO) < 1e-9 && (exact.1 - 1.0).abs() < 1e-9, "{exact:?}");
        assert_eq!(circle_through(&[Vec2::ZERO, Vec2::X, Vec2::X * 2.0]), None);
    }

    /// The scan at a slice's point runs along the prism where its triangle faces square to the
    /// prism and the slices at other levels pass there too.
    #[test]
    fn scan_runs_along_a_prism_where_it_faces_square_and_other_levels_agree() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let (positions, triangles) = extruded(&square, 0.0, 10.0, 4);
        let scan = Scan::new(&positions, &triangles);
        let normal = |t: usize| scan.corners(t).and_then(|[a, b, c]| (b - a).cross(c - a).normalized()).unwrap();
        let side = (0..triangles.len()).find(|&t| normal(t).z.abs() < 1e-9).expect("a side triangle");
        let cap = (0..triangles.len()).find(|&t| normal(t).z.abs() > 0.99).expect("a cap triangle");
        let at = [Vec2::new(5.0, 0.0), Vec2::new(5.0, 0.0)];
        let tris = [side, cap];
        let ring: Vec<Vec2> = square.iter().map(|c| Vec2::new(c[0], c[1])).collect();
        let cut = [(&at[..], &tris[..])];
        // Other levels show the same outline: the side point runs along, the cap's does not.
        let same = runs_along(&scan, &cut, Vec3::Z, 0.0, &[vec![ring.clone()], vec![ring.clone()]], 0.1);
        assert_eq!(same, [[true, false]]);
        // One level has nothing there (a slot across the prism at the middle only).
        let moved: Vec<Vec2> = ring.iter().map(|p| *p + Vec2::new(0.0, 3.0)).collect();
        let differs = runs_along(&scan, &cut, Vec3::Z, 0.0, &[vec![ring], vec![moved]], 0.1);
        assert_eq!(differs, [[false, false]]);
        // Too short a prism to slice elsewhere: by facing alone.
        assert_eq!(runs_along(&scan, &cut, Vec3::Z, 0.0, &[], 0.1), [[true, false]]);
        // The other levels were sliced and gave no loop at all (the scan has a patch missing
        // there): nothing is there to agree, and nothing overflows finding that out.
        assert_eq!(runs_along(&scan, &cut, Vec3::Z, 0.0, &[Vec::new(), Vec::new()], 0.1), [[false, false]]);
    }

    /// An open half-section with neither end on the axis (a wheel rim's skin: the scan stops at
    /// both lips, on the same barrel face) is closed straight across from end to end, the closing
    /// line a wall of its own: not round that face's circle, which bulged far past the scan.
    #[test]
    fn an_open_run_is_closed_straight_across_not_along_its_end_wall() {
        let center = Vec2::new(15.0, 40.0);
        let radius = (15.0f64 * 15.0 + 30.0 * 30.0).sqrt();
        let mut walls = vec![
            (0, Wall::Circle { center, radius }),
            (1, Wall::Line { point: Vec2::new(6.0, 0.0), direction: Vec2::Y }),
            (2, Wall::Line { point: Vec2::new(0.0, 4.0), direction: Vec2::X }),
            (3, Wall::Line { point: Vec2::new(24.0, 0.0), direction: Vec2::Y }),
            // The axis.
            (usize::MAX, Wall::Line { point: Vec2::ZERO, direction: Vec2::X }),
        ];
        // The lips at the two ends are the highest of it: the barrel face dips between them.
        let on_circle = |x: f64| Vec2::new(x, center.y - (radius * radius - (x - center.x) * (x - center.x)).sqrt());
        let (mut points, mut on) = (Vec::new(), Vec::new());
        let mut add = |p: Vec2, wall: usize| {
            points.push(p);
            on.push(Some(wall));
        };
        for i in 0..=12 {
            add(on_circle(0.5 * f64::from(i)), 0);
        }
        let top = on_circle(6.0).y;
        for i in 1..12 {
            add(Vec2::new(6.0, top + (4.0 - top) * f64::from(i) / 12.0), 1);
        }
        for i in 0..=18 {
            add(Vec2::new(6.0 + f64::from(i), 4.0), 2);
        }
        for i in 1..12 {
            add(Vec2::new(24.0, 4.0 + (top - 4.0) * f64::from(i) / 12.0), 3);
        }
        for i in 0..=12 {
            add(on_circle(24.0 + 0.5 * f64::from(i)), 0);
        }
        // Read as a closed loop, its two ends on the circle are one stretch: the circle closes it
        // from one side to the other, and the lips at the run's ends are lost.
        let round = outline_of(&walls, &points, &on, 0.2, Some((0.05, None))).expect("an outline");
        assert!(round.pieces.len() == 4 && ring(&round).iter().all(|p| p.y < 9.0), "the barrel's circle drawn across: {round:?}");
        // As the open run it is: one part, with the line that closes it.
        let parts = above_axis_open(&points, &on, 4, 5, 0.4);
        assert_eq!(parts.len(), 1);
        let (closed, closed_on, line) = &parts[0];
        walls.push((usize::MAX, line.expect("closed straight across")));
        let o = outline_of(&walls, closed, closed_on, 0.2, Some((0.05, None))).expect("an outline");
        let n = o.corners.len();
        let closing = (0..n).find(|&k| o.corners[k].dist(Vec2::new(30.0, 10.0)) < 0.05 && o.corners[(k + 1) % n].dist(Vec2::new(0.0, 10.0)) < 0.05);
        assert!(closing.is_some_and(|k| o.pieces[k] == Piece::Line), "{o:?}");
        let r = ring(&o);
        assert!(is_simple_ring(&r) && r.iter().all(|p| p.y <= 10.05 && p.y >= 3.95), "within the run's own reach: {r:?}");
        // A run that comes down to the axis is closed along the axis, as before: no closing line.
        let grounded: Vec<Vec2> = std::iter::once(Vec2::new(0.0, -1.0)).chain(points.iter().copied()).collect();
        let walls_on: Vec<Option<usize>> = std::iter::once(Some(0)).chain(on.iter().copied()).collect();
        assert!(above_axis_open(&grounded, &walls_on, 4, 5, 0.4).iter().all(|part| part.2.is_none()));
    }

    /// A rounded rectangle 40 × 30, corners R 5, two of whose rounds have one radius all the way
    /// along: those two are taken out for sharp corners (the lines run on to where they cross), the
    /// other two stay; each corner knows which way the material lies, so where its lines have moved
    /// in by a draft it has moved in along both.
    #[test]
    fn rounds_of_one_radius_are_taken_out_for_sharp_corners_to_fillet() {
        let (w, h, r) = (40.0, 30.0, 5.0);
        let corner = |x: f64, y: f64| Vec2::new(x, y);
        // Counter-clockwise from the bottom side: line, round, line, round, ...
        let outline = Outline {
            corners: vec![
                corner(r, 0.0),
                corner(w - r, 0.0),
                corner(w, r),
                corner(w, h - r),
                corner(w - r, h),
                corner(r, h),
                corner(0.0, h - r),
                corner(0.0, r),
            ],
            pieces: vec![
                Piece::Line,
                Piece::Arc { center: corner(w - r, r), ccw: true },
                Piece::Line,
                Piece::Arc { center: corner(w - r, h - r), ccw: true },
                Piece::Line,
                Piece::Arc { center: corner(r, h - r), ccw: true },
                Piece::Line,
                Piece::Arc { center: corner(r, r), ccw: true },
            ],
            faces: (0..8).map(Some).collect(),
        };
        let (sharp, corners) = sharpened(std::slice::from_ref(&outline), |face| matches!(face, 1 | 5).then_some(r));
        assert_eq!(corners.len(), 2, "{corners:?}");
        assert!(corners[0].at.dist(corner(w, 0.0)) < 1e-9 && corners[1].at.dist(corner(0.0, h)) < 1e-9, "{corners:?}");
        assert!(corners.iter().all(|c| c.radius == r));
        let o = &sharp[0];
        assert_eq!((o.pieces.len(), o.pieces.iter().filter(|p| matches!(p, Piece::Arc { .. })).count()), (6, 2), "{o:?}");
        assert!(o.corners.iter().any(|c| c.dist(corner(w, 0.0)) < 1e-9) && o.corners.iter().any(|c| c.dist(corner(0.0, h)) < 1e-9), "{o:?}");
        assert_eq!(o.faces, [Some(0), Some(2), Some(3), Some(4), Some(6), Some(7)]);
        // Both walls lean in by 1: the corner moves in by 1 along each.
        assert!(corners[0].moved(1.0).dist(corner(w - 1.0, 1.0)) < 1e-9, "{:?}", corners[0].moved(1.0));
        assert!(corners[1].moved(1.0).dist(corner(1.0, h - 1.0)) < 1e-9, "{:?}", corners[1].moved(1.0));
        // Walked the other way round, the material is still inside.
        let reversed = Outline {
            corners: outline.corners.iter().rev().copied().collect(),
            pieces: (0..8)
                .map(|k| match outline.pieces[(14 - k) % 8] {
                    Piece::Arc { center, ccw } => Piece::Arc { center, ccw: !ccw },
                    Piece::Line => Piece::Line,
                })
                .collect(),
            faces: (0..8).map(|k| Some((14 - k) % 8)).collect(),
        };
        let (_, back) = sharpened(std::slice::from_ref(&reversed), |face| (face == 1).then_some(r));
        assert!(back.len() == 1 && back[0].at.dist(corner(w, 0.0)) < 1e-9 && back[0].moved(1.0).dist(corner(w - 1.0, 1.0)) < 1e-9, "{back:?}");
        // No round of one radius: nothing changes.
        let (same, none) = sharpened(std::slice::from_ref(&outline), |_| None);
        assert!(none.is_empty() && same[0] == outline);
    }

    /// A gear's outline (20 teeth, tips at 22, roots at 17.5, each tooth a little off the others as
    /// a scan's fit leaves them) repeats 20 times round its middle, wherever it lies; a square 4
    /// times; a circle and an L do not repeat.
    #[test]
    fn a_gears_outline_repeats_once_for_each_tooth() {
        let gear = |teeth: usize, shift: Vec2| -> Vec<Vec2> {
            let mut ring = Vec::new();
            for tooth in 0..teeth {
                // Root, flank up, tip, flank down: eight places a tooth, the tooth a hair off true.
                let wobble = 0.02 * ((tooth * 7 % 5) as f64 - 2.0);
                for (part, radius) in [(0.0, 17.5), (0.2, 17.5), (0.3, 20.0), (0.4, 22.0 + wobble), (0.6, 22.0 + wobble), (0.7, 20.0), (0.8, 17.5)] {
                    let angle = TAU * (tooth as f64 + part) / teeth as f64;
                    ring.push(shift + Vec2::new(angle.cos(), angle.sin()) * radius);
                }
            }
            ring
        };
        let found = rotational(&gear(20, Vec2::new(31.0, -8.0)), 0.08).expect("a gear");
        assert_eq!(found.order, 20, "{found:?}");
        assert!(found.centre.dist(Vec2::new(31.0, -8.0)) < 0.05 && (found.tip - 22.0).abs() < 0.06 && (found.root - 17.5).abs() < 0.3, "{found:?}");
        assert_eq!(rotational(&gear(37, Vec2::ZERO), 0.08).map(|r| r.order), Some(37));
        let square = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        assert_eq!(rotational(&square, 0.05).map(|r| r.order), Some(4));
        let circle: Vec<Vec2> = (0..96).map(|k| TAU * f64::from(k) / 96.0).map(|a| Vec2::new(a.cos(), a.sin()) * 12.0).collect();
        assert_eq!(rotational(&circle, 0.05), None, "a circle repeats any number of times");
        let ell = [Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), Vec2::new(30.0, 8.0), Vec2::new(8.0, 8.0), Vec2::new(8.0, 20.0), Vec2::new(0.0, 20.0)];
        assert_eq!(rotational(&ell, 0.05), None);
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
        let outlines = prism_outlines(&walls, &[(points, on)], 0.5, None);
        assert_eq!(outlines.len(), 1);
        let o = &outlines[0];
        assert_eq!(o.corners.len(), 4);
        for corner in [Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0), Vec2::new(30.0, 20.0), Vec2::new(0.0, 20.0)] {
            assert!(o.corners.iter().any(|c| c.dist(corner) < 1e-9), "{corner:?} in {:?}", o.corners);
        }
    }
}
