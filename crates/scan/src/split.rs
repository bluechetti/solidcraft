//! Split Mesh: a scan cut by a cutter (a plane or a fitted shape, another body or scan, a 3D
//! curve on the scan, a sketch's outlines carried through space), after SurfaceCraft's mesh
//! cutter. The cutter is a field of space, negative on one side and positive on the other (a
//! signed distance where it is one). Every triangle it passes through is clipped along it, the
//! new vertices on the old edges, found on the cutter and shared by the triangles either side, so
//! each side is whole.
//!
//! A cutter that ends (a body, a curve's ribbon, a sketch held to a slab) cuts only where the cut
//! touches it: a run of the cut that never comes near it is where its field only runs on across
//! another part of the scan, and the triangles either side of it stay joined, each piece so made
//! going to the side it is mostly on. A cut that does not divide a piece slits it, and says so.
//! An envelope cuts twice, half its thickness either side, and what lies between is a slab of its
//! own.

use std::collections::{HashMap, HashSet};

use solidcore_record::{Region2, Seg2, Vec2, Vec3};
use solidcraft_geom::Mesh;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::{Scan, UnionFind, in_plane, welded};

/// A field of space: negative on one side of the cutter, positive on the other.
type Field = Box<dyn Fn(Vec3) -> f64 + Send + Sync>;

/// Whether a place on a cut lies on the cutter itself, within a slack (mm), and not where its
/// field only runs on past its sides.
type Covers = Box<dyn Fn(Vec3, f64) -> bool + Send + Sync>;

/// How far from a cutter that ends a crossing may lie and still be on it (mm).
const ON_THE_CUTTER: f64 = 0.01;

/// What a cutter's two sides are called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    /// A plane or a surface: behind it and in front (where its normal points).
    BehindInFront,
    /// A closed shape, body or outline: inside and outside.
    InsideOutside,
    /// An open curve: to its right and to its left, seen from the way the scan faces.
    RightLeft,
    /// Triangles chosen: the selected and the rest.
    SelectedRest,
}

impl Sides {
    /// The negative side's name and the positive side's.
    pub fn names(self) -> (&'static str, &'static str) {
        match self {
            Sides::BehindInFront => ("behind", "in_front"),
            Sides::InsideOutside => ("inside", "outside"),
            Sides::RightLeft => ("right", "left"),
            Sides::SelectedRest => ("selected", "rest"),
        }
    }
}

/// Why a scan cannot be cut.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitError {
    /// The scan has no triangles.
    Empty,
    /// The cutter is no number at a point of the scan (a body with no triangles near it).
    Undefined,
    /// The curve has too few points, or none apart.
    CurveTooShort,
    /// The scan under the curve has no way it faces.
    NoFacing,
    /// A closed curve goes too far round the scan to be seen across from one side.
    CurveAround,
    /// The scan under an open curve faces along it, so it has no side to cut from.
    CurveAlong,
    /// A closed curve encloses nothing seen across from the scan.
    EnclosesNothing,
    /// A body or a sketch with nothing to cut with.
    NothingToCutWith(String),
    /// The cutter misses the scan: every triangle is on one side.
    Misses,
    /// The cut does not divide the scan: it slits it, and all of it stays on one side.
    Slit,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::Empty => write!(f, "the scan has no triangles"),
            SplitError::Undefined => write!(f, "the cutter has no side at some point of the scan"),
            SplitError::CurveTooShort => write!(f, "the curve is too short to cut with"),
            SplitError::NoFacing => write!(f, "the scan under the curve has no way it faces"),
            SplitError::CurveAround => write!(
                f,
                "the curve goes too far round the scan to be seen across from one side: cut with a curve the scan faces across, or with a surface"
            ),
            SplitError::CurveAlong => write!(f, "the scan under the curve faces along it, so the curve has no side to cut from"),
            SplitError::EnclosesNothing => write!(f, "the curve encloses nothing seen across from the scan"),
            SplitError::NothingToCutWith(why) => write!(f, "{why}"),
            SplitError::Misses => write!(f, "the cutter does not cut the scan: all of it lies on one side"),
            SplitError::Slit => write!(
                f,
                "the cut does not divide the scan: it only slits it, so all of it stays on one side; carry the cutter right across, or give the curve a longer reach"
            ),
        }
    }
}

impl std::error::Error for SplitError {}

/// What cuts: a field of space and, for a cutter that ends, where it does.
pub struct Cutter {
    value: Field,
    covers: Option<Covers>,
    pub sides: Sides,
}

impl std::fmt::Debug for Cutter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cutter").field("sides", &self.sides).field("ends", &self.covers.is_some()).finish()
    }
}

impl Cutter {
    /// The cutter's value at `p`: negative on one side, positive on the other.
    pub fn value(&self, p: Vec3) -> f64 {
        (self.value)(p)
    }

    /// Whether the cutter ends (a run of a cut never near it is no cut).
    pub fn ends(&self) -> bool {
        self.covers.is_some()
    }

    /// A plane or a fitted shape, which goes on for ever: its signed distance (a plane's front
    /// is where its normal points, a round shape's outside is positive).
    pub fn shape(shape: Shape) -> Cutter {
        let sides = if matches!(shape, Shape::Plane { .. }) { Sides::BehindInFront } else { Sides::InsideOutside };
        Cutter { value: Box::new(move |p| shape.distance(p)), covers: None, sides }
    }

    /// A mesh (a body's surface, another scan) as a cutter: the signed distance to its nearest
    /// place, outside positive by its winding, so a solid cuts into inside and outside and an open
    /// surface into behind and in front. It ends where the mesh does.
    pub fn mesh(positions: &[Vec3], triangles: &[[u32; 3]]) -> Result<Cutter, SplitError> {
        let normals = PseudoNormals::new(positions, triangles);
        if normals.faces.is_empty() {
            return Err(SplitError::NothingToCutWith("the cutting body has no surface".into()));
        }
        let closed = normals.closed;
        let mesh = Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), ..Mesh::default() };
        let tree = std::sync::Arc::new(TriangleTree::new(&mesh));
        let near = tree.clone();
        let value = Box::new(move |p: Vec3| {
            let Some((distance, t)) = near.nearest(p) else { return f64::NAN };
            match normals.side(p, t as usize) {
                Some(s) => s * distance,
                None => distance,
            }
        });
        let covers: Covers = Box::new(move |p, slack| tree.nearest(p).is_some_and(|(d, _)| d <= ON_THE_CUTTER + slack));
        Ok(Cutter { value, covers: Some(covers), sides: if closed { Sides::InsideOutside } else { Sides::BehindInFront } })
    }

    /// A 3D curve lying on the scan, carried straight through space square to a plane of its own:
    /// a closed curve the plane it lies nearest, turned to face `facing` (the way the scan faces
    /// under it), cutting into inside and outside; an open one the plane along it that `facing`
    /// is most square to, carried on past its ends by `reach` (at least 1 mm), cutting into right
    /// and left.
    pub fn curve(points: &[Vec3], closed: bool, facing: Vec3, reach: f64) -> Result<Cutter, SplitError> {
        let mut path: Vec<Vec3> = Vec::new();
        for &p in points {
            if p.is_finite() && path.last().is_none_or(|&last| last.dist(p) > 1e-9) {
                path.push(p);
            }
        }
        if closed && path.len() > 1 && path[0].dist(path[path.len() - 1]) < 1e-6 {
            path.pop();
        }
        if path.len() < if closed { 3 } else { 2 } {
            return Err(SplitError::CurveTooShort);
        }
        let look = facing.normalized().ok_or(SplitError::NoFacing)?;
        let centre = path.iter().fold(Vec3::ZERO, |s, p| s + *p) / path.len() as f64;
        let mut spread = [[0.0f64; 3]; 3];
        for p in &path {
            let d = *p - centre;
            let d = [d.x, d.y, d.z];
            for (row, di) in spread.iter_mut().zip(d) {
                for (cell, dj) in row.iter_mut().zip(d) {
                    *cell += di * dj;
                }
            }
        }
        // Least spread first.
        let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0).ok_or(SplitError::CurveTooShort)?;
        let normal = if closed {
            let n = axes[0].normalized().ok_or(SplitError::CurveTooShort)?;
            let n = if n.dot(look) < 0.0 { -n } else { n };
            if !(n.dot(look) >= 0.5) {
                return Err(SplitError::CurveAround);
            }
            n
        } else {
            let along = axes[2].normalized().ok_or(SplitError::CurveTooShort)?;
            let n = look - along * look.dot(along);
            if !(n.len() > 0.3) {
                return Err(SplitError::CurveAlong);
            }
            n.normalized().ok_or(SplitError::CurveAlong)?
        };
        let (u, v) = in_plane(normal);
        let flat = move |p: Vec3| Vec2::new((p - centre).dot(u), (p - centre).dot(v));
        let line: Vec<Vec2> = path.iter().map(|&p| flat(p)).collect();
        if closed {
            if !(loop_area(&line).abs() > 1e-6) {
                return Err(SplitError::EnclosesNothing);
            }
            return Ok(Cutter { value: Box::new(move |p| signed_to_loop(&line, flat(p))), covers: None, sides: Sides::InsideOutside });
        }
        let line = extended(&line, reach.max(1.0));
        if line.len() < 2 {
            return Err(SplitError::CurveTooShort);
        }
        Ok(Cutter { value: Box::new(move |p| open_field(&line, flat(p))), covers: None, sides: Sides::RightLeft })
    }

    /// A sketch's outlines carried straight through space, square to its plane (`origin`, the
    /// unit `x` and `y`): its closed regions cut into inside and outside; with none, one open
    /// curve (`open`, in the plane) cuts into right and left, carried on past its ends by `reach`
    /// (at least 1 mm).
    pub fn sketch(origin: Vec3, x: Vec3, y: Vec3, regions: &[Region2], open: Option<&[Vec2]>, reach: f64) -> Result<Cutter, SplitError> {
        let flat = move |p: Vec3| Vec2::new((p - origin).dot(x), (p - origin).dot(y));
        if !regions.is_empty() {
            let loops: Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)> =
                regions.iter().map(|r| (sampled(&r.outer), r.holes.iter().map(sampled).collect())).filter(|(o, _)| o.len() >= 3).collect();
            if loops.is_empty() {
                return Err(SplitError::NothingToCutWith("the sketch's outlines enclose nothing".into()));
            }
            let value = Box::new(move |p: Vec3| {
                let q = flat(p);
                loops.iter().fold(f64::INFINITY, |best, (outer, holes)| {
                    let d = holes.iter().fold(signed_to_loop(outer, q), |d, hole| d.max(-signed_to_loop(hole, q)));
                    best.min(d)
                })
            });
            return Ok(Cutter { value, covers: None, sides: Sides::InsideOutside });
        }
        let line =
            open.ok_or_else(|| SplitError::NothingToCutWith("the sketch has no curve to cut with: draw lines or arcs, or a closed loop".into()))?;
        let line = extended(line, reach.max(1.0));
        if line.len() < 2 {
            return Err(SplitError::CurveTooShort);
        }
        Ok(Cutter { value: Box::new(move |p| open_field(&line, flat(p))), covers: None, sides: Sides::RightLeft })
    }

    /// The same cutter moved `by` to its positive side (exact for a cutter that is a signed
    /// distance).
    pub fn offset(self, by: f64) -> Cutter {
        let value = self.value;
        let covers = self.covers.map(|c| -> Covers { Box::new(move |p, slack| c(p, slack + by.abs())) });
        Cutter { value: Box::new(move |p| value(p) - by), covers, sides: self.sides }
    }

    /// The cutter held to a slab: it ends `reach` either side of the plane through `point` square
    /// to the unit `normal` (a sketch's outlines, carried only so far).
    pub fn slab(mut self, point: Vec3, normal: Vec3, reach: f64) -> Cutter {
        self.covers = Some(Box::new(move |p, slack| (p - point).dot(normal).abs() <= reach + slack));
        self
    }

    /// The cutter as a ribbon: it ends `reach` from `path`, so a run of the cut that never comes
    /// that near the curve is no cut, however far the field itself goes on.
    pub fn ribbon(mut self, path: &[Vec3], reach: f64) -> Cutter {
        let path = path.to_vec();
        self.covers = Some(Box::new(move |p, slack| {
            let near = path.windows(2).fold(f64::INFINITY, |best, w| best.min(p.dist_to_segment(w[0], w[1])));
            near.min(path.first().map_or(f64::INFINITY, |q| q.dist(p))) <= reach + slack
        }));
        self
    }
}

/// A scan cut: its own vertices first, then those the cut made, every triangle (whole ones in
/// their old order, the pieces of a clipped one where it was, wound as it was) and the side of
/// each.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// -1 or +1, the cutter's negative or positive side; 0 between the two cuts of an envelope.
    pub side: Vec<i8>,
    /// The triangle of the scan each is, or is a piece of.
    pub origin: Vec<u32>,
    /// Triangles clipped into pieces on both sides, and vertices the cut added.
    pub crossed: usize,
    pub added: usize,
    /// The new edge as pieces, and its length.
    pub segments: Vec<[Vec3; 2]>,
    pub length: f64,
    /// How far the middle of a piece of the new edge is from the cutter, at most.
    pub chord_error: f64,
    /// The cut goes through a piece without dividing it: it slit it, and left it in one piece.
    pub slit: bool,
    /// The cutter stops short of the part: the cut carries on past its side to finish.
    pub carried_on: bool,
}

impl Cut {
    /// How many triangles are on `side`.
    pub fn count(&self, side: i8) -> usize {
        self.side.iter().filter(|&&s| s == side).count()
    }

    /// The mesh of the triangles on `sides`, the vertices they use kept in their order.
    pub fn keep(&self, sides: &[i8]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let kept: Vec<usize> = (0..self.side.len()).filter(|&t| sides.contains(&self.side[t])).collect();
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        let mut triangles = Vec::with_capacity(kept.len());
        for t in kept {
            let tri = self.triangles[t].map(|i| {
                let slot = &mut remap[i as usize];
                if *slot == u32::MAX {
                    *slot = positions.len() as u32;
                    positions.push(self.positions[i as usize]);
                }
                *slot
            });
            triangles.push(tri);
        }
        (positions, triangles)
    }

    /// The runs of the new edge, end to end, and whether each closes on itself.
    pub fn runs(&self) -> Vec<(Vec<Vec3>, bool)> {
        chained(&self.segments)
    }

    /// This cut followed by `next`, made on all of what this one left, in its order: the two cuts
    /// of an envelope. Triangles past the first are on its side (+1), those short of the second
    /// on its side (-1), and those between are 0, a slab of their own.
    fn followed_by(self, next: Cut) -> Cut {
        let side = next
            .origin
            .iter()
            .zip(&next.side)
            .map(|(&o, &s)| {
                if self.side.get(o as usize).is_some_and(|&f| f > 0) {
                    1
                } else if s < 0 {
                    -1
                } else {
                    0
                }
            })
            .collect();
        let origin = next.origin.iter().map(|&o| self.origin.get(o as usize).copied().unwrap_or(o)).collect();
        let mut segments = self.segments;
        segments.extend(next.segments);
        Cut {
            positions: next.positions,
            triangles: next.triangles,
            side,
            origin,
            crossed: self.crossed + next.crossed,
            added: self.added + next.added,
            segments,
            length: self.length + next.length,
            chord_error: self.chord_error.max(next.chord_error),
            slit: self.slit || next.slit,
            carried_on: self.carried_on || next.carried_on,
        }
    }
}

/// The scan split by `cutter`: cut once, or with an `envelope` of that thickness twice, half of
/// it either side, the slab between a side of its own (0). `facing`, when given, holds the cut to
/// the triangles facing that way; the others are given the new vertices on their edges but keep
/// to the side they are mostly on.
pub fn split(scan: &Scan, cutter: Cutter, envelope: f64, facing: Option<Vec3>) -> Result<Cut, SplitError> {
    let made = if envelope > 0.0 && envelope.is_finite() {
        let half = envelope / 2.0;
        let sides = cutter.sides;
        let shared = std::sync::Arc::new(cutter);
        let (a, b) = (shared.clone(), shared.clone());
        let ends = shared.covers.is_some();
        let wrap = |c: std::sync::Arc<Cutter>, by: f64| Cutter {
            value: Box::new({
                let c = c.clone();
                move |p| c.value(p) - by
            }),
            covers: ends.then(|| -> Covers { Box::new(move |p, slack| c.covers.as_ref().is_some_and(|f| f(p, slack + by.abs()))) }),
            sides,
        };
        let first = cut(scan, &wrap(a, half), facing)?;
        let next = {
            let left = Scan::new(&first.positions, &first.triangles);
            cut(&left, &wrap(b, -half), facing)?
        };
        first.followed_by(next)
    } else {
        cut(scan, &cutter, facing)?
    };
    let ones = [made.count(-1), made.count(0), made.count(1)];
    if ones.iter().filter(|&&n| n > 0).count() < 2 {
        return Err(if made.slit { SplitError::Slit } else { SplitError::Misses });
    }
    // A cutter lying wholly on a face of the scan crosses nothing: that is no cut.
    if made.segments.is_empty() {
        return Err(SplitError::Misses);
    }
    Ok(made)
}

/// The scan parted along the edge of the triangles `inside` (one flag a triangle): those are the
/// negative side, the rest the positive; no triangle is clipped. The edge between them is the cut.
pub fn by_triangles(scan: &Scan, inside: &[bool]) -> Result<Cut, SplitError> {
    if scan.triangles.is_empty() {
        return Err(SplitError::Empty);
    }
    let side: Vec<i8> = (0..scan.triangles.len()).map(|t| if inside.get(t).copied().unwrap_or(false) { -1 } else { 1 }).collect();
    let rep = welded(scan.positions);
    let key = |a: u32, b: u32| {
        let (ra, rb) = (rep[a as usize], rep[b as usize]);
        (ra.min(rb), ra.max(rb))
    };
    // An edge with a triangle on each side is the cut.
    let mut first: HashMap<(u32, u32), (usize, u32, u32)> = HashMap::with_capacity(scan.triangles.len() * 2);
    let mut segments = Vec::new();
    for (t, tri) in scan.triangles.iter().enumerate() {
        if tri.iter().any(|&v| v as usize >= scan.positions.len()) {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            match first.get(&key(a, b)) {
                Some(&(other, _, _)) if side[other] != side[t] => segments.push([scan.positions[a as usize], scan.positions[b as usize]]),
                Some(_) => {}
                None => {
                    first.insert(key(a, b), (t, a, b));
                }
            }
        }
    }
    if side.iter().all(|&s| s == side[0]) || segments.is_empty() {
        return Err(SplitError::Misses);
    }
    let length = segments.iter().map(|s| s[0].dist(s[1])).sum();
    Ok(Cut {
        positions: scan.positions.to_vec(),
        triangles: scan.triangles.to_vec(),
        side,
        origin: (0..scan.triangles.len() as u32).collect(),
        crossed: 0,
        added: 0,
        segments,
        length,
        chord_error: 0.0,
        slit: false,
        carried_on: false,
    })
}

/// The scan clipped along `cutter`, every triangle on one side or the other.
pub fn cut(scan: &Scan, cutter: &Cutter, facing: Option<Vec3>) -> Result<Cut, SplitError> {
    let n = scan.positions.len();
    if n == 0 || scan.triangles.is_empty() {
        return Err(SplitError::Empty);
    }
    let mut p: Vec<Vec3> = scan.positions.to_vec();
    let eps = 1e-7 * scan.extent();
    let mut f: Vec<f64> = crate::par::map(n, |v| if p[v].is_finite() { cutter.value(p[v]) } else { 0.0 });
    for (v, x) in f.iter_mut().enumerate() {
        if !x.is_finite() && p[v].is_finite() {
            return Err(SplitError::Undefined);
        }
        if x.abs() <= eps || !x.is_finite() {
            *x = 0.0;
        }
    }
    let rep = welded(scan.positions);
    let key = |a: u32, b: u32| {
        let (ra, rb) = (rep.get(a as usize).copied().unwrap_or(a), rep.get(b as usize).copied().unwrap_or(b));
        (u64::from(ra.min(rb)) << 32) | u64::from(ra.max(rb))
    };
    let mut created: HashMap<u64, u32> = HashMap::new();
    // The place on the edge from a to b where the cutter is zero, found on the edge itself so the
    // new vertex is on the old surface: secant steps, bisection every third or when one strays.
    let mut crossing = |a: u32, b: u32, p: &mut Vec<Vec3>, f: &mut Vec<f64>| -> u32 {
        let k = key(a, b);
        if let Some(&known) = created.get(&k) {
            return known;
        }
        let (pa, pb) = (p[a as usize], p[b as usize]);
        let (mut lo, mut hi, mut flo, mut fhi) = (0.0, 1.0, f[a as usize], f[b as usize]);
        let mut t = flo / (flo - fhi);
        for step in 0..64 {
            let fq = cutter.value(pa.lerp(pb, t));
            if fq.abs() <= 1e-3 * eps || !fq.is_finite() {
                break;
            }
            if (fq < 0.0) == (flo < 0.0) {
                (lo, flo) = (t, fq);
            } else {
                (hi, fhi) = (t, fq);
            }
            if hi - lo < 1e-15 {
                break;
            }
            let secant = lo - flo * (hi - lo) / (fhi - flo);
            t = if step % 3 == 2 || !(secant > lo && secant < hi) { (lo + hi) / 2.0 } else { secant };
        }
        p.push(pa.lerp(pb, t));
        f.push(0.0);
        let id = (p.len() - 1) as u32;
        created.insert(k, id);
        id
    };

    let count = scan.triangles.len();
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(count + count / 8);
    let mut side: Vec<i8> = Vec::with_capacity(count + count / 8);
    let mut origin: Vec<u32> = Vec::with_capacity(count + count / 8);
    let (mut crossed, mut segments, mut length, mut chord_error) = (0usize, Vec::new(), 0.0, 0.0f64);
    // Edges lying on the cutter itself (both ends at zero), with whether a triangle on each side
    // has one, in the order first met.
    let mut flat: Vec<(u32, u32, bool, bool)> = Vec::new();
    let mut flat_at: HashMap<u64, usize> = HashMap::new();
    let emit = |polygon: &[u32], s: i8, t: usize, p: &[Vec3], triangles: &mut Vec<[u32; 3]>, side: &mut Vec<i8>, origin: &mut Vec<u32>| {
        let mut put = |a: u32, b: u32, c: u32| {
            if a != b && b != c && a != c {
                triangles.push([a, b, c]);
                side.push(s);
                origin.push(t as u32);
            }
        };
        match polygon {
            [a, b, c] => put(*a, *b, *c),
            // The shorter diagonal makes the better triangles.
            [a, b, c, d] => {
                if p[*a as usize].dist(p[*c as usize]) <= p[*b as usize].dist(p[*d as usize]) {
                    put(*a, *b, *c);
                    put(*a, *c, *d);
                } else {
                    put(*b, *c, *d);
                    put(*b, *d, *a);
                }
            }
            [first, rest @ ..] if rest.len() >= 2 => {
                for w in rest.windows(2) {
                    put(*first, w[0], w[1]);
                }
            }
            _ => {}
        }
    };
    for (t, tri) in scan.triangles.iter().enumerate() {
        let v = *tri;
        if v.iter().any(|&i| i as usize >= n) {
            continue;
        }
        let d = v.map(|i| f[i as usize]);
        let (any_positive, any_negative) = (d.iter().any(|&x| x > 0.0), d.iter().any(|&x| x < 0.0));
        if !(any_positive && any_negative) {
            // Not crossed: kept as it is, even a degenerate one.
            let s: i8 = if any_negative { -1 } else { 1 };
            let zeros: Vec<usize> = (0..3).filter(|&k| d[k] == 0.0).collect();
            if let ([z0, z1], Some(third)) = (zeros.as_slice(), (0..3).find(|&k| d[k] != 0.0)) {
                let (a, b) = (v[*z0], v[*z1]);
                let slot = *flat_at.entry(key(a, b)).or_insert_with(|| {
                    flat.push((a, b, false, false));
                    flat.len() - 1
                });
                if d[third] > 0.0 {
                    flat[slot].2 = true;
                } else {
                    flat[slot].3 = true;
                }
            }
            triangles.push(v);
            side.push(s);
            origin.push(t as u32);
            continue;
        }
        let [a, b, c] = v.map(|i| p[i as usize]);
        let mostly: i8 = if cutter.value((a + b + c) / 3.0) <= 0.0 { -1 } else { 1 };
        let cuttable = facing.is_none_or(|w| (b - a).cross(c - a).dot(w) > 0.0);
        let (mut up, mut down, mut zeros) = (Vec::with_capacity(4), Vec::with_capacity(4), Vec::with_capacity(2));
        for k in 0..3 {
            let (from, to, da, db) = (v[k], v[(k + 1) % 3], d[k], d[(k + 1) % 3]);
            if da >= 0.0 {
                up.push(from);
            }
            if da <= 0.0 {
                down.push(from);
            }
            if da == 0.0 {
                zeros.push(from);
            }
            if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                // The same two places in the same order from either side of the edge.
                let (ra, rb) = (rep[from as usize], rep[to as usize]);
                let m = if ra <= rb { crossing(from, to, &mut p, &mut f) } else { crossing(to, from, &mut p, &mut f) };
                up.push(m);
                down.push(m);
                zeros.push(m);
            }
        }
        if cuttable {
            crossed += 1;
            emit(&up, 1, t, &p, &mut triangles, &mut side, &mut origin);
            emit(&down, -1, t, &p, &mut triangles, &mut side, &mut origin);
            if let [z0, z1] = zeros.as_slice() {
                let (q0, q1) = (p[*z0 as usize], p[*z1 as usize]);
                segments.push([q0, q1]);
                length += q0.dist(q1);
                chord_error = chord_error.max(cutter.value((q0 + q1) / 2.0).abs());
            }
        } else {
            emit(&up, mostly, t, &p, &mut triangles, &mut side, &mut origin);
            emit(&down, mostly, t, &p, &mut triangles, &mut side, &mut origin);
        }
    }
    for &(a, b, positive, negative) in &flat {
        if positive && negative {
            let (q0, q1) = (p[a as usize], p[b as usize]);
            segments.push([q0, q1]);
            length += q0.dist(q1);
        }
    }
    let made =
        Cut { added: p.len() - n, positions: p, triangles, side, origin, crossed, segments, length, chord_error, slit: false, carried_on: false };
    Ok(match &cutter.covers {
        Some(covers) => held_to(made, covers, &rep),
        None => made,
    })
}

/// The cut as a cutter that ends: a run of the new edge that touches the cutter is a cut (and may
/// carry on past its side to finish); a run that never touches it is only where its field runs
/// on, and is no cut. The triangles either side of such a run are joined, and each piece so made
/// goes to the side it is mostly on. A piece with a cut inside it is slit, not divided.
fn held_to(mut cut: Cut, covers: &Covers, rep: &[u32]) -> Cut {
    let bits = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    let mut past: HashSet<[u64; 3]> = HashSet::new();
    for (points, _) in chained(&cut.segments) {
        let on = points.iter().filter(|p| covers(**p, 0.0)).count();
        if on == 0 {
            past.extend(points.iter().map(|p| bits(*p)));
        } else if on < points.len() {
            cut.carried_on = true;
        }
    }
    if past.is_empty() {
        return cut;
    }
    let of = |v: u32| rep.get(v as usize).copied().unwrap_or(v);
    let count = cut.side.len();
    let mut joined = UnionFind::new(count);
    let mut first_on: HashMap<(u32, u32), usize> = HashMap::with_capacity(count * 2);
    let mut across: Vec<(usize, usize)> = Vec::new();
    for t in 0..count {
        for k in 0..3 {
            let (a, b) = (cut.triangles[t][k], cut.triangles[t][(k + 1) % 3]);
            let (ra, rb) = (of(a), of(b));
            let edge = (ra.min(rb), ra.max(rb));
            let Some(&other) = first_on.get(&edge) else {
                first_on.insert(edge, t);
                continue;
            };
            let beyond = past.contains(&bits(cut.positions[a as usize])) && past.contains(&bits(cut.positions[b as usize]));
            if cut.side[t] != cut.side[other] && !beyond {
                across.push((t, other));
            } else {
                joined.join(t, other);
            }
        }
    }
    let mut balance = vec![0i64; count];
    for t in 0..count {
        balance[joined.find(t)] += i64::from(cut.side[t]);
    }
    for t in 0..count {
        cut.side[t] = if balance[joined.find(t)] < 0 { -1 } else { 1 };
    }
    cut.slit = across.iter().any(|&(a, b)| joined.find(a) == joined.find(b));
    cut.segments.retain(|s| !past.contains(&bits(s[0])));
    cut.length = cut.segments.iter().map(|s| s[0].dist(s[1])).sum();
    cut.crossed = cut.segments.len();
    cut
}

/// The pieces of a cut's edge joined end to end: each run, and whether it closes on itself.
pub fn chained(segments: &[[Vec3; 2]]) -> Vec<(Vec<Vec3>, bool)> {
    let key = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    let mut at: HashMap<[u64; 3], Vec<usize>> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        at.entry(key(s[0])).or_default().push(i);
        at.entry(key(s[1])).or_default().push(i);
    }
    let ends_at = |p: Vec3| at.get(&key(p)).map_or(0, Vec::len);
    let mut used = vec![false; segments.len()];
    let mut runs = Vec::new();
    // Runs that end first, from their ends; what is left closes.
    let ends: Vec<usize> = (0..segments.len()).filter(|&i| ends_at(segments[i][0]) == 1 || ends_at(segments[i][1]) == 1).collect();
    for start in ends.into_iter().chain(0..segments.len()) {
        if used[start] {
            continue;
        }
        used[start] = true;
        let [a, b] = segments[start];
        let (first, mut tip) = if ends_at(a) == 1 { (a, b) } else { (b, a) };
        let mut points = vec![first, tip];
        while let Some(&next) = at.get(&key(tip)).and_then(|list| list.iter().find(|&&i| !used[i])) {
            used[next] = true;
            tip = if key(segments[next][0]) == key(tip) { segments[next][1] } else { segments[next][0] };
            points.push(tip);
        }
        let closed = points.len() > 3 && key(points[0]) == key(tip);
        if closed {
            points.pop();
        }
        runs.push((points, closed));
    }
    runs
}

/// A mesh's angle-weighted pseudo-normals (faces, edges, vertices), which tell inside from outside
/// at the nearest place on it whether that is inside a face, on an edge or at a corner.
struct PseudoNormals {
    corners: Vec<[Vec3; 3]>,
    faces: Vec<Vec3>,
    welded: Vec<[u32; 3]>,
    vertex: HashMap<u32, Vec3>,
    edge: HashMap<(u32, u32), Vec3>,
    closed: bool,
}

impl PseudoNormals {
    fn new(positions: &[Vec3], triangles: &[[u32; 3]]) -> PseudoNormals {
        let rep = welded(positions);
        let (mut corners, mut faces, mut welded_tris) = (Vec::new(), Vec::new(), Vec::new());
        let mut vertex: HashMap<u32, Vec3> = HashMap::new();
        let mut edge: HashMap<(u32, u32), Vec3> = HashMap::new();
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in triangles {
            let Some(c) = tri.iter().map(|&i| positions.get(i as usize).copied().filter(|p| p.is_finite())).collect::<Option<Vec<Vec3>>>() else {
                corners.push([Vec3::ZERO; 3]);
                faces.push(Vec3::ZERO);
                welded_tris.push([u32::MAX; 3]);
                continue;
            };
            let w = tri.map(|i| rep[i as usize]);
            let n = (c[1] - c[0]).cross(c[2] - c[0]).normalized().unwrap_or(Vec3::ZERO);
            for k in 0..3 {
                let (prev, here, next) = (c[(k + 2) % 3], c[k], c[(k + 1) % 3]);
                let angle = match ((prev - here).normalized(), (next - here).normalized()) {
                    (Some(a), Some(b)) => a.dot(b).clamp(-1.0, 1.0).acos(),
                    _ => 0.0,
                };
                *vertex.entry(w[k]).or_insert(Vec3::ZERO) += n * angle;
                let e = (w[k].min(w[(k + 1) % 3]), w[k].max(w[(k + 1) % 3]));
                *edge.entry(e).or_insert(Vec3::ZERO) += n;
                *uses.entry(e).or_insert(0) += 1;
            }
            corners.push([c[0], c[1], c[2]]);
            faces.push(n);
            welded_tris.push(w);
        }
        let closed = !uses.is_empty() && uses.values().all(|&u| u == 2);
        PseudoNormals { corners, faces, welded: welded_tris, vertex, edge, closed }
    }

    /// The side of `p` from its nearest place on triangle `t`: -1 behind (inside), +1 in front,
    /// `None` on the surface.
    fn side(&self, p: Vec3, t: usize) -> Option<f64> {
        let ([a, b, c], w) = (*self.corners.get(t)?, *self.welded.get(t)?);
        let q = closest_on_triangle(p, a, b, c);
        let off = p - q;
        if !(off.len() > 0.0) {
            return None;
        }
        // Where q lies: a corner, an edge, or inside the face (barycentric weights near zero).
        let (e0, e1, d) = (b - a, c - a, q - a);
        let (d00, d01, d11, d20, d21) = (e0.dot(e0), e0.dot(e1), e1.dot(e1), d.dot(e0), d.dot(e1));
        let denom = d00 * d11 - d01 * d01;
        let normal = if denom > 0.0 {
            let v = (d11 * d20 - d01 * d21) / denom;
            let wt = (d00 * d21 - d01 * d20) / denom;
            let weights = [1.0 - v - wt, v, wt];
            let tiny = 1e-9;
            let small: Vec<usize> = (0..3).filter(|&k| weights[k] < tiny).collect();
            match small.as_slice() {
                // Two weights at zero: at the third corner.
                [i, j] => {
                    let k = 3 - i - j;
                    self.vertex.get(&w[k]).copied()
                }
                // One at zero: on the edge between the other two.
                [i] => {
                    let (x, y) = (w[(i + 1) % 3], w[(i + 2) % 3]);
                    self.edge.get(&(x.min(y), x.max(y))).copied()
                }
                _ => self.faces.get(t).copied(),
            }
        } else {
            self.faces.get(t).copied()
        };
        let n = normal.filter(|n| n.len() > 0.0).or_else(|| self.faces.get(t).copied())?;
        Some(if off.dot(n) < 0.0 { -1.0 } else { 1.0 })
    }
}

/// Points along a sketch loop, each segment by its own measure (a line its ends, an arc by its
/// sweep, a curve by enough steps).
fn sampled(lp: &solidcore_record::Loop2) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::new();
    for seg in &lp.segs {
        let steps = match seg {
            Seg2::Line { .. } => 1,
            Seg2::Arc { sweep, .. } => ((sweep.abs() / std::f64::consts::TAU * 96.0).ceil() as usize).clamp(4, 96),
            _ => 32,
        };
        for k in 0..steps {
            let q = seg.point_at(k as f64 / steps as f64);
            if out.last().is_none_or(|last| last.dist(q) > 1e-12) {
                out.push(q);
            }
        }
    }
    if out.len() > 1 && out[0].dist(out[out.len() - 1]) < 1e-9 {
        out.pop();
    }
    out
}

/// Twice the signed area of a closed loop (counter-clockwise positive).
fn loop_area(lp: &[Vec2]) -> f64 {
    (0..lp.len()).map(|i| lp[i].cross(lp[(i + 1) % lp.len()])).sum::<f64>() / 2.0
}

/// Whether `p` is inside the closed loop (even-odd).
fn inside(lp: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    for i in 0..lp.len() {
        let (a, b) = (lp[i], lp[(i + 1) % lp.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            inside = !inside;
        }
    }
    inside
}

/// The distance from `p` to the closed loop, negative inside it.
fn signed_to_loop(lp: &[Vec2], p: Vec2) -> f64 {
    let mut best = f64::INFINITY;
    for i in 0..lp.len() {
        best = best.min(to_segment(p, lp[i], lp[(i + 1) % lp.len()]));
    }
    if inside(lp, p) { -best } else { best }
}

fn to_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let along = b - a;
    let l2 = along.dot(along);
    let t = if l2 > 0.0 { ((p - a).dot(along) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a.lerp(b, t))
}

/// The distance from `p` to an open curve: positive on its left, negative on its right; at a
/// corner, the side is told by the bisector of the two pieces that meet there.
fn open_field(line: &[Vec2], p: Vec2) -> f64 {
    let (mut best, mut sign) = (f64::INFINITY, 1.0);
    for i in 0..line.len().saturating_sub(1) {
        let (a, b) = (line[i], line[i + 1]);
        let along = b - a;
        let l2 = along.dot(along);
        if !(l2 > 0.0) {
            continue;
        }
        let t = ((p - a).dot(along) / l2).clamp(0.0, 1.0);
        let d = p.dist(a.lerp(b, t));
        if !(d < best) {
            continue;
        }
        best = d;
        let left = Vec2::new(-along.y, along.x) / l2.sqrt();
        sign = if t > 0.0 && t < 1.0 {
            if (p - a).dot(left) >= 0.0 { 1.0 } else { -1.0 }
        } else {
            // At a corner: the normals of its two pieces added.
            let corner = if t == 0.0 { i } else { i + 1 };
            let other = if t == 0.0 { i.checked_sub(1) } else { (i + 2 < line.len()).then_some(i + 1) };
            let mut n = left;
            if let Some(o) = other {
                let oa = line[o + 1] - line[o];
                if oa.len() > 0.0 {
                    n += Vec2::new(-oa.y, oa.x) / oa.len();
                }
            }
            if (p - line[corner]).dot(n) >= 0.0 { 1.0 } else { -1.0 }
        };
    }
    sign * best
}

/// The curve (repeated points dropped) with each end carried on `distance` the way it ended.
fn extended(points: &[Vec2], distance: f64) -> Vec<Vec2> {
    let mut line: Vec<Vec2> = Vec::new();
    for &p in points {
        if line.last().is_none_or(|last| last.dist(p) > 1e-12) {
            line.push(p);
        }
    }
    if line.len() < 2 {
        return line;
    }
    let (Some(first), Some(last)) = ((line[1] - line[0]).normalized(), (line[line.len() - 1] - line[line.len() - 2]).normalized()) else {
        return line;
    };
    let mut out = Vec::with_capacity(line.len() + 2);
    out.push(line[0] - first * distance);
    out.extend_from_slice(&line);
    out.push(line[line.len() - 1] + last * distance);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A box 40 × 30 × 20 of 20 × 20 squares a side.
    fn block() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        fine_cuboid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0), 20)
    }

    fn plane(z: f64) -> Cutter {
        Cutter::shape(Shape::Plane { point: Vec3::new(0.0, 0.0, z), normal: Vec3::Z })
    }

    #[test]
    fn a_plane_cuts_a_box_into_two_halves_along_one_closed_run() {
        let (p, t) = block();
        let cut = split(&Scan::new(&p, &t), plane(7.3), 0.0, None).unwrap();
        let runs = cut.runs();
        assert_eq!(runs.len(), 1, "one run round the box");
        assert!(runs[0].1, "it closes");
        assert!((cut.length - 140.0).abs() < 1e-6, "the box's girth: {}", cut.length);
        assert!(cut.segments.iter().flatten().all(|q| (q.z - 7.3).abs() < 1e-9), "the new edge lies on the plane");
        assert!(!cut.slit && !cut.carried_on);
        let (above, top) = cut.keep(&[1]);
        let (below, bottom) = cut.keep(&[-1]);
        assert!(above.iter().all(|q| q.z >= 7.3 - 1e-9) && below.iter().all(|q| q.z <= 7.3 + 1e-9));
        assert!(!top.is_empty() && !bottom.is_empty());
        // Each side's triangles add up to the box's area on that side.
        let area = |p: &[Vec3], t: &[[u32; 3]]| -> f64 {
            t.iter().map(|&[a, b, c]| (p[b as usize] - p[a as usize]).cross(p[c as usize] - p[a as usize]).len() / 2.0).sum()
        };
        assert!((area(&above, &top) - (1200.0 + 140.0 * 12.7)).abs() < 1e-6);
        assert!((area(&below, &bottom) - (1200.0 + 140.0 * 7.3)).abs() < 1e-6);
    }

    #[test]
    fn an_envelope_leaves_a_slab_of_its_own_between_two_cuts() {
        let (p, t) = block();
        let cut = split(&Scan::new(&p, &t), plane(10.0), 1.0, None).unwrap();
        assert!(cut.count(-1) > 0 && cut.count(0) > 0 && cut.count(1) > 0);
        let (slab, _) = cut.keep(&[0]);
        assert!(slab.iter().all(|q| (q.z - 10.0).abs() <= 0.5 + 1e-9), "the slab is within half the envelope either side");
        assert_eq!(cut.runs().len(), 2, "a run at each side of the slab");
    }

    #[test]
    fn a_plane_past_the_scan_misses_it() {
        let (p, t) = block();
        assert_eq!(split(&Scan::new(&p, &t), plane(25.0), 0.0, None).err(), Some(SplitError::Misses));
        assert_eq!(cut(&Scan::new(&[], &[]), &plane(0.0), None).err(), Some(SplitError::Empty));
    }

    #[test]
    fn a_cylinder_cuts_a_round_hole_out_of_the_top_and_bottom() {
        let (p, t) = block();
        let tube = Shape::Cylinder { point: Vec3::new(20.0, 15.0, 0.0), axis: Vec3::Z, radius: 6.0 };
        let cut = split(&Scan::new(&p, &t), Cutter::shape(tube), 0.0, None).unwrap();
        let runs = cut.runs();
        assert_eq!(runs.len(), 2, "a ring on the top and one on the bottom");
        for (points, closed) in &runs {
            assert!(closed);
            assert!(points.iter().all(|q| (q.dist(Vec3::new(20.0, 15.0, q.z)) - 6.0).abs() < 1e-6), "on the cylinder");
        }
    }

    #[test]
    fn a_body_cuts_where_it_reaches_and_nowhere_else() {
        let (p, t) = block();
        // A small block through the box's top face only.
        let (cp, ct) = fine_cuboid(Vec3::new(10.0, 10.0, 15.0), Vec3::new(20.0, 20.0, 25.0), 4);
        let cutter = Cutter::mesh(&cp, &ct).unwrap();
        assert_eq!(cutter.sides, Sides::InsideOutside);
        let cut = split(&Scan::new(&p, &t), cutter, 0.0, None).unwrap();
        let (inside, _) = cut.keep(&[-1]);
        assert!(inside.iter().all(|q| q.z >= 15.0 - 1e-9 && (10.0 - 1e-9..=20.0 + 1e-9).contains(&q.x)), "inside the small block");
        let runs = cut.runs();
        assert_eq!(runs.len(), 1, "one ring where the block goes through the box's top");
        assert!(runs[0].1);
    }

    #[test]
    fn a_closed_curve_cuts_right_through_unless_held_to_a_ribbon() {
        let (p, t) = block();
        let square = [Vec3::new(5.0, 5.0, 20.0), Vec3::new(15.0, 5.0, 20.0), Vec3::new(15.0, 15.0, 20.0), Vec3::new(5.0, 15.0, 20.0)];
        let through = split(&Scan::new(&p, &t), Cutter::curve(&square, true, Vec3::Z, 0.0).unwrap(), 0.0, None).unwrap();
        assert_eq!(through.runs().len(), 2, "the top and the bottom, seen through");
        let mut path = square.to_vec();
        path.push(square[0]);
        let held = Cutter::curve(&square, true, Vec3::Z, 0.0).unwrap().ribbon(&path, 1.0);
        let cut = split(&Scan::new(&p, &t), held, 0.0, None).unwrap();
        assert_eq!(cut.runs().len(), 1, "only the top, under the curve");
        let (inside, _) = cut.keep(&[-1]);
        assert!(inside.iter().all(|q| (q.z - 20.0).abs() < 1e-9), "the bottom is no cut, and stays outside");
        assert!(!cut.slit);
    }

    #[test]
    fn an_open_curve_cuts_across_the_box_into_right_and_left() {
        let (p, t) = block();
        let across = [Vec3::new(20.5, 0.0, 20.0), Vec3::new(20.5, 30.0, 20.0)];
        let cutter = Cutter::curve(&across, false, Vec3::Z, 1.0).unwrap();
        assert_eq!(cutter.sides, Sides::RightLeft);
        let cut = split(&Scan::new(&p, &t), cutter, 0.0, None).unwrap();
        let (right, _) = cut.keep(&[-1]);
        assert!(right.iter().all(|q| q.x >= 20.5 - 1e-9), "right of the curve, seen from above");
        assert_eq!(cut.runs().len(), 1);
    }

    #[test]
    fn a_tube_cut_along_one_side_only_is_slit_and_stays_one_piece() {
        // A tube of radius 10 along Z, 40 long, open at both ends.
        let (p, t) = crate::test_meshes::revolved(&[[0.0, 10.0], [10.0, 10.0], [20.0, 10.0], [30.0, 10.0], [40.0, 10.0]], 48, false);
        let along = [Vec3::new(10.0, 0.0, 5.0), Vec3::new(10.0, 0.0, 35.0)];
        // Through: the plane along the curve cuts both sides, two halves.
        let through = split(&Scan::new(&p, &t), Cutter::curve(&along, false, Vec3::X, 1.0).unwrap(), 0.0, None).unwrap();
        assert_eq!(through.runs().len(), 2);
        assert!(!through.slit);
        // Held to a ribbon round the curve: the far side is no cut, and the tube is slit.
        let held = Cutter::curve(&along, false, Vec3::X, 1.0).unwrap().ribbon(&along, 1.0);
        let made = cut(&Scan::new(&p, &t), &held, None).unwrap();
        assert!(made.slit, "a cut that divides nothing slits");
        assert!(made.carried_on, "the curve stops short of the tube's ends");
        assert_eq!(made.runs().len(), 1, "only the side under the curve");
        let held = Cutter::curve(&along, false, Vec3::X, 1.0).unwrap().ribbon(&along, 1.0);
        assert_eq!(split(&Scan::new(&p, &t), held, 0.0, None).err(), Some(SplitError::Slit), "split says so");
    }

    #[test]
    fn a_sketch_circle_cuts_as_a_cylinder_does() {
        let (p, t) = block();
        let region = Region2 { outer: solidcore_record::Loop2::circle(Vec2::new(20.0, 15.0), 6.0), holes: Vec::new() };
        let cutter = Cutter::sketch(Vec3::ZERO, Vec3::X, Vec3::Y, &[region], None, 0.0).unwrap();
        let cut = split(&Scan::new(&p, &t), cutter, 0.0, None).unwrap();
        assert_eq!(cut.runs().len(), 2);
        // Held to a slab round the top: only the top is cut.
        let region = Region2 { outer: solidcore_record::Loop2::circle(Vec2::new(20.0, 15.0), 6.0), holes: Vec::new() };
        let held = Cutter::sketch(Vec3::ZERO, Vec3::X, Vec3::Y, &[region], None, 0.0).unwrap().slab(Vec3::new(0.0, 0.0, 20.0), Vec3::Z, 2.0);
        assert_eq!(split(&Scan::new(&p, &t), held, 0.0, None).unwrap().runs().len(), 1);
    }

    #[test]
    fn facing_holds_the_cut_to_the_triangles_facing_that_way() {
        let (p, t) = block();
        let tube = Shape::Cylinder { point: Vec3::new(20.0, 15.0, 0.0), axis: Vec3::Z, radius: 6.0 };
        let cut = split(&Scan::new(&p, &t), Cutter::shape(tube), 0.0, Some(Vec3::Z)).unwrap();
        assert_eq!(cut.runs().len(), 1, "the top, not the bottom");
    }

    #[test]
    fn triangles_chosen_part_along_their_edge() {
        let (p, t) = block();
        // The top's triangles.
        let top: Vec<bool> = t.iter().map(|tri| tri.iter().all(|&v| (p[v as usize].z - 20.0).abs() < 1e-9)).collect();
        let cut = by_triangles(&Scan::new(&p, &t), &top).unwrap();
        assert_eq!(cut.count(-1), top.iter().filter(|x| **x).count());
        let runs = cut.runs();
        assert_eq!((runs.len(), runs[0].1), (1, true), "one ring round the top");
        assert!((cut.length - 140.0).abs() < 1e-9);
        assert_eq!(by_triangles(&Scan::new(&p, &t), &vec![false; t.len()]).err(), Some(SplitError::Misses));
    }
}
