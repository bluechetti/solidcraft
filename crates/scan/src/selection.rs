//! Selections of a scan's triangles, for the scan tools that work on part of a scan: made by
//! brushing over the scan, by a polygon drawn round what is seen, by growing a smooth region from
//! a place, or from a closed curve drawn round it on the scan (SurfaceCraft's Select Inside
//! Curve); and what is made from one: its outline smoothed and laid on the scan (SurfaceCraft's
//! Curve from Selection Edge).
//!
//! A [`Topology`] of the scan holds what these need: each triangle's welded corners, its
//! neighbours across its edges, the triangles round each corner, its middle, normal and area, and
//! the nearest triangle to a place. A selection is one flag a triangle.

use solidcore_record::Vec3;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::{Scan, par, welded};

/// No triangle.
const NONE: u32 = u32::MAX;
/// The most triangles a piece closed in by a grown region may have and still be taken into it as
/// noise (specks of a few triangles each); a blind hole or a pocket is many more.
const CLOSED_IN: usize = 64;
/// A piece closed in by a grown region this small is a speck of noise whichever way it faces.
const SPECK: usize = 8;
/// How many rings of triangles round each one its settled direction is read from, and how many
/// times it is settled to its side of a crease (see `Topology::settled_facing`).
const RINGS: usize = 3;
const SETTLE: usize = 4;

/// The most cells across a polygon's depth picture.
const RASTER: usize = 1024;

/// At most this many places along one outline are smoothed and laid on the scan.
pub const PLACES: usize = 240;

/// An outline of fewer triangle sides than this is a speck, not a hole.
const SHORTEST_OUTLINE: usize = 6;

/// How the scan is seen: the way the view looks (unit), and the eye for a perspective view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub direction: Vec3,
    pub eye: Option<Vec3>,
}

/// The scan's triangles as selections read them.
pub struct Topology {
    tree: std::sync::Arc<TriangleTree>,
    /// The scan's points and triangles, shared with its body.
    mesh: std::sync::Arc<solidcraft_geom::Mesh>,
    /// Each triangle's welded corners, when welding changed any ([`Topology::corners`]).
    welded: Option<Vec<[u32; 3]>>,
    /// The triangle across each edge (corner k to corner k + 1), or [`NONE`].
    across: Vec<[u32; 3]>,
    /// The triangles round each welded corner, packed.
    fan_start: Vec<u32>,
    fan: Vec<u32>,
    /// Each welded corner's normal: the triangles' round it, each by its area (for
    /// [`Topology::smooth`]).
    vertex_normals: Vec<Vec3>,
    /// The scan's typical triangle side, in mm.
    pub edge: f64,
}

impl std::fmt::Debug for Topology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Topology").field("triangles", &self.corners().len()).field("edge", &self.edge).finish()
    }
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// Why a selection's outline could not be laid on the scan as curves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeProblem {
    /// Nothing is selected, or all of a closed part is: there is no outline.
    NoEdge,
    /// The outline is too short to follow.
    TooShort,
    /// The smoothed outline leaves the scan: smooth over less.
    LeavesScan,
}

impl std::fmt::Display for EdgeProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EdgeProblem::NoEdge => "the selection has no outline: select part of the scan, not all of a closed part",
            EdgeProblem::TooShort => "the selection's outline is too short to follow: select a larger region",
            EdgeProblem::LeavesScan => "the smoothed outline leaves the scan: smooth it over less",
        })
    }
}

/// Why the inside of a curve could not be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsideProblem {
    /// The curve does not come back round to where it starts.
    Open,
    /// The curve is not on the scan.
    OffTheScan,
    /// No triangle of the scan is inside it.
    Nothing,
}

impl std::fmt::Display for InsideProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            InsideProblem::Open => "the curve is open: it must come back round to where it starts",
            InsideProblem::OffTheScan => "the curve is not on the scan: draw it on the scan",
            InsideProblem::Nothing => "no triangle of the scan is inside the curve",
        })
    }
}

/// The length of a closed loop.
fn loop_length(ring: &[Vec3]) -> f64 {
    (0..ring.len()).map(|i| ring[i].dist(ring[(i + 1) % ring.len()])).sum()
}

/// The closed loop `ring` (its first place not repeated) at `count` places evenly along it.
fn resampled(ring: &[Vec3], count: usize) -> Option<Vec<Vec3>> {
    if ring.len() < 3 || count < 3 {
        return None;
    }
    let mut along = vec![0.0];
    for i in 0..ring.len() {
        along.push(along[i] + ring[i].dist(ring[(i + 1) % ring.len()]));
    }
    let length = along[ring.len()];
    if !(length > 0.0) {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    let mut segment = 0;
    for k in 0..count {
        let s = length * k as f64 / count as f64;
        while segment < ring.len() - 1 && along[segment + 1] < s {
            segment += 1;
        }
        let (a, b) = (ring[segment], ring[(segment + 1) % ring.len()]);
        let span = along[segment + 1] - along[segment];
        out.push(if span > 0.0 { a + (b - a) * ((s - along[segment]) / span) } else { a });
    }
    Some(out)
}

/// The closed loop of evenly spaced `places` smoothed over `width` mm along it (a Gaussian), the
/// shrinking a smoothing brings to a curved loop taken back (twice the smoothed loop less the
/// smoothed loop smoothed again). `spacing` is how far apart the places are.
fn smoothed(places: &[Vec3], spacing: f64, width: f64) -> Vec<Vec3> {
    let n = places.len();
    if n < 3 || !(spacing > 0.0 && width > 0.0) {
        return places.to_vec();
    }
    // Never over more than an eighth of the loop, so a small loop is not smoothed to a point.
    let sigma = (width / spacing).min(n as f64 / 8.0);
    if sigma < 0.5 {
        return places.to_vec();
    }
    let reach = (3.0 * sigma).ceil() as i64;
    let weights: Vec<f64> = (-reach..=reach).map(|m| (-0.5 * (m as f64 / sigma).powi(2)).exp()).collect();
    let total: f64 = weights.iter().sum();
    let n = n as i64;
    let blur = |line: &[Vec3]| -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                (-reach..=reach).fold(Vec3::ZERO, |sum, m| sum + line[((i + m).rem_euclid(n)) as usize] * (weights[(m + reach) as usize] / total))
            })
            .collect()
    };
    let once = blur(places);
    let twice = blur(&once);
    once.iter().zip(&twice).map(|(a, b)| *a * 2.0 - *b).collect()
}

/// Whether `q` is inside the closed polygon `ring` (even-odd).
fn inside_polygon(q: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut odd = false;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) {
            odd = !odd;
        }
    }
    odd
}

/// A triangle's middle, its unit normal (none: zero) and its area, from its corners as the scan
/// gives them (none for one with a corner out of range: all zero).
fn face_of(scan: &Scan, t: usize) -> (Vec3, Vec3, f64) {
    let Some([a, b, c]) = scan.corners(t) else { return (Vec3::ZERO, Vec3::ZERO, 0.0) };
    let cross = (b - a).cross(c - a);
    ((a + b + c) / 3.0, cross.normalized().unwrap_or(Vec3::ZERO), cross.len() / 2.0)
}

impl Topology {
    pub fn new(scan: &Scan) -> Topology {
        let mesh = std::sync::Arc::new(solidcraft_geom::Mesh {
            positions: scan.positions.to_vec(),
            triangles: scan.triangles.to_vec(),
            ..Default::default()
        });
        let tree = std::sync::Arc::new(TriangleTree::over(mesh.clone()));
        Topology::over(mesh, tree)
    }

    /// [`Topology::new`] of the scan `mesh` (shared with its body, not copied), with its triangle
    /// tree built already (shared with Curve on Scan's projector). A triangle's middle, area and
    /// smoothed normal are worked out from the mesh when asked for, not kept: for a scan of 28
    /// million triangles they took 2.2 GB, its welded corners (the scan's own triangles, when
    /// welding changed none) 336 MB more.
    pub fn over(mesh: std::sync::Arc<solidcraft_geom::Mesh>, tree: std::sync::Arc<TriangleTree>) -> Topology {
        let scan = Scan::new(&mesh.positions, &mesh.triangles);
        let weld = welded(scan.positions);
        let count = scan.triangle_count();
        let welded_corners: Option<Vec<[u32; 3]>> = (!weld.iter().enumerate().all(|(i, &w)| w as usize == i))
            .then(|| (0..count).map(|t| scan.triangles[t].map(|v| weld.get(v as usize).copied().unwrap_or(v))).collect());
        drop(weld);
        let corners: &[[u32; 3]] = welded_corners.as_deref().unwrap_or(scan.triangles);
        // Each edge, sorted by its corners, says the two triangles it joins.
        let mut edges: Vec<(u64, u32, u8)> = Vec::with_capacity(3 * count);
        for (t, c) in corners.iter().enumerate() {
            if scan.corners(t).is_none() || c[0] == c[1] || c[1] == c[2] || c[2] == c[0] {
                continue;
            }
            for k in 0..3u8 {
                edges.push((pair(c[k as usize], c[(k as usize + 1) % 3]), t as u32, k));
            }
        }
        par::sort_unstable(&mut edges);
        let mut across = vec![[NONE; 3]; count];
        let mut i = 0;
        while i < edges.len() {
            let mut j = i + 1;
            while j < edges.len() && edges[j].0 == edges[i].0 {
                j += 1;
            }
            // Two triangles on an edge are neighbours; on an edge three or more share, each takes
            // the next round.
            if j - i >= 2 {
                for k in i..j {
                    let next = if k + 1 < j { k + 1 } else { i };
                    across[edges[k].1 as usize][edges[k].2 as usize] = edges[next].1;
                }
            }
            i = j;
        }
        // The triangles at each vertex, in order, laid out one vertex after another: counted,
        // then placed (a list for each vertex was millions of allocations).
        let vertices = scan.positions.len();
        let mut fan_start = vec![0u32; vertices + 1];
        for &v in corners.iter().flatten() {
            if let Some(n) = fan_start.get_mut(v as usize + 1) {
                *n += 1;
            }
        }
        for v in 0..vertices {
            fan_start[v + 1] += fan_start[v];
        }
        let mut fill: Vec<u32> = fan_start[..vertices].to_vec();
        let mut fan = vec![0u32; fan_start[vertices] as usize];
        for (t, c) in corners.iter().enumerate() {
            for &v in c {
                if let Some(at) = fill.get_mut(v as usize) {
                    if let Some(slot) = fan.get_mut(*at as usize) {
                        *slot = t as u32;
                    }
                    *at += 1;
                }
            }
        }
        drop(fill);
        let edge = crate::mirror_fill::typical_edge(&scan);
        let mut topology = Topology { tree, mesh: mesh.clone(), welded: None, across, fan_start, fan, vertex_normals: Vec::new(), edge };
        topology.vertex_normals = par::map(vertices, |v| {
            topology.fan_of(v as u32).iter().fold(Vec3::ZERO, |s, &f| {
                let (_, normal, area) = face_of(&scan, f as usize);
                s + normal * area
            })
        });
        topology.welded = welded_corners;
        topology
    }

    pub fn triangle_count(&self) -> usize {
        self.corners().len()
    }

    /// Each triangle's welded corners: the scan's own triangles when welding changed none.
    fn corners(&self) -> &[[u32; 3]] {
        self.welded.as_deref().unwrap_or(&self.mesh.triangles)
    }

    /// The scan as the topology was built over it.
    fn scan(&self) -> Scan<'_> {
        Scan::new(&self.mesh.positions, &self.mesh.triangles)
    }

    /// Triangle `t`'s middle (zero for one with a corner out of range).
    pub fn middle(&self, t: usize) -> Vec3 {
        face_of(&self.scan(), t).0
    }

    /// Triangle `t`'s area (zero for one with a corner out of range).
    pub fn area(&self, t: usize) -> f64 {
        face_of(&self.scan(), t).2
    }

    /// Triangle `t`'s normal smoothed over the triangles round its corners (its own where they
    /// cancel out), as it was kept before it was worked out when asked for.
    fn smooth(&self, t: usize) -> Option<Vec3> {
        let corners = self.corners().get(t)?;
        let sum = corners.iter().fold(Vec3::ZERO, |s, &v| s + self.vertex_normals.get(v as usize).copied().unwrap_or(Vec3::ZERO));
        Some(sum.normalized().unwrap_or_else(|| face_of(&self.scan(), t).1))
    }

    fn fan_of(&self, v: u32) -> &[u32] {
        let (a, b) =
            (self.fan_start.get(v as usize).copied().unwrap_or(0) as usize, self.fan_start.get(v as usize + 1).copied().unwrap_or(0) as usize);
        self.fan.get(a..b).unwrap_or_default()
    }

    fn neighbours(&self, t: usize) -> impl Iterator<Item = usize> + '_ {
        self.across.get(t).into_iter().flatten().filter(|&&n| n != NONE).map(|&n| n as usize)
    }

    /// The triangle nearest `p`, and how far it is.
    pub fn nearest(&self, p: Vec3) -> Option<(usize, f64)> {
        self.tree.nearest(p).map(|(d, t)| (t as usize, d))
    }

    /// The nearest place of the scan to `p`, and how far it is.
    fn nearest_place(&self, scan: &Scan, p: Vec3) -> Option<(Vec3, f64)> {
        let (t, d) = self.nearest(p)?;
        let [a, b, c] = scan.corners(t)?;
        Some((closest_on_triangle(p, a, b, c), d))
    }

    /// The triangles reached across edges from `seeds` that `keep` keeps (each seed only if it
    /// keeps it too).
    fn flood(&self, seeds: impl IntoIterator<Item = usize>, keep: impl Fn(usize, usize) -> bool) -> Vec<bool> {
        let mut taken = vec![false; self.corners().len()];
        let mut queue: Vec<usize> = Vec::new();
        for s in seeds {
            if s < taken.len() && !taken[s] {
                taken[s] = true;
                queue.push(s);
            }
        }
        while let Some(t) = queue.pop() {
            for n in self.neighbours(t) {
                if !taken[n] && keep(t, n) {
                    taken[n] = true;
                    queue.push(n);
                }
            }
        }
        taken
    }

    /// A brush of `radius` at `centre`: the triangles whose middles are within `radius` of it,
    /// reached across the scan from the triangle under it (so a brush on one wall does not take
    /// the wall behind it).
    pub fn brush(&self, centre: Vec3, radius: f64) -> Vec<bool> {
        let Some((seed, _)) = self.nearest(centre) else { return vec![false; self.corners().len()] };
        let r2 = radius * radius;
        self.flood([seed], |_, n| (self.middle(n) - centre).len2() <= r2)
    }

    /// The region round triangle `seed`: grown across edges while the scan faces within `angle`
    /// (radians) of the way it faces at the seed, its normals smoothed over the triangles round
    /// each corner so the scan's noise does not stop it. A flat face is taken up to where its
    /// rounds and edges turn away; on a curved one, the part that faces about the same way.
    pub fn region(&self, seed: usize, angle: f64) -> Vec<bool> {
        let cos = angle.cos();
        let Some(facing) = self.smooth(seed) else { return vec![false; self.corners().len()] };
        let mut taken = self.flood([seed], |_, n| self.smooth(n).is_some_and(|s| facing.dot(s) >= cos));
        // Triangles the noise turned, closed in by the region, are part of it.
        self.close_in(&mut taken, CLOSED_IN, |t| self.smooth(t).unwrap_or(Vec3::ZERO), angle);
        taken
    }

    /// The smooth region round triangle `seed`: grown across edges while the scan turns less than
    /// `angle` (radians) from one triangle to the next, so up to its creases: a whole hole's wall,
    /// a fillet with the faces it runs into, where [`Self::region`], held to the seed's facing,
    /// takes a strip of a round wall. Each triangle's direction is its own averaged with the
    /// triangles beside it that face within `angle` of it, `SMOOTHING_PASSES` times: the scan's
    /// noise is evened out, but a crease is not (averaged with the faces across it too, a block's
    /// 90° edges blurred into turns small enough to cross, and a click on one face took the whole
    /// block).
    ///
    /// A triangle is taken only when it faces within `angle` of the one it is reached from and of
    /// every taken triangle beside that one: a scan's meshing cuts a crease's corner with a row of
    /// triangles between its faces, so a 45° crease turns twice by about 22° and, step by step, was
    /// crossed at 30°. Pieces the region closes in that the noise turned too far (`CLOSED_IN`) are
    /// taken too.
    pub fn smooth_region(&self, seed: usize, angle: f64) -> Vec<bool> {
        let n = self.corners().len();
        if seed >= n {
            return vec![false; n];
        }
        let cos = angle.cos();
        let scan = self.scan();
        let facing = self.settled_facing(&scan, angle);
        let mut taken = vec![false; n];
        taken[seed] = true;
        let mut queue = vec![seed];
        while let Some(t) = queue.pop() {
            for m in self.neighbours(t).collect::<Vec<_>>() {
                if taken[m] || facing[t].dot(facing[m]) < cos {
                    continue;
                }
                if self.neighbours(t).any(|k| taken[k] && facing[k].dot(facing[m]) < cos) {
                    continue;
                }
                taken[m] = true;
                queue.push(m);
            }
        }
        self.close_in(&mut taken, CLOSED_IN, |t| facing[t], angle);
        taken
    }

    /// Each triangle's direction with the scan's noise taken out and its creases kept: from its
    /// own, `SETTLE` times the area-weighted average of the triangles within `RINGS` edges of it
    /// that face within `angle` of the direction so far (the first time), then within half of it
    /// (a mean shift). Averaged across a crease it would lean over it; each triangle settles on the
    /// side it is on instead, so a crease stays one sharp turn, and a fine scan's noise (0.015 mm on
    /// 0.1 mm triangles tilts single triangles by 30°) no longer lets a region walk across a 45°
    /// crease in steps under 30°.
    fn settled_facing(&self, scan: &Scan, angle: f64) -> Vec<Vec3> {
        let n = self.corners().len();
        let (wide, near) = (angle.cos(), (angle / 2.0).cos());
        let weighted: Vec<Vec3> = par::map(n, |t| {
            let (_, normal, area) = face_of(scan, t);
            normal * area
        });
        par::map(n, |t| {
            let mut ring = vec![t];
            let mut from = 0;
            for _ in 0..RINGS {
                let to = ring.len();
                for i in from..to {
                    for m in self.neighbours(ring[i]) {
                        if !ring.contains(&m) {
                            ring.push(m);
                        }
                    }
                }
                from = to;
            }
            let mut facing = weighted[t].normalized().unwrap_or(Vec3::Z);
            for k in 0..SETTLE {
                let within = if k == 0 { wide } else { near };
                let sum = ring
                    .iter()
                    .filter_map(|&m| weighted[m].normalized().filter(|u| u.dot(facing) >= within).map(|_| weighted[m]))
                    .fold(Vec3::ZERO, |s, w| s + w);
                facing = sum.normalized().unwrap_or(facing);
            }
            facing
        })
    }

    /// Take the pieces of the scan that `taken` closes in (every edge round them onto it, none on
    /// the scan's rim) of at most `SPECK` triangles, or of at most `most` that face, on the whole
    /// (`facing`), within `angle` of the region round them: the noise turned them too far to be grown into, and left
    /// as specks they would be holes in what a tool is given. A small face of its own (a can's
    /// end, closed in by its wall) faces another way and is left out.
    fn close_in(&self, taken: &mut [bool], most: usize, facing: impl Fn(usize) -> Vec3, angle: f64) {
        let cos = angle.cos();
        let mut seen = vec![false; taken.len()];
        for s in 0..taken.len() {
            if taken[s] || seen[s] {
                continue;
            }
            seen[s] = true;
            let mut piece = vec![s];
            // Closed: no edge on the scan's rim; and beside the region (a loose closed speck
            // elsewhere touches nothing taken).
            let (mut closed, mut beside, mut round) = (true, false, Vec3::ZERO);
            let mut i = 0;
            while let Some(&t) = piece.get(i) {
                i += 1;
                for &m in self.across.get(t).into_iter().flatten() {
                    if m == NONE {
                        closed = false;
                        continue;
                    }
                    let m = m as usize;
                    if taken[m] {
                        beside = true;
                        round += facing(m);
                    } else if !seen[m] {
                        seen[m] = true;
                        piece.push(m);
                    }
                }
            }
            if !closed || !beside || piece.len() > most {
                continue;
            }
            // A speck of a few triangles is noise whichever way it faces; a larger piece only when
            // it faces with the region.
            let along = || match (round.normalized(), piece.iter().fold(Vec3::ZERO, |s, &t| s + facing(t)).normalized()) {
                (Some(round), Some(own)) => round.dot(own) >= cos,
                _ => false,
            };
            if piece.len() <= SPECK || along() {
                for t in piece {
                    taken[t] = true;
                }
            }
        }
    }

    /// The triangles inside the polygon `ring` (world places), as the view sees them: their
    /// middles inside the polygon on the picture, and (unless `through`) not hidden behind other
    /// triangles of the scan there.
    pub fn seen_inside(&self, scan: &Scan, ring: &[Vec3], view: &View, through: bool) -> Vec<bool> {
        let mut taken = vec![false; self.corners().len()];
        let Some(d) = view.direction.normalized() else { return taken };
        let Some(u) = d.cross(if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).normalized() else { return taken };
        let v = d.cross(u);
        // A place on the picture, and how deep it is.
        let picture = |p: Vec3| -> Option<([f64; 2], f64)> {
            match view.eye {
                Some(eye) => {
                    let q = p - eye;
                    let depth = q.dot(d);
                    (depth > 1e-9).then(|| ([q.dot(u) / depth, q.dot(v) / depth], depth))
                }
                None => Some(([p.dot(u), p.dot(v)], p.dot(d))),
            }
        };
        let polygon: Vec<[f64; 2]> = ring.iter().filter_map(|p| picture(*p).map(|(q, _)| q)).collect();
        if polygon.len() < 3 {
            return taken;
        }
        let (lo, hi) = polygon.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), q| {
            ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
        });
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        if !(span > 0.0) {
            return taken;
        }
        let cell = span / RASTER as f64;
        let (w, h) = (((hi[0] - lo[0]) / cell).ceil() as usize + 1, ((hi[1] - lo[1]) / cell).ceil() as usize + 1);
        let at = |q: [f64; 2]| -> Option<usize> {
            let (x, y) = ((q[0] - lo[0]) / cell, (q[1] - lo[1]) / cell);
            (x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h).then(|| y as usize * w + x as usize)
        };
        // The polygon filled row by row.
        let mut mask = vec![false; w * h];
        for row in 0..h {
            let y = lo[1] + (row as f64 + 0.5) * cell;
            let mut xs: Vec<f64> = (0..polygon.len())
                .filter_map(|k| {
                    let (a, b) = (polygon[k], polygon[(k + 1) % polygon.len()]);
                    ((a[1] > y) != (b[1] > y)).then(|| a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]))
                })
                .collect();
            xs.sort_by(f64::total_cmp);
            for pair in xs.chunks(2) {
                if let [x0, x1] = *pair {
                    let (c0, c1) =
                        (((x0 - lo[0]) / cell - 0.5).ceil().max(0.0) as usize, (((x1 - lo[0]) / cell - 0.5).floor().max(-1.0) + 1.0) as usize);
                    for c in c0..c1.min(w) {
                        mask[row * w + c] = true;
                    }
                }
            }
        }
        // The middles inside the polygon, on the picture, and how deep they are.
        let projected: Vec<Option<([f64; 2], f64)>> = par::map(self.corners().len(), |t| {
            let mid = picture(self.middle(t))?;
            mask[at(mid.0)?].then_some(mid)
        });
        if through {
            for (t, p) in projected.iter().enumerate() {
                taken[t] = p.is_some();
            }
            return taken;
        }
        // The nearest depth in each cell: every triangle's cells (those whose middles it covers,
        // and the one its middle is in), so nothing behind the scan's near side shows.
        let mut depth = vec![f64::INFINITY; w * h];
        for t in 0..self.corners().len() {
            let Some((mid, dm)) = picture(self.middle(t)) else { continue };
            if let Some(i) = at(mid) {
                depth[i] = depth[i].min(dm);
            }
            let Some(tri) = scan.corners(t) else { continue };
            let pts: Vec<([f64; 2], f64)> = tri.iter().filter_map(|p| picture(*p)).collect();
            let [(p0, z0), (p1, z1), (p2, z2)] = pts[..] else { continue };
            let (bx0, bx1) = (p0[0].min(p1[0]).min(p2[0]), p0[0].max(p1[0]).max(p2[0]));
            let (by0, by1) = (p0[1].min(p1[1]).min(p2[1]), p0[1].max(p1[1]).max(p2[1]));
            if bx1 < lo[0] || by1 < lo[1] || bx0 > hi[0] + cell || by0 > hi[1] + cell {
                continue;
            }
            let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (p1[1] - p0[1]);
            if area.abs() < 1e-300 {
                continue;
            }
            let (cx0, cx1) = (((bx0 - lo[0]) / cell - 0.5).ceil().max(0.0) as usize, ((bx1 - lo[0]) / cell - 0.5).floor());
            let (cy0, cy1) = (((by0 - lo[1]) / cell - 0.5).ceil().max(0.0) as usize, ((by1 - lo[1]) / cell - 0.5).floor());
            if cx1 < 0.0 || cy1 < 0.0 {
                continue;
            }
            for cy in cy0..=(cy1 as usize).min(h - 1) {
                for cx in cx0..=(cx1 as usize).min(w - 1) {
                    let q = [lo[0] + (cx as f64 + 0.5) * cell, lo[1] + (cy as f64 + 0.5) * cell];
                    let w1 = ((q[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (q[1] - p0[1])) / area;
                    let w2 = ((p1[0] - p0[0]) * (q[1] - p0[1]) - (q[0] - p0[0]) * (p1[1] - p0[1])) / area;
                    let w0 = 1.0 - w1 - w2;
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                        let z = w0 * z0 + w1 * z1 + w2 * z2;
                        let i = cy * w + cx;
                        depth[i] = depth[i].min(z);
                    }
                }
            }
        }
        // Seen: no deeper than the nearest in its cell, give or take the scan's own roughness
        // and the slope across a cell.
        for (t, p) in projected.iter().enumerate() {
            let Some((mid, dm)) = *p else { continue };
            let Some(i) = at(mid) else { continue };
            let across_cell = if view.eye.is_some() { cell * dm } else { cell };
            taken[t] = dm <= depth[i] + 2.0 * self.edge + 2.0 * across_cell;
        }
        taken
    }

    /// The selected triangles in pieces: each piece the selected triangles reached from one
    /// another across edges, largest area first (ties by their first triangle, so the same flags
    /// give the same order).
    pub fn pieces(&self, selected: &[bool]) -> Vec<Vec<usize>> {
        let on = |t: usize| selected.get(t).copied().unwrap_or(false);
        let mut seen = vec![false; self.corners().len()];
        let mut pieces: Vec<(f64, Vec<usize>)> = Vec::new();
        for start in 0..seen.len() {
            if seen[start] || !on(start) {
                continue;
            }
            seen[start] = true;
            let mut piece = vec![start];
            let mut next = 0;
            while let Some(&t) = piece.get(next) {
                next += 1;
                for n in self.neighbours(t) {
                    if !seen[n] && on(n) {
                        seen[n] = true;
                        piece.push(n);
                    }
                }
            }
            let area = piece.iter().map(|&t| self.area(t)).sum();
            pieces.push((area, piece));
        }
        pieces.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1[0].cmp(&b.1[0])));
        pieces.into_iter().map(|(_, piece)| piece).collect()
    }

    /// The selection grown by `rings` rings of triangles (those sharing a corner with it), or
    /// shrunk by them when `rings` is negative.
    pub fn rings(&self, selected: &mut [bool], rings: i32) {
        let grow = rings > 0;
        for _ in 0..rings.unsigned_abs() {
            // The corners where selected and unselected triangles meet: a ring is the triangles
            // round them on the side that changes.
            let mut on = vec![0u8; self.fan_start.len().saturating_sub(1)];
            for (t, c) in self.corners().iter().enumerate() {
                let bit = if selected.get(t).copied().unwrap_or(false) { 1 } else { 2 };
                for &v in c {
                    if let Some(o) = on.get_mut(v as usize) {
                        *o |= bit;
                    }
                }
            }
            let mut changed = false;
            for (t, c) in self.corners().iter().enumerate() {
                if selected.get(t).copied().unwrap_or(false) != grow && c.iter().any(|&v| on.get(v as usize) == Some(&3)) {
                    selected[t] = grow;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The selection's outlines: the loops of the edges between a selected triangle and one that
    /// is not (or none), each as its corners in order, longest first.
    pub fn outlines(&self, scan: &Scan, selected: &[bool]) -> Vec<Vec<Vec3>> {
        let on = |t: u32| t != NONE && selected.get(t as usize).copied().unwrap_or(false);
        // Each outline edge from corner a to corner b, as the selected triangle winds it.
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for (t, c) in self.corners().iter().enumerate() {
            if !selected.get(t).copied().unwrap_or(false) {
                continue;
            }
            for k in 0..3 {
                if !on(self.across[t][k]) {
                    edges.push((c[k], c[(k + 1) % 3]));
                }
            }
        }
        edges.sort_unstable();
        let mut used = vec![false; edges.len()];
        let mut loops: Vec<Vec<Vec3>> = Vec::new();
        for first in 0..edges.len() {
            if used[first] {
                continue;
            }
            let mut ring = Vec::new();
            let mut at = first;
            loop {
                used[at] = true;
                let (a, b) = edges[at];
                if let Some(p) = scan.positions.get(a as usize) {
                    ring.push(*p);
                }
                if b == edges[first].0 {
                    break;
                }
                // The next edge out of b not yet walked.
                let start = edges.partition_point(|e| e.0 < b);
                match (start..edges.len()).take_while(|&i| edges[i].0 == b).find(|&i| !used[i]) {
                    Some(next) => at = next,
                    None => break,
                }
            }
            if ring.len() >= 3 {
                loops.push(ring);
            }
        }
        loops.sort_by(|a, b| loop_length(b).total_cmp(&loop_length(a)));
        loops
    }

    /// The selection's outline smoothed over `width` mm (the shrinking taken back) and laid on
    /// the scan, each a closed line, its first place repeated at its end: the longest alone, or
    /// with `every` each hole's too (a hole of fewer than six triangle sides left out). And how
    /// many outlines were left out.
    pub fn edge_lines(&self, scan: &Scan, selected: &[bool], every: bool, width: f64, reach: f64) -> Result<(Vec<Vec<Vec3>>, usize), EdgeProblem> {
        let outlines = self.outlines(scan, selected);
        if outlines.is_empty() {
            return Err(EdgeProblem::NoEdge);
        }
        let kept: Vec<&Vec<Vec3>> = outlines.iter().filter(|l| l.len() >= SHORTEST_OUTLINE).collect();
        if kept.is_empty() {
            return Err(EdgeProblem::TooShort);
        }
        let chosen = if every { kept.clone() } else { kept[..1].to_vec() };
        let mut lines = Vec::with_capacity(chosen.len());
        for outline in chosen {
            // Four places for each triangle side along it, between 32 and PLACES.
            let count = (4 * outline.len()).clamp(32, PLACES);
            let Some(places) = resampled(outline, count) else { continue };
            let spacing = loop_length(outline) / count as f64;
            let mut laid = Vec::with_capacity(count + 1);
            for p in smoothed(&places, spacing, width) {
                match self.nearest_place(scan, p) {
                    Some((q, d)) if d <= reach => laid.push(q),
                    _ => return Err(EdgeProblem::LeavesScan),
                }
            }
            if let Some(&first) = laid.first() {
                laid.push(first);
            }
            lines.push(laid);
        }
        let left = outlines.len() - lines.len();
        Ok((lines, left))
    }

    /// The triangles inside the closed curve `curve` drawn on the scan: those under it that lie
    /// inside it, and those reached from them across the scan without crossing it, so the far
    /// side of a part is not taken. Inside is judged on the plane the curve lies most nearly in.
    pub fn inside_curve(&self, curve: &[Vec3], reach: f64) -> Result<Vec<bool>, InsideProblem> {
        let length: f64 = curve.windows(2).map(|w| w[0].dist(w[1])).sum();
        let closed = curve.len() >= 4 && curve.first().zip(curve.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6 * length.max(1.0));
        if !closed {
            return Err(InsideProblem::Open);
        }
        let ring = &curve[..curve.len() - 1];
        // The curve's plane: its middle and its normal (Newell's), the polygon on it.
        let middle = ring.iter().fold(Vec3::ZERO, |s, p| s + *p) / ring.len() as f64;
        let normal = (0..ring.len()).fold(Vec3::ZERO, |s, i| s + (ring[i] - middle).cross(ring[(i + 1) % ring.len()] - middle));
        let n = normal.normalized().ok_or(InsideProblem::Nothing)?;
        let u = n.cross(if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).normalized().ok_or(InsideProblem::Nothing)?;
        let v = n.cross(u);
        let flat = |p: Vec3| [(p - middle).dot(u), (p - middle).dot(v)];
        let polygon: Vec<[f64; 2]> = ring.iter().map(|p| flat(*p)).collect();
        // The triangles under the curve, from places a third of a triangle side apart.
        let step = (self.edge / 3.0).max(length / 200_000.0);
        let mut under = vec![false; self.corners().len()];
        for w in curve.windows(2) {
            let parts = (w[0].dist(w[1]) / step).ceil().max(1.0) as usize;
            for k in 0..parts {
                let p = w[0] + (w[1] - w[0]) * (k as f64 / parts as f64);
                match self.nearest(p) {
                    Some((t, d)) if d <= reach => under[t] = true,
                    _ => return Err(InsideProblem::OffTheScan),
                }
            }
        }
        let inside = |t: usize| inside_polygon(flat(self.middle(t)), &polygon);
        let seeds: Vec<usize> = (0..self.corners().len())
            .filter(|&t| under[t])
            .flat_map(|t| self.neighbours(t).collect::<Vec<_>>())
            .filter(|&n| !under[n] && inside(n))
            .collect();
        let mut taken = self.flood(seeds, |_, n| !under[n]);
        for t in 0..taken.len() {
            if under[t] && inside(t) {
                taken[t] = true;
            }
        }
        if !taken.iter().any(|&t| t) {
            return Err(InsideProblem::Nothing);
        }
        Ok(taken)
    }
}

/// What a place on the scan snaps to, in the order it is preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    /// A corner: a vertex where feature edges (the scan's edge or creases) meet at an angle.
    Corner,
    /// The scan's own edge (a side of one triangle only).
    Boundary,
    /// A sharp fold: the two triangles meet at more than the crease angle.
    Crease,
    /// A vertex of the mesh.
    Vertex,
}

/// A place a click snaps to on the scan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub at: Vec3,
    pub kind: SnapKind,
}

/// How far a place is from `p`: straight, or (`toward`, a unit view direction) across the line of
/// sight through `p`, as the screen shows it.
#[derive(Clone, Copy)]
struct Off {
    p: Vec3,
    toward: Option<Vec3>,
}

impl Off {
    /// `x - p` with its part along the line of sight taken out.
    fn across(self, x: Vec3) -> Vec3 {
        let v = x - self.p;
        self.toward.map_or(v, |d| v - d * v.dot(d))
    }

    fn of(self, x: Vec3) -> f64 {
        self.across(x).len()
    }

    /// The point of the segment `a` to `b` nearest `p` so measured.
    fn on_segment(self, a: Vec3, b: Vec3) -> Vec3 {
        let (sa, sb) = (self.across(a), self.across(b));
        let d = sb - sa;
        let l2 = d.len2();
        if l2 <= 1e-24 { a } else { a + (b - a) * (-sa.dot(d) / l2).clamp(0.0, 1.0) }
    }
}

/// The most triangles a snap looks over round the place clicked (a far view of a fine scan).
const SNAP_MOST: usize = 60_000;

impl Topology {
    /// The triangles of the scan within `reach` of `p`, reached across it from `t` (the one under
    /// `p`) through corners within reach: the surface round the place, not the far side of a thin
    /// part. At most [`SNAP_MOST`], nearest rings first.
    fn round(&self, t: usize, off: Off, reach: f64) -> Vec<usize> {
        let scan = self.scan();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::from([t]);
        let mut out = vec![t];
        let mut k = 0;
        while k < out.len() && out.len() < SNAP_MOST {
            let f = out[k];
            k += 1;
            let (Some(ids), Some(places)) = (self.corners().get(f), scan.corners(f)) else { continue };
            for (&v, at) in ids.iter().zip(places) {
                if off.of(at) > reach {
                    continue;
                }
                for &n in self.fan_of(v) {
                    if seen.insert(n as usize) {
                        out.push(n as usize);
                    }
                }
            }
        }
        out.truncate(SNAP_MOST);
        out
    }

    /// What `p` (a place on the scan) snaps to within `reach` mm: a corner (a vertex on the scan's
    /// outline or on a crease where they turn), else the nearest point of the scan's edge or of a
    /// crease (two triangles meeting at more than `crease_degrees`), else the nearest vertex within
    /// half the reach. Looks over the scan round `p` as far as the reach (reached across the scan
    /// from the triangle under it), so it costs the same on a scan of millions of triangles and
    /// finds an edge a few pixels away however fine the scan is. `None` when nothing is within
    /// reach.
    pub fn snap(&self, p: Vec3, reach: f64, crease_degrees: f64) -> Option<Snap> {
        self.snap_seen(p, None, reach, crease_degrees)
    }

    /// [`Self::snap`] for a click seen along the unit direction `toward`: the reach is measured
    /// across the line of sight through `p`, as the pointer's reach on the screen is. A fold seen
    /// nearly edge-on is a few pixels from a click on the scan beside it, though the click lands
    /// far from it along the line of sight.
    pub fn snap_seen(&self, p: Vec3, toward: Option<Vec3>, reach: f64, crease_degrees: f64) -> Option<Snap> {
        if !(reach.is_finite() && reach > 0.0) {
            return None;
        }
        let toward = toward.and_then(|d| d.normalized());
        let off = Off { p, toward };
        let (t, _) = self.nearest(p)?;
        let scan = self.scan();
        let near = self.round(t, off, reach);
        let cos = crease_degrees.to_radians().cos();
        // The ways the feature edges leave each corner within reach, and the nearest edge.
        let mut leaving: std::collections::HashMap<u32, (Vec3, Vec<Vec3>)> = std::collections::HashMap::new();
        let mut best_edge: Option<(f64, Vec3, SnapKind)> = None;
        let mut best_vertex: Option<(f64, Vec3)> = None;
        for &f in &near {
            let (Some([a, b, c]), Some(&[ia, ib, ic])) = (scan.corners(f), self.corners().get(f)) else { continue };
            let own = face_of(&scan, f).1;
            for (k, ((x, ix), (y, iy))) in [((a, ia), (b, ib)), ((b, ib), (c, ic)), ((c, ic), (a, ia))].into_iter().enumerate() {
                let across = self.across.get(f).map_or(NONE, |e| e[k]);
                let kind = if across == NONE {
                    Some(SnapKind::Boundary)
                } else {
                    let other = face_of(&scan, across as usize).1;
                    (own.dot(other) < cos).then_some(SnapKind::Crease)
                };
                let Some(kind) = kind else { continue };
                if let Some(d) = (y - x).normalized() {
                    for (v, at, way) in [(ix, x, d), (iy, y, d * -1.0)] {
                        if off.of(at) <= reach {
                            leaving.entry(v).or_insert_with(|| (at, Vec::new())).1.push(way);
                        }
                    }
                }
                let q = off.on_segment(x, y);
                let d = off.of(q);
                if d <= reach && best_edge.is_none_or(|(bd, _, _)| d < bd) {
                    best_edge = Some((d, q, kind));
                }
            }
            for v in [a, b, c] {
                let d = off.of(v);
                if d <= reach / 2.0 && best_vertex.is_none_or(|(bd, _)| d < bd) {
                    best_vertex = Some((d, v));
                }
            }
        }
        // A corner is where two feature edges leave at more than the crease angle apart (a straight
        // run has its two ways opposite: no corner; the same edge seen from both its triangles
        // leaves the same way: not two edges).
        let turns = |ways: &[Vec3]| ways.iter().enumerate().any(|(i, d)| ways[i + 1..].iter().any(|e| d.dot(*e) > -cos && d.dot(*e) < cos));
        let corner = leaving
            .values()
            .filter(|(_, ways)| turns(ways))
            .map(|(at, _)| (off.of(*at), *at))
            .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.x.total_cmp(&b.1.x)));
        if let Some((_, at)) = corner {
            return Some(Snap { at, kind: SnapKind::Corner });
        }
        if let Some((_, at, kind)) = best_edge {
            return Some(Snap { at, kind });
        }
        best_vertex.map(|(_, at)| Snap { at, kind: SnapKind::Vertex })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, revolved};

    /// A ball of radius 20 about the origin, its poles on Z.
    fn ball() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let profile: Vec<[f64; 2]> = (0..=48)
            .map(|k| {
                let a = std::f64::consts::PI * f64::from(k) / 48.0;
                [-20.0 * a.cos(), 20.0 * a.sin()]
            })
            .collect();
        revolved(&profile, 96, false)
    }

    /// How far from the pole (+Z) a place is, in degrees.
    fn polar(p: Vec3) -> f64 {
        (p.z / p.len()).clamp(-1.0, 1.0).acos().to_degrees()
    }

    fn count(selected: &[bool]) -> usize {
        selected.iter().filter(|s| **s).count()
    }

    /// A floor (z = 0, x < 0) folding up by 40 degrees at x = 0, seen nearly along the floor: a
    /// click on the floor 8 mm short of the fold is a few tenths of a millimetre from it on the
    /// screen, so it snaps to the fold measured across the line of sight, and not by straight
    /// distance.
    #[test]
    fn a_fold_seen_nearly_edge_on_is_snapped_to() {
        let (n, cell) = (40usize, 0.5);
        let rise = 40f64.to_radians();
        let mut positions = Vec::new();
        for i in 0..=2 * n {
            for j in 0..=20 {
                let x = (i as f64 - n as f64) * cell;
                let y = j as f64 * cell;
                positions.push(if x <= 0.0 { Vec3::new(x, y, 0.0) } else { Vec3::new(x * rise.cos(), y, x * rise.sin()) });
            }
        }
        let at = |i: usize, j: usize| (i * 21 + j) as u32;
        let mut triangles = Vec::new();
        for i in 0..2 * n {
            for j in 0..20 {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let topology = Topology::new(&Scan::new(&positions, &triangles));
        let toward = Vec3::new(1.0, 0.0, -0.05);
        let click = Vec3::new(-8.0, 5.0, 0.0);
        let fold = topology.snap_seen(click, Some(toward), 1.0, 30.0).expect("the fold is within reach on the screen");
        assert_eq!(fold.kind, SnapKind::Crease);
        assert!(fold.at.x.abs() < 1e-6 && fold.at.z.abs() < 1e-6 && (fold.at.y - 5.0).abs() < 0.5, "on the fold: {:?}", fold.at);
        let not_fold = |s: Option<Snap>| s.is_none_or(|s| s.kind != SnapKind::Crease);
        assert!(not_fold(topology.snap(click, 1.0, 30.0)), "8 mm away in straight distance");
        // Seen from straight above, the same click is 8 mm from the fold on the screen too.
        assert!(not_fold(topology.snap_seen(click, Some(Vec3::new(0.0, 0.0, -1.0)), 1.0, 30.0)));
    }

    /// On a box a click near its top's edge snaps to the crease, near a corner to the corner, in
    /// the middle of the top to nothing; on an open sheet its edge is the boundary; and with no
    /// reach there is nothing to snap to.
    #[test]
    fn a_click_snaps_to_a_corner_a_crease_the_edge_or_nothing() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 30);
        let topology = Topology::new(&Scan::new(&positions, &triangles));
        let corner = topology.snap(Vec3::new(29.6, 19.7, 10.0), 1.0, 30.0).expect("near the corner");
        assert_eq!(corner.kind, SnapKind::Corner);
        assert!(corner.at.dist(Vec3::new(30.0, 20.0, 10.0)) < 1e-6, "{:?}", corner.at);
        let crease = topology.snap(Vec3::new(15.0, 19.7, 10.0), 1.0, 30.0).expect("near the top's edge");
        assert_eq!(crease.kind, SnapKind::Crease);
        assert!((crease.at.y - 20.0).abs() < 1e-6 && (crease.at.z - 10.0).abs() < 1e-6, "on the edge: {:?}", crease.at);
        let vertex = topology.snap(Vec3::new(15.1, 10.1, 10.0), 1.0, 30.0).expect("a vertex of the mesh is near");
        assert_eq!(vertex.kind, SnapKind::Vertex);
        assert!(vertex.at.dist(Vec3::new(15.0, 10.0, 10.0)) < 1e-6, "{:?}", vertex.at);
        assert!(topology.snap(Vec3::new(15.5, 10.5, 10.0), 1.0, 30.0).is_none(), "the middle of a flat cell is nothing near");
        assert!(topology.snap(Vec3::new(29.6, 19.7, 10.0), 0.0, 30.0).is_none() && topology.snap(Vec3::new(1.0, 1.0, 1.0), f64::NAN, 30.0).is_none());
        // An open sheet: one triangle pair, its rim the boundary.
        let sheet = Topology::new(&Scan::new(
            &[Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(0.0, 10.0, 0.0)],
            &[[0, 1, 2], [0, 2, 3]],
        ));
        let rim = sheet.snap(Vec3::new(5.0, 0.3, 0.0), 1.0, 30.0).expect("near the sheet's edge");
        assert_eq!(rim.kind, SnapKind::Boundary);
        assert!(rim.at.y.abs() < 1e-9, "{:?}", rim.at);
        assert!(sheet.snap(Vec3::new(5.0, 5.0, 0.0), 1.0, 30.0).is_none(), "the diagonal inside is no feature");
        // A fine sheet seen from afar: the reach spans many triangles, and the edge 1.5 mm away is
        // still found (not only the triangles touching the one under the click).
        let n = 200u32;
        let at = |i: u32, j: u32| Vec3::new(f64::from(i) * 0.1, f64::from(j) * 0.1, 0.0);
        let positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| at(i, j))).collect();
        let w = n + 1;
        let triangles: Vec<[u32; 3]> = (0..n)
            .flat_map(|j| {
                (0..n).flat_map(move |i| [[j * w + i, j * w + i + 1, (j + 1) * w + i + 1], [j * w + i, (j + 1) * w + i + 1, (j + 1) * w + i]])
            })
            .collect();
        let fine = Topology::new(&Scan::new(&positions, &triangles));
        let edge = fine.snap(Vec3::new(10.0, 1.53, 0.0), 2.0, 30.0).expect("the sheet's edge within reach");
        assert_eq!(edge.kind, SnapKind::Boundary);
        assert!(edge.at.y.abs() < 1e-9 && (edge.at.x - 10.0).abs() < 1e-9, "straight across onto it: {:?}", edge.at);
        let corner = fine.snap(Vec3::new(1.2, 1.1, 0.0), 2.0, 30.0).expect("the sheet's corner within reach");
        assert_eq!(corner.kind, SnapKind::Corner);
        assert!(corner.at.dist(Vec3::ZERO) < 1e-9, "{:?}", corner.at);
    }

    /// A can (a cylinder of radius 10, flat ends): from a click on its round wall, the region held to
    /// the click's facing takes a strip of the wall; the smooth region takes the whole wall, round
    /// to where it meets the ends, and neither end.
    #[test]
    fn a_smooth_region_takes_a_whole_round_wall() {
        let (positions, triangles) = crate::test_meshes::revolved(&[[0.0, 0.0], [0.0, 10.0], [20.0, 10.0], [20.0, 0.0]], 64, false);
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let seed = topology.nearest(Vec3::new(10.0, 0.0, 10.0)).unwrap().0;
        let wall = (0..topology.triangle_count()).filter(|&t| {
            let m = topology.middle(t);
            m.z > 1e-6 && m.z < 20.0 - 1e-6
        });
        let on_wall = wall.count();
        let strip = topology.region(seed, 30f64.to_radians());
        let whole = topology.smooth_region(seed, 30f64.to_radians());
        assert!(count(&strip) < on_wall / 2, "facing: a strip, {} of {on_wall}", count(&strip));
        assert_eq!(count(&whole), on_wall, "up to the creases: the whole wall");
        assert!((0..topology.triangle_count()).all(|t| !whole[t] || (topology.middle(t).z > 1e-6 && topology.middle(t).z < 20.0 - 1e-6)), "no end");
    }

    /// A finely meshed box: the smooth region from a click on its top takes the top and stops at
    /// its 90° edges (with directions averaged across the edges too, it took the whole box).
    #[test]
    fn a_smooth_region_stops_at_a_box_edge() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 30);
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let top = topology.smooth_region(topology.nearest(Vec3::new(15.0, 10.0, 10.0)).unwrap().0, 30f64.to_radians());
        let on_top = (0..topology.triangle_count()).filter(|&t| topology.middle(t).z > 10.0 - 1e-9).count();
        assert_eq!(count(&top), on_top, "the top and nothing else");
    }

    /// A countersunk part as a fine scan would have it: a flat top (Z = 10, out to R20), a 45° cone
    /// down to a drilled wall (R2), every point jittered by up to 0.03 mm. The noise tilts single
    /// triangles by up to 30° or so; from a click on the top the smooth region takes the whole top
    /// (no specks left out of it) and stops at the cone's 45° crease (it walked across it in steps
    /// under 30° and took the cone and the drilled wall); from the cone, the cone alone.
    #[test]
    fn a_smooth_region_on_a_noisy_scan_stops_at_a_countersink() {
        use crate::test_meshes::steps;
        let mut profile = steps([10.0, 20.0], [10.0, 6.0], 56);
        profile.extend(steps([10.0, 6.0], [6.0, 2.0], 22).into_iter().skip(1));
        profile.extend(steps([6.0, 2.0], [0.0, 2.0], 24).into_iter().skip(1));
        let (mut positions, triangles) = revolved(&profile, 256, false);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut jitter = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 0.06 - 0.03
        };
        for p in &mut positions {
            *p += Vec3::new(jitter(), jitter(), jitter());
        }
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let radius = |t: usize| {
            let m = topology.middle(t);
            m.x.hypot(m.y)
        };
        let angle = 30f64.to_radians();
        let top = topology.smooth_region(topology.nearest(Vec3::new(13.0, 0.0, 10.0)).unwrap().0, angle);
        let on_top: Vec<usize> = (0..topology.triangle_count()).filter(|&t| radius(t) > 6.2).collect();
        let below = (0..topology.triangle_count()).filter(|&t| top[t] && radius(t) < 5.8).count();
        let left_out = on_top.iter().filter(|&&t| !top[t]).count();
        assert_eq!(below, 0, "the cone and the drilled wall are not the top");
        assert_eq!(left_out, 0, "no specks of the top are left out, of {}", on_top.len());
        let cone = topology.smooth_region(topology.nearest(Vec3::new(4.0, 0.0, 8.0)).unwrap().0, angle);
        assert!(
            (0..topology.triangle_count()).all(|t| !cone[t] || (radius(t) > 1.8 && radius(t) < 6.2 && topology.middle(t).z > 5.8)),
            "the cone alone"
        );
        assert!(count(&cone) > 22 * 256, "the whole cone: {}", count(&cone));
    }

    /// A brush on a box's top takes a disc of it, never the bottom; the smooth region from the top
    /// stops at the box's edges; grown and shrunk again by a ring it is as it was.
    #[test]
    fn a_brush_and_a_region_keep_to_the_top_of_a_box() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 30);
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let brushed = topology.brush(Vec3::new(15.0, 10.0, 10.0), 4.0);
        assert!(count(&brushed) > 10);
        for (t, on) in brushed.iter().enumerate() {
            if *on {
                let m = topology.middle(t);
                assert!(m.z > 10.0 - 1e-9 && m.dist(Vec3::new(15.0, 10.0, 10.0)) <= 4.0, "on the top, within the brush: {m:?}");
            }
        }
        let top = topology.region(topology.nearest(Vec3::new(15.0, 10.0, 10.0)).unwrap().0, 30f64.to_radians());
        let on_top = (0..topology.triangle_count()).filter(|&t| topology.middle(t).z > 10.0 - 1e-9).count();
        assert_eq!(count(&top), on_top, "the whole top and nothing else");
        assert!((0..topology.triangle_count()).all(|t| !top[t] || topology.middle(t).z > 10.0 - 1e-9));
        let mut changed = brushed.clone();
        topology.rings(&mut changed, 2);
        assert!(count(&changed) > count(&brushed));
        topology.rings(&mut changed, -2);
        assert!((0..changed.len()).all(|t| changed[t] || !brushed[t]), "grown and shrunk by as many rings, it holds what it did");
        assert!(count(&changed) < count(&brushed) + count(&brushed) / 5, "and little more: {} from {}", count(&changed), count(&brushed));
        topology.rings(&mut changed, -100);
        assert_eq!(count(&changed), 0, "shrunk away");
        // Its outline is one loop round the top.
        let outlines = topology.outlines(&scan, &top);
        assert_eq!(outlines.len(), 1);
        assert!((loop_length(&outlines[0]) - 100.0).abs() < 1e-6, "the top's perimeter: {}", loop_length(&outlines[0]));
    }

    /// Two brushes apart on a box's top and one on its bottom are three pieces, the largest
    /// first; brushes that overlap are one piece; nothing selected is no piece.
    #[test]
    fn a_selection_falls_into_pieces_largest_first() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 30);
        let topology = Topology::new(&Scan::new(&positions, &triangles));
        let either = |a: &[bool], b: &[bool]| a.iter().zip(b).map(|(x, y)| *x || *y).collect::<Vec<_>>();
        let small = topology.brush(Vec3::new(5.0, 5.0, 10.0), 2.0);
        let big = topology.brush(Vec3::new(22.0, 12.0, 10.0), 4.0);
        let under = topology.brush(Vec3::new(15.0, 10.0, 0.0), 3.0);
        let all = either(&either(&small, &big), &under);
        let pieces = topology.pieces(&all);
        assert_eq!(pieces.len(), 3);
        let area = |p: &[usize]| p.iter().map(|&t| topology.area(t)).sum::<f64>();
        assert!(area(&pieces[0]) >= area(&pieces[1]) && area(&pieces[1]) >= area(&pieces[2]));
        assert!(pieces[0].iter().all(|&t| big[t]), "the largest is the big brush");
        assert!(pieces[2].iter().all(|&t| small[t]), "the smallest is the small one");
        assert_eq!(pieces.iter().map(Vec::len).sum::<usize>(), count(&all), "every selected triangle in one piece");
        let overlapping = either(&big, &topology.brush(Vec3::new(19.0, 10.0, 10.0), 4.0));
        assert_eq!(topology.pieces(&overlapping).len(), 1);
        assert!(topology.pieces(&vec![false; topology.triangle_count()]).is_empty());
    }

    /// A box seen from above through a polygon round all of it: what is seen is its top; through
    /// the scan, its bottom too. A polygon round half the top takes that half.
    #[test]
    fn a_polygon_takes_what_is_seen_or_everything_through() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 30);
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let down = View { direction: -Vec3::Z, eye: None };
        let round = [Vec3::new(-5.0, -5.0, 20.0), Vec3::new(35.0, -5.0, 20.0), Vec3::new(35.0, 25.0, 20.0), Vec3::new(-5.0, 25.0, 20.0)];
        let seen = topology.seen_inside(&scan, &round, &down, false);
        let bottom = (0..topology.triangle_count()).filter(|&t| seen[t] && topology.middle(t).z < 1e-9).count();
        let top = (0..topology.triangle_count()).filter(|&t| topology.middle(t).z > 10.0 - 1e-9).count();
        assert_eq!(bottom, 0, "the bottom is hidden");
        assert_eq!((0..topology.triangle_count()).filter(|&t| seen[t] && topology.middle(t).z > 10.0 - 1e-9).count(), top, "all the top is seen");
        let through = topology.seen_inside(&scan, &round, &down, true);
        assert_eq!(count(&through), topology.triangle_count(), "through the scan, all of it");
        let half = [Vec3::new(-5.0, -5.0, 20.0), Vec3::new(15.0, -5.0, 20.0), Vec3::new(15.0, 25.0, 20.0), Vec3::new(-5.0, 25.0, 20.0)];
        let left = topology.seen_inside(&scan, &half, &down, false);
        assert!((0..topology.triangle_count()).all(|t| !left[t] || topology.middle(t).x < 15.0 + 1e-9), "only the left half");
        // From an eye above, the same.
        let eye = View { direction: -Vec3::Z, eye: Some(Vec3::new(15.0, 10.0, 200.0)) };
        let seen = topology.seen_inside(&scan, &round, &eye, false);
        assert_eq!((0..topology.triangle_count()).filter(|&t| seen[t] && topology.middle(t).z < 1e-9).count(), 0, "in perspective too");
    }

    /// A ragged round selection on a ball (its triangles within 40° of an axis leaning 30° from
    /// the ball's own, so its outline is a staircase of triangle sides) gives back the circle it
    /// follows, smoothed and on the ball; a band of the ball has its second edge as a second
    /// curve when every edge is asked for.
    #[test]
    fn a_selections_edge_comes_back_as_its_circle() {
        let (positions, triangles) = ball();
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let axis = Vec3::new(30f64.to_radians().sin(), 0.0, 30f64.to_radians().cos());
        let from_axis = |p: Vec3| (p.dot(axis) / p.len()).clamp(-1.0, 1.0).acos().to_degrees();
        let cap: Vec<bool> = (0..topology.triangle_count()).map(|t| from_axis(topology.middle(t)) < 40.0).collect();
        let (lines, left) = topology.edge_lines(&scan, &cap, false, 2.0 * topology.edge, 1.0).unwrap();
        assert_eq!((lines.len(), left), (1, 0));
        // How far a place is from the 40° circle, along the ball.
        let off = |p: &Vec3| 20.0 * (from_axis(*p) - 40.0).abs().to_radians();
        let ragged = topology.outlines(&scan, &cap)[0].iter().map(off).fold(0.0, f64::max);
        let smooth = lines[0].iter().map(off).fold(0.0, f64::max);
        assert!(ragged > 0.6, "the triangles' outline is ragged: {ragged}");
        assert!(smooth < 0.4, "the curve is within 0.4 mm of the circle: {smooth} (ragged {ragged})");
        assert!(lines[0].iter().all(|p| (p.len() - 20.0).abs() < 0.05), "on the ball");
        assert!(lines[0].first().zip(lines[0].last()).is_some_and(|(a, b)| a == b), "closed");
        let band: Vec<bool> = (0..topology.triangle_count()).map(|t| (30.0..60.0).contains(&polar(topology.middle(t)))).collect();
        let (lines, _) = topology.edge_lines(&scan, &band, true, 2.0 * topology.edge, 1.0).unwrap();
        assert_eq!(lines.len(), 2, "the band's two edges");
        assert_eq!(topology.edge_lines(&scan, &vec![true; cap.len()], false, 1.0, 1.0), Err(EdgeProblem::NoEdge), "all of a closed ball");
    }

    /// A circle drawn 40° from the pole of a ball takes the cap inside it and nothing below it,
    /// the far side of the ball included; an open curve, or one off the ball, is refused.
    #[test]
    fn inside_a_circle_on_a_ball_is_its_cap() {
        let (positions, triangles) = ball();
        let scan = Scan::new(&positions, &triangles);
        let topology = Topology::new(&scan);
        let (r, z) = (20.0 * 40f64.to_radians().sin(), 20.0 * 40f64.to_radians().cos());
        let circle: Vec<Vec3> = (0..=200)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / 200.0;
                Vec3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect();
        let inside = topology.inside_curve(&circle, 1.0).unwrap();
        for (t, m) in (0..topology.triangle_count()).map(|t| (t, topology.middle(t))) {
            let a = polar(m);
            if a < 36.0 {
                assert!(inside[t], "inside the cap at {a}°");
            }
            if a > 44.0 {
                assert!(!inside[t], "nothing below it: {a}°");
            }
        }
        assert_eq!(topology.inside_curve(&circle[..150], 1.0), Err(InsideProblem::Open));
        let far: Vec<Vec3> = circle.iter().map(|p| *p + Vec3::new(0.0, 0.0, 30.0)).collect();
        assert_eq!(topology.inside_curve(&far, 1.0), Err(InsideProblem::OffTheScan));
    }
}
