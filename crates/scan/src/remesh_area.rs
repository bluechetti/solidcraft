//! Remesh within an area: the scan's triangles in an area made again as even triangles of about
//! one edge length, while every triangle outside it, and the area's edge with them, stays as it
//! is (Remesh itself makes the whole scan again from a grid, so part of a scan would leave a new
//! patch to stitch into the old one). Isotropic remeshing after Botsch and Kobbelt: edges longer
//! than four thirds of the length are split, those shorter than four fifths collapsed, edges
//! flipped towards six edges at each vertex, and the vertices moved along the surface to even it
//! and put back on the scan as it was. A vertex of a triangle outside the area, of the scan's open
//! rim, or of a crease (sharper than 70°, or 45° with `sharp`) stays where it is, so the area
//! keeps its outline and its edges.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::curve::Projector;

/// Rounds of split, collapse, flip and smoothing.
const ROUNDS: usize = 5;
/// Faces meeting at more than this (radians) make a sharp edge, kept with `sharp`.
const SHARP: f64 = std::f64::consts::FRAC_PI_4;
/// A crease kept even without `sharp` (a box's edge): moved across, its vertices would land on
/// either face and leave it ragged.
const CREASE: f64 = 70.0 * std::f64::consts::PI / 180.0;
/// The most triangles an area may come to (a cell far smaller than the scan's triangles).
pub const MOST_TRIANGLES: usize = 20_000_000;

/// What the area remeshed came to: the whole scan, its area's triangles even.
#[derive(Clone, Debug)]
pub struct AreaRemeshed {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// The area's triangles before and after.
    pub before: usize,
    pub after: usize,
    /// The area's new vertices' distance from the scan as it was: root mean square and largest.
    pub new_rms: f64,
    pub new_max: f64,
    /// The area's old vertices' distance from the new surface: root mean square, 95th percentile,
    /// and the share more than two lengths off it.
    pub scan_rms: f64,
    pub scan_p95: f64,
    pub uncovered: f64,
}

/// Why the area was not remeshed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaRemeshError {
    /// The area holds no triangle of the scan, or the length is not one.
    Empty,
    /// The length is so small against the area that it would make too many triangles.
    TooFine,
}

impl std::fmt::Display for AreaRemeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AreaRemeshError::Empty => "the area holds none of the scan's triangles",
            AreaRemeshError::TooFine => "the cell is too small for the area: it would make more than 20 million triangles",
        })
    }
}

/// The scan with the triangles `area` marks (one flag a triangle) made again as even triangles of
/// about `length` (mm); with `sharp`, its sharp edges kept.
pub fn remesh_area(scan: &Scan, area: &[bool], length: f64, sharp: bool) -> Result<AreaRemeshed, AreaRemeshError> {
    if !(length.is_finite() && length > 0.0) {
        return Err(AreaRemeshError::Empty);
    }
    let mut m = Work::new(scan, area);
    let before = m.area_count();
    if before == 0 {
        return Err(AreaRemeshError::Empty);
    }
    let area_size: f64 = (0..m.tris.len()).filter(|&t| m.alive[t] && m.in_area[t]).map(|t| m.tri_area(t)).sum();
    // An even triangle of side `length` covers √3/4 length².
    if area_size / (0.433 * length * length) > MOST_TRIANGLES as f64 {
        return Err(AreaRemeshError::TooFine);
    }
    let crease = if sharp { SHARP } else { CREASE };
    m.hold(crease);
    let on = Projector::new(scan);
    let (high, low) = (4.0 / 3.0 * length, 4.0 / 5.0 * length);
    for _ in 0..ROUNDS {
        m.split_long(high);
        m.collapse_short(low, high);
        m.flip_to_valence();
        m.smooth(&on);
    }
    // Last, edges flipped to the wider angles (Delaunay): where held vertices close together meet
    // longer new edges (the scan's rim), the triangles there would otherwise be slivers.
    for _ in 0..3 {
        if m.flip_to_angles() == 0 {
            break;
        }
    }
    let (positions, triangles, new_vertices) = m.compact();
    // How far the new area lies from the scan, and the scan from it.
    let mut off: Vec<f64> = new_vertices.iter().filter_map(|&v| positions.get(v as usize)).filter_map(|p| on.nearest(*p)).map(|(_, d)| d).collect();
    let (new_rms, new_max) = rms_max(&off);
    let made = Scan::new(&positions, &triangles);
    let back = Projector::new(&made);
    let olds: Vec<u32> = area_vertices(scan, area);
    off = olds.iter().filter_map(|&v| scan.positions.get(v as usize)).filter_map(|p| back.nearest(*p)).map(|(_, d)| d).collect();
    let (scan_rms, _) = rms_max(&off);
    off.sort_by(f64::total_cmp);
    let scan_p95 = off.get((off.len() * 95) / 100).or(off.last()).copied().unwrap_or(0.0);
    let uncovered = off.iter().filter(|d| **d > 2.0 * length).count() as f64 / off.len().max(1) as f64;
    let after = m.area_count();
    Ok(AreaRemeshed { positions, triangles, before, after, new_rms, new_max, scan_rms, scan_p95, uncovered })
}

/// Root mean square and largest of `values` (0 for none).
fn rms_max(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let rms = (values.iter().map(|d| d * d).sum::<f64>() / values.len() as f64).sqrt();
    (rms, values.iter().copied().fold(0.0, f64::max))
}

/// The vertices of the triangles `area` marks, each once.
fn area_vertices(scan: &Scan, area: &[bool]) -> Vec<u32> {
    let mut seen = vec![false; scan.positions.len()];
    let mut out = Vec::new();
    for (t, tri) in scan.triangles.iter().enumerate() {
        if area.get(t).copied().unwrap_or(false) {
            for &v in tri {
                if let Some(s) = seen.get_mut(v as usize)
                    && !*s
                {
                    *s = true;
                    out.push(v);
                }
            }
        }
    }
    out
}

/// The mesh as it is worked on: triangles that die are marked, not removed; each vertex knows its
/// triangles (the dead among them dropped as they are met).
struct Work {
    pos: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    alive: Vec<bool>,
    in_area: Vec<bool>,
    around: Vec<Vec<u32>>,
    held: Vec<bool>,
    gone: Vec<bool>,
    /// The area's creases as the scan had them (each edge's ends, lower first).
    creases: std::collections::HashSet<(u32, u32)>,
}

impl Work {
    fn new(scan: &Scan, area: &[bool]) -> Work {
        let weld = crate::welded(scan.positions);
        let pos: Vec<Vec3> = scan.positions.to_vec();
        let mut tris = Vec::with_capacity(scan.triangle_count());
        let mut in_area = Vec::with_capacity(scan.triangle_count());
        for (t, tri) in scan.triangles.iter().enumerate() {
            let w = tri.map(|v| weld.get(v as usize).copied().unwrap_or(v));
            tris.push(w);
            in_area.push(area.get(t).copied().unwrap_or(false));
        }
        let alive: Vec<bool> =
            tris.iter().enumerate().map(|(t, w)| scan.corners(t).is_some() && w[0] != w[1] && w[1] != w[2] && w[2] != w[0]).collect();
        let mut around = vec![Vec::new(); pos.len()];
        for (t, tri) in tris.iter().enumerate() {
            if alive[t] {
                for &v in tri {
                    if let Some(a) = around.get_mut(v as usize) {
                        a.push(t as u32);
                    }
                }
            }
        }
        let n = pos.len();
        Work { pos, tris, alive, in_area, around, held: vec![false; n], gone: vec![false; n], creases: Default::default() }
    }

    fn area_count(&self) -> usize {
        (0..self.tris.len()).filter(|&t| self.alive[t] && self.in_area[t]).count()
    }

    fn tri_area(&self, t: usize) -> f64 {
        let [a, b, c] = self.tris[t].map(|v| self.pos[v as usize]);
        (b - a).cross(c - a).len() / 2.0
    }

    fn normal(&self, t: usize) -> Option<Vec3> {
        let [a, b, c] = self.tris[t].map(|v| self.pos[v as usize]);
        (b - a).cross(c - a).normalized()
    }

    /// The live triangles round `v`.
    fn live(&self, v: u32) -> impl Iterator<Item = usize> + '_ {
        self.around.get(v as usize).into_iter().flatten().map(|&t| t as usize).filter(move |&t| self.alive[t] && self.tris[t].contains(&v))
    }

    /// The live triangles with the edge `a`–`b`, each once (a vertex's list can hold a triangle
    /// twice: one that left it and came back, as a flip back does).
    fn edge_tris(&self, a: u32, b: u32) -> Vec<usize> {
        let mut out: Vec<usize> = self.live(a).filter(|&t| self.tris[t].contains(&b)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The live triangles round `v`, each once.
    fn live_once(&self, v: u32) -> Vec<usize> {
        let mut out: Vec<usize> = self.live(v).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The vertices joined to `v` by an edge.
    fn ring(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self.live(v).flat_map(|t| self.tris[t]).filter(|&w| w != v).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Vertices that stay where they are: of a triangle outside the area, of the open rim (an edge
    /// of one triangle, or of three or more), and of a crease sharper than `crease` (radians).
    fn hold(&mut self, crease: f64) {
        for t in 0..self.tris.len() {
            if self.alive[t] && !self.in_area[t] {
                for v in self.tris[t] {
                    self.held[v as usize] = true;
                }
            }
        }
        for t in 0..self.tris.len() {
            if !(self.alive[t] && self.in_area[t]) {
                continue;
            }
            let tri = self.tris[t];
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let faces = self.edge_tris(a, b);
                let open = faces.len() != 2;
                let creased = faces.len() == 2
                    && match (self.normal(faces[0]), self.normal(faces[1])) {
                        (Some(n), Some(m)) => n.dot(m) < crease.cos(),
                        _ => true,
                    };
                if open || creased {
                    self.held[a as usize] = true;
                    self.held[b as usize] = true;
                }
                if creased {
                    self.creases.insert((a.min(b), a.max(b)));
                }
            }
        }
    }

    /// The edges inside the area: both triangles in it.
    fn area_edges(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for t in 0..self.tris.len() {
            if !(self.alive[t] && self.in_area[t]) {
                continue;
            }
            let tri = self.tris[t];
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if a < b {
                    out.push((a, b));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&(a, b)| {
            let faces = self.edge_tris(a, b);
            faces.len() == 2 && faces.iter().all(|&t| self.in_area[t])
        });
        out
    }

    fn len(&self, a: u32, b: u32) -> f64 {
        self.pos[a as usize].dist(self.pos[b as usize])
    }

    fn add_vertex(&mut self, p: Vec3) -> u32 {
        self.pos.push(p);
        self.around.push(Vec::new());
        self.held.push(false);
        self.gone.push(false);
        (self.pos.len() - 1) as u32
    }

    fn add_tri(&mut self, tri: [u32; 3], in_area: bool) -> usize {
        self.tris.push(tri);
        self.alive.push(true);
        self.in_area.push(in_area);
        let t = self.tris.len() - 1;
        for v in tri {
            self.around[v as usize].push(t as u32);
        }
        t
    }

    /// Whether the edge `a`–`b` is one of the scan's creases: it is neither split (a middle point
    /// would be free and wander off it) nor flipped. Read once from the scan as it was: a sliver
    /// made on the way would look like one.
    fn creased(&self, a: u32, b: u32) -> bool {
        self.creases.contains(&(a.min(b), a.max(b)))
    }

    /// Split every edge longer than `high` at its middle (both triangles in two).
    fn split_long(&mut self, high: f64) {
        for (a, b) in self.area_edges() {
            if self.len(a, b) <= high || self.tris.len() > 4 * MOST_TRIANGLES || self.creased(a, b) {
                continue;
            }
            let faces = self.edge_tris(a, b);
            if faces.len() != 2 {
                continue;
            }
            let m = self.add_vertex((self.pos[a as usize] + self.pos[b as usize]) * 0.5);
            for t in faces {
                let tri = self.tris[t];
                // In `t` the end after the other on its winding becomes the middle; the new
                // triangle takes the other end's place, so both keep the winding.
                let first = tri.map(|v| if v == b { m } else { v });
                let second = tri.map(|v| if v == a { m } else { v });
                self.tris[t] = first;
                self.around[m as usize].push(t as u32);
                self.add_tri(second, true);
            }
        }
    }

    /// Collapse every edge shorter than `low` into one of its ends, when no edge it leaves is longer
    /// than `high`, no triangle turns over, and the mesh stays a surface there.
    fn collapse_short(&mut self, low: f64, high: f64) {
        for (a, b) in self.area_edges() {
            if self.gone[a as usize] || self.gone[b as usize] || self.len(a, b) >= low {
                continue;
            }
            // The free end goes into the other; a free pair meets at its middle.
            let (from, to, at) = match (self.held[a as usize], self.held[b as usize]) {
                (true, true) => continue,
                (true, false) => (b, a, self.pos[a as usize]),
                (false, true) => (a, b, self.pos[b as usize]),
                (false, false) => (a, b, (self.pos[a as usize] + self.pos[b as usize]) * 0.5),
            };
            let faces = self.edge_tris(from, to);
            if faces.len() != 2 || faces.iter().any(|&t| !self.in_area[t]) {
                continue;
            }
            // The link: the two ends share exactly the two opposite corners.
            let (ring_from, ring_to) = (self.ring(from), self.ring(to));
            let shared = ring_from.iter().filter(|v| ring_to.contains(v)).count();
            if shared != 2 {
                continue;
            }
            if ring_from.iter().chain(&ring_to).any(|&w| w != from && w != to && self.pos[w as usize].dist(at) > high) {
                continue;
            }
            // No triangle round either end turns over or goes flat.
            let turns = |w: &Work, v: u32| {
                w.live(v).filter(|t| !faces.contains(t)).any(|t| {
                    let before = w.normal(t);
                    let moved = w.tris[t].map(|x| if x == from || x == to { at } else { w.pos[x as usize] });
                    let after = (moved[1] - moved[0]).cross(moved[2] - moved[0]).normalized();
                    match (before, after) {
                        (Some(n), Some(m)) => n.dot(m) < 0.5,
                        _ => true,
                    }
                })
            };
            if turns(self, from) || turns(self, to) {
                continue;
            }
            for &t in &faces {
                self.alive[t] = false;
            }
            let moving = self.live_once(from);
            for t in moving {
                self.tris[t] = self.tris[t].map(|x| if x == from { to } else { x });
                self.around[to as usize].push(t as u32);
            }
            self.pos[to as usize] = at;
            self.gone[from as usize] = true;
            self.around[from as usize].clear();
        }
    }

    /// Flip each edge whose flip brings its four corners nearer six edges each (fewer at the
    /// area's edge), when neither new triangle turns over.
    fn flip_to_valence(&mut self) {
        for (a, b) in self.area_edges() {
            // Two triangles in the area, or the edge is not one to flip.
            let [t1, t2] = match self.edge_tris(a, b)[..] {
                [t1, t2] if self.in_area[t1] && self.in_area[t2] => [t1, t2],
                _ => continue,
            };
            if self.creased(a, b) {
                continue;
            }
            let other = |t: usize| self.tris[t].into_iter().find(|&v| v != a && v != b);
            let (Some(c), Some(d)) = (other(t1), other(t2)) else { continue };
            if c == d || self.ring(c).contains(&d) {
                continue;
            }
            let target = |v: u32| if self.held[v as usize] { 4 } else { 6 };
            let valence = |v: u32| self.ring(v).len() as i64;
            let off = |v: u32, change: i64| (valence(v) + change - target(v)).abs();
            let before = off(a, 0) + off(b, 0) + off(c, 0) + off(d, 0);
            let after = off(a, -1) + off(b, -1) + off(c, 1) + off(d, 1);
            if after >= before {
                continue;
            }
            // `t1` runs a→b; the new pair keeps the winding: (a, d, c) and (b, c, d).
            let runs_ab = |t: usize| {
                let tri = self.tris[t];
                (0..3).any(|k| tri[k] == a && tri[(k + 1) % 3] == b)
            };
            let (t_ab, t_ba) = if runs_ab(t1) { (t1, t2) } else { (t2, t1) };
            let (c, d) = (other(t_ab).unwrap_or(c), other(t_ba).unwrap_or(d));
            let new1 = [a, d, c];
            let new2 = [b, c, d];
            let n_old = match (self.normal(t_ab), self.normal(t_ba)) {
                (Some(n), Some(m)) => n + m,
                _ => continue,
            };
            let face_normal = |tri: [u32; 3]| {
                let [p, q, r] = tri.map(|v| self.pos[v as usize]);
                (q - p).cross(r - p).normalized()
            };
            let ok = |tri| face_normal(tri).is_some_and(|n| n.dot(n_old) > 0.0);
            if !(ok(new1) && ok(new2)) {
                continue;
            }
            self.tris[t_ab] = new1;
            self.tris[t_ba] = new2;
            self.around[d as usize].push(t_ab as u32);
            self.around[c as usize].push(t_ba as u32);
        }
    }

    /// Flip each edge whose two opposite angles add up to more than a half turn (the Delaunay
    /// test, on the surface), when neither new triangle turns over: the smallest angles grow.
    /// How many were flipped.
    fn flip_to_angles(&mut self) -> usize {
        let mut flipped = 0;
        for (a, b) in self.area_edges() {
            // Two triangles in the area, or the edge is not one to flip.
            let [t1, t2] = match self.edge_tris(a, b)[..] {
                [t1, t2] if self.in_area[t1] && self.in_area[t2] => [t1, t2],
                _ => continue,
            };
            if self.creased(a, b) {
                continue;
            }
            let runs_ab = |t: usize| {
                let tri = self.tris[t];
                (0..3).any(|k| tri[k] == a && tri[(k + 1) % 3] == b)
            };
            let (t_ab, t_ba) = if runs_ab(t1) { (t1, t2) } else { (t2, t1) };
            let other = |t: usize| self.tris[t].into_iter().find(|&v| v != a && v != b);
            let (Some(c), Some(d)) = (other(t_ab), other(t_ba)) else { continue };
            if c == d || self.ring(c).contains(&d) {
                continue;
            }
            let p = |v: u32| self.pos[v as usize];
            let angle =
                |at: Vec3, x: Vec3, y: Vec3| (x - at).normalized().zip((y - at).normalized()).map_or(0.0, |(u, w)| u.dot(w).clamp(-1.0, 1.0).acos());
            if angle(p(c), p(a), p(b)) + angle(p(d), p(a), p(b)) <= std::f64::consts::PI + 1e-9 {
                continue;
            }
            let (new1, new2) = ([a, d, c], [b, c, d]);
            let n_old = match (self.normal(t_ab), self.normal(t_ba)) {
                (Some(n), Some(m)) => n + m,
                _ => continue,
            };
            let face_normal = |tri: [u32; 3]| {
                let [x, y, z] = tri.map(|v| self.pos[v as usize]);
                (y - x).cross(z - x).normalized()
            };
            if !(face_normal(new1).is_some_and(|n| n.dot(n_old) > 0.0) && face_normal(new2).is_some_and(|n| n.dot(n_old) > 0.0)) {
                continue;
            }
            self.tris[t_ab] = new1;
            self.tris[t_ba] = new2;
            self.around[d as usize].push(t_ab as u32);
            self.around[c as usize].push(t_ba as u32);
            flipped += 1;
        }
        flipped
    }

    /// Move each free vertex of the area towards the middle of its neighbours along the surface,
    /// then back onto the scan as it was.
    fn smooth(&mut self, on: &Projector) {
        let free: Vec<u32> = (0..self.pos.len() as u32)
            .filter(|&v| !self.held[v as usize] && !self.gone[v as usize] && self.live(v).next().is_some() && self.live(v).all(|t| self.in_area[t]))
            .collect();
        let moved: Vec<(u32, Vec3)> = free
            .iter()
            .filter_map(|&v| {
                let ring = self.ring(v);
                if ring.is_empty() {
                    return None;
                }
                let p = self.pos[v as usize];
                let middle = ring.iter().fold(Vec3::ZERO, |s, &w| s + self.pos[w as usize]) / ring.len() as f64;
                let n = self.live_once(v).into_iter().filter_map(|t| self.normal(t)).fold(Vec3::ZERO, |s, n| s + n).normalized()?;
                let step = middle - p;
                let along = step - n * step.dot(n);
                let q = on.nearest(p + along).map_or(p + along, |(q, _)| q);
                q.is_finite().then_some((v, q))
            })
            .collect();
        for (v, q) in moved {
            self.pos[v as usize] = q;
        }
    }

    /// The live mesh, its vertices renumbered, and the new numbers of the area's vertices.
    fn compact(&self) -> (Vec<Vec3>, Vec<[u32; 3]>, Vec<u32>) {
        let mut id = vec![u32::MAX; self.pos.len()];
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let mut area = Vec::new();
        for t in 0..self.tris.len() {
            if !self.alive[t] {
                continue;
            }
            let tri = self.tris[t].map(|v| {
                let slot = &mut id[v as usize];
                if *slot == u32::MAX {
                    *slot = positions.len() as u32;
                    positions.push(self.pos[v as usize]);
                    if self.in_area[t] && !self.held[v as usize] {
                        area.push(*slot);
                    }
                }
                *slot
            });
            triangles.push(tri);
        }
        (positions, triangles, area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A box's top remeshed to triangles three times as long: outside it every triangle is as it
    /// was; the top has fewer, even triangles, still on the top, and the box stays closed.
    #[test]
    fn only_the_area_is_remeshed() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, size, 30);
        let scan = Scan::new(&positions, &triangles);
        let top = |t: &[u32; 3]| t.iter().all(|&v| (positions[v as usize].z - size.z).abs() < 1e-9);
        let area: Vec<bool> = triangles.iter().map(top).collect();
        let edge = 1.0;
        let done = remesh_area(&scan, &area, 3.0 * edge, true).unwrap();
        let key = |t: [Vec3; 3]| {
            let mut k: Vec<[u64; 3]> = t.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect();
            k.sort_unstable();
            k
        };
        let made: std::collections::HashSet<_> = done.triangles.iter().map(|t| key(t.map(|v| done.positions[v as usize]))).collect();
        let outside = triangles.iter().zip(&area).filter(|(_, a)| !**a);
        assert!(outside.clone().all(|(t, _)| made.contains(&key(t.map(|v| positions[v as usize])))), "every triangle outside as it was");
        assert!(done.after < done.before / 3, "fewer, longer triangles on the top: {} from {}", done.after, done.before);
        assert!(done.positions.iter().all(|p| p.z <= size.z + 1e-6), "nothing lifted off the top");
        assert!(done.new_max < 1e-6, "the top's new vertices lie on it: {}", done.new_max);
        // Closed still: every edge has two triangles.
        let mut uses = std::collections::HashMap::new();
        for t in &done.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(uses.values().all(|n| *n == 2), "a hole or a fold left: {:?}", uses.values().filter(|n| **n != 2).count());
        // Finer: more triangles; nothing to remesh and absurd lengths are refused.
        let finer = remesh_area(&scan, &area, 0.5 * edge, true).unwrap();
        assert!(finer.after > done.before, "{} from {}", finer.after, done.before);
        assert_eq!(remesh_area(&scan, &vec![false; triangles.len()], 1.0, true).unwrap_err(), AreaRemeshError::Empty);
        assert_eq!(remesh_area(&scan, &area, 1e-6, true).unwrap_err(), AreaRemeshError::TooFine);
        assert_eq!(remesh_area(&scan, &area, f64::NAN, true).unwrap_err(), AreaRemeshError::Empty);
    }

    /// Half a tube remeshed: the new vertices lie on the tube, no triangle is a sliver or turned,
    /// and its open ends stay where they were.
    #[test]
    fn a_curved_area_stays_on_its_surface() {
        let profile: Vec<[f64; 2]> = (0..=20).map(|j| [f64::from(j), 10.0]).collect();
        let (positions, triangles) = crate::test_meshes::revolved(&profile, 96, false);
        let scan = Scan::new(&positions, &triangles);
        let area: Vec<bool> = triangles.iter().map(|t| t.iter().all(|&v| positions[v as usize].y >= 0.0)).collect();
        let done = remesh_area(&scan, &area, 1.5, false).unwrap();
        // On the tube: radius 10 round its axis (the revolved profile turns about Z).
        for p in &done.positions {
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!((r - 10.0).abs() < 0.05, "off the tube: {p:?} r {r}");
        }
        // No sliver: every triangle's smallest angle above 15°, at the held open ends too.
        let angle = |p: Vec3, q: Vec3, r: Vec3| (q - p).normalized().zip((r - p).normalized()).map_or(0.0, |(u, w)| u.dot(w).clamp(-1.0, 1.0).acos());
        let thin = done
            .triangles
            .iter()
            .map(|t| t.map(|v| done.positions[v as usize]))
            .filter(|[a, b, c]| angle(*a, *b, *c).min(angle(*b, *c, *a)).min(angle(*c, *a, *b)) < 15f64.to_radians())
            .count();
        assert_eq!(thin, 0, "slivers among {}", done.triangles.len());
        assert!(done.new_max < 0.05 && done.uncovered == 0.0, "{} {}", done.new_max, done.uncovered);
    }

    /// A noisy, unevenly cut sheet remeshed at about its own edge: many flips in a pass, some taking
    /// away edges listed earlier in it. It never fails, and every edge still has at most two
    /// triangles.
    #[test]
    fn a_rough_sheet_is_remeshed_without_failing() {
        let n = 40u32;
        let mut seed = 7u64;
        let mut r = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        let positions: Vec<Vec3> = (0..=n)
            .flat_map(|j| (0..=n).map(move |i| (i, j)))
            .map(|(i, j)| Vec3::new(f64::from(i) + 0.4 * r(), f64::from(j) + 0.4 * r(), 0.2 * r()))
            .collect();
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                if (i + j) % 2 == 0 {
                    triangles.extend([[k, k + 1, k + n + 2], [k, k + n + 2, k + n + 1]]);
                } else {
                    triangles.extend([[k, k + 1, k + n + 1], [k + 1, k + n + 2, k + n + 1]]);
                }
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let area = vec![true; triangles.len()];
        for length in [0.7, 1.0, 1.6] {
            let done = remesh_area(&scan, &area, length, false).unwrap();
            let mut uses = std::collections::HashMap::new();
            for t in &done.triangles {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
            assert!(uses.values().all(|u| *u <= 2), "{length}: an edge of three triangles");
        }
    }

    /// An area over a box's edge, remeshed without `sharp`: the edge stays, its line still on the
    /// surface (moved across the edge, the vertices would cut its corner
    /// and leave it ragged).
    #[test]
    fn a_box_edge_in_the_area_stays_an_edge() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, size, 30);
        let scan = Scan::new(&positions, &triangles);
        let near = Vec3::new(15.0, 0.0, 10.0);
        let area: Vec<bool> = triangles.iter().map(|t| t.iter().all(|&v| positions[v as usize].dist(near) < 6.0)).collect();
        assert!(area.iter().filter(|a| **a).count() > 50);
        let done = remesh_area(&scan, &area, 2.0, false).unwrap();
        // The edge line is still on the surface (cut across, the corner would sit off it).
        let made = Scan::new(&done.positions, &done.triangles);
        let on = Projector::new(&made);
        for i in 0..=20 {
            let p = Vec3::new(11.0 + 0.4 * f64::from(i), 0.0, size.z);
            let off = on.nearest(p).map_or(f64::INFINITY, |(_, d)| d);
            assert!(off < 1e-6, "the edge cut at {p:?}: {off}");
        }
    }
}
