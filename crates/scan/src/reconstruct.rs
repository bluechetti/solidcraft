//! What Reconstruct reads off the shapes Find All found, after SurfaceCraft's: the prisms they
//! bound, the part as turned when most of it lies about one axis, the shapes they repeat, and the
//! round holes no profile holds. Only the fitted shapes, their areas and how far each runs are
//! read, so a proposal says what the shapes say.
//!
//! - **Prisms**: along each direction a plane faces, the part is cut into slabs at the planes
//!   facing that way and where walls end; slabs one after another with the same walls (planes
//!   square to the direction, cylinders along it) are one extrusion ([`extrusions`]), from the
//!   first slab's level to the last's, so a plate under a boss is the plate to its own top and the
//!   boss from there. Its middle is where to slice the scan for its profile.
//! - **A turned part** ([`revolve`]): round faces about one axis, with the planes square to it,
//!   covering most of what the shapes cover.
//! - **Repeats** ([`repeats`]): shapes alike and evenly spaced, along a line, round an axis, in a
//!   grid, or drilled along the radius all round an axis (a turned part's cross holes); and
//!   which of them a solid can be built with as one feature and a pattern ([`pattern_plans`]).
//! - **Holes** ([`holes`]): the round holes no prism's profile or turned profile holds, each but
//!   the first of a repeat (whose pattern makes the rest).

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{PI, TAU};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::{Kind, Shape};
use crate::segment::{Outcome, Segment};
use crate::{Scan, in_plane};

/// Directions this close are one direction (a cosine: about 2°).
const SAME_DIRECTION: f64 = 0.9994;
/// A plane this close to square to a direction is a wall along it (a sine: about 2°).
const SQUARE: f64 = 0.035;
/// Walls may lean this far from running along a prism (a sine: about 10°) and still be its walls,
/// when they all lean alike: a moulded or cast part's draft.
const MOST_DRAFT: f64 = 0.174;
/// The cosine of that lean, for a round's axis.
const MOST_DRAFT_COS: f64 = 0.985;
/// Walls leaning within this of their mean lean alike (radians: about 0.5°).
const SAME_DRAFT: f64 = 0.009;
/// A mean lean smaller than this is no draft (radians: about 0.3°): the walls are square.
const LEAST_DRAFT: f64 = 0.005;
/// A shell's walls are at most this share of the part's size thick.
const MOST_WALL: f64 = 0.15;
/// Wall thicknesses within this share of each other are one thickness.
const SAME_WALL: f64 = 0.1;
/// A shell's paired faces face at least this many ways (a lid and walls round it).
const LEAST_SHELL_SIDES: usize = 3;
/// ... and are at least this share of the area of the part's flat faces.
const LEAST_SHELL_SHARE: f64 = 0.5;
/// A prism's sides have to be this many walls at least, or a cylinder, which is a side by itself.
const LEAST_WALLS: usize = 3;
/// Radii within this share of each other are one radius.
const SAME_RADIUS: f64 = 0.02;
/// Areas within this share of each other are one area.
const SAME_AREA: f64 = 0.15;
/// Spacings within this share of their mean are even.
const EVEN: f64 = 0.03;
/// The sine of the least angle between a grid's two directions.
const NOT_IN_LINE: f64 = 0.1;
/// Fewer than this is not a repeat: any two shapes are evenly spaced.
const LEAST_REPEATS: usize = 3;
/// Holes drilled along the radius round an axis: each one's axis passes this close to the point
/// they all cross, as a share of its radius.
const RADIAL_CROSSING: f64 = 0.1;
/// A round this close to another's axis stands on it, as a share of the radius.
const SAME_AXIS: f64 = 0.05;
/// Round faces about one axis are at least this many, or any lone cylinder would be a turned part.
const LEAST_ROUND: usize = 2;
/// Faces about the axis cover at least this share of what the shapes cover.
const MOSTLY_TURNED: f64 = 0.75;
/// Round shapes about an axis covering at least this share, with faces repeated evenly round it
/// ([`spokes_about`]) covering the rest up to [`MOSTLY_TURNED`], are a turned part with spokes.
const SPOKED_TURNED: f64 = 0.5;
/// ... of which faces that turn (cylinders, cones, tori, balls: not the flat faces square to the
/// axis) cover at least this share: a block's four sides lie evenly round a boss on it, but its
/// flat top and bottom do not make it turned.
const SPOKED_ROUND: f64 = 0.3;
/// Alike faces round an axis: sizes within this share, tilts to the axis within this (as the
/// cosine), distances from it within this share of the largest.
const SPOKE_SIZE: f64 = 0.12;
const SPOKE_TILT: f64 = 0.05;
const SPOKE_REACH: f64 = 0.08;
/// Places round the axis within this share of the step from where an even ring puts them.
const SPOKE_EVEN: f64 = 0.15;

/// Round shapes about one axis covering at least this share are reported as an axis to rebuild
/// about by hand, though the part is no turned part.
const PARTLY_TURNED: f64 = 0.25;
/// A face this close to the axis lies on it, as a share of the part's size.
const ON_THE_AXIS: f64 = 0.05;
/// How far short of the plane at its end a wall may stop and still run to it (a fillet, the
/// scan's edge), as a share of the part's size.
const WALL_START: f64 = 0.03;
/// How far apart two planes along a direction may lie and be one level, as a share of the reach.
const SAME_LEVEL: f64 = 0.25;
/// An extrusion is offered while it explains at least this share of what the shapes cover that
/// those before it do not.
const LEAST_NEW: f64 = 0.02;

/// A shape and how much of the scan it covers (none for a piece of another face's surface,
/// [`Reading::same`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub shape: Shape,
    pub area: f64,
}

/// Two planes facing apart and the walls between them; or one plane and the walls that run from
/// it, its far end some other surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Prism {
    /// Along the prism (unit), from the end at `low` to the end at `high`.
    pub direction: Vec3,
    /// A point the levels are measured from along `direction`.
    pub origin: Vec3,
    pub low: f64,
    pub high: f64,
    /// The end planes and the walls, as indices into the faces; with a far end that is no plane,
    /// the one plane twice.
    pub ends: [usize; 2],
    pub walls: Vec<usize>,
    /// Whether one end is no plane: it is then as far as all the walls run.
    pub open: bool,
    /// How far its walls lean in, in radians: positive where the section shrinks from `low` to
    /// `high` (a moulded part's draft), 0 for walls square to its ends.
    pub draft: f64,
}

impl Prism {
    /// How long it is, end to end.
    pub fn depth(&self) -> f64 {
        self.high - self.low
    }

    /// The point halfway along, where a slice shows the profile.
    pub fn middle(&self) -> Vec3 {
        self.origin + self.direction * (0.5 * (self.low + self.high))
    }
}

/// How the shapes of a repeat are laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    /// One after another by `step`.
    Linear { step: Vec3 },
    /// Round the unit `axis` through `centre`, `angle` radians apart.
    Circular { centre: Vec3, axis: Vec3, angle: f64 },
    /// `columns` one after another by `step`, in `rows` one after another by `across`; row by row
    /// in the members, the first at the corner both steps run from.
    Grid { step: Vec3, across: Vec3, columns: usize, rows: usize },
}

/// Shapes alike and evenly spaced.
#[derive(Clone, Debug, PartialEq)]
pub struct Repeat {
    pub kind: Kind,
    /// As indices into the faces, in order along the layout.
    pub members: Vec<usize>,
    pub layout: Layout,
}

/// A repeat a solid can be built with as one feature and a pattern of it.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternPlan {
    pub repeat: Repeat,
    /// The feature to repeat: the round faces on the first place's axis.
    pub first: Vec<usize>,
    /// The same at every other place: what the pattern makes.
    pub rest: Vec<usize>,
    /// How far the worst place lies from where the even pattern puts it.
    pub off: f64,
}

/// An axis: a point on it and its unit direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub point: Vec3,
    pub direction: Vec3,
}

/// Faces about one axis, covering most of what the shapes cover.
#[derive(Clone, Debug, PartialEq)]
pub struct Revolve {
    pub axis: Axis,
    /// As indices into the faces.
    pub members: Vec<usize>,
    /// The share of the faces' area they cover.
    pub share: f64,
}

/// Faces repeated evenly round a turned part's axis (a wheel's spokes): `count` places, and the
/// faces at them, which the turned profile leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct Spokes {
    pub count: usize,
    pub faces: Vec<usize>,
    /// Where the spokes lie round the axis (a face of each), radians, low to high (about the axis
    /// in [`crate::in_plane`]'s directions square to it).
    pub angles: Vec<f64>,
    /// The share of the shapes' area they cover.
    pub share: f64,
}

/// A round hole to cut: where the scan has it, from its mouth to as deep as it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    /// The face it is, as an index into the faces.
    pub face: usize,
    /// On its axis at its mouth (the end further from the middle of the scan), and as far in as
    /// the scan has it.
    pub mouth: Vec3,
    pub bottom: Vec3,
    pub radius: f64,
    /// For the first of a row or a ring of holes alike: how many there are and how laid out.
    pub repeat: Option<(usize, Layout)>,
    /// A hole as a Hole feature makes it (a counterbore, a countersink, a drill point, a tap
    /// drill), for Hole It; none for a plain round hole, which Cut It cuts.
    pub form: Option<crate::hole_form::Formed>,
}

impl Hole {
    pub fn depth(&self) -> f64 {
        self.mouth.dist(self.bottom)
    }
}

/// A round along an edge of the part that no profile Reconstruct sketches holds (across a prism's
/// ends, round a hole's mouth): the face it is, its radius as fitted, and a point on the sharp
/// edge of the model it rounds, for Fillet from Scan to find that edge by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeRound {
    pub face: usize,
    pub radius: f64,
    pub edge: Vec3,
}

/// A flat chamfer along an edge of the part that no profile Reconstruct sketches holds: the face
/// it is, how far it cuts back along each of the two faces it joins, and a point on the sharp edge
/// of the model it cuts, for Chamfer It to find that edge by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeChamfer {
    pub face: usize,
    pub distance: f64,
    pub edge: Vec3,
}

/// How unequal a chamfer's two legs may be, as a share of the longer.
const CHAMFER_LEGS_EVEN: f64 = 0.15;
/// The longest leg of a chamfer and the shortest, as shares of the part's size. A flat face that
/// cuts back further is a side of the part (an octagon's), not a chamfer.
const CHAMFER_MOST: f64 = 0.08;
const CHAMFER_LEAST: f64 = 0.002;
/// How far off square to the edge a chamfer's normal may lean (the dot product).
const CHAMFER_SQUARE: f64 = 0.05;
/// The most flat faces searched for a chamfer's two neighbours (each is tried against every pair).
const CHAMFER_PLANES: usize = 120;

/// The middle of a segment's triangles, from a sample of them.
fn middle_of(scan: &Scan, segment: &Segment) -> Vec3 {
    let step = (segment.triangles.len() / 2000).max(1);
    let corners: Vec<Vec3> = segment.triangles.iter().step_by(step).filter_map(|&t| scan.corners(t)).flatten().collect();
    corners.iter().fold(Vec3::ZERO, |s, p| s + *p) / corners.len().max(1) as f64
}

/// The flat faces that are chamfers: a face whose two flat neighbours (each at least as big) meet
/// in an edge it cuts back evenly, square to that edge, not further than [`CHAMFER_MOST`] of the
/// part. `middles` are the middles of the faces' triangles; the faces in `skip` are not tried.
/// Each pair of neighbours is judged by how the chamfer's plane cuts the two faces' planes from
/// the sharp edge they would make: a leg along each, on the side of the face's middle.
fn chamfers(faces: &[Face], middles: &[Vec3], size: f64, skip: &BTreeSet<usize>) -> Vec<EdgeChamfer> {
    let planes: Vec<usize> = (0..faces.len()).filter(|&k| faces[k].area > 0.0 && plane(&faces[k]).is_some()).collect();
    if planes.len() > CHAMFER_PLANES || planes.len() < 3 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for &k in planes.iter().filter(|k| !skip.contains(k)) {
        let Some((_, nk)) = plane(&faces[k]) else { continue };
        let c = middles[k];
        let mut best: Option<(f64, EdgeChamfer)> = None;
        for (i, &a) in planes.iter().enumerate() {
            for &b in &planes[i + 1..] {
                if a == k || b == k || faces[a].area < faces[k].area || faces[b].area < faces[k].area {
                    continue;
                }
                let (Some((pa, na)), Some((pb, nb))) = (plane(&faces[a]), plane(&faces[b])) else { continue };
                let Some(e) = na.cross(nb).normalized() else { continue };
                // Two faces that make an edge, not nearly parallel; a chamfer is square to it.
                if na.cross(nb).len() < 0.2 || nk.dot(e).abs() > CHAMFER_SQUARE {
                    continue;
                }
                let rows = [[na.x, na.y, na.z], [nb.x, nb.y, nb.z], [e.x, e.y, e.z]];
                let Some(x) = crate::symmetry::solve3(&rows, [na.dot(pa), nb.dot(pb), e.dot(c)]) else { continue };
                let p0 = Vec3::new(x[0], x[1], x[2]);
                // The leg along each face: from the sharp edge, across the face towards its middle,
                // to where the chamfer's plane cuts it.
                let leg = |n: Vec3, face: usize| -> Option<(f64, Vec3)> {
                    let along = e.cross(n).normalized()?;
                    let along = if along.dot(middles[face] - p0) < 0.0 { -along } else { along };
                    let cuts = along.dot(nk);
                    (cuts.abs() > 0.1).then(|| (((c - p0).dot(nk)) / cuts, along))
                };
                let (Some((ta, ua)), Some((tb, ub))) = (leg(na, a), leg(nb, b)) else { continue };
                let (longer, shorter) = (ta.max(tb), ta.min(tb));
                if shorter < CHAMFER_LEAST * size || longer > CHAMFER_MOST * size || (longer - shorter) > CHAMFER_LEGS_EVEN * longer {
                    continue;
                }
                // The chamfer lies between the two cuts, not off to one side of them.
                let between = (p0 + ua * ta + p0 + ub * tb) * 0.5;
                if between.dist(c) > 0.6 * longer {
                    continue;
                }
                let found = EdgeChamfer { face: k, distance: 0.5 * (ta + tb), edge: p0 };
                if best.as_ref().is_none_or(|(s, _)| longer - shorter < *s) {
                    best = Some((longer - shorter, found));
                }
            }
        }
        found.extend(best.map(|(_, f)| f));
    }
    found
}

/// Where the sharp edge a round cylinder (`segment`) rounds lies: where the two flat faces it is
/// tangent to meet, level with the middle of the round. `None` with no such two faces: a round
/// joins two faces, and a cylinder with none tangent to it either side is a curved face of the
/// part in its own right (a wheel spoke's flank, 64 mm in radius, was proposed as a fillet at an
/// edge guessed √2 radii out, which no fillet could be measured at). `faces` are the reading's (a
/// face of no area is a piece of another and is passed by).
fn edge_of_round(scan: &Scan, segment: &Segment, faces: &[Face]) -> Option<Vec3> {
    let Some(Shape::Cylinder { point, axis, radius }) = segment.shape else { return None };
    let axis = axis.normalized()?;
    let step = (segment.triangles.len() / 2000).max(1);
    let corners: Vec<Vec3> = segment.triangles.iter().step_by(step).filter_map(|&t| scan.corners(t)).flatten().collect();
    let middle = corners.iter().fold(Vec3::ZERO, |s, p| s + *p) / corners.len().max(1) as f64;
    let foot = point + axis * (middle - point).dot(axis);
    let out = (middle - foot).normalized()?;
    // The flat faces along it, as far from its axis as its radius: each with the way from the axis
    // to where it touches.
    let beside: Vec<(Vec3, Vec3, Vec3)> = faces
        .iter()
        .filter(|f| f.area > 0.0)
        .filter_map(plane)
        .filter_map(|(p, n)| {
            let off = (foot - p).dot(n);
            let touch = n * -off.signum();
            (n.dot(axis).abs() <= SQUARE && (off.abs() - radius).abs() <= 0.25 * radius && touch.dot(out) >= 0.17).then_some((p, n, touch))
        })
        .collect();
    let side = |touch: Vec3| axis.dot(out.cross(touch)).signum();
    let best = |sign: f64| beside.iter().filter(|b| side(b.2) == sign).max_by(|a, b| a.2.dot(out).total_cmp(&b.2.dot(out)));
    if let (Some(a), Some(b)) = (best(1.0), best(-1.0)) {
        let rows = [[a.1.x, a.1.y, a.1.z], [b.1.x, b.1.y, b.1.z], [axis.x, axis.y, axis.z]];
        if let Some(x) = crate::symmetry::solve3(&rows, [a.1.dot(a.0), b.1.dot(b.0), axis.dot(middle)]) {
            let edge = Vec3::new(x[0], x[1], x[2]);
            if edge.dist(foot) <= 3.0 * radius {
                return Some(edge);
            }
        }
    }
    None
}

#[cfg(test)]
mod edge_of_round_tests {
    use super::*;

    /// A quarter round of radius 5 along Z between the planes x = 5 and y = 5 (tangent to both)
    /// rounds the edge where they meet; the same strip with no flat face tangent to it either side
    /// (a curved face of the part, as a wheel spoke's flank is) rounds no edge, and with only one
    /// of the two it rounds none either.
    #[test]
    fn a_round_is_an_edges_only_between_two_faces_tangent_to_it() {
        let (around, along, radius) = (12u32, 4u32, 5.0);
        let mut positions = Vec::new();
        for i in 0..=around {
            let a = std::f64::consts::FRAC_PI_2 * f64::from(i) / f64::from(around);
            for j in 0..=along {
                positions.push(Vec3::new(radius * a.cos(), radius * a.sin(), 10.0 * f64::from(j)));
            }
        }
        let at = |i: u32, j: u32| i * (along + 1) + j;
        let mut triangles = Vec::new();
        for i in 0..around {
            for j in 0..along {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let round = Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius };
        let segment = Segment { triangles: (0..triangles.len()).collect(), shape: Some(round), area: 300.0, rms: 0.0 };
        let face = |shape: Shape| Face { shape, area: 100.0 };
        let (right, top) =
            (Shape::Plane { point: Vec3::new(5.0, 0.0, 0.0), normal: Vec3::X }, Shape::Plane { point: Vec3::new(0.0, 5.0, 0.0), normal: Vec3::Y });
        let edge = edge_of_round(&scan, &segment, &[face(round), face(right), face(top)]).expect("between its two faces");
        assert!((edge.x - 5.0).abs() < 1e-9 && (edge.y - 5.0).abs() < 1e-9, "where the two faces meet: {edge:?}");
        assert_eq!(edge_of_round(&scan, &segment, &[face(round)]), None, "a curved face on its own");
        assert_eq!(edge_of_round(&scan, &segment, &[face(round), face(right)]), None, "one face is no edge");
    }
}

fn plane(face: &Face) -> Option<(Vec3, Vec3)> {
    if let Shape::Plane { point, normal } = face.shape { Some((point, normal.normalized()?)) } else { None }
}

fn unit(v: Vec3) -> Vec3 {
    v.normalized().unwrap_or(Vec3::ZERO)
}

/// The extrusions the shapes make, the one that explains most of them first. Along each
/// direction a plane faces, the part is cut into slabs at the levels where its section can change:
/// every plane facing that way or back, and where a wall ends with no such plane at it. A slab's
/// section is the walls that run right across it; slabs one after another with the same section
/// are one extrusion. Of every direction's, the one that explains the most of what the shapes cover
/// is taken first, then the one that explains most of the rest, while one still explains a share
/// worth offering: as a part is modelled, the main body first. `span` gives how far a face runs
/// along a direction, least and most; `size` is the part's. The faces `ignored` (a hole's floor,
/// which the hole's cut makes) are neither walls nor ends; the faces `unscored` (the rounds along
/// edges, which Fillet It makes where no profile holds them) are walls but explain nothing, so a
/// block with rounded top edges is extruded upright and filleted, not extruded on its side
/// because its side profile holds two of the rounds.
///
/// Walls that all lean alike from running along a direction (by up to [`MOST_DRAFT`]) are a
/// drafted prism's; `sign` turns the scan's winding out of its material, which says which way a
/// wall leans. Without `drafts`, only square walls are read, as before drafts were.
pub fn extrusions(
    faces: &[Face],
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    size: f64,
    ignored: &BTreeSet<usize>,
    unscored: &BTreeSet<usize>,
    sign: f64,
    drafts: bool,
) -> Vec<Prism> {
    let reach = WALL_START * size;
    let mut order: Vec<usize> = (0..faces.len()).filter(|&k| !ignored.contains(&k) && plane(&faces[k]).is_some()).collect();
    order.sort_by(|&a, &b| faces[b].area.total_cmp(&faces[a].area).then(a.cmp(&b)));
    let mut directions: Vec<Vec3> = Vec::new();
    for &k in &order {
        let Some((_, normal)) = plane(&faces[k]) else { continue };
        if !directions.iter().any(|d| d.dot(normal).abs() >= SAME_DIRECTION) {
            directions.push(normal);
        }
    }
    let mut candidates: Vec<(Prism, Vec<(usize, f64)>)> =
        directions.iter().flat_map(|&d| stacked(faces, d, span, reach, ignored, sign, drafts)).collect();
    let total: f64 = faces.iter().map(|f| f.area).sum();
    let mut explained = vec![0.0; faces.len()];
    let gain = |shares: &[(usize, f64)], explained: &[f64]| {
        shares.iter().filter(|(k, _)| !unscored.contains(k)).map(|&(k, s)| faces[k].area * ((explained[k] + s).min(1.0) - explained[k])).sum::<f64>()
    };
    // How many rounds along edges a candidate's walls hold: of two that explain as much, the one
    // holding fewer is the part as modelled (a block with rounded top edges stands upright, its
    // rounds filleted, rather than lying on its side with two of them in its profile).
    let held = |shares: &[(usize, f64)]| shares.iter().filter(|(k, _)| unscored.contains(k)).count();
    let mut chosen = Vec::new();
    loop {
        let best = candidates.iter().enumerate().map(|(i, c)| (i, gain(&c.1, &explained), held(&c.1))).fold(
            None,
            |best: Option<(usize, f64, usize)>, (i, g, h)| match best {
                Some((_, b, bh)) if b >= g + TIE * total || (b + TIE * total > g && (bh < h || (bh == h && b >= g))) => best,
                _ => Some((i, g, h)),
            },
        );
        let Some((i, _, _)) = best.filter(|b| b.1 >= LEAST_NEW * total) else { break };
        let (prism, shares) = candidates.remove(i);
        for (k, s) in shares {
            explained[k] = (explained[k] + s).min(1.0);
        }
        chosen.push(prism);
    }
    chosen
}

/// `prisms` made to meet exactly where they meet on the scan: those read along nearly one
/// direction (within [`SAME_DIRECTION`]) along exactly one (the first's, on the grid's axis when
/// it is that near one), and their ends at nearly one level (within the scan's `tolerance`) at
/// exactly one. Read each on its own, prisms stacked on a floor leave it a few thousandths of a
/// millimetre and a few millionths of a radian apart in each, which the kernel's join cannot merge
/// where two or three such meet at a corner.
fn shared(mut prisms: Vec<Prism>, faces: &[Face], tolerance: f64) -> Vec<Prism> {
    // Each prism's ends as levels along its own direction, read off its end planes where it has
    // them (a plane's middle), else where it was read.
    let level_of_end = |k: usize, d: Vec3, fallback: f64| plane(&faces[k]).map_or(fallback, |(point, _)| point.dot(d));
    let mut groups: Vec<(Vec3, Vec<usize>)> = Vec::new();
    for (k, prism) in prisms.iter().enumerate() {
        let d = prism.direction;
        match groups.iter_mut().find(|g| g.0.dot(d).abs() >= SAME_DIRECTION) {
            Some(g) => g.1.push(k),
            None => groups.push((on_grid(unit(d)), vec![k])),
        }
    }
    for (d, members) in groups {
        if d == Vec3::ZERO {
            continue;
        }
        // Each member's ends, as levels along `d`.
        let mut ends: Vec<(usize, f64, f64)> = Vec::new();
        for &k in &members {
            let p = &prisms[k];
            let sign = if p.direction.dot(d) < 0.0 { -1.0 } else { 1.0 };
            let along = d * sign;
            let base = p.origin.dot(p.direction);
            let (low, high) = (base + p.low, base + p.high);
            // Which end plane is at which end: an open prism names its one plane twice.
            let at_low =
                |e: usize| plane(&faces[e]).is_none_or(|(point, _)| (point.dot(p.direction) - low).abs() <= (point.dot(p.direction) - high).abs());
            let low_new =
                if !p.open || at_low(p.ends[0]) { level_of_end(p.ends[0], along, p.origin.dot(along) + p.low) } else { p.origin.dot(along) + p.low };
            let high_new = if !p.open || !at_low(p.ends[1]) {
                level_of_end(p.ends[1], along, p.origin.dot(along) + p.high)
            } else {
                p.origin.dot(along) + p.high
            };
            // As levels along `d`.
            ends.push((k, low_new * sign, high_new * sign));
        }
        // Levels within the tolerance of each other are one: their mean.
        let mut levels: Vec<f64> = ends.iter().flat_map(|e| [e.1, e.2]).collect();
        levels.sort_by(f64::total_cmp);
        let mut clusters: Vec<(f64, usize)> = Vec::new();
        for l in levels {
            match clusters.last_mut() {
                Some((sum, n)) if l - *sum / *n as f64 <= tolerance => {
                    *sum += l;
                    *n += 1;
                }
                _ => clusters.push((l, 1)),
            }
        }
        let snap = |l: f64| clusters.iter().map(|(sum, n)| sum / *n as f64).find(|m| (m - l).abs() <= tolerance).unwrap_or(l);
        for (k, low, high) in ends {
            let p = &mut prisms[k];
            let sign = if p.direction.dot(d) < 0.0 { -1.0 } else { 1.0 };
            let along = d * sign;
            let (low, high) = (snap(low) * sign, snap(high) * sign);
            let base = p.origin.dot(along);
            if high - low > 0.0 {
                p.direction = along;
                p.low = low - base;
                p.high = high - base;
            }
        }
    }
    prisms
}

/// Two prisms explaining within this share of the whole scan's area explain as much
/// ([`extrusions`] then takes the one whose walls hold fewer rounds).
const TIE: f64 = 0.001;

/// A direction this near a world axis is along it (a sine: about 0.1°).
const ON_GRID: f64 = 0.00175;

/// `direction` (unit) as the part was placed: the world axis it lies along when within
/// [`ON_GRID`] of one (a scan stood on its base or squared to the grid leaves its faces a few
/// thousandths of a degree off the axes, which a model built along them should not keep), else
/// as it is.
pub fn on_grid(direction: Vec3) -> Vec3 {
    [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .map(|axis| if direction.dot(axis) < 0.0 { -axis } else { axis })
        .find(|axis| direction.cross(*axis).len() <= ON_GRID)
        .unwrap_or(direction)
}

/// A level along a direction where the section may change, and the planes facing that way or back
/// at it.
struct Level {
    at: f64,
    caps: Vec<usize>,
    /// The wall ends read at it while no plane lies at it (it lies at their mean), and their least
    /// and most.
    free: (f64, usize),
    reach: (f64, f64),
}

/// The extrusions along `direction`, low to high, each with what it explains: (face, share).
fn stacked(
    faces: &[Face],
    direction: Vec3,
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    reach: f64,
    ignored: &BTreeSet<usize>,
    sign: f64,
    drafts: bool,
) -> Vec<(Prism, Vec<(usize, f64)>)> {
    let noise = SAME_LEVEL * reach;
    let facing = |k: usize| plane(&faces[k]).map(|(point, normal)| (point.dot(direction), normal.dot(direction)));
    let kept = || (0..faces.len()).filter(|k| !ignored.contains(k));
    let caps: Vec<(usize, f64)> = kept().filter_map(|k| facing(k).filter(|f| f.1.abs() >= SAME_DIRECTION).map(|f| (k, f.0))).collect();
    let walls: Vec<(usize, f64, f64)> =
        kept().filter(|&k| lean(&faces[k], direction, sign).is_some()).filter_map(|k| span(k, direction).map(|(a, b)| (k, a, b))).collect();
    // The planes, and where a wall ends with no plane just outside its end.
    let mut ends: Vec<(f64, Option<usize>)> = caps.iter().map(|&(k, at)| (at, Some(k))).collect();
    for &(_, a, b) in &walls {
        if !caps.iter().any(|c| c.1 <= a + noise && c.1 >= a - reach) {
            ends.push((a, None));
        }
        if !caps.iter().any(|c| c.1 >= b - noise && c.1 <= b + reach) {
            ends.push((b, None));
        }
    }
    ends.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut levels: Vec<Level> = Vec::new();
    for (at, cap) in ends {
        if let Some(last) = levels.last_mut() {
            let close = if cap.is_some() && !last.caps.is_empty() { noise } else { reach };
            if at - last.at <= close {
                match cap {
                    Some(k) => {
                        if last.caps.is_empty() {
                            last.at = at;
                        }
                        last.caps.push(k);
                    }
                    None if last.caps.is_empty() => {
                        last.free = (last.free.0 + at, last.free.1 + 1);
                        last.at = last.free.0 / last.free.1 as f64;
                        last.reach = (last.reach.0.min(at), last.reach.1.max(at));
                    }
                    None => {}
                }
                continue;
            }
        }
        levels.push(match cap {
            Some(k) => Level { at, caps: vec![k], free: (0.0, 0), reach: (at, at) },
            None => Level { at, caps: Vec::new(), free: (at, 1), reach: (at, at) },
        });
    }
    let nearest = |x: f64| levels.iter().map(|l| l.at).min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs())).unwrap_or(x);
    let low_end = |a: f64| {
        levels
            .iter()
            .map(|l| l.at)
            .filter(|&l| l <= a + noise && l >= a - reach)
            .fold(None, |m: Option<f64>, l| Some(m.map_or(l, |m| m.max(l))))
            .unwrap_or_else(|| nearest(a))
    };
    let high_end = |b: f64| {
        levels
            .iter()
            .map(|l| l.at)
            .filter(|&l| l >= b - noise && l <= b + reach)
            .fold(None, |m: Option<f64>, l| Some(m.map_or(l, |m| m.min(l))))
            .unwrap_or_else(|| nearest(b))
    };
    let runs_of: Vec<(usize, f64, f64)> = walls.iter().map(|&(k, a, b)| (k, low_end(a), high_end(b))).collect();
    // Each slab's section, the walls right across it, where they are enough to bound anything.
    let sections: Vec<Option<(Vec<usize>, f64)>> = levels
        .windows(2)
        .map(|pair| {
            let (low, high) = (pair[0].at, pair[1].at);
            let across: Vec<usize> = runs_of.iter().filter(|w| w.1 <= low && w.2 >= high).map(|w| w.0).collect();
            let (across, draft) = section_of(faces, &across, direction, sign, drafts);
            let cylinders = across.iter().any(|&k| matches!(faces[k].shape, Shape::Cylinder { .. }));
            (across.len() >= LEAST_WALLS || cylinders).then_some((across, draft))
        })
        .collect();
    let mut found = Vec::new();
    let mut start = 0;
    while start < sections.len() {
        let Some((section, draft)) = &sections[start] else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < sections.len() && sections[end].as_ref().is_some_and(|(w, d)| w == section && d == draft) {
            end += 1;
        }
        let goes_on = (start > 0 && sections[start - 1].is_some(), end < sections.len() && sections[end].is_some());
        if let Some(extrusion) = extrusion(faces, direction, (&levels[start], &levels[end]), goes_on, (section, *draft), &runs_of) {
            found.push(extrusion);
        }
        start = end;
    }
    found
}

/// The extrusion from level `low` to `high` with the walls `section`, and what it explains, if it
/// is one: at each end a plane facing out of it, more of the part beyond it (`goes_on`), or no
/// plane at all where its walls stop (a domed top), not both of those last. A plane at an end
/// facing into it, with nothing beyond, is the floor of a slot.
fn extrusion(
    faces: &[Face],
    direction: Vec3,
    (low, high): (&Level, &Level),
    goes_on: (bool, bool),
    (section, draft): (&[usize], f64),
    runs: &[(usize, f64, f64)],
) -> Option<(Prism, Vec<(usize, f64)>)> {
    let out = |level: &Level, up: bool| -> Vec<usize> {
        level.caps.iter().copied().filter(|&k| plane(&faces[k]).is_some_and(|(_, n)| (n.dot(direction) > 0.0) == up)).collect()
    };
    let (below, above) = (out(low, false), out(high, true));
    let (bounded_below, bounded_above) = (!below.is_empty() || goes_on.0, !above.is_empty() || goes_on.1);
    let (closed_below, closed_above) = (bounded_below || low.caps.is_empty(), bounded_above || high.caps.is_empty());
    if !(closed_below && closed_above && (bounded_below || bounded_above)) {
        return None;
    }
    let largest = |list: &[usize]| list.iter().copied().max_by(|&a, &b| faces[a].area.total_cmp(&faces[b].area).then(b.cmp(&a)));
    let (first, second) = (largest(&below).or_else(|| largest(&low.caps)), largest(&above).or_else(|| largest(&high.caps)));
    let ends = match (first, second) {
        (Some(a), Some(b)) => [a, b],
        (Some(a), None) => [a, a],
        (None, Some(b)) => [b, b],
        (None, None) => return None,
    };
    let (origin, _) = plane(&faces[ends[0]])?;
    let at = origin.dot(direction);
    let mut shares: Vec<(usize, f64)> = below.iter().chain(&above).map(|&k| (k, 1.0)).collect();
    for &(k, least, most) in runs.iter().filter(|r| section.contains(&r.0)) {
        let along = (most.min(high.at) - least.max(low.at)).max(0.0);
        shares.push((k, if most > least { (along / (most - least)).min(1.0) } else { 1.0 }));
    }
    let open = first.is_none() || second.is_none();
    // An end with no plane is as far as the shortest wall runs, so a slice halfway crosses them all.
    let (from, to) = (if low.caps.is_empty() { low.reach.1 } else { low.at }, if high.caps.is_empty() { high.reach.0 } else { high.at });
    // A drafted prism lies along the grid's axis when it is that near it: the kernel shells a
    // tapered body reliably only so. A square one is put on one direction with the prisms it
    // stacks with once all are read ([`shared`]).
    let direction = if draft != 0.0 { on_grid(direction) } else { direction };
    Some((Prism { direction, origin, low: from - at, high: to - at, ends, walls: section.to_vec(), open, draft }, shares))
}

/// How far a face leans from running along `direction`, when it is near enough to be a wall: for
/// a plane, `Some` of the angle its outward normal (the winding times `sign`) turns from square
/// toward `direction` (positive where it faces along `direction`: the part narrows that way); for
/// a cylinder or a cone of little taper about an axis near `direction`, `Some(None)` (a round
/// follows the walls beside it).
fn lean(face: &Face, direction: Vec3, sign: f64) -> Option<Option<f64>> {
    match face.shape {
        Shape::Plane { normal, .. } => {
            let s = (unit(normal) * sign).dot(direction);
            (s.abs() <= MOST_DRAFT).then(|| Some(s.clamp(-1.0, 1.0).asin()))
        }
        Shape::Cylinder { axis, .. } => (unit(axis).dot(direction).abs() >= MOST_DRAFT_COS).then_some(None),
        Shape::Cone { axis, half_angle, .. } => {
            (unit(axis).dot(direction).abs() >= MOST_DRAFT_COS && half_angle.abs() <= MOST_DRAFT.asin()).then_some(None)
        }
        _ => None,
    }
}

/// The walls `across` a slab as a prism takes them, and its draft: when the planes among them lean
/// alike (each within [`SAME_DRAFT`] of their mean by area, the mean at least [`LEAST_DRAFT`]),
/// all of them with the rounds between them, leaning by that mean; else only the walls along
/// `direction` ([`is_wall`]), square. A shelled part's outside and inside lean opposite ways, so
/// its walls together are square or nothing, as before drafts were read.
fn section_of(faces: &[Face], across: &[usize], direction: Vec3, sign: f64, drafts: bool) -> (Vec<usize>, f64) {
    if !drafts {
        return (across.iter().copied().filter(|&k| is_wall(&faces[k], direction)).collect(), 0.0);
    }
    let leans: Vec<(usize, f64)> = across.iter().filter_map(|&k| lean(&faces[k], direction, sign).flatten().map(|l| (k, l))).collect();
    let weight: f64 = leans.iter().map(|&(k, _)| faces[k].area).sum();
    if weight > 0.0 {
        let mean = leans.iter().map(|&(k, l)| faces[k].area * l).sum::<f64>() / weight;
        if mean.abs() >= LEAST_DRAFT && leans.iter().all(|&(_, l)| (l - mean).abs() <= SAME_DRAFT) {
            return (across.to_vec(), mean);
        }
    }
    (across.iter().copied().filter(|&k| is_wall(&faces[k], direction)).collect(), 0.0)
}

/// Whether a face runs along `direction`: a plane square to the ends, or a cylinder along it.
fn is_wall(face: &Face, direction: Vec3) -> bool {
    match face.shape {
        Shape::Plane { normal, .. } => unit(normal).dot(direction).abs() <= SQUARE,
        Shape::Cylinder { axis, .. } => unit(axis).dot(direction).abs() >= SAME_DIRECTION,
        _ => false,
    }
}

/// Whether `face` lies about `axis`, and whether it is round there: a cylinder, cone or torus on
/// it, or a sphere centred on it; a plane square to it counts and is not round.
fn about(face: &Face, axis: &Axis, size: f64) -> Option<bool> {
    let square = |d: Vec3| unit(d).dot(axis.direction).abs() >= SAME_DIRECTION;
    let on = |p: Vec3| {
        let v = p - axis.point;
        (v - axis.direction * v.dot(axis.direction)).len() <= ON_THE_AXIS * size
    };
    match face.shape {
        Shape::Plane { normal, .. } => square(normal).then_some(false),
        Shape::Cylinder { point, axis, .. } => (square(axis) && on(point)).then_some(true),
        Shape::Cone { apex, axis, .. } => (square(axis) && on(apex)).then_some(true),
        Shape::Torus { center, axis, .. } => (square(axis) && on(center)).then_some(true),
        Shape::Sphere { center, .. } => on(center).then_some(true),
    }
}

/// The axis the round shapes share (each weighted by its area): of the cylinders, cones and tori,
/// the one whose axis most of them lie along (within 5° and a twentieth of `size`), refined over
/// those: its direction their weighted mean (with the planes square to it), its point the one
/// nearest all their axes. `None` without a round.
pub fn turning_axis(shapes: &[(Shape, f64)], size: f64) -> Option<Axis> {
    let lines: Vec<(Vec3, Vec3, f64)> = shapes.iter().filter_map(|&(shape, weight)| shape.axis().map(|(p, d)| (p, unit(d), weight))).collect();
    if lines.is_empty() {
        return None;
    }
    let parallel = 5f64.to_radians().cos();
    let distance = |a: (Vec3, Vec3, f64), b: (Vec3, Vec3, f64)| {
        let v = b.0 - a.0;
        (v - a.1 * v.dot(a.1)).len()
    };
    let members = |seed: usize| -> Vec<usize> {
        (0..lines.len()).filter(|&k| lines[k].1.dot(lines[seed].1).abs() >= parallel && distance(lines[seed], lines[k]) <= size / 20.0).collect()
    };
    let weights: Vec<f64> = (0..lines.len()).map(|s| members(s).iter().map(|&k| lines[k].2).sum()).collect();
    let seed = (0..lines.len()).fold(0, |best, k| if weights[k] > weights[best] { k } else { best });
    let chosen = members(seed);
    let reference = lines[seed].1;
    let agree = |d: Vec3| if d.dot(reference) >= 0.0 { d } else { -d };
    let mut total = Vec3::ZERO;
    for &k in &chosen {
        total += agree(lines[k].1) * lines[k].2;
    }
    for &(shape, weight) in shapes {
        let Shape::Plane { normal, .. } = shape else { continue };
        let n = unit(normal);
        if n.dot(reference).abs() >= parallel {
            total += agree(n) * weight;
        }
    }
    let direction = total.normalized()?;
    // The point nearest all the members' axes: Σ w (I − d dᵀ) x = Σ w (I − d dᵀ) p, with a little
    // of the mean point along the axis, which the lines leave free.
    let (mut matrix, mut right, mut mean, mut weight) = ([[0.0f64; 3]; 3], Vec3::ZERO, Vec3::ZERO, 0.0);
    let d = [direction.x, direction.y, direction.z];
    for &k in &chosen {
        let (p, w) = (lines[k].0, lines[k].2);
        let along = p.dot(direction);
        right += (p - direction * along) * w;
        mean += p * w;
        weight += w;
        for (i, row) in matrix.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += w * (f64::from(u8::from(i == j)) - d[i] * d[j]);
            }
        }
    }
    let mean = mean / weight.max(1e-300);
    let hold = 1e-6 * weight;
    for (i, row) in matrix.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell += hold * d[i] * d[j];
        }
    }
    right += direction * (hold * direction.dot(mean));
    let point = crate::symmetry::solve3(&matrix, [right.x, right.y, right.z]).map_or(lines[seed].0, |x| Vec3::new(x[0], x[1], x[2]));
    Some(Axis { point, direction })
}

/// The part as turned, if most of what the shapes cover lies about one axis. `size` is the part's.
pub fn revolve(faces: &[Face], size: f64) -> Option<Revolve> {
    round_axis(faces, size).filter(|r| r.share >= MOSTLY_TURNED)
}

/// The axis round shapes share ([`LEAST_ROUND`] of them or more), with the faces about it and
/// their share, however little of the part it is: a wheel's rim and hub turn about it while its
/// flat spokes do not, so it is no turned part [`revolve`] reads, but the axis is what a person
/// rebuilding it by hand needs first.
pub fn round_axis(faces: &[Face], size: f64) -> Option<Revolve> {
    let weighted: Vec<(Shape, f64)> = faces.iter().map(|f| (f.shape, f.area)).collect();
    let axis = turning_axis(&weighted, size)?;
    // A piece of another face's surface (no area of its own) is read with that face.
    let on: Vec<(usize, bool)> =
        faces.iter().enumerate().filter(|(_, f)| f.area > 0.0).filter_map(|(k, f)| about(f, &axis, size).map(|round| (k, round))).collect();
    let total: f64 = faces.iter().map(|f| f.area).sum();
    let members: Vec<usize> = on.iter().map(|m| m.0).collect();
    let share = members.iter().map(|&k| faces[k].area).sum::<f64>() / total.max(1e-300);
    (on.iter().filter(|m| m.1).count() >= LEAST_ROUND).then_some(Revolve { axis, members, share })
}

/// The faces of `faces` not in `members` that repeat evenly round `axis` (a wheel's spokes, a
/// hub's bosses): grouped by kind, size, tilt to the axis and distance from it, each group's
/// places round the axis evenly spaced. The count most of their area repeats at (a group of twice
/// as many, a spoke's two mirrored sides, counts too). `None` with fewer than three places.
pub fn spokes_about(faces: &[Face], axis: &Axis, members: &[usize]) -> Option<Spokes> {
    let (across, other) = crate::in_plane(axis.direction);
    struct Seen {
        k: usize,
        kind: Kind,
        size: f64,
        tilt: f64,
        reach: f64,
        angle: f64,
    }
    let seen: Vec<Seen> = faces
        .iter()
        .enumerate()
        .filter(|(k, f)| f.area > 0.0 && !members.contains(k))
        .map(|(k, f)| {
            let (kind, size, direction, place) = match f.shape {
                Shape::Plane { point, normal } => (Kind::Plane, f.area, Some(normal), point),
                Shape::Cylinder { point, axis, radius } => (Kind::Cylinder, radius, Some(axis), point),
                Shape::Sphere { center, radius } => (Kind::Sphere, radius, None, center),
                Shape::Cone { apex, axis, half_angle } => (Kind::Cone, half_angle, Some(axis), apex),
                Shape::Torus { center, axis, minor, .. } => (Kind::Torus, minor, Some(axis), center),
            };
            let d = place - axis.point;
            let off = d - axis.direction * d.dot(axis.direction);
            let tilt = direction.and_then(|v| v.normalized()).map_or(0.0, |v| v.dot(axis.direction).abs());
            Seen { k, kind, size, tilt, reach: off.len(), angle: off.dot(other).atan2(off.dot(across)).rem_euclid(TAU) }
        })
        .collect();
    let total: f64 = faces.iter().map(|f| f.area).sum();
    let mut used = vec![false; seen.len()];
    // (count, faces, area) of each even group.
    let mut groups: Vec<(usize, Vec<usize>, f64)> = Vec::new();
    for i in 0..seen.len() {
        if used[i] {
            continue;
        }
        let a = &seen[i];
        let group: Vec<usize> = (i..seen.len())
            .filter(|&j| {
                let b = &seen[j];
                !used[j]
                    && a.kind == b.kind
                    && (a.size - b.size).abs() <= SPOKE_SIZE * a.size.max(b.size)
                    && (a.tilt - b.tilt).abs() <= SPOKE_TILT
                    && (a.reach - b.reach).abs() <= SPOKE_REACH * a.reach.max(b.reach).max(1e-9)
            })
            .collect();
        if group.len() < 3 || a.reach <= 0.0 {
            continue;
        }
        let mut angles: Vec<f64> = group.iter().map(|&j| seen[j].angle).collect();
        angles.sort_by(f64::total_cmp);
        let step = TAU / angles.len() as f64;
        let even = (0..angles.len()).all(|n| {
            let gap = if n + 1 < angles.len() { angles[n + 1] - angles[n] } else { angles[0] + TAU - angles[n] };
            (gap - step).abs() <= SPOKE_EVEN * step
        });
        if even {
            for &j in &group {
                used[j] = true;
            }
            groups.push((group.len(), group.iter().map(|&j| seen[j].k).collect(), group.iter().map(|&j| faces[seen[j].k].area).sum()));
        }
    }
    // The count most area repeats at; a group of twice as many counts towards it.
    let count = groups.iter().map(|g| g.0).filter(|&n| n >= 3).max_by(|&x, &y| {
        let area = |n: usize| groups.iter().filter(|g| g.0 == n || g.0 == 2 * n).map(|g| g.2).sum::<f64>();
        area(x).total_cmp(&area(y)).then(y.cmp(&x))
    })?;
    let mut chosen: Vec<usize> = groups.iter().filter(|g| g.0 == count || g.0 == 2 * count).flat_map(|g| g.1.iter().copied()).collect();
    chosen.sort_unstable();
    let area: f64 = chosen.iter().map(|&k| faces[k].area).sum();
    // Where the spokes are: one face of each, the largest group with one per spoke (a group of
    // twice as many has a spoke's two flanks, and halfway between those is inside the spoke).
    let one_each = groups.iter().filter(|g| g.0 == count).max_by(|a, b| a.2.total_cmp(&b.2))?;
    let mut angles: Vec<f64> = seen.iter().filter(|f| one_each.1.contains(&f.k)).map(|f| f.angle).collect();
    angles.sort_by(f64::total_cmp);
    Some(Spokes { count, faces: chosen, angles, share: area / total.max(1e-300) })
}

/// What a shape is, for telling alike from unlike: its kind, its size, the direction it holds.
struct Likeness {
    kind: Kind,
    size: f64,
    direction: Option<Vec3>,
    place: Vec3,
}

fn likeness(face: &Face) -> Option<Likeness> {
    // A piece of another face's surface is no shape of its own to repeat.
    if !(face.area > 0.0) {
        return None;
    }
    match face.shape {
        Shape::Cylinder { point, axis, radius } => Some(Likeness { kind: Kind::Cylinder, size: radius, direction: Some(unit(axis)), place: point }),
        Shape::Sphere { center, radius } => Some(Likeness { kind: Kind::Sphere, size: radius, direction: None, place: center }),
        Shape::Plane { point, normal } => Some(Likeness { kind: Kind::Plane, size: face.area, direction: Some(unit(normal)), place: point }),
        _ => None,
    }
}

fn alike(a: &Likeness, b: &Likeness) -> bool {
    let share = if a.kind == Kind::Plane { SAME_AREA } else { SAME_RADIUS };
    a.kind == b.kind
        && (a.size - b.size).abs() <= share * a.size.max(b.size)
        && match (a.direction, b.direction) {
            (Some(x), Some(y)) => x.dot(y).abs() >= SAME_DIRECTION,
            _ => true,
        }
}

/// Where a shape lies among those alike: a cylinder by where its axis crosses a plane square to
/// it, a plane by its level along its normal.
fn place_of(like: &Likeness) -> Vec3 {
    let direction = like.direction.unwrap_or(Vec3::Z);
    match like.kind {
        Kind::Cylinder => like.place - direction * like.place.dot(direction),
        Kind::Plane => direction * like.place.dot(direction).abs(),
        _ => like.place,
    }
}

/// Whether `other` stands on the axis `round` (a point and a unit direction), within a share of
/// `radius`.
fn on_axis_of(round: (Vec3, Vec3), other: &Shape, radius: f64) -> bool {
    other.axis().is_some_and(|(point, direction)| {
        let v = point - round.0;
        unit(direction).dot(round.1).abs() >= SAME_DIRECTION && (v - round.1 * v.dot(round.1)).len() <= SAME_AXIS * radius
    })
}

/// The repeats of cylinders a solid can be built with as a pattern: none of a place's faces is a
/// round of an edge (`rounds`). Where each place has the same kinds of round on its axis, a
/// counterbore or countersink goes with its hole; where they differ, only what repeats is planned.
pub fn pattern_plans(faces: &[Face], repeats: &[Repeat], rounds: &BTreeSet<usize>) -> Vec<PatternPlan> {
    let mut taken = vec![false; faces.len()];
    let mut plans = Vec::new();
    for repeat in repeats.iter().filter(|r| r.kind == Kind::Cylinder) {
        let place = |member: usize| -> Vec<usize> {
            let Shape::Cylinder { radius, .. } = faces[member].shape else { return Vec::new() };
            let Some((p, d)) = faces[member].shape.axis() else { return Vec::new() };
            (0..faces.len()).filter(|&k| on_axis_of((p, unit(d)), &faces[k].shape, radius)).collect()
        };
        let mut places: Vec<Vec<usize>> = repeat.members.iter().map(|&m| place(m)).collect();
        let kinds = |list: &[usize]| {
            let mut kinds: Vec<u8> = list.iter().map(|&k| faces[k].shape.kind() as u8).collect();
            kinds.sort_unstable();
            kinds
        };
        if places.iter().any(|p| kinds(p) != kinds(&places[0])) {
            places = repeat.members.iter().map(|&m| vec![m]).collect();
        }
        let Some(first) = places.first().cloned() else { continue };
        if places.iter().flatten().any(|k| rounds.contains(k) || taken[*k]) {
            continue;
        }
        for &k in places.iter().flatten() {
            taken[k] = true;
        }
        let rest = places[1..].concat();
        let off = off_the_layout(faces, repeat);
        plans.push(PatternPlan { repeat: repeat.clone(), first, rest, off });
    }
    plans
}

/// `p` turned `angle` radians about the unit `axis` through the origin.
fn turned(p: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let (s, c) = angle.sin_cos();
    p * c + axis.cross(p) * s + axis * (axis.dot(p) * (1.0 - c))
}

/// How far the worst of a repeat's places lies from the nearest place of its even layout.
fn off_the_layout(faces: &[Face], repeat: &Repeat) -> f64 {
    let likes: Vec<Likeness> = repeat.members.iter().filter_map(|&m| likeness(&faces[m])).collect();
    let radial = likes.iter().any(|l| l.direction.zip(likes[0].direction).is_some_and(|(a, b)| a.dot(b).abs() < SAME_DIRECTION));
    let places: Vec<Vec3> = match repeat.layout {
        Layout::Circular { centre, .. } if radial => likes.iter().filter_map(|l| ring_place(l, centre)).collect(),
        _ => likes.iter().map(place_of).collect(),
    };
    let Some(&start) = places.first() else { return 0.0 };
    let even: Vec<Vec3> = match repeat.layout {
        Layout::Linear { step } => (0..places.len()).map(|k| start + step * k as f64).collect(),
        Layout::Circular { centre, axis, angle } => (0..places.len()).map(|k| centre + turned(start - centre, axis, angle * k as f64)).collect(),
        Layout::Grid { step, across, columns, rows } => {
            (0..rows).flat_map(|row| (0..columns).map(move |column| start + step * column as f64 + across * row as f64)).collect()
        }
    };
    places.iter().map(|p| even.iter().map(|e| p.dist(*e)).fold(f64::INFINITY, f64::min)).fold(0.0, f64::max)
}

/// The repeats among the faces, those of the most members first.
pub fn repeats(faces: &[Face]) -> Vec<Repeat> {
    let likes: Vec<Option<Likeness>> = faces.iter().map(likeness).collect();
    let mut grouped = vec![false; faces.len()];
    let mut found: Vec<Repeat> = Vec::new();
    for first in 0..faces.len() {
        let Some(like) = &likes[first] else { continue };
        if grouped[first] {
            continue;
        }
        let members: Vec<usize> =
            (first..faces.len()).filter(|&k| !grouped[k] && likes[k].as_ref().is_some_and(|other| alike(like, other))).collect();
        if members.len() < LEAST_REPEATS {
            continue;
        }
        let places: Vec<Vec3> = members.iter().filter_map(|&k| likes[k].as_ref().map(place_of)).collect();
        let normal = like.direction.filter(|_| like.kind != Kind::Plane);
        let laid_out = in_a_line(&members, &places, like.kind)
            .or_else(|| round_a_circle(&members, &places, like.kind, normal))
            .or_else(|| in_a_grid(&members, &places, like.kind));
        if let Some(repeat) = laid_out {
            for &k in &repeat.members {
                grouped[k] = true;
            }
            found.push(repeat);
        }
    }
    // What is left of the cylinders of one radius may be a ring drilled along the radius.
    for first in 0..faces.len() {
        let Some(like) = likes[first].as_ref().filter(|l| l.kind == Kind::Cylinder && !grouped[first]) else { continue };
        let members: Vec<usize> = (first..faces.len())
            .filter(|&k| {
                !grouped[k]
                    && likes[k]
                        .as_ref()
                        .is_some_and(|o| o.kind == Kind::Cylinder && (o.size - like.size).abs() <= SAME_RADIUS * like.size.max(o.size))
            })
            .collect();
        if let Some(repeat) = radial_ring(&members, &likes) {
            for &k in &repeat.members {
                grouped[k] = true;
            }
            found.push(repeat);
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.members.len()));
    found
}

/// Where cylinder `like` lies in a ring drilled along the radius about `centre`: a unit along its
/// axis from the centre, the way out to where it was fitted.
fn ring_place(like: &Likeness, centre: Vec3) -> Option<Vec3> {
    let direction = like.direction?;
    let out = (like.place - centre).dot(direction);
    (out.abs() > SAME_AXIS * like.size).then(|| centre + direction * out.signum())
}

/// The ring among `members` (cylinders of one radius) drilled along the radius about one axis: at
/// least [`LEAST_REPEATS`] whose axes are square to the axis two of them make and pass within
/// [`RADIAL_CROSSING`] of their radius of the point they cross, evenly all the way round.
fn radial_ring(members: &[usize], likes: &[Option<Likeness>]) -> Option<Repeat> {
    let line = |k: usize| likes[k].as_ref().and_then(|l| Some((l.place, l.direction?, l.size)));
    for (i, &a) in members.iter().enumerate() {
        let (p, d, radius) = line(a)?;
        for &b in &members[i + 1..] {
            let Some((q, e, _)) = line(b) else { continue };
            let axis = d.cross(e);
            if axis.len() < (1.0 - SAME_DIRECTION * SAME_DIRECTION).sqrt() {
                continue;
            }
            let axis = unit(axis);
            let w = p - q;
            let (dd, de, ee, dw, ew) = (d.dot(d), d.dot(e), e.dot(e), d.dot(w), e.dot(w));
            let across = dd * ee - de * de;
            let (s, t) = ((de * ew - ee * dw) / across, (dd * ew - de * dw) / across);
            let (on_a, on_b) = (p + d * s, q + e * t);
            if on_a.dist(on_b) > RADIAL_CROSSING * radius {
                continue;
            }
            let centre = (on_a + on_b) * 0.5;
            let off_line = |k: usize| {
                let Some((r, f, _)) = line(k) else { return f64::INFINITY };
                let v = centre - r;
                (v - f * v.dot(f)).len()
            };
            let ring: Vec<usize> = members
                .iter()
                .copied()
                .filter(|&k| line(k).is_some_and(|(_, f, _)| f.dot(axis).abs() <= SQUARE) && off_line(k) <= RADIAL_CROSSING * radius)
                .collect();
            if ring.len() < LEAST_REPEATS {
                continue;
            }
            let Some(places) = ring.iter().map(|&k| likes[k].as_ref().and_then(|l| ring_place(l, centre))).collect::<Option<Vec<Vec3>>>() else {
                continue;
            };
            if let Some(repeat) = round_a_circle(&ring, &places, Kind::Cylinder, Some(axis)) {
                return Some(repeat);
            }
        }
    }
    None
}

/// The members, if they lie along a line evenly spaced.
fn in_a_line(members: &[usize], places: &[Vec3], kind: Kind) -> Option<Repeat> {
    let mut far = (0, 0, 0.0);
    for a in 0..places.len() {
        for b in a + 1..places.len() {
            let d = places[a].dist(places[b]);
            if d > far.2 {
                far = (a, b, d);
            }
        }
    }
    if !(far.2 > 0.0) {
        return None;
    }
    let along = (places[far.1] - places[far.0]) / far.2;
    let mut order: Vec<(f64, usize)> = places.iter().enumerate().map(|(k, p)| ((*p - places[far.0]).dot(along), k)).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    let step = far.2 / (places.len() - 1) as f64;
    for (index, (at, k)) in order.iter().enumerate() {
        let off = (places[*k] - places[far.0] - along * *at).len();
        if (*at - step * index as f64).abs() > EVEN * step || off > EVEN * step {
            return None;
        }
    }
    Some(Repeat { kind, members: order.iter().map(|o| members[o.1]).collect(), layout: Layout::Linear { step: along * step } })
}

/// The members, if they lie round a circle at even angles. `axis` is the direction they hold,
/// when they hold one; otherwise it is worked out.
fn round_a_circle(members: &[usize], places: &[Vec3], kind: Kind, axis: Option<Vec3>) -> Option<Repeat> {
    let middle = places.iter().fold(Vec3::ZERO, |s, p| s + *p) / places.len() as f64;
    let axis = match axis {
        Some(axis) => axis,
        None => {
            let a = places[0] - middle;
            unit(places.iter().map(|p| a.cross(*p - middle)).max_by(|x, y| x.len2().total_cmp(&y.len2()))?)
        }
    };
    if axis == Vec3::ZERO {
        return None;
    }
    let (across, other) = in_plane(axis);
    let mut around: Vec<(f64, usize, f64)> = places
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let d = *p - middle;
            let (x, y) = (d.dot(across), d.dot(other));
            (y.atan2(x).rem_euclid(TAU), k, x.hypot(y))
        })
        .collect();
    let radius = around.iter().map(|a| a.2).sum::<f64>() / around.len() as f64;
    if !(radius > 0.0) || around.iter().any(|a| (a.2 - radius).abs() > EVEN * radius) {
        return None;
    }
    around.sort_by(|a, b| a.0.total_cmp(&b.0));
    let angle = TAU / around.len() as f64;
    for pair in around.windows(2) {
        if ((pair[1].0 - pair[0].0) - angle).abs() > EVEN * angle {
            return None;
        }
    }
    Some(Repeat { kind, members: around.iter().map(|a| members[a.1]).collect(), layout: Layout::Circular { centre: middle, axis, angle } })
}

/// The members, if they fill a grid of at least two rows and two columns, every place taken, none
/// twice. Its directions may lean, as a sheared grid's do. The steps are a fit to every place.
fn in_a_grid(members: &[usize], places: &[Vec3], kind: Kind) -> Option<Repeat> {
    let between: Vec<Vec3> = (0..places.len()).flat_map(|a| (a + 1..places.len()).map(move |b| places[b] - places[a])).collect();
    let shortest = |steps: &mut dyn Iterator<Item = Vec3>| steps.filter(|s| s.len() > 0.0).min_by(|a, b| a.len2().total_cmp(&b.len2()));
    // Each along its largest component's positive way, whatever order the places came in.
    let forward = |s: Vec3| if [s.x, s.y, s.z].into_iter().max_by(|a, b| a.abs().total_cmp(&b.abs())).is_some_and(|c| c < 0.0) { -s } else { s };
    let one = forward(shortest(&mut between.iter().copied())?);
    let two = forward(shortest(&mut between.iter().copied().filter(|s| unit(one).cross(unit(*s)).len() >= NOT_IN_LINE))?);
    let (oo, ot, tt) = (one.dot(one), one.dot(two), two.dot(two));
    let square = oo * tt - ot * ot;
    if !(square > 0.0) {
        return None;
    }
    let cell = |p: Vec3| {
        let (o, t) = ((p - places[0]).dot(one), (p - places[0]).dot(two));
        (((o * tt - t * ot) / square).round() as i64, ((t * oo - o * ot) / square).round() as i64)
    };
    let counted: Vec<(i64, i64)> = places.iter().map(|p| cell(*p)).collect();
    let (left, low) = (counted.iter().map(|c| c.0).min()?, counted.iter().map(|c| c.1).min()?);
    let cells: Vec<(usize, usize)> = counted.iter().map(|c| ((c.0 - left) as usize, (c.1 - low) as usize)).collect();
    let columns = cells.iter().map(|c| c.0).max()? + 1;
    let rows = cells.iter().map(|c| c.1).max()? + 1;
    if columns < 2 || rows < 2 || columns * rows != places.len() {
        return None;
    }
    let mut taken = vec![false; places.len()];
    for &(column, row) in &cells {
        if std::mem::replace(taken.get_mut(row * columns + column)?, true) {
            return None;
        }
    }
    let middle = places.iter().fold(Vec3::ZERO, |s, p| s + *p) / places.len() as f64;
    let centred = |k: usize, size: usize| k as f64 - (size - 1) as f64 / 2.0;
    let sum = |column: bool| {
        let (mut along, mut weight) = (Vec3::ZERO, 0.0);
        for (place, c) in places.iter().zip(&cells) {
            let at = if column { centred(c.0, columns) } else { centred(c.1, rows) };
            along += (*place - middle) * at;
            weight += at * at;
        }
        along / weight
    };
    let (step, across) = (sum(true), sum(false));
    let limit = EVEN * step.len().min(across.len());
    for (place, c) in places.iter().zip(&cells) {
        if (middle + step * centred(c.0, columns) + across * centred(c.1, rows)).dist(*place) > limit {
            return None;
        }
    }
    let mut order: Vec<usize> = (0..places.len()).collect();
    order.sort_by_key(|&k| (cells[k].1, cells[k].0));
    Some(Repeat { kind, members: order.iter().map(|&k| members[k]).collect(), layout: Layout::Grid { step, across, columns, rows } })
}

/// The round holes among the faces (`is_hole` tells them) that `explained` (what a prism's
/// profile or the turned profile holds) leaves out; where holes repeat, only the first is kept,
/// with the repeat. `span` is how far a face runs along a direction; `middle` the scan's.
pub fn holes(
    faces: &[Face],
    is_hole: &dyn Fn(usize) -> bool,
    explained: &BTreeSet<usize>,
    repeats: &[Repeat],
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    middle: Vec3,
) -> Vec<Hole> {
    let hole = |k: usize| !explained.contains(&k) && is_hole(k);
    let mut repeated: BTreeMap<usize, (usize, Layout)> = BTreeMap::new();
    let mut rest = BTreeSet::new();
    for repeat in repeats.iter().filter(|r| r.kind == Kind::Cylinder && r.members.iter().all(|&k| hole(k))) {
        if repeat.members.iter().any(|k| repeated.contains_key(k) || rest.contains(k)) || matches!(repeat.layout, Layout::Grid { .. }) {
            continue;
        }
        repeated.insert(repeat.members[0], (repeat.members.len(), repeat.layout));
        rest.extend(repeat.members[1..].iter().copied());
    }
    (0..faces.len())
        .filter(|&k| hole(k) && !rest.contains(&k))
        .filter_map(|k| {
            let Shape::Cylinder { point, axis, radius } = faces[k].shape else { return None };
            let axis = axis.normalized()?;
            let (least, most) = span(k, axis)?;
            let at = |t: f64| point + axis * (t - point.dot(axis));
            let (a, b) = (at(least), at(most));
            let (mouth, bottom) = if a.dist(middle) >= b.dist(middle) { (a, b) } else { (b, a) };
            Some(Hole { face: k, mouth, bottom, radius, repeat: repeated.get(&k).copied(), form: None })
        })
        .collect()
}

/// The holes a Hole feature makes, as holes: where the bores of several repeat (a row or a ring
/// of counterbores alike), only the first, with the repeat.
fn formed_holes(formed: Vec<crate::hole_form::Formed>, repeats: &[Repeat]) -> Vec<Hole> {
    let bores: BTreeMap<usize, usize> = formed.iter().enumerate().map(|(i, f)| (f.bore, i)).collect();
    let mut repeated: BTreeMap<usize, (usize, Layout)> = BTreeMap::new();
    let mut rest = BTreeSet::new();
    for repeat in repeats.iter().filter(|r| r.kind == Kind::Cylinder && !matches!(r.layout, Layout::Grid { .. })) {
        let Some(first) = repeat.members.first().and_then(|k| bores.get(k)) else { continue };
        let alike = |k: &usize| {
            bores.get(k).is_some_and(|&i| {
                let (a, b) = (&formed[*first], &formed[i]);
                (a.counterbore, a.countersink, a.depth, a.tip, a.thread, a.radius)
                    == (b.counterbore, b.countersink, b.depth, b.tip, b.thread, b.radius)
            })
        };
        if repeat.members.iter().all(alike) && !repeat.members.iter().any(|k| rest.contains(k) || repeated.contains_key(k)) {
            repeated.insert(repeat.members[0], (repeat.members.len(), repeat.layout));
            rest.extend(repeat.members[1..].iter().copied());
        }
    }
    formed
        .into_iter()
        .filter(|f| !rest.contains(&f.bore))
        .map(|f| Hole { face: f.bore, mouth: f.mouth, bottom: f.bottom, radius: f.radius, repeat: repeated.get(&f.bore).copied(), form: Some(f) })
        .collect()
}

/// A part made of walls of one thickness, open at an end (a moulded housing, a tray, a box without
/// its lid): built as its outside, then shelled.
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    /// How thick its walls are, in mm.
    pub thickness: f64,
    /// The inner faces: each a plane behind an outer one, facing the other way, `thickness` in,
    /// or a round inside an outer round about the same axis, `thickness` smaller. Set aside, so
    /// the prisms are read off the outside alone.
    pub inner: Vec<usize>,
    /// The outer faces with an inner face behind them.
    pub outer: Vec<usize>,
    /// The faces the shell leaves open: ends of the main prism with no inner face behind them,
    /// and a point on each, on its plane inside the prism's outline.
    pub open: Vec<usize>,
    pub points: Vec<Vec3>,
}

/// The walls of one thickness the flat faces make, if they make a shell: each pair of planes
/// facing apart with material between them, no thicker than [`MOST_WALL`] of the part, one in
/// front of the other (the outer the one facing away from the part's `centre`); the thickness
/// the pairs share most by area; a shell when those pairs face [`LEAST_SHELL_SIDES`] ways or more
/// and hold [`LEAST_SHELL_SHARE`] of the flat faces' area. A plate is one pair, one way: no shell.
/// The rounds inside outer rounds (`is_hole` for material round them) that much smaller about the
/// same axis are inner too. `open` and `points` are left for the prisms to say.
fn shell_of(
    faces: &[Face],
    middles: &[Vec3],
    centre: Vec3,
    size: f64,
    sign: f64,
    skip: &BTreeSet<usize>,
    is_hole: &dyn Fn(usize) -> bool,
) -> Option<Shell> {
    let planes: Vec<(usize, Vec3, Vec3)> = (0..faces.len())
        .filter(|k| !skip.contains(k) && faces[*k].area > 0.0)
        .filter_map(|k| plane(&faces[k]).map(|(point, normal)| (k, point, unit(normal) * sign)))
        .collect();
    let flat_area: f64 = planes.iter().map(|p| faces[p.0].area).sum();
    // (thickness, outer, inner)
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, &(a, pa, na)) in planes.iter().enumerate() {
        for &(b, pb, nb) in &planes[i + 1..] {
            if na.dot(nb) > -SAME_DIRECTION {
                continue;
            }
            let t = (pa - pb).dot(na);
            if !(t > 0.0 && t <= MOST_WALL * size) {
                continue;
            }
            // One in front of the other: their middles apart across the planes by less than
            // half the smaller face's width.
            let (ma, mb) = (middles.get(a).copied().unwrap_or(pa), middles.get(b).copied().unwrap_or(pb));
            let off = mb - ma;
            let across = (off - na * off.dot(na)).len();
            if across > 0.5 * faces[a].area.min(faces[b].area).sqrt() {
                continue;
            }
            let outer_first = (ma - centre).dot(na) >= (mb - centre).dot(nb);
            pairs.push(if outer_first { (t, a, b) } else { (t, b, a) });
        }
    }
    let weight = |p: &(f64, usize, usize)| faces[p.1].area + faces[p.2].area;
    let best = pairs
        .iter()
        .map(|&(t, ..)| {
            let near: Vec<&(f64, usize, usize)> = pairs.iter().filter(|q| (q.0 - t).abs() <= SAME_WALL * t).collect();
            let area: f64 = near.iter().map(|q| weight(q)).sum();
            (area, t)
        })
        .max_by(|x, y| x.0.total_cmp(&y.0))?;
    let held: Vec<&(f64, usize, usize)> = pairs.iter().filter(|q| (q.0 - best.1).abs() <= SAME_WALL * best.1).collect();
    let thickness = held.iter().map(|q| q.0 * weight(q)).sum::<f64>() / best.0;
    let mut sides: Vec<Vec3> = Vec::new();
    for q in &held {
        let Some((_, n)) = plane(&faces[q.1]) else { continue };
        let n = unit(n);
        if !sides.iter().any(|s| s.dot(n) >= SAME_DIRECTION) {
            sides.push(n);
        }
    }
    if sides.len() < LEAST_SHELL_SIDES || best.0 < LEAST_SHELL_SHARE * flat_area {
        return None;
    }
    let mut outer: BTreeSet<usize> = held.iter().map(|q| q.1).collect();
    let mut inner: BTreeSet<usize> = held.iter().map(|q| q.2).collect();
    // The rounds inside outer rounds about the same axis, a wall thinner.
    let round = |k: usize| match faces[k].shape {
        Shape::Cylinder { point, axis, radius } => Some((point, unit(axis), radius)),
        _ => None,
    };
    for j in (0..faces.len()).filter(|&j| !skip.contains(&j) && is_hole(j)) {
        let Some((pj, aj, rj)) = round(j) else { continue };
        let outside = (0..faces.len()).filter(|&k| k != j && !skip.contains(&k) && !is_hole(k)).find(|&k| {
            round(k).is_some_and(|(pk, ak, rk)| {
                let off = pj - pk;
                aj.dot(ak).abs() >= MOST_DRAFT_COS
                    && (off - ak * off.dot(ak)).len() <= SAME_WALL * rk.max(thickness)
                    && (rk - rj - thickness).abs() <= SAME_WALL.max(0.2) * thickness
            })
        });
        if let Some(k) = outside {
            inner.insert(j);
            outer.insert(k);
        }
    }
    Some(Shell { thickness, inner: inner.into_iter().collect(), outer: outer.into_iter().collect(), open: Vec::new(), points: Vec::new() })
}

/// What Reconstruct reads off a scan's Find All.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The shapes found, in the order of the outcome's segments (shape `k` is segment `k`). A
    /// surface Find All found in pieces is read as its largest: that face has their area together,
    /// and each other piece none.
    pub faces: Vec<Face>,
    /// Each face's surface ([`same_surfaces`]): the face that stands for it, itself unless it is a
    /// piece of a larger one.
    pub same: Vec<usize>,
    /// The share of the scan's area in shapes.
    pub in_shapes: f64,
    pub prisms: Vec<Prism>,
    /// How many more prisms the shapes seemed to make whose slice halfway along runs round no
    /// walls of theirs, so Extrude It could not build them: left out of `prisms`.
    pub unread_prisms: usize,
    pub revolve: Option<Revolve>,
    /// With no turned part: the axis round shapes still share ([`round_axis`]), when they cover at
    /// least [`PARTLY_TURNED`] of the shapes (a wheel, a hub with spokes or bosses).
    pub partly_turned: Option<Revolve>,
    /// A turned part's faces repeated evenly round its axis (a wheel's spokes), left out of its
    /// profile: Revolve It turns the part with the openings between them filled.
    pub spokes: Option<Spokes>,
    /// Why the turned part's profile cannot be read (a face about the axis missing, or one too
    /// many), when there is a turned part and Revolve It could not build it.
    pub turned_unread: Option<String>,
    pub repeats: Vec<Repeat>,
    /// The repeats of round holes or bosses a solid can be built with as a pattern.
    pub plans: Vec<PatternPlan>,
    pub holes: Vec<Hole>,
    /// The faces that are rounds along an edge (a fillet), not features of their own.
    pub rounds: BTreeSet<usize>,
    /// The rounds along an edge no profile holds, for Fillet It: cylinders (a torus where two
    /// rounds meet is made by filleting their edges).
    pub fillets: Vec<EdgeRound>,
    /// The flat chamfers along an edge no profile holds, for Chamfer It.
    pub chamfers: Vec<EdgeChamfer>,
    /// Walls of one thickness, open at an end: the prisms are read off the outside, and Shell It
    /// hollows the body they make.
    pub shell: Option<Shell>,
    /// The part's size, the diagonal of its box.
    pub size: f64,
    /// Prisms alike (a rib, a boss, a tab: the same depth along the same direction, as many walls
    /// of about the same area) evenly along a line or round a circle: each repeat's `members` are
    /// indices into `prisms`, the first the one to extrude, the rest made by patterning it.
    pub prism_repeats: Vec<Repeat>,
    /// A coil spring the tori make, for Coil It.
    pub coil: Option<crate::coil::Coil>,
    /// A bar bent along a flat path (cylinders and the tori of its bends), for Pipe It.
    pub pipe: Option<crate::pipe::Pipe>,
    /// With no prism, turned part, pipe or coil read: the part as sections lofted, for Loft It.
    pub loft: Option<crate::loft::Loft>,
}

/// The sign that turns the scan's winding out of its material: the sign of the volume its
/// triangles bound (most scans are wound outward, even with gaps).
pub fn outward(scan: &Scan) -> f64 {
    let volume: f64 = (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).map(|[a, b, c]| a.dot(b.cross(c))).sum();
    if volume < 0.0 { -1.0 } else { 1.0 }
}

/// Whether the cylinder of `segment` is a hole (material round it), as its triangles face: out of
/// the material, as wound times `outward`, toward its axis.
pub fn is_round_hole(scan: &Scan, segment: &Segment, outward: f64) -> bool {
    let Some(Shape::Cylinder { point, axis, .. }) = segment.shape else { return false };
    let Some(axis) = axis.normalized() else { return false };
    let facing: f64 = segment
        .triangles
        .iter()
        .filter_map(|&t| scan.corners(t))
        .map(|[a, b, c]| {
            let middle = (a + b + c) / 3.0 - point;
            (b - a).cross(c - a).dot(middle - axis * middle.dot(axis))
        })
        .sum();
    facing * outward < 0.0
}

/// Whether the cylinder of `segment` is a hole Cut It should drill: material round it
/// ([`is_round_hole`]) at least half way round its axis. A narrower hollow strip is the round in an
/// inside corner or the side of a groove, which a cut through the part would ruin it with.
pub fn is_hole_to_cut(scan: &Scan, segment: &Segment, outward: f64) -> bool {
    is_round_hole(scan, segment, outward) && arc_covered(scan, segment) >= PI
}

/// How far round its axis a round segment's triangles reach, in radians (`TAU` all the way).
fn arc_covered(scan: &Scan, segment: &Segment) -> f64 {
    let Some((point, axis)) = segment.shape.and_then(|s| s.axis()) else { return TAU };
    let Some(axis) = axis.normalized() else { return TAU };
    let (u, v) = in_plane(axis);
    let mut angles: Vec<f64> = segment
        .triangles
        .iter()
        .filter_map(|&t| scan.corners(t))
        .map(|[a, b, c]| (a + b + c) / 3.0 - point)
        .map(|d| d.dot(v).atan2(d.dot(u)))
        .collect();
    if angles.len() < 2 {
        return 0.0;
    }
    angles.sort_by(f64::total_cmp);
    let mut gap = angles[0] + TAU - angles[angles.len() - 1];
    for pair in angles.windows(2) {
        gap = gap.max(pair[1] - pair[0]);
    }
    TAU - gap
}

/// Whether a round segment is the round of an edge: a torus, or a cylinder narrow beside the part
/// that covers less than half a turn.
fn is_edge_round(scan: &Scan, segment: &Segment, size: f64) -> bool {
    match segment.shape {
        Some(Shape::Torus { .. }) => true,
        Some(Shape::Cylinder { radius, .. }) => radius < 0.1 * size && arc_covered(scan, segment) < PI,
        _ => false,
    }
}

/// Whether `floor` is the floor of the round hole `hole`: a plane square to its axis, its middle
/// on the axis, no bigger than half as much again as the hole's end.
fn is_floor_of(hole: &Face, floor: &Face) -> bool {
    let (Shape::Cylinder { point, axis, radius }, Shape::Plane { point: middle, normal }) = (hole.shape, floor.shape) else { return false };
    let axis = unit(axis);
    let v = middle - point;
    unit(normal).dot(axis).abs() >= SAME_DIRECTION
        && (v - axis * v.dot(axis)).len() <= SAME_AXIS * radius.max(1.0) * 4.0
        && floor.area <= 1.5 * PI * radius * radius
}

/// How far segment `segment` runs along `direction`, least and most.
pub fn span(scan: &Scan, segment: &Segment, direction: Vec3) -> Option<(f64, f64)> {
    segment
        .triangles
        .iter()
        .filter_map(|&t| scan.corners(t))
        .flatten()
        .map(|p| p.dot(direction))
        .fold(None, |range: Option<(f64, f64)>, d| Some(range.map_or((d, d), |(least, most)| (least.min(d), most.max(d)))))
}

/// Planes facing this nearly the same way, or cylinders whose axes lie this nearly along each
/// other (a cosine, about 3°: a narrow strip of a round holds its axis no better), may be one
/// surface.
const SAME_SURFACE: f64 = 0.9986;
/// How many of a shape's triangles [`same_surfaces`] fits and measures, at most.
const SURFACE_SAMPLES: usize = 240;

/// One surface as [`same_surfaces`] gathers it: the face that stands for it, every `step`th
/// triangle of each of its pieces (so each piece weighs as much as it covers), and the shape
/// fitted to them.
struct Surface {
    face: usize,
    step: usize,
    sample: Vec<usize>,
    shape: Shape,
}

/// `fitted` turned to face as `was` does: a plane fitted afresh faces either way, and which way a
/// face faces is which side its material is on.
fn facing_as(fitted: Shape, was: &Shape) -> Shape {
    match (fitted, *was) {
        (Shape::Plane { point, normal }, Shape::Plane { normal: before, .. }) if normal.dot(before) < 0.0 => Shape::Plane { point, normal: -normal },
        (Shape::Cylinder { point, axis, radius }, Shape::Cylinder { axis: before, .. }) if axis.dot(before) < 0.0 => {
            Shape::Cylinder { point, axis: -axis, radius }
        }
        _ => fitted,
    }
}

/// The shapes Find All found in pieces of one surface (a parting line or a crease of noise across
/// a face, a round cut in two along its length): each shape's surface, as an index into `shaped`
/// (its largest piece, else the shape itself), and for each surface in pieces the shape fitted to
/// them all. A piece joins a larger surface of its kind (a plane facing the same way, a cylinder
/// about nearly the same axis with nearly the same radius) when one shape fitted to both lies
/// within the scan's `tolerance` of each (as a root mean square). Each piece is measured against
/// the surface gathered so far, never one piece against another, so a gently curved face Find
/// All read as several planes is not run together into one.
pub fn same_surfaces(scan: &Scan, shaped: &[&Segment], tolerance: f64) -> (Vec<usize>, BTreeMap<usize, Shape>) {
    let n = shaped.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| shaped[b].area.total_cmp(&shaped[a].area).then(a.cmp(&b)));
    let corners = |triangles: &[usize]| -> Vec<Vec3> { triangles.iter().filter_map(|&t| scan.corners(t)).flatten().collect() };
    let within = |shape: &Shape, points: &[Vec3]| {
        !points.is_empty() && (points.iter().map(|&p| shape.distance(p).powi(2)).sum::<f64>() / points.len() as f64).sqrt() <= tolerance
    };
    let mut same: Vec<usize> = (0..n).collect();
    let mut surfaces: Vec<Surface> = Vec::new();
    for &k in &order {
        let Some(shape) = shaped[k].shape else { continue };
        let step = (shaped[k].triangles.len() / SURFACE_SAMPLES).max(1);
        let sample: Vec<usize> = shaped[k].triangles.iter().step_by(step).copied().collect();
        let points = corners(&sample);
        let Some(middle) = (!points.is_empty()).then(|| points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f64) else { continue };
        // Of a kind with the surface, and near enough that fitting both is worth it: a plane's
        // middle on the surface, a cylinder's axis along the surface's.
        let kin = |surface: &Surface| match (surface.shape, shape) {
            (Shape::Plane { normal: m, .. }, Shape::Plane { normal: n, .. }) => {
                unit(m).dot(unit(n)) >= SAME_SURFACE && surface.shape.distance(middle).abs() <= 4.0 * tolerance
            }
            (Shape::Cylinder { point: p, axis: a, radius: r }, Shape::Cylinder { point: q, axis: b, radius: s }) => {
                let (a, v) = (unit(a), q - p);
                let off = (v - a * v.dot(a)).len();
                let reach = 4.0 * tolerance + 0.05 * r.max(s);
                a.dot(unit(b)).abs() >= SAME_SURFACE && (r - s).abs() <= reach && off <= reach
            }
            _ => false,
        };
        let joined = surfaces.iter_mut().filter(|s| kin(s)).find_map(|surface| {
            let weighed: Vec<usize> = shaped[k].triangles.iter().step_by(surface.step).copied().collect();
            let both: Vec<usize> = surface.sample.iter().chain(&weighed).copied().collect();
            let fitted = facing_as(crate::fit::refined(scan, &both, &surface.shape, 4 * SURFACE_SAMPLES)?, &surface.shape);
            (fitted.kind() == shape.kind() && within(&fitted, &corners(&surface.sample)) && within(&fitted, &points)).then(|| {
                surface.sample = both;
                surface.shape = fitted;
                surface.face
            })
        });
        match joined {
            Some(face) => same[k] = face,
            None => surfaces.push(Surface { face: k, step, sample, shape }),
        }
    }
    let fitted =
        surfaces.into_iter().filter(|s| same.iter().enumerate().any(|(k, &f)| f == s.face && k != s.face)).map(|s| (s.face, s.shape)).collect();
    (same, fitted)
}

/// The fewest triangles a selection [`read_region`] reads, and the fewest of a shape's (and the
/// least share of the selection's area) for the shape to be read in it: a shape the selection
/// only grazes is left out, its triangles freeform.
const REGION_LEAST: usize = 50;
const REGION_SHAPE_LEAST: usize = 12;
const REGION_SHAPE_SHARE: f64 = 0.005;

/// What Reconstruct reads off part of a scan ([`read_region`]): the part's triangles, a smaller
/// scan on the same points; the shapes Find All found there, as an outcome of that smaller scan;
/// and what they say.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub triangles: Vec<[u32; 3]>,
    pub outcome: Outcome,
    pub reading: Reading,
}

impl Region {
    /// The part as a scan, on `scan`'s points.
    pub fn scan<'a>(&'a self, scan: &Scan<'a>) -> Scan<'a> {
        Scan::new(scan.positions, &self.triangles)
    }
}

/// What the shapes of `outcome` (Find All on the whole of `scan`) say about the triangles
/// `selected` (one flag a triangle): a turned boss, a prism, a hole of a part too busy to read
/// whole, read alone, to build as a feature of its own. The selected triangles are a smaller scan
/// on the same points; each shape keeps its triangles there (one the selection only grazes is
/// left freeform); which way is out of the material is the whole scan's, as an open patch's own
/// winding cannot tell; sizes are the whole part's. `None` for a selection of fewer than 50
/// triangles.
pub fn read_region(scan: &Scan, outcome: &Outcome, selected: &[bool]) -> Option<Region> {
    let mut index = vec![u32::MAX; scan.triangle_count()];
    let mut triangles = Vec::new();
    for (t, slot) in index.iter_mut().enumerate() {
        if selected.get(t).copied().unwrap_or(false) && scan.corners(t).is_some() {
            *slot = triangles.len() as u32;
            triangles.push(scan.triangles[t]);
        }
    }
    if triangles.len() < REGION_LEAST {
        return None;
    }
    let part = Scan::new(scan.positions, &triangles);
    let area_of = |t: usize| part.corners(t).map_or(0.0, |[a, b, c]| (b - a).cross(c - a).len() / 2.0);
    let total: f64 = (0..triangles.len()).map(area_of).sum();
    let (mut shaped, mut freeform) = (Vec::new(), Vec::new());
    for segment in &outcome.segments {
        let inside: Vec<usize> =
            segment.triangles.iter().filter_map(|&t| index.get(t).copied()).filter(|&i| i != u32::MAX).map(|i| i as usize).collect();
        if inside.is_empty() {
            continue;
        }
        let area: f64 = inside.iter().map(|&t| area_of(t)).sum();
        let kept = Segment { triangles: inside, shape: segment.shape, area, rms: segment.rms };
        match segment.shape {
            Some(_) if kept.triangles.len() >= REGION_SHAPE_LEAST && area >= REGION_SHAPE_SHARE * total => shaped.push(kept),
            _ => freeform.push(Segment { shape: None, rms: 0.0, ..kept }),
        }
    }
    let in_shapes = shaped.iter().map(|s| s.area).sum::<f64>() / total.max(1e-300);
    shaped.extend(freeform);
    let outcome = Outcome { segments: shaped, noise: outcome.noise, tolerance: outcome.tolerance, in_shapes, cancelled: outcome.cancelled };
    let reading = read_signed(&part, &outcome, outward(scan));
    Some(Region { triangles, outcome, reading })
}

/// What the shapes of `outcome` (Find All on `scan`) say.
pub fn read(scan: &Scan, outcome: &Outcome) -> Reading {
    read_signed(scan, outcome, outward(scan))
}

/// [`read`], out of the material being the way the scan's winding times `sign` faces.
fn read_signed(scan: &Scan, outcome: &Outcome, sign: f64) -> Reading {
    let shaped: Vec<&Segment> = outcome.segments.iter().take_while(|s| s.shape.is_some()).collect();
    let (same, fitted) = same_surfaces(scan, &shaped, outcome.tolerance);
    let piece = |k: usize| same.get(k).is_some_and(|&s| s != k);
    let pieces: BTreeSet<usize> = (0..shaped.len()).filter(|&k| piece(k)).collect();
    // A surface in pieces is read whole: the shape fitted to them all, its area theirs together,
    // a piece's none.
    let mut faces: Vec<Face> = shaped.iter().filter_map(|s| s.shape.map(|shape| Face { shape, area: s.area })).collect();
    for (&k, &shape) in &fitted {
        faces[k].shape = shape;
    }
    for &k in &pieces {
        faces[same[k]].area += faces[k].area;
        faces[k].area = 0.0;
    }
    let size = scan.extent();
    let span_of = |k: usize, d: Vec3| {
        (0..shaped.len()).filter(|&j| same[j] == k).filter_map(|j| span(scan, shaped[j], d)).reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    };
    // A round surface in pieces, its triangles together, for how far round it goes.
    let whole = |k: usize| -> std::borrow::Cow<'_, Segment> {
        if pieces.iter().any(|&j| same[j] == k) {
            let mut joined = shaped[k].clone();
            for &j in pieces.iter().filter(|&&j| same[j] == k) {
                joined.triangles.extend_from_slice(&shaped[j].triangles);
                joined.area += shaped[j].area;
            }
            std::borrow::Cow::Owned(joined)
        } else {
            std::borrow::Cow::Borrowed(shaped[k])
        }
    };
    // Read once: the floors below ask of every pair of faces.
    let holes_at: Vec<bool> = (0..shaped.len()).map(|k| !piece(k) && is_hole_to_cut(scan, &whole(k), sign)).collect();
    let hole = |k: usize| holes_at.get(k).copied().unwrap_or(false);
    // A hole's floor is the hole's: a flat face on its axis, square to it, about as big as its end.
    let floors: BTreeSet<usize> = (0..faces.len()).filter(|&j| (0..faces.len()).any(|k| hole(k) && is_floor_of(&faces[k], &faces[j]))).collect();
    let mut revolve = revolve(&faces, size);
    // A wheel: round about an axis, the rest repeated evenly round it.
    let mut spokes = None;
    if revolve.is_none()
        && let Some(round) = round_axis(&faces, size).filter(|r| {
            let total: f64 = faces.iter().map(|f| f.area).sum();
            let turning: f64 = r.members.iter().filter(|&&k| !matches!(faces[k].shape, Shape::Plane { .. })).map(|&k| faces[k].area).sum();
            r.share >= SPOKED_TURNED && turning >= SPOKED_ROUND * total
        })
        && let Some(found) = spokes_about(&faces, &round.axis, &round.members).filter(|sp| round.share + sp.share >= MOSTLY_TURNED)
    {
        revolve = Some(round);
        spokes = Some(found);
    }
    let partly_turned = if revolve.is_none() { round_axis(&faces, size).filter(|r| r.share >= PARTLY_TURNED) } else { None };
    // Holes as a Hole feature makes them (a counterbore, a countersink, a blind hole's point, a
    // tap drill): Hole It drills each whole, so none of their faces is a prism's wall or end, whose
    // levels would cut the prism into slabs at a counterbore's step or a blind hole's end.
    let formed = crate::hole_form::read(&faces, &hole, &span_of, crate::surface_centre(scan).unwrap_or(Vec3::ZERO), outcome.tolerance, size, &pieces);
    let in_formed: BTreeSet<usize> = formed.iter().flat_map(|f| f.faces.iter().copied()).collect();
    let ignored: BTreeSet<usize> = floors.union(&pieces).chain(&in_formed).copied().collect();
    // The rounds along edges, read before the prisms, which they do not choose.
    let rounds: BTreeSet<usize> = (0..shaped.len()).filter(|&k| !piece(k) && is_edge_round(scan, &whole(k), size)).collect();
    // The chamfers along edges, read with the rounds: walls that explain nothing, so a block with
    // chamfered top edges is extruded upright and chamfered, not extruded on its side because its
    // side profile holds two of the chamfers.
    let middles: Vec<Vec3> = (0..shaped.len()).map(|k| middle_of(scan, &whole(k))).collect();
    let chamfer_skip: BTreeSet<usize> = ignored.union(&rounds).copied().chain((0..shaped.len()).filter(|&k| hole(k))).collect();
    let chamfered = chamfers(&faces, &middles, size, &chamfer_skip);
    let unscored: BTreeSet<usize> = rounds.iter().copied().chain(chamfered.iter().map(|c| c.face)).collect();
    // Walls of one thickness: the prisms are read off the outside, the inner faces set aside, while
    // that leaves the main prism an end with no inner face behind it for the shell to open.
    let centre = crate::surface_centre(scan).unwrap_or(Vec3::ZERO);
    let walled = if revolve.is_none() {
        // Material round a round, however far round it goes: an inside corner is a quarter turn.
        let hollow: Vec<bool> = (0..shaped.len()).map(|k| !piece(k) && is_round_hole(scan, &whole(k), sign)).collect();
        shell_of(&faces, &middles, centre, size, sign, &ignored, &|k| hollow.get(k).copied().unwrap_or(false))
    } else {
        None
    };
    // Only a prism Extrude It can build: not the steps of a turned part read twice over (walls all
    // about the turning axis, or about it with holes through them, which Cut It makes: extruded
    // too, such a slab lies on the turned body's own faces, which the kernel's boolean refuses or
    // leaves broken for the cuts after it, T-LESS part 4); not a hole (sides all holes, cut and
    // not extruded); and one whose slice halfway along runs round its walls (walls that bound
    // nothing, a round's sliver of cylinder alone, two faces side by side, are no prism). The
    // prisms kept, and how many were read.
    let weld = std::cell::OnceCell::new();
    let usable = |mut list: Vec<Prism>| -> (Vec<Prism>, usize) {
        if let Some(revolve) = &revolve {
            list.retain(|p| !p.walls.iter().all(|&w| revolve.members.contains(&w) || hole(w)));
        }
        list.retain(|p| !p.walls.iter().all(|&w| hole(w)));
        let read = list.len();
        if !list.is_empty() {
            let weld = weld.get_or_init(|| crate::welded(scan.positions));
            list.retain(|p| crate::profile::prism_profile_from(scan, weld, outcome, &faces, &same, size, p).is_ok());
        }
        (list, read)
    };
    let explains = |list: &[Prism]| -> f64 {
        let held: BTreeSet<usize> = list.iter().flat_map(|p| p.walls.iter().chain(&p.ends)).copied().collect();
        held.iter().map(|&k| faces[k].area).sum()
    };
    let mut shell = None;
    let mut chosen: Option<(Vec<Prism>, usize)> = None;
    if let Some(mut walls) = walled {
        let outside: BTreeSet<usize> = ignored.iter().chain(&walls.inner).copied().collect();
        let read = extrusions(&faces, &span_of, size, &outside, &unscored, sign, true);
        // A shell's outside is all walls with an inner face behind them (its rounds aside); a
        // bent sheet (a bracket) pairs its faces too, but its edges have nothing behind them.
        let closed_round = |p: &Prism| p.walls.iter().all(|&w| plane(&faces[w]).is_none() || walls.outer.contains(&w));
        if let Some(main) = read.first().filter(|p| closed_round(p)) {
            let mut ends = main.ends.to_vec();
            ends.dedup();
            walls.open = ends.into_iter().filter(|e| !walls.outer.contains(e)).collect();
            walls.points = walls
                .open
                .iter()
                .filter_map(|&e| {
                    let (point, normal) = plane(&faces[e])?;
                    let n = unit(normal);
                    let m = main.middle();
                    Some(m - n * (m - point).dot(n))
                })
                .collect();
        }
        if !walls.open.is_empty() {
            let (list, read) = usable(read);
            if !list.is_empty() {
                chosen = Some((list, read));
                shell = Some(walls);
            }
        }
    }
    // Drafts read where they make a prism Extrude It can build that explains more than the square
    // walls do: on a noisy scan, square walls can lean alike by chance into a drafted section whose
    // slice no profile goes round, which would hide the square prism there.
    let (prisms, read_prisms) = chosen.unwrap_or_else(|| {
        let square = usable(extrusions(&faces, &span_of, size, &ignored, &unscored, sign, false));
        let drafted = usable(extrusions(&faces, &span_of, size, &ignored, &unscored, sign, true));
        if explains(&drafted.0) > explains(&square.0) { drafted } else { square }
    });
    let unread_prisms = read_prisms - prisms.len();
    let prisms = shared(prisms, &faces, outcome.tolerance);
    let inner: BTreeSet<usize> = shell.iter().flat_map(|w| w.inner.iter().copied()).collect();
    let repeats = repeats(&faces);
    let between = spokes.as_ref().map(|sp| crate::profile::between_spokes(&sp.angles)).unwrap_or_default();
    let turned = revolve.as_ref().map(|r| crate::profile::turned_profile_of(scan, outcome, &faces, &same, size, r, &between));
    let turned_unread = turned.as_ref().and_then(|t| t.as_ref().err().cloned());
    // The faces the turned profile runs along. A bore about the axis the profile does not run
    // along (one read off a slice that comes down to the axis there) is no part of the revolved
    // body: it is cut, as a hole is, or the revolve fills it.
    let in_profile: Option<BTreeSet<usize>> = turned.and_then(|t| t.ok()).map(|(_, outline, _, _)| outline.faces.iter().flatten().copied().collect());
    let explained: BTreeSet<usize> = match &revolve {
        Some(revolve) => revolve.members.iter().copied().filter(|&k| !hole(k) || in_profile.as_ref().is_none_or(|p| p.contains(&k))).collect(),
        None => prisms.iter().flat_map(|p| p.walls.iter().chain(&p.ends)).chain(&inner).copied().collect(),
    };
    let plain_explained: BTreeSet<usize> = explained.union(&in_formed).copied().collect();
    let mut holes = holes(&faces, &hole, &plain_explained, &repeats, &span_of, centre);
    holes.extend(formed_holes(formed, &repeats));
    let plans = pattern_plans(&faces, &repeats, &rounds);
    // The rounds no profile holds: not a wall of a prism, nor a face of a turned part that is read.
    let profiled: BTreeSet<usize> = prisms
        .iter()
        .flat_map(|p| p.walls.iter().copied())
        .chain(inner.iter().copied())
        .chain(revolve.iter().filter(|_| turned_unread.is_none()).flat_map(|r| r.members.iter().copied()))
        .collect();
    let fillets: Vec<EdgeRound> = rounds
        .iter()
        .filter(|k| !profiled.contains(k))
        .filter_map(|&k| match faces[k].shape {
            Shape::Cylinder { radius, .. } => edge_of_round(scan, &whole(k), &faces).map(|edge| EdgeRound { face: k, radius, edge }),
            _ => None,
        })
        .collect();
    let chamfers: Vec<EdgeChamfer> = chamfered.into_iter().filter(|c| !profiled.contains(&c.face)).collect();
    let corners_of = |k: usize| -> Vec<Vec3> { whole(k).triangles.iter().filter_map(|&t| scan.corners(t)).flatten().collect() };
    let coil = crate::coil::read(&faces, &corners_of, outcome.tolerance);
    let pipe = if coil.is_some() {
        None
    } else {
        crate::pipe::read(
            &faces,
            &|k| !piece(k) && is_round_hole(scan, &whole(k), sign),
            &span_of,
            &|k, centre, axis, u| crate::pipe::angular_extent(corners_of(k).into_iter(), centre, axis, u),
            outcome.tolerance,
        )
    };
    // A pipe's straight runs read as prisms too: Pipe It builds them.
    let mut prisms = prisms;
    if let Some(pipe) = &pipe {
        prisms.retain(|p| !p.walls.iter().all(|w| pipe.faces.contains(w)));
    }
    let prism_repeats = prism_repeats(&faces, &prisms);
    let loft = (prisms.is_empty() && revolve.is_none() && coil.is_none() && pipe.is_none())
        .then(|| crate::loft::read(scan, &faces, LOFT_SECTIONS).ok())
        .flatten();
    // A coil's or a loft's pieces, and a pipe's bends, are no rounds along an edge.
    let (mut fillets, mut chamfers) = (fillets, chamfers);
    if coil.is_some() || loft.is_some() {
        fillets.clear();
        chamfers.clear();
    }
    if let Some(pipe) = &pipe {
        fillets.retain(|f| !pipe.faces.contains(&f.face));
    }
    Reading {
        faces,
        same,
        in_shapes: outcome.in_shapes,
        prisms,
        unread_prisms,
        revolve,
        partly_turned,
        spokes,
        turned_unread,
        repeats,
        plans,
        holes,
        rounds,
        fillets,
        chamfers,
        shell,
        size,
        prism_repeats,
        coil,
        pipe,
        loft,
    }
}

/// How unequal alike prisms' depths may be, as a share of the deeper.
const SAME_DEPTH: f64 = 0.02;

/// The prisms alike (see [`Reading::prism_repeats`]) laid out evenly, at least [`LEAST_REPEATS`]
/// of them; never the first prism, the main body the rest stand on.
pub fn prism_repeats(faces: &[Face], prisms: &[Prism]) -> Vec<Repeat> {
    let walls_area = |p: &Prism| p.walls.iter().map(|&w| faces.get(w).map_or(0.0, |f| f.area)).sum::<f64>();
    let alike = |a: &Prism, b: &Prism| {
        a.direction.dot(b.direction).abs() >= SAME_DIRECTION
            && (a.depth() - b.depth()).abs() <= SAME_DEPTH * a.depth().max(b.depth())
            && a.walls.len() == b.walls.len()
            && a.draft == b.draft
            && (walls_area(a) - walls_area(b)).abs() <= SAME_AREA * walls_area(a).max(walls_area(b))
    };
    let mut grouped = vec![false; prisms.len()];
    let mut found = Vec::new();
    for first in 1..prisms.len() {
        if grouped[first] {
            continue;
        }
        let members: Vec<usize> = (first..prisms.len()).filter(|&k| !grouped[k] && alike(&prisms[first], &prisms[k])).collect();
        if members.len() < LEAST_REPEATS {
            continue;
        }
        let places: Vec<Vec3> = members.iter().map(|&k| prisms[k].middle()).collect();
        let laid_out =
            in_a_line(&members, &places, Kind::Plane).or_else(|| round_a_circle(&members, &places, Kind::Plane, Some(prisms[first].direction)));
        if let Some(repeat) = laid_out {
            for &k in &repeat.members {
                grouped[k] = true;
            }
            found.push(repeat);
        }
    }
    found
}

/// How many sections Reconstruct reads a lofted part with.
pub const LOFT_SECTIONS: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;

    fn plane_at(point: Vec3, normal: Vec3, area: f64) -> Face {
        Face { shape: Shape::Plane { point, normal }, area }
    }

    fn cylinder(point: Vec3, axis: Vec3, radius: f64) -> Face {
        Face { shape: Shape::Cylinder { point, axis, radius }, area: 100.0 }
    }

    /// A wheel's spokes: five alike faces evenly round the axis (each a spoke's flank, tilted to it)
    /// are found as five places; the same faces strewn unevenly are none.
    #[test]
    fn spokes_evenly_round_the_axis_are_found() {
        let axis = Axis { point: Vec3::ZERO, direction: Vec3::Z };
        let flank = |degrees: f64| {
            let (s, c) = degrees.to_radians().sin_cos();
            plane_at(Vec3::new(60.0 * c, 60.0 * s, 10.0), Vec3::new(-s, c, 0.3), 400.0)
        };
        let round = vec![cylinder(Vec3::ZERO, Vec3::Z, 100.0), cylinder(Vec3::ZERO, Vec3::Z, 30.0)];
        let mut faces = round.clone();
        faces.extend([0.0, 72.0, 144.0, 216.0, 288.0].map(flank));
        let found = spokes_about(&faces, &axis, &[0, 1]).expect("five spokes");
        assert_eq!((found.count, found.faces.clone()), (5, vec![2, 3, 4, 5, 6]), "{found:?}");
        let mut strewn = round;
        strewn.extend([0.0, 30.0, 100.0, 200.0, 250.0].map(flank));
        assert!(spokes_about(&strewn, &axis, &[0, 1]).is_none());
    }

    /// How far a box from `low` to `high` runs along `direction`, least and most.
    fn along(low: Vec3, high: Vec3, direction: Vec3) -> (f64, f64) {
        (0..8)
            .map(|c| {
                Vec3::new(if c & 1 == 0 { low.x } else { high.x }, if c & 2 == 0 { low.y } else { high.y }, if c & 4 == 0 { low.z } else { high.z })
                    .dot(direction)
            })
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(least, most), d| (least.min(d), most.max(d)))
    }

    fn read(given: &[(Face, Vec3, Vec3)], size: f64) -> Vec<Prism> {
        let faces: Vec<Face> = given.iter().map(|g| g.0).collect();
        extrusions(&faces, &|k, d| given.get(k).map(|g| along(g.1, g.2, d)), size, &BTreeSet::new(), &BTreeSet::new(), 1.0, true)
    }

    /// A plane face facing `normal` across the box from `low` to `high`.
    fn rect(normal: Vec3, low: Vec3, high: Vec3) -> (Face, Vec3, Vec3) {
        let d = high - low;
        let mut sides = [d.x, d.y, d.z];
        sides.sort_by(|a, b| b.total_cmp(a));
        (plane_at((low + high) * 0.5, normal, sides[0] * sides[1]), low, high)
    }

    fn block() -> Vec<(Face, Vec3, Vec3)> {
        let (x, y, z) = (20.0, 15.0, 10.0);
        vec![
            rect(Vec3::Z, Vec3::new(-x, -y, z), Vec3::new(x, y, z)),
            rect(-Vec3::Z, Vec3::new(-x, -y, -z), Vec3::new(x, y, -z)),
            rect(Vec3::X, Vec3::new(x, -y, -z), Vec3::new(x, y, z)),
            rect(-Vec3::X, Vec3::new(-x, -y, -z), Vec3::new(-x, y, z)),
            rect(Vec3::Y, Vec3::new(-x, y, -z), Vec3::new(x, y, z)),
            rect(-Vec3::Y, Vec3::new(-x, -y, -z), Vec3::new(x, -y, z)),
        ]
    }

    #[test]
    fn prisms_alike_in_a_row_are_one_repeat_but_never_the_main_body() {
        // The main body, then four tabs 3 deep along Z, 12 apart along X, and one tab unlike them.
        let wall = |x: f64| plane_at(Vec3::new(x, 0.0, 0.0), Vec3::X, 30.0);
        let mut faces = vec![plane_at(Vec3::ZERO, Vec3::Z, 1000.0)];
        let mut prisms =
            vec![Prism { direction: Vec3::Z, origin: Vec3::ZERO, low: -5.0, high: 0.0, ends: [0, 0], walls: vec![0], open: false, draft: 0.0 }];
        for (k, x) in [0.0, 12.0, 24.0, 36.0, 60.0].into_iter().enumerate() {
            faces.push(wall(x));
            faces.push(wall(x + 2.0));
            let depth = if k == 4 { 6.0 } else { 3.0 };
            prisms.push(Prism {
                direction: Vec3::Z,
                origin: Vec3::new(x + 1.0, 0.0, 0.0),
                low: 0.0,
                high: depth,
                ends: [0, 0],
                walls: vec![faces.len() - 2, faces.len() - 1],
                open: false,
                draft: 0.0,
            });
        }
        let found = prism_repeats(&faces, &prisms);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].members, vec![1, 2, 3, 4]);
        let Layout::Linear { step } = found[0].layout else { panic!("{found:?}") };
        assert!((step.len() - 12.0).abs() < 1e-9, "{step:?}");
    }

    #[test]
    fn a_block_is_one_extrusion_along_its_largest_faces() {
        let found = read(&block(), 60.0);
        assert_eq!(found.len(), 1, "{found:?}");
        let top = &found[0];
        assert!((top.depth() - 20.0).abs() < 1e-9 && top.direction.dot(Vec3::Z).abs() > 0.999, "{top:?}");
        assert_eq!(top.walls.len(), 4);
        assert!(top.middle().z.abs() < 1e-9 && top.ends.contains(&0) && top.ends.contains(&1) && !top.open);
    }

    /// A plate 60 × 40 from z 0 to 4 with a boss 20 × 20 on it up to z 12: the plate, then the boss
    /// from the plate's top.
    #[test]
    fn a_plate_under_a_boss_is_the_plate_and_then_the_boss() {
        let plate = (Vec3::new(-30.0, -20.0, 0.0), Vec3::new(30.0, 20.0, 4.0));
        let boss = (Vec3::new(-10.0, -10.0, 4.0), Vec3::new(10.0, 10.0, 12.0));
        let side = |normal: Vec3, (low, high): (Vec3, Vec3)| {
            let (mut a, mut b) = (low, high);
            if normal.x > 0.5 {
                a.x = high.x
            } else if normal.x < -0.5 {
                b.x = low.x
            } else if normal.y > 0.5 {
                a.y = high.y
            } else {
                b.y = low.y
            }
            rect(normal, a, b)
        };
        let mut given = vec![
            rect(-Vec3::Z, plate.0, Vec3::new(30.0, 20.0, 0.0)),
            rect(Vec3::Z, Vec3::new(-30.0, -20.0, 4.0), plate.1),
            rect(Vec3::Z, Vec3::new(-10.0, -10.0, 12.0), boss.1),
        ];
        for n in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y] {
            given.push(side(n, plate));
            given.push(side(n, boss));
        }
        let found = read(&given, 80.0);
        assert_eq!(found.len(), 2, "{found:?}");
        let depths: Vec<f64> = found.iter().map(|p| p.depth()).collect();
        assert!(depths.iter().any(|d| (d - 4.0).abs() < 1e-9) && depths.iter().any(|d| (d - 8.0).abs() < 1e-9), "{depths:?}");
    }

    #[test]
    fn holes_in_a_row_round_a_flange_and_in_a_grid_are_repeats() {
        let row: Vec<Face> = [0.0, 15.0, 30.0, 45.0].iter().map(|&x| cylinder(Vec3::new(x, 5.0, 2.0), Vec3::Z, 3.0)).collect();
        let found = repeats(&row);
        assert_eq!(found.len(), 1);
        assert!(matches!(found[0].layout, Layout::Linear { step } if (step.len() - 15.0).abs() < 1e-9));
        let ring: Vec<Face> =
            (0..6).map(|k| f64::from(k) * TAU / 6.0).map(|a| cylinder(Vec3::new(40.0 * a.cos(), 40.0 * a.sin(), 7.0), Vec3::Z, 4.0)).collect();
        let found = repeats(&ring);
        assert!(matches!(found[0].layout, Layout::Circular { angle, .. } if (angle - TAU / 6.0).abs() < 1e-9), "{found:?}");
        let grid: Vec<Face> =
            (0..3).flat_map(|r| (0..4).map(move |c| cylinder(Vec3::new(10.0 * f64::from(c), 8.0 * f64::from(r), 0.0), Vec3::Z, 2.0))).collect();
        let found = repeats(&grid);
        assert!(matches!(found[0].layout, Layout::Grid { columns: 4, rows: 3, .. } | Layout::Grid { columns: 3, rows: 4, .. }), "{found:?}");
        let plans = pattern_plans(&grid, &found, &BTreeSet::new());
        assert_eq!(plans.len(), 1);
        assert!(plans[0].off < 1e-9 && plans[0].rest.len() == 11);
    }

    /// Three holes drilled along the radius round a shaft about Z: a ring about the shaft's axis.
    #[test]
    fn holes_drilled_along_the_radius_are_a_ring() {
        let faces: Vec<Face> = (0..3)
            .map(|k| f64::from(k) * TAU / 3.0)
            .map(|a| cylinder(Vec3::new(14.0 * a.cos(), 14.0 * a.sin(), 20.0), Vec3::new(a.cos(), a.sin(), 0.0), 3.0))
            .collect();
        let found = repeats(&faces);
        assert_eq!(found.len(), 1, "{found:?}");
        let Layout::Circular { centre, axis, angle } = found[0].layout else { panic!() };
        assert!(centre.dist(Vec3::new(0.0, 0.0, 20.0)) < 1e-9 && axis.dot(Vec3::Z).abs() > 0.999 && (angle - TAU / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_stepped_shaft_is_a_revolve_and_a_block_is_not() {
        let faces = vec![
            Face { shape: Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius: 20.0 }, area: 2500.0 },
            Face { shape: Shape::Cylinder { point: Vec3::new(0.0, 0.0, 30.0), axis: -Vec3::Z, radius: 10.0 }, area: 1200.0 },
            Face { shape: Shape::Plane { point: Vec3::ZERO, normal: -Vec3::Z }, area: 1250.0 },
            Face { shape: Shape::Plane { point: Vec3::new(0.0, 0.0, 20.0), normal: Vec3::Z }, area: 940.0 },
            Face { shape: Shape::Plane { point: Vec3::new(0.0, 0.0, 40.0), normal: Vec3::Z }, area: 310.0 },
        ];
        let found = revolve(&faces, 90.0).unwrap();
        assert_eq!(found.members.len(), 5);
        assert!(found.axis.direction.dot(Vec3::Z).abs() > 0.9999 && (found.axis.point - Vec3::Z * found.axis.point.z).len() < 1e-6);
        let block: Vec<Face> = block().into_iter().map(|g| g.0).collect();
        assert!(revolve(&block, 60.0).is_none());
    }

    /// A box's floor, and one side of it read as a sliver of a wide round (as Find All reads a
    /// fillet on a noisy scan): the shapes alone make an open prism, but its slice halfway runs on
    /// the round for one short arc and on nothing else of it, so Extrude It could not build it.
    /// Reconstruct leaves it out and counts it.
    #[test]
    fn walls_that_go_round_nothing_are_no_prism() {
        let n = 8;
        let (positions, triangles) = crate::test_meshes::fine_cuboid(Vec3::ZERO, Vec3::new(20.0, 20.0, 10.0), n);
        let scan = Scan::new(&positions, &triangles);
        // The sides in `fine_cuboid`'s order: −X, +X, −Y, +Y, −Z, +Z.
        let side = |k: usize| -> Vec<usize> { (k * 2 * (n * n) as usize..(k + 1) * 2 * (n * n) as usize).collect() };
        let segment = |triangles: Vec<usize>, shape: Shape| Segment { triangles, shape: Some(shape), area: 200.0, rms: 0.0 };
        let outcome = Outcome {
            segments: vec![
                segment(side(4), Shape::Plane { point: Vec3::ZERO, normal: -Vec3::Z }),
                segment(side(0), Shape::Cylinder { point: Vec3::new(30.0, 10.0, 0.0), axis: Vec3::Z, radius: 30.0 }),
            ],
            noise: 0.0,
            tolerance: 0.01,
            in_shapes: 0.3,
            cancelled: false,
        };
        let faces: Vec<Face> = outcome.segments.iter().map(|s| Face { shape: s.shape.unwrap(), area: s.area }).collect();
        let span_of = |k: usize, d: Vec3| outcome.segments.get(k).and_then(|s| span(&scan, s, d));
        assert_eq!(
            extrusions(&faces, &span_of, scan.extent(), &BTreeSet::new(), &BTreeSet::new(), 1.0, true).len(),
            1,
            "the shapes alone make a prism"
        );
        let reading = super::read(&scan, &outcome);
        assert!(reading.prisms.is_empty(), "{:?}", reading.prisms);
        assert_eq!(reading.unread_prisms, 1);
    }

    /// A block whose −Y side Find All found as two planes (a parting line across it), the smaller
    /// fitted a little turned: one surface, read as the larger, so the prism has the block's four
    /// sides and its profile four lines, not a fifth wall bridged across.
    #[test]
    fn a_side_found_in_two_pieces_is_one_wall() {
        let n = 8;
        let (positions, triangles) = crate::test_meshes::fine_cuboid(Vec3::ZERO, Vec3::new(20.0, 20.0, 10.0), n);
        let scan = Scan::new(&positions, &triangles);
        // The sides in `fine_cuboid`'s order: −X, +X, −Y, +Y, −Z, +Z.
        let per = 2 * (n * n) as usize;
        let side = |k: usize| -> Vec<usize> { (k * per..(k + 1) * per).collect() };
        let segment = |triangles: Vec<usize>, shape: Shape, area: f64| Segment { triangles, shape: Some(shape), area, rms: 0.0 };
        let plane = |point: Vec3, normal: Vec3| Shape::Plane { point, normal };
        let turned = Vec3::new(0.3f64.to_radians().sin(), -0.3f64.to_radians().cos(), 0.0);
        let outcome = Outcome {
            segments: vec![
                segment(side(4), plane(Vec3::ZERO, -Vec3::Z), 400.0),
                segment(side(5), plane(Vec3::new(0.0, 0.0, 10.0), Vec3::Z), 400.0),
                segment(side(0), plane(Vec3::ZERO, -Vec3::X), 200.0),
                segment(side(1), plane(Vec3::new(20.0, 0.0, 0.0), Vec3::X), 200.0),
                segment(side(3), plane(Vec3::new(0.0, 20.0, 0.0), Vec3::Y), 200.0),
                segment(side(2)[..per / 2].to_vec(), plane(Vec3::ZERO, -Vec3::Y), 100.0),
                segment(side(2)[per / 2..].to_vec(), plane(Vec3::new(10.0, 0.01, 5.0), turned), 99.0),
            ],
            noise: 0.0,
            tolerance: 0.05,
            in_shapes: 1.0,
            cancelled: false,
        };
        let reading = super::read(&scan, &outcome);
        assert_eq!(reading.same, vec![0, 1, 2, 3, 4, 5, 5], "{:?}", reading.same);
        assert_eq!(reading.faces[5].area, 199.0);
        assert_eq!(reading.faces[6].area, 0.0);
        let prism = reading.prisms.iter().find(|p| p.direction.dot(Vec3::Z).abs() > 0.99).expect("a prism along Z");
        assert!(!prism.walls.contains(&6), "{prism:?}");
        let (_, outlines) = crate::profile::prism_profile(&scan, &outcome, &reading, prism).unwrap();
        assert_eq!(outlines.len(), 1, "{outlines:?}");
        assert_eq!(outlines[0].pieces, vec![crate::profile::Piece::Line; 4], "{:?}", outlines[0]);
    }

    /// A washer's bore goes all the way round its axis and is a hole to cut. A quarter of it, as
    /// the round in an inside corner shows on a scan, is hollow too, but no hole: Reconstruct
    /// proposed cutting such strips through a scanned case (3 mm bores 30 mm long).
    #[test]
    fn a_hollow_strip_less_than_half_round_is_no_hole() {
        let sides = 48;
        let (positions, triangles) = crate::test_meshes::revolved(&[[0.0, 5.0], [10.0, 5.0], [10.0, 10.0], [0.0, 10.0]], sides, true);
        let scan = Scan::new(&positions, &triangles);
        let bore = Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius: 5.0 };
        // The bore is the first pair of rings: two triangles each step round.
        let segment = |steps: u32| Segment { triangles: (0..2 * steps as usize).collect(), shape: Some(bore), area: 100.0, rms: 0.0 };
        let (whole, quarter) = (segment(sides), segment(sides / 4));
        let sign = outward(&scan);
        assert!(is_round_hole(&scan, &whole, sign) && is_round_hole(&scan, &quarter, sign), "both are hollow");
        assert!(is_hole_to_cut(&scan, &whole, sign));
        assert!(!is_hole_to_cut(&scan, &quarter, sign));
    }
}
