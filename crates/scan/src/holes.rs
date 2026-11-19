//! Holes in a scan and the patches that close them (Liepa, "Filling Holes in Meshes", 2003), as
//! SurfaceCraft's hole filler does it: each hole's ring is triangulated with the least area,
//! refined until its triangles are about as big as those round the hole, and faired so the patch
//! carries the surface's shape across (the squared cotangent Laplacian, least squares by
//! preconditioned conjugate gradients).
//!
//! A hole is a closed ring of open edges through each of its points once. The longest ring of
//! each piece is that piece's outer rim (a scan seen from one side ends there), not a hole, unless
//! it spans less than a quarter of the piece's area (a closed part with one hole). A ring
//! that pinches, dead-ends or runs into an edge of three triangles has no one obvious patch and is
//! counted but left.

use std::collections::{BTreeMap, HashMap, HashSet};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, UnionFind, welded};

/// The undirected key of a pair of points.
fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// One closed ring of open edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    /// The scan's points round it (the first of each set of points at one place), in the order a
    /// triangle closing it runs through them.
    pub points: Vec<u32>,
    /// Its length, in mm.
    pub perimeter: f64,
    /// Whether it is its piece's outer rim rather than a hole.
    pub rim: bool,
}

/// The scan's rings, shortest first, and the open edges in none.
#[derive(Clone, Debug, PartialEq)]
pub struct Survey {
    pub rings: Vec<Ring>,
    pub unfillable_edges: usize,
}

impl Survey {
    /// The holes of at most `limit` edges.
    pub fn holes(&self, limit: usize) -> impl Iterator<Item = &Ring> {
        self.rings.iter().filter(move |r| !r.rim && r.points.len() <= limit)
    }

    /// The ring passing nearest `at` (holes and rims alike), by its points on the scan
    /// `positions`: how a hole clicked on is found again.
    pub fn nearest(&self, positions: &[Vec3], at: Vec3) -> Option<usize> {
        let far = |r: &Ring| r.points.iter().filter_map(|&i| positions.get(i as usize)).map(|p| p.dist(at)).fold(f64::INFINITY, f64::min);
        (0..self.rings.len()).min_by(|&a, &b| far(&self.rings[a]).total_cmp(&far(&self.rings[b])))
    }

    /// The rings nearest each of the points `at`, each once, in the survey's order.
    pub fn chosen(&self, positions: &[Vec3], at: &[Vec3]) -> Vec<&Ring> {
        let mut picked = vec![false; self.rings.len()];
        for &p in at {
            if let Some(k) = self.nearest(positions, p) {
                picked[k] = true;
            }
        }
        self.rings.iter().zip(picked).filter(|(_, p)| *p).map(|(r, _)| r).collect()
    }
}

/// The scan's rings.
pub fn survey(scan: &Scan) -> Survey {
    survey_with(scan, &welded(scan.positions))
}

/// The scan's rings with its points joined as `weld` says (each position's representative).
pub(crate) fn survey_with(scan: &Scan, weld: &[u32]) -> Survey {
    let tri = |t: usize| -> Option<[u32; 3]> {
        scan.corners(t)?;
        let [a, b, c] = scan.triangles.get(t)?.map(|i| weld[i as usize]);
        (a != b && b != c && c != a).then_some([a, b, c])
    };
    // Each edge once per triangle, sorted: an edge met once is open.
    let mut keys: Vec<u64> = (0..scan.triangle_count()).filter_map(tri).flat_map(|[a, b, c]| [pair(a, b), pair(b, c), pair(c, a)]).collect();
    keys.sort_unstable();
    let mut open: HashSet<u64> = HashSet::new();
    let mut i = 0;
    while i < keys.len() {
        let mut j = i + 1;
        while j < keys.len() && keys[j] == keys[i] {
            j += 1;
        }
        if j - i == 1 {
            open.insert(keys[i]);
        }
        i = j;
    }
    if open.is_empty() {
        return Survey { rings: Vec::new(), unfillable_edges: 0 };
    }
    // A triangle that runs an open edge from a to b leaves the missing one to run it from b to a,
    // so a hole's own ring follows the edges reversed.
    let mut following: HashMap<u32, Vec<u32>> = HashMap::new();
    for [a, b, c] in (0..scan.triangle_count()).filter_map(tri) {
        for (from, to) in [(a, b), (b, c), (c, a)] {
            if open.contains(&pair(from, to)) {
                following.entry(to).or_default().push(from);
            }
        }
    }
    let mut rings: Vec<Ring> = Vec::new();
    let mut ring_edges = 0;
    for ring in loops(following) {
        let at = |v: u32| scan.positions[v as usize];
        let perimeter = (0..ring.len()).map(|k| at(ring[k]).dist(at(ring[(k + 1) % ring.len()]))).sum();
        ring_edges += ring.len();
        rings.push(Ring { points: ring, perimeter, rim: false });
    }
    rings.sort_by(|l, r| (l.points.len(), l.points[0]).cmp(&(r.points.len(), r.points[0])));
    // Each piece's longest ring is its rim, unless it spans little of the piece (a closed part
    // with a hole in it has one ring, and it is a hole).
    let mut sets = UnionFind::new(scan.positions.len());
    for [a, b, c] in (0..scan.triangle_count()).filter_map(tri) {
        sets.join(a as usize, b as usize);
        sets.join(b as usize, c as usize);
    }
    let mut piece_area: HashMap<usize, f64> = HashMap::new();
    for t in 0..scan.triangle_count() {
        if let (Some([a, ..]), Some([p, q, r])) = (tri(t), scan.corners(t)) {
            *piece_area.entry(sets.find(a as usize)).or_insert(0.0) += (q - p).cross(r - p).len() / 2.0;
        }
    }
    let spans = |ring: &Ring| {
        let at = |k: usize| scan.positions[ring.points[k % ring.points.len()] as usize];
        let origin = at(0);
        let twice = (1..ring.points.len()).fold(Vec3::ZERO, |s, k| s + (at(k) - origin).cross(at(k + 1) - origin));
        twice.len() / 2.0
    };
    let mut longest: BTreeMap<usize, usize> = BTreeMap::new();
    for (k, ring) in rings.iter().enumerate() {
        let piece = sets.find(ring.points[0] as usize);
        let longer = |other: &Ring| (ring.perimeter, ring.points.len()) > (other.perimeter, other.points.len());
        if longest.get(&piece).is_none_or(|&best| longer(&rings[best])) {
            longest.insert(piece, k);
        }
    }
    for (piece, k) in longest {
        let area = piece_area.get(&piece).copied().unwrap_or(0.0);
        rings[k].rim = spans(&rings[k]) >= 0.25 * area;
    }
    Survey { rings, unfillable_edges: open.len() - ring_edges }
}

/// The open edges' rings: each closed walk along them, `following[v]` naming the points a walk
/// may go on to from `v`. Where holes touch at a point (a pinch, more than one way on), the walk
/// is cut into simple loops at the point it comes back to, so each hole is a ring of its own. Edges
/// no closed walk takes (a dead end, a loop of two) are left out.
fn loops(mut following: HashMap<u32, Vec<u32>>) -> Vec<Vec<u32>> {
    let mut found = Vec::new();
    let mut firsts: Vec<u32> = following.keys().copied().collect();
    firsts.sort_unstable();
    for first in firsts {
        let mut path = vec![first];
        let mut at: HashMap<u32, usize> = HashMap::from([(first, 0)]);
        while let Some(&current) = path.last() {
            let Some(next) = following.get_mut(&current).and_then(Vec::pop) else {
                // A dead end: the path's edges are in no ring. Start again from its first point.
                if path.len() == 1 {
                    break;
                }
                path.truncate(1);
                at.retain(|_, k| *k == 0);
                continue;
            };
            if let Some(&k) = at.get(&next) {
                let ring: Vec<u32> = path.split_off(k + 1);
                let ring: Vec<u32> = std::iter::once(next).chain(ring).collect();
                at.retain(|_, i| *i <= k);
                if ring.len() >= 3 {
                    found.push(ring);
                }
            } else {
                at.insert(next, path.len());
                path.push(next);
            }
        }
    }
    found
}

/// A scan with holes closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Filled {
    /// The scan's points then the patches' new ones, its triangles then the patches'.
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub filled: usize,
    /// Holes within the limit that could not be closed.
    pub failed: usize,
    pub added_triangles: usize,
    /// Each of those: where, how long round, and why.
    pub left: Vec<Unfilled>,
}

/// A hole that could not be closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Unfilled {
    /// The middle of its ring, and how many edges it has.
    pub at: Vec3,
    pub edges: usize,
    /// Why no patch closes it.
    pub why: &'static str,
}

/// Why no patch closes a ring.
const IN_A_LINE: &str = "its points lie in one line, so no triangle closes it";
const BLOCKED: &str = "every way across it would join two of its points the scan already joins by an edge";
const FOLDS: &str = "no flat patch closes it without folding";
const NOT_FINITE: &str = "the patch across it came out with a point that is no number";

/// The rings among `holes` that [`fill_rings`] cannot close, each with why: for saying so when
/// none could be (it returns nothing then).
pub fn unfillable(scan: &Scan, holes: &[&Ring], fair: bool) -> Vec<Unfilled> {
    let weld = welded(scan.positions);
    let around = Surroundings::new(scan, &weld, holes);
    holes.iter().filter_map(|ring| patch(ring, scan.positions, &around, fair).err().map(|why| unfilled(ring, scan.positions, why))).collect()
}

fn unfilled(ring: &Ring, positions: &[Vec3], why: &'static str) -> Unfilled {
    let n = ring.points.len().max(1) as f64;
    Unfilled { at: ring.points.iter().fold(Vec3::ZERO, |s, &v| s + positions[v as usize]) / n, edges: ring.points.len(), why }
}

/// Closes every hole of at most `limit` edges; `fair` shapes each patch to carry the surface across
/// (else it stays the flat triangulation, refined). `None` when no hole is closed.
pub fn fill(scan: &Scan, survey: &Survey, limit: usize, fair: bool) -> Option<Filled> {
    fill_rings(scan, &survey.holes(limit).collect::<Vec<_>>(), fair)
}

/// Closes the rings `holes` (rims too, when given: an open end chosen to be capped).
pub fn fill_rings(scan: &Scan, holes: &[&Ring], fair: bool) -> Option<Filled> {
    if holes.is_empty() {
        return None;
    }
    let weld = welded(scan.positions);
    let around = Surroundings::new(scan, &weld, holes);
    let mut positions = scan.positions.to_vec();
    let mut triangles = scan.triangles.to_vec();
    let (mut filled, mut left) = (0, Vec::new());
    for &ring in holes {
        let patch = match patch(ring, scan.positions, &around, fair) {
            Ok(patch) => patch,
            Err(why) => {
                left.push(unfilled(ring, scan.positions, why));
                continue;
            }
        };
        let n = ring.points.len();
        let base = positions.len();
        let global = |local: usize| if local < n { ring.points[local] } else { (base + local - n) as u32 };
        positions.extend(patch.added);
        triangles.extend(patch.triangles.iter().map(|t| t.map(global)));
        filled += 1;
    }
    (filled > 0).then(|| Filled { added_triangles: triangles.len() - scan.triangle_count(), positions, triangles, filled, failed: left.len(), left })
}

/// What the scan looks like round the holes: the triangles touching a ring (how the surface goes on
/// past the hole, for the fairing) and the edges already joining two ring points (which a patch
/// must not make again).
struct Surroundings {
    triangles: Vec<[u32; 3]>,
    at: HashMap<u32, Vec<usize>>,
    edges: HashSet<u64>,
}

impl Surroundings {
    fn new(scan: &Scan, weld: &[u32], rings: &[&Ring]) -> Self {
        let on: HashSet<u32> = rings.iter().flat_map(|r| r.points.iter().copied()).collect();
        let (mut triangles, mut at, mut edges) = (Vec::new(), HashMap::<u32, Vec<usize>>::new(), HashSet::new());
        for (t, tri) in scan.triangles.iter().enumerate() {
            if scan.corners(t).is_none() {
                continue;
            }
            let [a, b, c] = tri.map(|i| weld[i as usize]);
            if a == b || b == c || c == a {
                continue;
            }
            let flags = [on.contains(&a), on.contains(&b), on.contains(&c)];
            if !flags.iter().any(|f| *f) {
                continue;
            }
            let k = triangles.len();
            triangles.push([a, b, c]);
            for (v, f) in [a, b, c].into_iter().zip(flags) {
                if f {
                    at.entry(v).or_default().push(k);
                }
            }
            for ((p, q), (fp, fq)) in [((a, b), (flags[0], flags[1])), ((b, c), (flags[1], flags[2])), ((c, a), (flags[2], flags[0]))] {
                if fp && fq {
                    edges.insert(pair(p, q));
                }
            }
        }
        Surroundings { triangles, at, edges }
    }
}

/// The unit normal of the ring `points` winds about (Newell's), `None` for one with no area.
fn ring_normal(points: &[Vec3]) -> Option<Vec3> {
    let mut n = Vec3::ZERO;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.normalized()
}

/// The triangulation of the polygon `points` (in order) with the least total area, by dynamic
/// programming over its sub-polygons, every triangle on three of its own points; a diagonal
/// `forbidden` refuses is not used. `None` when nothing triangulates it.
fn least_area(points: &[Vec3], forbidden: impl Fn(usize, usize) -> bool) -> Option<Vec<[usize; 3]>> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    if n == 3 {
        return Some(vec![[0, 1, 2]]);
    }
    // A little for being long and thin breaks ties between equal areas toward compact triangles.
    let compactness = 0.01;
    // The way the ring winds (its Newell normal): a triangle turned against it (on a flat ring
    // that is not convex, the least area can take one, folded over its neighbours) costs as if it
    // were a hundred times its size.
    let winding = ring_normal(points);
    let mut cost = vec![0f64; n * n];
    let mut split = vec![usize::MAX; n * n];
    for span in 2..n {
        for i in 0..(n - span) {
            let j = i + span;
            let mut best = (f64::INFINITY, usize::MAX);
            // The whole polygon's closing side is an edge; any other side is a diagonal.
            if (i == 0 && j == n - 1) || !forbidden(i, j) {
                for k in (i + 1)..j {
                    let inner = cost[i * n + k] + cost[k * n + j];
                    if !(inner < best.0) {
                        continue;
                    }
                    let (ik, ij, kj) = (points[k] - points[i], points[j] - points[i], points[j] - points[k]);
                    let area = 0.5 * ik.cross(ij).len();
                    let edges = ik.len2() + ij.len2() + kj.len2();
                    // Three points in a line make no triangle.
                    if !(area > 1e-10 * edges) {
                        continue;
                    }
                    let against = winding.is_some_and(|w| ik.cross(ij).dot(w) < 0.0);
                    let area_cost = if against { 100.0 * area } else { area };
                    let total = inner + area_cost + compactness * (edges / (4.0 * 3.0f64.sqrt()) - area);
                    if total < best.0 {
                        best = (total, k);
                    }
                }
            }
            cost[i * n + j] = best.0;
            split[i * n + j] = best.1;
        }
    }
    if !cost[n - 1].is_finite() {
        return None;
    }
    let mut out = Vec::new();
    let mut pending = vec![(0, n - 1)];
    while let Some((i, j)) = pending.pop() {
        if j - i < 2 {
            continue;
        }
        let k = split[i * n + j];
        if k == usize::MAX {
            return None;
        }
        out.push([i, k, j]);
        pending.push((i, k));
        pending.push((k, j));
    }
    Some(out)
}

/// Rings up to this many edges are triangulated whole ([`least_area`] is cubic in their length);
/// longer ones through every few of their points, then each stretch between.
const WHOLE_RING: usize = 300;

fn triangulated(points: &[Vec3], forbidden: impl Fn(usize, usize) -> bool) -> Option<Vec<[usize; 3]>> {
    let n = points.len();
    if n <= WHOLE_RING {
        return least_area(points, forbidden);
    }
    let step = n.div_ceil(WHOLE_RING);
    'starts: for offset in 0..step {
        let corners: Vec<usize> = (offset..n).step_by(step).collect();
        let next = |j: usize| corners[(j + 1) % corners.len()];
        if corners.iter().enumerate().any(|(j, &c)| forbidden(c, next(j))) {
            continue;
        }
        let coarse_points: Vec<Vec3> = corners.iter().map(|&k| points[k]).collect();
        let Some(coarse) = least_area(&coarse_points, |a, b| forbidden(corners[a], corners[b])) else { continue };
        let mut triangles: Vec<[usize; 3]> = coarse.iter().map(|t| t.map(|k| corners[k])).collect();
        for (j, &corner) in corners.iter().enumerate() {
            let stretch: Vec<usize> = (0..).map(|k| (corner + k) % n).take_while(|&k| k != next(j)).chain([next(j)]).collect();
            if stretch.len() < 3 {
                continue;
            }
            let stretch_points: Vec<Vec3> = stretch.iter().map(|&k| points[k]).collect();
            let Some(filled) = least_area(&stretch_points, |a, b| forbidden(stretch[a], stretch[b])) else { continue 'starts };
            triangles.extend(filled.iter().map(|t| t.map(|k| stretch[k])));
        }
        return Some(triangles);
    }
    None
}

/// The triangles that close one ring: points `0..n` are the ring's, the rest new.
struct Patch {
    added: Vec<Vec3>,
    triangles: Vec<[usize; 3]>,
}

fn patch(ring: &Ring, positions: &[Vec3], around: &Surroundings, fair: bool) -> Result<Patch, &'static str> {
    let points: Vec<Vec3> = ring.points.iter().map(|&v| positions[v as usize]).collect();
    let forbidden = |i: usize, j: usize| around.edges.contains(&pair(ring.points[i], ring.points[j]));
    let Some(coarse) = triangulated(&points, forbidden) else {
        // Why: no three of its points make a triangle; or they do, but only across a pair of its
        // points the scan already joins; or neither, and it folds.
        let n = points.len();
        // Points in one line wind about nothing, and neither do a few points doubled back on
        // themselves: the area they go round, against their length squared.
        let length: f64 = (0..n).map(|k| points[k].dist(points[(k + 1) % n])).sum();
        let round = (0..n).fold(Vec3::ZERO, |s, k| s + points[k].cross(points[(k + 1) % n])).len() / 2.0;
        let any_triangle = round > 1e-9 * length * length;
        let any_blocked = around.edges.len() > 0 && (0..n).any(|i| (i + 2..n).any(|j| !(i == 0 && j == n - 1) && forbidden(i, j)));
        if !any_triangle {
            return Err(IN_A_LINE);
        }
        if !any_blocked {
            return Err(FOLDS);
        }
        // Blocked every way across: a point put at its middle and a triangle to each of its edges
        // joins no two of its points, when the middle sees every edge from the same side.
        let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f64;
        let winding = ring_normal(&points).ok_or(IN_A_LINE)?;
        let seen = (0..n).all(|k| {
            let (a, b) = (points[k], points[(k + 1) % n]);
            let normal = (b - a).cross(middle - a);
            normal.dot(winding) > 1e-10 * ((b - a).len2() + (middle - a).len2())
        });
        if !seen {
            return Err(BLOCKED);
        }
        return Ok(Patch { added: vec![middle], triangles: (0..n).map(|k| [k, (k + 1) % n, n]).collect() });
    };
    let mut mesh = PatchMesh::new(&points, coarse);
    mesh.refine(&forbidden);
    if fair && mesh.points.len() > points.len() {
        // Faired only where that turns no triangle over: on a ragged flat ring fairing pulled
        // points past the ring's dents and folded the patch (marker holes on a CAD plate drew
        // crescents of folded triangles).
        let before = mesh.points.clone();
        let folded = |m: &PatchMesh| {
            ring_normal(&points).map_or(0, |w| {
                m.triangles.iter().filter(|t| (m.points[t[1]] - m.points[t[0]]).cross(m.points[t[2]] - m.points[t[0]]).dot(w) < 0.0).count()
            })
        };
        let was = folded(&mesh);
        mesh.fair(&ring.points, positions, around);
        if folded(&mesh) > was {
            mesh.points = before;
        }
    }
    let added = mesh.points[points.len()..].to_vec();
    if added.iter().all(|p| p.is_finite()) { Ok(Patch { added, triangles: mesh.triangles }) } else { Err(NOT_FINITE) }
}

/// A patch being made, with the edge length round it each point asks for.
struct PatchMesh {
    points: Vec<Vec3>,
    scale: Vec<f64>,
    triangles: Vec<[usize; 3]>,
    ring: usize,
}

/// One row of the fairing's system: a point's scaled Laplacian, weights · free points + constant.
struct Row {
    columns: Vec<usize>,
    weights: Vec<f64>,
    constant: Vec3,
}

impl PatchMesh {
    /// A patch never grows past this many points.
    const LIMIT: usize = 20_000;

    fn new(ring: &[Vec3], triangles: Vec<[usize; 3]>) -> Self {
        let n = ring.len();
        let lengths: Vec<f64> = (0..n).map(|k| ring[k].dist(ring[(k + 1) % n])).collect();
        let mean = lengths.iter().sum::<f64>() / n.max(1) as f64;
        // Each ring point asks for its two edges' mean, never less than a quarter of the ring's,
        // so a run of tiny edges cannot ask for endless refinement.
        let scale = (0..n).map(|k| (0.5 * (lengths[(k + n - 1) % n] + lengths[k])).max(0.25 * mean)).collect();
        PatchMesh { points: ring.to_vec(), scale, triangles, ring: n }
    }

    /// Splits triangles larger than their surroundings want at their middles, then flips edges
    /// toward well-shaped triangles, until nothing splits.
    fn refine(&mut self, forbidden: &impl Fn(usize, usize) -> bool) {
        let alpha = 2.0f64.sqrt();
        for _ in 0..40 {
            let mut splits = 0;
            let mut kept = Vec::with_capacity(self.triangles.len());
            for &[a, b, c] in &self.triangles {
                let middle = (self.points[a] + self.points[b] + self.points[c]) / 3.0;
                let wanted = (self.scale[a] + self.scale[b] + self.scale[c]) / 3.0;
                let d = [a, b, c].map(|v| alpha * middle.dist(self.points[v]));
                let big = d.iter().all(|x| *x > wanted) && d[0] > self.scale[a] && d[1] > self.scale[b] && d[2] > self.scale[c];
                if big && self.points.len() < Self::LIMIT {
                    let m = self.points.len();
                    self.points.push(middle);
                    self.scale.push(wanted);
                    kept.extend([[a, b, m], [b, c, m], [c, a, m]]);
                    splits += 1;
                } else {
                    kept.push([a, b, c]);
                }
            }
            self.triangles = kept;
            if splits == 0 {
                break;
            }
            self.relax(forbidden);
        }
    }

    /// Flips inner edges whose opposite angles sum past a half turn (Delaunay), unless the flip
    /// would fold the patch or join two points already joined.
    fn relax(&mut self, forbidden: &impl Fn(usize, usize) -> bool) {
        for _ in 0..8 {
            let mut owners: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
            for (k, t) in self.triangles.iter().enumerate() {
                for (p, q) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    owners.entry(pair(p as u32, q as u32)).or_default().push(k);
                }
            }
            let (mut touched, mut created) = (HashSet::new(), HashSet::new());
            let mut flips = 0;
            for (&key, sharing) in &owners {
                let [first, second] = sharing.as_slice() else { continue };
                let (first, second) = (*first, *second);
                if touched.contains(&first) || touched.contains(&second) {
                    continue;
                }
                let (p, q) = ((key >> 32) as usize, (key & 0xFFFF_FFFF) as usize);
                let Some((i, j, k)) = rotated(self.triangles[first], p, q).or_else(|| rotated(self.triangles[first], q, p)) else { continue };
                let Some((j2, i2, l)) = rotated(self.triangles[second], j, i) else { continue };
                if !(j2 == j && i2 == i && k != l) {
                    continue;
                }
                let new_key = pair(k as u32, l as u32);
                if owners.contains_key(&new_key) || created.contains(&new_key) || (k < self.ring && l < self.ring && forbidden(k, l)) {
                    continue;
                }
                let pts = &self.points;
                if !(angle(pts[k], pts[i], pts[j]) + angle(pts[l], pts[i], pts[j]) > std::f64::consts::PI + 1e-9) {
                    continue;
                }
                // The two new triangles face the way the old pair did.
                let reference = (pts[j] - pts[i]).cross(pts[k] - pts[i]) + (pts[i] - pts[j]).cross(pts[l] - pts[j]);
                let (n1, n2) = ((pts[l] - pts[i]).cross(pts[k] - pts[i]), (pts[j] - pts[l]).cross(pts[k] - pts[l]));
                if !(n1.dot(reference) > 0.0 && n2.dot(reference) > 0.0) {
                    continue;
                }
                self.triangles[first] = [i, l, k];
                self.triangles[second] = [l, j, k];
                touched.insert(first);
                touched.insert(second);
                created.insert(new_key);
                flips += 1;
            }
            if flips == 0 {
                break;
            }
        }
    }

    /// Moves the new points to the smoothest surface meeting the ring: least squares on the scaled
    /// cotangent Laplacian at every point of the patch, a ring point's taking in the scan round it.
    fn fair(&mut self, ring: &[u32], positions: &[Vec3], around: &Surroundings) {
        let n = self.ring;
        let rows_count = self.points.len();
        let free = rows_count - n;
        if free == 0 {
            return;
        }
        let ring_index: HashMap<u32, usize> = ring.iter().enumerate().map(|(k, &v)| (v, k)).collect();
        let mut outside: HashMap<u32, usize> = HashMap::new();
        let mut outside_points: Vec<Vec3> = Vec::new();
        let mut node = |v: u32| -> usize {
            if let Some(&k) = ring_index.get(&v) {
                return k;
            }
            *outside.entry(v).or_insert_with(|| {
                outside_points.push(positions[v as usize]);
                rows_count + outside_points.len() - 1
            })
        };
        let mut all = self.triangles.clone();
        let mut included = HashSet::new();
        for v in ring {
            for &k in around.at.get(v).into_iter().flatten() {
                if included.insert(k) {
                    all.push(around.triangles[k].map(&mut node));
                }
            }
        }
        let points = &self.points;
        let position = |k: usize| if k < rows_count { points[k] } else { outside_points[k - rows_count] };
        // Cotangent weights and point areas from the patch as it stands (keeping the system linear),
        // the cotangents capped so a sliver cannot rule.
        let mut weights: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); rows_count];
        let mut area = vec![0f64; rows_count];
        for c in &all {
            let p = c.map(position);
            let cross = (p[1] - p[0]).cross(p[2] - p[0]).len();
            if !(cross > 1e-300) {
                continue;
            }
            for corner in 0..3 {
                let (here, next, prev) = (p[corner], p[(corner + 1) % 3], p[(corner + 2) % 3]);
                let cot = ((next - here).dot(prev - here) / cross).clamp(-20.0, 20.0);
                let (a, b) = (c[(corner + 1) % 3], c[(corner + 2) % 3]);
                if a < rows_count {
                    *weights[a].entry(b).or_insert(0.0) += 0.5 * cot;
                }
                if b < rows_count {
                    *weights[b].entry(a).or_insert(0.0) += 0.5 * cot;
                }
                if c[corner] < rows_count {
                    area[c[corner]] += cross / 6.0;
                }
            }
        }
        let mut rows: Vec<Row> = Vec::new();
        for v in 0..rows_count {
            if !(area[v] > 0.0) {
                continue;
            }
            let scale = 1.0 / area[v].sqrt();
            let total: f64 = weights[v].values().sum();
            let mut row = Row { columns: Vec::new(), weights: Vec::new(), constant: Vec3::ZERO };
            let mut add = |other: usize, w: f64| {
                if other >= n && other < rows_count {
                    row.columns.push(other - n);
                    row.weights.push(w * scale);
                } else {
                    row.constant += position(other) * (w * scale);
                }
            };
            add(v, -total);
            for (&other, &w) in &weights[v] {
                add(other, w);
            }
            if !row.columns.is_empty() {
                rows.push(row);
            }
        }
        // Least squares: the sum over rows of |weights · x + constant|², by conjugate gradients
        // with the diagonal as preconditioner.
        let multiply = |x: &[Vec3]| -> Vec<Vec3> {
            let mut out = vec![Vec3::ZERO; free];
            for row in &rows {
                let value = row.columns.iter().zip(&row.weights).fold(Vec3::ZERO, |s, (&c, &w)| s + x[c] * w);
                for (&c, &w) in row.columns.iter().zip(&row.weights) {
                    out[c] += value * w;
                }
            }
            out
        };
        let dot = |a: &[Vec3], b: &[Vec3]| a.iter().zip(b).map(|(p, q)| p.dot(*q)).sum::<f64>();
        let mut rhs = vec![Vec3::ZERO; free];
        let mut diagonal = vec![0f64; free];
        for row in &rows {
            for (&c, &w) in row.columns.iter().zip(&row.weights) {
                rhs[c] -= row.constant * w;
                diagonal[c] += w * w;
            }
        }
        let largest = diagonal.iter().copied().fold(0.0, f64::max);
        for d in &mut diagonal {
            *d = d.max(1e-12 * largest).max(1e-300);
        }
        let mut x: Vec<Vec3> = self.points[n..].to_vec();
        let mut residual: Vec<Vec3> = rhs.iter().zip(multiply(&x)).map(|(r, m)| *r - m).collect();
        let mut direction: Vec<Vec3> = residual.iter().zip(&diagonal).map(|(r, d)| *r / *d).collect();
        let mut pre = direction.clone();
        let mut agreement = dot(&residual, &pre);
        let target = 1e-20 * dot(&rhs, &rhs).max(1e-300);
        for _ in 0..600 {
            if !(dot(&residual, &residual) > target) {
                break;
            }
            let image = multiply(&direction);
            let curvature = dot(&direction, &image);
            if !(curvature > 0.0) {
                break;
            }
            let step = agreement / curvature;
            for k in 0..free {
                x[k] += direction[k] * step;
                residual[k] -= image[k] * step;
                pre[k] = residual[k] / diagonal[k];
            }
            let updated = dot(&residual, &pre);
            let blend = updated / agreement;
            agreement = updated;
            for k in 0..free {
                direction[k] = pre[k] + direction[k] * blend;
            }
        }
        if x.iter().all(|p| p.is_finite()) {
            self.points[n..].copy_from_slice(&x);
        }
    }
}

/// The triangle's points as (first, second, third) when it runs from `first` to `second`.
fn rotated(t: [usize; 3], first: usize, second: usize) -> Option<(usize, usize, usize)> {
    (0..3).find(|&r| t[r] == first && t[(r + 1) % 3] == second).map(|r| (t[r], t[(r + 1) % 3], t[(r + 2) % 3]))
}

fn angle(apex: Vec3, a: Vec3, b: Vec3) -> f64 {
    let (u, v) = (a - apex, b - apex);
    let lengths = u.len() * v.len();
    if !(lengths > 0.0) {
        return 0.0;
    }
    (u.dot(v) / lengths).clamp(-1.0, 1.0).acos()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grid of unit squares facing +Z, some left out.
    fn plate(cells: u32, missing: impl Fn(u32, u32) -> bool) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for y in 0..=cells {
            for x in 0..=cells {
                positions.push(Vec3::new(f64::from(x), f64::from(y), 0.0));
            }
        }
        let v = |x: u32, y: u32| y * (cells + 1) + x;
        let mut triangles = Vec::new();
        for y in 0..cells {
            for x in 0..cells {
                if !missing(x, y) {
                    triangles.push([v(x, y), v(x + 1, y), v(x + 1, y + 1)]);
                    triangles.push([v(x, y), v(x + 1, y + 1), v(x, y + 1)]);
                }
            }
        }
        (positions, triangles)
    }

    /// A square hole in a plate is closed flat and facing the plate's way; the plate's own rim is
    /// not a hole.
    #[test]
    fn a_square_hole_in_a_plate_is_closed_flat() {
        let (positions, triangles) = plate(10, |x, y| (4..=5).contains(&x) && (4..=5).contains(&y));
        let scan = Scan::new(&positions, &triangles);
        let found = survey(&scan);
        assert_eq!(found.rings.len(), 2);
        let hole = found.holes(300).next().unwrap();
        assert_eq!(hole.points.len(), 8);
        assert!((hole.perimeter - 8.0).abs() < 1e-9);
        assert!(found.rings.iter().any(|r| r.rim && r.points.len() == 40), "the rim");
        let done = fill(&scan, &found, 300, true).unwrap();
        assert_eq!((done.filled, done.failed), (1, 0));
        assert!(done.positions.iter().all(|p| p.z.abs() < 1e-9), "flat");
        let after = survey(&Scan::new(&done.positions, &done.triangles));
        assert_eq!(after.rings.len(), 1, "only the rim is left");
        let area: f64 = done.triangles[triangles.len()..]
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| done.positions[i as usize]);
                let n = (b - a).cross(c - a);
                assert!(n.z > 0.0, "facing as the plate does");
                n.len() / 2.0
            })
            .sum();
        assert!((area - 4.0).abs() < 1e-9, "the hole's area: {area}");
        assert!(fill(&scan, &found, 6, true).is_none(), "a limit below the hole closes nothing");
        // A whole plate's one ring spans all of it: its rim.
        let (positions, triangles) = plate(10, |_, _| false);
        let whole = survey(&Scan::new(&positions, &triangles));
        assert!(whole.rings.len() == 1 && whole.rings[0].rim);
    }

    /// Two square holes touching at one corner (a pinch, as fused scans leave them) are two holes,
    /// both closed, with no open edge left over for Repair to be sent to.
    /// A square hole whose two diagonals the scan already has as edges (a rib across it either
    /// way): no triangle on three of its corners closes it, so a point at its middle does, with
    /// a triangle to each side. Its points in one line, or its middle not seeing every side, and
    /// it is left, with why.
    #[test]
    fn a_hole_blocked_every_way_across_is_closed_from_its_middle() {
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 0.0), Vec3::new(0.0, 4.0, 0.0)];
        let ring = Ring { points: vec![0, 1, 2, 3], perimeter: 16.0, rim: false };
        let ribs = Surroundings { triangles: Vec::new(), at: HashMap::new(), edges: HashSet::from([pair(0, 2), pair(1, 3)]) };
        let patch = patch(&ring, &positions, &ribs, true).expect("closed from its middle");
        assert_eq!(patch.added, vec![Vec3::new(2.0, 2.0, 0.0)]);
        assert_eq!(patch.triangles, vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]]);
        // An L of six corners, its two inside corners' way across blocked like every other: its
        // middle lies outside it, past the inside corner.
        let ell = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        ];
        let ring = Ring { points: (0..6).collect(), perimeter: 40.0, rim: false };
        let every: HashSet<u64> = (0..6u32).flat_map(|i| (i + 2..6).map(move |j| pair(i, j))).collect();
        let blocked = Surroundings { triangles: Vec::new(), at: HashMap::new(), edges: every };
        assert_eq!(super::patch(&ring, &ell, &blocked, true).err(), Some(BLOCKED));
        let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0, Vec3::X * 3.0];
        let ring = Ring { points: vec![0, 1, 2, 3], perimeter: 6.0, rim: false };
        let open = Surroundings { triangles: Vec::new(), at: HashMap::new(), edges: HashSet::new() };
        assert_eq!(super::patch(&ring, &line, &open, true).err(), Some(IN_A_LINE));
    }

    #[test]
    fn holes_touching_at_a_corner_are_two_holes() {
        let (positions, triangles) = plate(10, |x, y| (x, y) == (3, 3) || (x, y) == (4, 4));
        let scan = Scan::new(&positions, &triangles);
        let found = survey(&scan);
        assert_eq!(found.unfillable_edges, 0, "every open edge is in a ring");
        let holes: Vec<_> = found.holes(300).collect();
        assert_eq!(holes.len(), 2, "{:?}", found.rings);
        assert!(holes.iter().all(|h| h.points.len() == 4));
        let done = fill(&scan, &found, 300, true).unwrap();
        assert_eq!((done.filled, done.failed), (2, 0));
        let after = survey(&Scan::new(&done.positions, &done.triangles));
        assert_eq!((after.rings.len(), after.unfillable_edges), (1, 0), "only the rim is left");
    }

    /// A sphere with a cap cut off: the patch carries the sphere's curve across, close to it.
    #[test]
    fn a_sphere_missing_a_cap_is_closed_round() {
        let (rings, segments, r) = (24u32, 48u32, 50.0);
        let mut positions = vec![Vec3::new(0.0, 0.0, -r)];
        for i in 1..rings {
            let polar = std::f64::consts::PI * f64::from(i) / f64::from(rings);
            for j in 0..segments {
                let a = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                positions.push(Vec3::new(r * polar.sin() * a.cos(), r * polar.sin() * a.sin(), -r * polar.cos()));
            }
        }
        let at = |i: u32, j: u32| 1 + (i - 1) * segments + j % segments;
        let mut triangles = Vec::new();
        // The bottom fan, and bands up to ring 20 of 24: the top cap is missing.
        for j in 0..segments {
            triangles.push([0, at(1, j + 1), at(1, j)]);
            for i in 1..20 {
                triangles.push([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let found = survey(&scan);
        // Its one ring spans little of it: a hole, not a rim.
        assert_eq!(found.rings.len(), 1);
        assert!(!found.rings[0].rim);
        let done = fill(&scan, &found, 300, true).unwrap();
        let worst = done.positions[positions.len()..].iter().map(|p| (p.len() - r).abs()).fold(0.0, f64::max);
        assert!(worst < 0.05 * r, "close to the sphere: {worst}");
        let flat = fill(&scan, &found, 300, false).unwrap();
        let flat_worst = flat.positions[positions.len()..].iter().map(|p| (p.len() - r).abs()).fold(0.0, f64::max);
        assert!(worst < flat_worst, "fairing carries the curve: {worst} < {flat_worst}");
    }
}
