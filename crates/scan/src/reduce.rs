//! Fewer triangles for a scan (SurfaceCraft's reducer): edges collapsed cheapest first, where a
//! collapse costs the squared distance of the new point from the planes of the faces that met at
//! the two it replaces (Garland and Heckbert's quadric errors), weighted by those faces' area.
//!
//! A collapse is refused where it would fold the surface (the link condition), turn a face over or
//! make it a sliver, pinch the rim, or touch an edge of three or more faces. With the rim kept, a
//! point on it never moves and a collapse across it is made dear. The candidates sit in a heap of
//! four children a node; a collapse leaves its point's old entries behind, which are dropped as
//! they come up and purged in bulk once there are many.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use solidcraft_geom::compare::TriangleTree;

use crate::{Scan, welded};

/// How far to reduce.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Stop at this many triangles or fewer (0: no such limit).
    pub target: usize,
    /// Stop before a collapse whose error (root mean square distance, mm) would pass this (0: none).
    pub max_error: f64,
    /// Keep the scan's open edges where they are.
    pub keep_rim: bool,
}

/// Which limit ended a reduction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    Target,
    Error,
    /// Nothing more could be collapsed.
    Done,
}

impl Stopped {
    pub fn name(self) -> &'static str {
        match self {
            Stopped::Target => "target",
            Stopped::Error => "error",
            Stopped::Done => "no_more_collapses",
        }
    }
}

/// A reduced scan and how it came about.
#[derive(Clone, Debug, PartialEq)]
pub struct Reduced {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub collapses: usize,
    pub stopped: Stopped,
    /// The largest error (root mean square, mm) of any collapse made.
    pub largest_error: f64,
}

/// Reduces the scan. `None` when it has no triangles or neither limit is set.
pub fn reduce(scan: &Scan, settings: Settings) -> Option<Reduced> {
    reduce_within(scan, settings, None)
}

/// [`reduce`] within an area: the triangles `area` marks (one flag a triangle of the scan; all of
/// them with `None`). A vertex of a triangle outside it stays where it is, so the rest of the scan
/// and the area's edge with it are as they were. `target` counts the whole scan's triangles.
pub fn reduce_within(scan: &Scan, settings: Settings, area: Option<&[bool]>) -> Option<Reduced> {
    if scan.triangle_count() == 0 || (settings.target == 0 && !(settings.max_error > 0.0)) {
        return None;
    }
    let mut r = Reducer::new(scan, settings, area, None);
    if r.tris.is_empty() {
        return None;
    }
    Some(r.run())
}

/// [`reduce`] held to the scan itself (after d0pawlus/SurfaceCraft's measured guard, adapted): a
/// collapse is made only when its new point, and the middle and edge midpoints at that point of every
/// face it leaves, lie within the smaller `budget` of its two points of `original` (the scan's own
/// surface, as a tree), no face shrinks below a thousandth of its area, and the two points merged
/// stay within the budget of the faces left (the last two after d0's `collapse_within`); the merged
/// point keeps that smaller budget. `budget` is per point of the scan
/// (`scan.positions`); points merged as one take the least of theirs, a missing or non-positive one
/// holds its point. `None` as for [`reduce`].
pub fn reduce_held(scan: &Scan, settings: Settings, budget: &[f64], original: &TriangleTree) -> Option<Reduced> {
    if scan.triangle_count() == 0 || (settings.target == 0 && !(settings.max_error > 0.0)) {
        return None;
    }
    let mut r = Reducer::new(scan, settings, None, Some((budget, original)));
    if r.tris.is_empty() {
        return None;
    }
    Some(r.run())
}

/// A held reduction refuses a collapse that shrinks a face below this share of its area.
const HELD_SLIVER: f64 = 1e-3;
/// …and one that leaves a needle (shape below this, about a 12° smallest angle, longer than
/// [`HELD_LONG`] budgets) among faces that bend more than about 10° ([`HELD_CURVED`]) from it.
const HELD_NEEDLE: f64 = 0.35;
/// The smallest corner (5°, in radians) a held collapse may make anywhere, flat or curved.
const HELD_FLOOR: f64 = 0.0873;
const HELD_LONG: f64 = 16.0;
const HELD_CURVED: f64 = 0.985;

/// A reduction held to the scan: the scan's surface and each point's budget (see [`reduce_held`]).
struct Held<'a> {
    tree: &'a TriangleTree,
    budget: Vec<f64>,
}

/// The sum of squared distances from some planes, as a quadratic form in (x, y, z, 1): a symmetric
/// 4 × 4 matrix, ten numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Quadric([f64; 10]);

impl Quadric {
    /// The plane through points p with n·p + o = 0 (n unit), scaled by `w`.
    fn plane(n: Vec3, o: f64, w: f64) -> Self {
        Quadric([
            w * n.x * n.x,
            w * n.x * n.y,
            w * n.x * n.z,
            w * n.x * o,
            w * n.y * n.y,
            w * n.y * n.z,
            w * n.y * o,
            w * n.z * n.z,
            w * n.z * o,
            w * o * o,
        ])
    }

    fn plus(self, other: Quadric) -> Quadric {
        Quadric(std::array::from_fn(|k| self.0[k] + other.0[k]))
    }

    fn value(&self, v: Vec3) -> f64 {
        let [a, b, c, d, e, f, g, h, i, j] = self.0;
        let (x, y, z) = (v.x, v.y, v.z);
        (a * x * x + 2.0 * b * x * y + 2.0 * c * x * z + 2.0 * d * x + e * y * y + 2.0 * f * y * z + 2.0 * g * y + h * z * z + 2.0 * i * z + j)
            .max(0.0)
    }

    /// Where the sum is least, when all three directions are well enough determined.
    fn least(&self) -> Option<Vec3> {
        let [a, b, c, d, e, f, g, h, i, _] = self.0;
        let m = [[a, b, c], [b, e, f], [c, f, h]];
        let trace = a + e + h;
        let det = a * (e * h - f * f) - b * (b * h - f * c) + c * (b * f - e * c);
        if !(trace > 0.0 && det.abs() > 1e-9 * (trace / 3.0).powi(3)) {
            return None;
        }
        let [x, y, z] = crate::symmetry::solve3(&m, [-d, -g, -i])?;
        let p = Vec3::new(x, y, z);
        p.is_finite().then_some(p)
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    /// What the collapse is ranked by: the surface's part and, at an open rim, the rim's penalty.
    cost: f64,
    /// The surface's part alone, a distance squared: what is reported and held to `max_error`.
    error: f64,
    a: u32,
    b: u32,
    version_a: u32,
    version_b: u32,
}

/// Children of a node of the heap.
const ARITY: usize = 4;
/// The heap is purged of stale entries once this share of its size has come up stale.
const PURGE_SHARE: usize = 16;

struct Reducer<'a> {
    settings: Settings,
    held: Option<Held<'a>>,
    pos: Vec<Vec3>,
    quadric: Vec<Quadric>,
    /// Each point's quadric of its faces alone, without the penalty that keeps an open rim: how
    /// far a collapse moves the surface, whatever it does to the rim's outline.
    surface: Vec<Quadric>,
    weight: Vec<f64>,
    tris: Vec<[u32; 3]>,
    alive: Vec<bool>,
    incident: Vec<Vec<u32>>,
    version: Vec<u32>,
    rim: Vec<bool>,
    locked: Vec<bool>,
    mark: Vec<u32>,
    mark2: Vec<u32>,
    stamp: u32,
    stamp2: u32,
    heap: Vec<Entry>,
    alive_count: usize,
}

fn key(x: u32, y: u32) -> u64 {
    (u64::from(x.min(y)) << 32) | u64::from(x.max(y))
}

fn has(t: [u32; 3], v: u32) -> bool {
    t.contains(&v)
}

impl<'a> Reducer<'a> {
    fn new(scan: &Scan, settings: Settings, area: Option<&[bool]>, guard: Option<(&[f64], &'a TriangleTree)>) -> Self {
        // Points at one place are one point, so a seam the file wrote is not an open edge.
        let weld = welded(scan.positions);
        let mut id = vec![u32::MAX; scan.positions.len()];
        let mut pos = Vec::new();
        for (v, &rep) in weld.iter().enumerate() {
            if rep as usize == v {
                id[v] = pos.len() as u32;
                pos.push(scan.positions[v]);
            }
        }
        let mut incident = vec![Vec::new(); pos.len()];
        let mut tris = Vec::with_capacity(scan.triangle_count());
        // A vertex of a triangle outside the area is held where it is.
        let mut held = vec![false; pos.len()];
        for (t, tri) in scan.triangles.iter().enumerate() {
            if scan.corners(t).is_none() {
                continue;
            }
            let [a, b, c] = tri.map(|i| id[weld[i as usize] as usize]);
            if area.is_some_and(|area| !area.get(t).copied().unwrap_or(false)) {
                for v in [a, b, c] {
                    if let Some(h) = held.get_mut(v as usize) {
                        *h = true;
                    }
                }
            }
            // A triangle that is a line or a point once points are merged is none.
            if a == b || b == c || c == a {
                continue;
            }
            let k = tris.len() as u32;
            tris.push([a, b, c]);
            for v in [a, b, c] {
                incident[v as usize].push(k);
            }
        }
        let (n, count) = (pos.len(), tris.len());
        // Each kept point's budget: the least of the scan points merged into it.
        let guard = guard.map(|(budget, tree)| {
            let mut b = vec![f64::INFINITY; n];
            for (v, &rep) in weld.iter().enumerate() {
                let k = id[rep as usize] as usize;
                let own = budget.get(v).copied().filter(|x| x.is_finite() && *x > 0.0).unwrap_or(0.0);
                if let Some(slot) = b.get_mut(k) {
                    *slot = slot.min(own);
                }
            }
            Held { tree, budget: b.into_iter().map(|x| if x.is_finite() { x } else { 0.0 }).collect() }
        });
        Reducer {
            settings,
            held: guard,
            pos,
            quadric: vec![Quadric::default(); n],
            surface: Vec::new(),
            weight: vec![0.0; n],
            alive: vec![true; count],
            tris,
            incident,
            version: vec![0; n],
            rim: vec![false; n],
            locked: held,
            mark: vec![0; n],
            mark2: vec![0; n],
            stamp: 0,
            stamp2: 0,
            heap: Vec::new(),
            alive_count: count,
        }
    }

    /// Each face's plane, weighted by its area, into its corners; and which edges have one face (the
    /// rim) or more than two (locked), by sorting every face's edges.
    fn prepare(&mut self) {
        let mut edges: Vec<(u64, u32)> = Vec::with_capacity(self.tris.len() * 3);
        for (t, &v) in self.tris.iter().enumerate() {
            let [p0, p1, p2] = v.map(|i| self.pos[i as usize]);
            let cross = (p1 - p0).cross(p2 - p0);
            let twice = cross.len();
            if twice > 0.0 && twice.is_finite() {
                let n = cross / twice;
                let plane = Quadric::plane(n, -n.dot(p0), twice / 2.0);
                for c in v {
                    self.quadric[c as usize] = self.quadric[c as usize].plus(plane);
                    self.weight[c as usize] += twice / 6.0;
                }
            }
            for (corner, (x, y)) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])].into_iter().enumerate() {
                edges.push((key(x, y), (3 * t + corner) as u32));
            }
        }
        edges.sort_unstable();
        let mut open = vec![false; self.tris.len() * 3];
        let mut start = 0;
        while start < edges.len() {
            let end = start + edges[start..].iter().take_while(|e| e.0 == edges[start].0).count();
            let (a, b) = ((edges[start].0 >> 32) as usize, (edges[start].0 & 0xFFFF_FFFF) as usize);
            match end - start {
                1 => {
                    open[edges[start].1 as usize] = true;
                    self.rim[a] = true;
                    self.rim[b] = true;
                }
                2 => {}
                // Three or more faces: not a surface there, and not to be touched.
                _ => {
                    self.locked[a] = true;
                    self.locked[b] = true;
                }
            }
            start = end;
        }
        self.surface.clone_from(&self.quadric);
        if !self.settings.keep_rim {
            return;
        }
        // A rim cut across is dear: a plane through each open edge, square to its face, weighted by
        // the edge's length.
        for (t, &v) in self.tris.iter().enumerate() {
            let [p0, p1, p2] = v.map(|i| self.pos[i as usize]);
            let Some(n) = (p1 - p0).cross(p2 - p0).normalized() else { continue };
            for (corner, (x, y)) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])].into_iter().enumerate() {
                if !open[3 * t + corner] {
                    continue;
                }
                let (a, b) = (self.pos[x as usize], self.pos[y as usize]);
                let length = a.dist(b);
                let Some(unit) = ((b - a) / length.max(1e-300)).cross(n).normalized().filter(|_| length > 0.0) else { continue };
                let penalty = Quadric::plane(unit, -unit.dot(a), 100.0 * length * length);
                for c in [x, y] {
                    self.quadric[c as usize] = self.quadric[c as usize].plus(penalty);
                }
            }
        }
    }

    fn less(l: &Entry, r: &Entry) -> bool {
        (l.cost, l.a, l.b) < (r.cost, r.a, r.b)
    }

    fn push(&mut self, entry: Entry) {
        self.heap.push(entry);
        let mut child = self.heap.len() - 1;
        while child > 0 {
            let parent = (child - 1) / ARITY;
            if !Self::less(&self.heap[child], &self.heap[parent]) {
                break;
            }
            self.heap.swap(child, parent);
            child = parent;
        }
    }

    fn sift_down(&mut self, mut parent: usize) {
        let count = self.heap.len();
        loop {
            let first = ARITY * parent + 1;
            if first >= count {
                break;
            }
            let mut smallest = first;
            for child in first + 1..(first + ARITY).min(count) {
                if Self::less(&self.heap[child], &self.heap[smallest]) {
                    smallest = child;
                }
            }
            if !Self::less(&self.heap[smallest], &self.heap[parent]) {
                break;
            }
            self.heap.swap(parent, smallest);
            parent = smallest;
        }
    }

    fn pop(&mut self) -> Option<Entry> {
        let top = *self.heap.first()?;
        let last = self.heap.pop()?;
        if !self.heap.is_empty() {
            self.heap[0] = last;
            self.sift_down(0);
        }
        Some(top)
    }

    /// Whether an entry is still its edge's cost: once either end has changed it never will be.
    fn live(&self, e: &Entry) -> bool {
        self.version[e.a as usize] == e.version_a && self.version[e.b as usize] == e.version_b
    }

    /// The stale entries dropped at once and the heap made again (Floyd's).
    fn purge(&mut self) {
        let version = &self.version;
        self.heap.retain(|e| version[e.a as usize] == e.version_a && version[e.b as usize] == e.version_b);
        if self.heap.len() > 1 {
            for start in (0..=(self.heap.len() - 2) / ARITY).rev() {
                self.sift_down(start);
            }
        }
    }

    /// What collapsing edge (a, b) costs (to rank it by, and the surface's part of that alone),
    /// where the point goes and which end it keeps; `None` when it may not be collapsed.
    fn candidate(&self, a: u32, b: u32) -> Option<(f64, f64, Vec3, u32)> {
        let (ia, ib) = (a as usize, b as usize);
        if self.locked[ia] || self.locked[ib] {
            return None;
        }
        let q = self.quadric[ia].plus(self.quadric[ib]);
        let w = (self.weight[ia] + self.weight[ib]).max(1e-300);
        let (pa, pb) = (self.pos[ia], self.pos[ib]);
        let (position, keeps) = match (self.rim[ia], self.rim[ib]) {
            // Both on the rim: along it only (checked by the caller), at the better end.
            (true, true) if self.settings.keep_rim => {
                if q.value(pa) <= q.value(pb) {
                    (pa, a)
                } else {
                    (pb, b)
                }
            }
            (true, true) => ((pa + pb) / 2.0, a),
            (true, false) => (pa, a),
            (false, true) => (pb, b),
            (false, false) => match q.least() {
                // Never far from the edge: an ill-conditioned answer can be anywhere.
                Some(best) if best.dist((pa + pb) / 2.0) <= 2.0 * pa.dist(pb) + 1e-12 => (best, a),
                Some(_) => ((pa + pb) / 2.0, a),
                None => {
                    let best = [pa, (pa + pb) / 2.0, pb].into_iter().min_by(|x, y| q.value(*x).total_cmp(&q.value(*y))).unwrap_or(pa);
                    (best, a)
                }
            },
        };
        let surface = self.surface[ia].plus(self.surface[ib]);
        Some((q.value(position) / w, surface.value(position).max(0.0) / w, position, keeps))
    }

    fn run(&mut self) -> Reduced {
        self.prepare();
        // Every edge once, at its cost.
        self.heap.reserve(self.tris.len() * 3 / 2);
        for v in 0..self.pos.len() {
            self.stamp = self.stamp.wrapping_add(1);
            for slot in 0..self.incident[v].len() {
                let tri = self.tris[self.incident[v][slot] as usize];
                for other in tri {
                    if other as usize > v && self.mark[other as usize] != self.stamp {
                        self.mark[other as usize] = self.stamp;
                        if let Some((cost, error, _, _)) = self.candidate(v as u32, other) {
                            self.push(Entry {
                                cost,
                                error,
                                a: v as u32,
                                b: other,
                                version_a: self.version[v],
                                version_b: self.version[other as usize],
                            });
                        }
                    }
                }
            }
        }
        let target = self.settings.target;
        let limit = if self.settings.max_error > 0.0 { self.settings.max_error * self.settings.max_error } else { f64::INFINITY };
        let (mut collapses, mut largest, mut stopped, mut stale) = (0, 0.0f64, Stopped::Done, 0usize);
        while let Some(entry) = self.pop() {
            if stale > self.heap.len() / PURGE_SHARE {
                self.purge();
                stale = 0;
            }
            if target > 0 && self.alive_count <= target {
                stopped = Stopped::Target;
                break;
            }
            if !self.live(&entry) {
                stale += 1;
                continue;
            }
            // Held to the error allowed, and reported, by how far the surface moves: the penalty
            // that keeps an open rim ranks a collapse there later but is no distance (a hundred
            // times an edge's length squared read as one made the error ten times too large).
            if entry.error > limit {
                stopped = Stopped::Error;
                break;
            }
            if self.collapse(entry) {
                collapses += 1;
                largest = largest.max(entry.error.sqrt());
            }
        }
        if stopped == Stopped::Done && target > 0 && self.alive_count <= target {
            stopped = Stopped::Target;
        }
        self.finish(collapses, stopped, largest)
    }

    /// Collapses the entry's edge if it may be.
    fn collapse(&mut self, entry: Entry) -> bool {
        let (a, b) = (entry.a, entry.b);
        let (ia, ib) = (a as usize, b as usize);
        let shared = self.incident[ia].iter().filter(|&&t| self.alive[t as usize] && has(self.tris[t as usize], b)).count();
        if !(1..=2).contains(&shared) {
            return false;
        }
        // Two rim points joined by an edge that is not the rim: collapsing it would pinch.
        if self.rim[ia] && self.rim[ib] && shared != 1 {
            return false;
        }
        let Some((_, _, p, keep)) = self.candidate(a, b) else { return false };
        let gone = if keep == a { b } else { a };
        let (ik, ig) = (keep as usize, gone as usize);
        // The link condition: the two may share no neighbours but those opposite the edge.
        self.stamp = self.stamp.wrapping_add(1);
        for &t in &self.incident[ik] {
            if self.alive[t as usize] {
                for other in self.tris[t as usize] {
                    if other != keep {
                        self.mark[other as usize] = self.stamp;
                    }
                }
            }
        }
        self.stamp2 = self.stamp2.wrapping_add(1);
        let mut common = 0;
        for &t in &self.incident[ig] {
            if !self.alive[t as usize] {
                continue;
            }
            for other in self.tris[t as usize] {
                let o = other as usize;
                if other != gone && other != keep && self.mark[o] == self.stamp && self.mark2[o] != self.stamp2 {
                    self.mark2[o] = self.stamp2;
                    common += 1;
                }
            }
        }
        if common != shared {
            return false;
        }
        // No face may turn over, or become a sliver, at the new place.
        for vertex in [ik, ig] {
            for &t in &self.incident[vertex] {
                if !self.alive[t as usize] {
                    continue;
                }
                let tri = self.tris[t as usize];
                if has(tri, keep) && has(tri, gone) {
                    continue;
                }
                let before = tri.map(|i| self.pos[i as usize]);
                let after = tri.map(|i| if i == keep || i == gone { p } else { self.pos[i as usize] });
                let nb = (before[1] - before[0]).cross(before[2] - before[0]);
                let na = (after[1] - after[0]).cross(after[2] - after[0]);
                let (lb, la) = (nb.len(), na.len());
                if la == 0.0 || la <= 1e-14 * lb {
                    return false;
                }
                if lb > 0.0 && (nb / lb).dot(na / la) < 0.2 {
                    return false;
                }
            }
        }
        // Held to the scan: the new point and every face it leaves stay within the smaller budget.
        let bound = match &self.held {
            Some(h) => {
                let bound = h.budget[ia].min(h.budget[ib]);
                let far = |q: Vec3| h.tree.nearest(q).is_none_or(|(d, _)| !(d <= bound));
                if far(p) {
                    return false;
                }
                let mut fan = Vec::new();
                for vertex in [ik, ig] {
                    for &t in &self.incident[vertex] {
                        let tri = self.tris[t as usize];
                        if !self.alive[t as usize] || (has(tri, keep) && has(tri, gone)) {
                            continue;
                        }
                        let before = tri.map(|i| self.pos[i as usize]);
                        let after = tri.map(|i| if i == keep || i == gone { p } else { self.pos[i as usize] });
                        // No face may shrink to a sliver (d0's rule: a thousandth of its area):
                        // a coarse mesh of slivers lays patches no tessellation can follow.
                        let (nb, na) = ((before[1] - before[0]).cross(before[2] - before[0]), (after[1] - after[0]).cross(after[2] - after[0]));
                        if na.len() < HELD_SLIVER * nb.len() {
                            return false;
                        }
                        // Nor may a fair face become one with a corner under HELD_FLOOR (a scan's own
                        // slivers may still be collapsed away): on a flat, collapses cost nothing and
                        // left fans of long needles that no flip or collapse mends afterwards.
                        if smallest_corner(&after) < HELD_FLOOR && smallest_corner(&before) >= HELD_FLOOR {
                            return false;
                        }
                        let middle = (after[0] + after[1] + after[2]) / 3.0;
                        if far(middle) || after.iter().filter(|q| **q != p).any(|q| far((*q + p) / 2.0)) {
                            return false;
                        }
                        fan.push(after);
                    }
                }
                // No long needle where the surface curves (a long thin face across a rounded rim
                // makes patches no tessellation can follow).
                let normal = |t: &[Vec3; 3]| {
                    let n = (t[1] - t[0]).cross(t[2] - t[0]);
                    let l = n.len();
                    (l > 0.0).then(|| n / l)
                };
                for t in &fan {
                    let sides = [t[0].dist(t[1]), t[1].dist(t[2]), t[2].dist(t[0])];
                    let long = sides.iter().copied().fold(0.0, f64::max);
                    let squares: f64 = sides.iter().map(|x| x * x).sum();
                    let area2 = (t[1] - t[0]).cross(t[2] - t[0]).len();
                    let shape = if squares > 0.0 { 2.0 * 3f64.sqrt() * area2 / squares } else { 0.0 };
                    if shape >= HELD_NEEDLE || long <= HELD_LONG * bound {
                        continue;
                    }
                    let Some(n) = normal(t) else { return false };
                    if fan.iter().filter_map(normal).any(|m| m.dot(n) < HELD_CURVED) {
                        return false;
                    }
                }
                // What the collapse takes away must stay covered: the two points it merges within
                // the bound of the faces it leaves (d0's rule), so a thin rim is not cut off.
                let covered = |q: Vec3| fan.iter().any(|t| solidcraft_geom::compare::closest_on_triangle(q, t[0], t[1], t[2]).dist(q) <= bound);
                if !covered(self.pos[ia]) || !covered(self.pos[ib]) {
                    return false;
                }
                Some(bound)
            }
            None => None,
        };
        // `gone` becomes `keep`, at `p`.
        for slot in 0..self.incident[ig].len() {
            let t = self.incident[ig][slot];
            if !self.alive[t as usize] {
                continue;
            }
            let tri = self.tris[t as usize];
            if has(tri, keep) {
                self.alive[t as usize] = false;
                self.alive_count -= 1;
                continue;
            }
            self.tris[t as usize] = tri.map(|c| if c == gone { keep } else { c });
            self.incident[ik].push(t);
        }
        self.incident[ig] = Vec::new();
        let alive = &self.alive;
        self.incident[ik].retain(|&t| alive[t as usize]);
        for slot in 0..self.incident[ik].len() {
            let tri = self.tris[self.incident[ik][slot] as usize];
            for other in tri {
                if other != keep {
                    let alive = &self.alive;
                    self.incident[other as usize].retain(|&t| alive[t as usize]);
                }
            }
        }
        self.pos[ik] = p;
        if let (Some(h), Some(bound)) = (self.held.as_mut(), bound) {
            h.budget[ik] = bound;
        }
        self.quadric[ik] = self.quadric[ia].plus(self.quadric[ib]);
        self.surface[ik] = self.surface[ia].plus(self.surface[ib]);
        self.weight[ik] = self.weight[ia] + self.weight[ib];
        self.rim[ik] = self.rim[ia] || self.rim[ib];
        self.locked[ik] = self.locked[ia] || self.locked[ib];
        self.version[ik] = self.version[ik].wrapping_add(1);
        self.version[ig] = u32::MAX;
        // The merged point's edges have new costs.
        self.stamp = self.stamp.wrapping_add(1);
        for slot in 0..self.incident[ik].len() {
            let tri = self.tris[self.incident[ik][slot] as usize];
            for other in tri {
                if other != keep && self.mark[other as usize] != self.stamp {
                    self.mark[other as usize] = self.stamp;
                    let (lo, hi) = (keep.min(other), keep.max(other));
                    if let Some((cost, error, _, _)) = self.candidate(lo, hi) {
                        self.push(Entry { cost, error, a: lo, b: hi, version_a: self.version[lo as usize], version_b: self.version[hi as usize] });
                    }
                }
            }
        }
        true
    }

    fn finish(&self, collapses: usize, stopped: Stopped, largest_error: f64) -> Reduced {
        let mut remap = vec![u32::MAX; self.pos.len()];
        let mut positions = Vec::new();
        let mut triangles = Vec::with_capacity(self.alive_count);
        for (t, tri) in self.tris.iter().enumerate() {
            if !self.alive[t] {
                continue;
            }
            triangles.push(tri.map(|c| {
                if remap[c as usize] == u32::MAX {
                    remap[c as usize] = positions.len() as u32;
                    positions.push(self.pos[c as usize]);
                }
                remap[c as usize]
            }));
        }
        Reduced { positions, triangles, collapses, stopped, largest_error }
    }
}

/// The smallest corner of a triangle (radians; 0 for a degenerate one).
fn smallest_corner(t: &[Vec3; 3]) -> f64 {
    (0..3)
        .map(|k| {
            let (u, v) = (t[(k + 1) % 3] - t[k], t[(k + 2) % 3] - t[k]);
            let l = u.len() * v.len();
            if l > 0.0 { (u.dot(v) / l).clamp(-1.0, 1.0).acos() } else { 0.0 }
        })
        .fold(std::f64::consts::PI, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A finely cut box reduces to its twelve triangles' worth without moving a corner: flat faces
    /// cost nothing to collapse, and the box stays the box.
    #[test]
    fn a_fine_box_reduces_to_a_coarse_one() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 10);
        let scan = Scan::new(&positions, &triangles);
        let done = reduce(&scan, Settings { target: 100, max_error: 0.0, keep_rim: true }).unwrap();
        assert!(done.triangles.len() <= 100 && done.stopped == Stopped::Target, "{} {:?}", done.triangles.len(), done.stopped);
        assert!(done.largest_error < 1e-6, "flat collapses: {}", done.largest_error);
        let (lo, hi) = done.positions.iter().fold((done.positions[0], done.positions[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        assert!(lo.dist(Vec3::ZERO) < 1e-9 && hi.dist(Vec3::new(30.0, 20.0, 10.0)) < 1e-9, "the box's corners kept: {lo:?} {hi:?}");
        // Every point still on the box's surface.
        let on = |p: &Vec3| [p.x, p.y, p.z].iter().zip([30.0, 20.0, 10.0]).any(|(c, s)| c.abs() < 1e-9 || (c - s).abs() < 1e-9);
        assert!(done.positions.iter().all(on));
    }

    /// A flat disc reduced to a few triangles: its rim is cut across, as it must be, but every
    /// point stays in the disc's plane, so the surface has not moved and the error says so. The
    /// rim's penalty still ranks the collapses (the rim goes last) but is no part of the error.
    #[test]
    fn a_flat_sheets_error_is_how_far_its_surface_moved_not_its_rim() {
        let (rings, spokes, radius) = (8u32, 48u32, 50.0);
        let mut positions = vec![Vec3::ZERO];
        for i in 1..=rings {
            for j in 0..spokes {
                let (r, a) = (radius * f64::from(i) / f64::from(rings), std::f64::consts::TAU * f64::from(j) / f64::from(spokes));
                positions.push(Vec3::new(r * a.cos(), r * a.sin(), 0.0));
            }
        }
        let at = |i: u32, j: u32| 1 + (i - 1) * spokes + j % spokes;
        let mut triangles = Vec::new();
        for j in 0..spokes {
            triangles.push([0, at(1, j), at(1, j + 1)]);
            for i in 1..rings {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let done = reduce(&scan, Settings { target: 8, max_error: 0.0, keep_rim: true }).unwrap();
        assert!(done.triangles.len() <= 8 && done.stopped == Stopped::Target, "{} {:?}", done.triangles.len(), done.stopped);
        assert!(done.positions.iter().all(|p| p.z.abs() < 1e-9), "still in its plane");
        assert!(done.largest_error < 1e-6, "nothing left the plane: {}", done.largest_error);
        // Held to an error, the same: a flat sheet is within any error, down to the target.
        let held = reduce(&scan, Settings { target: 8, max_error: 0.01, keep_rim: true }).unwrap();
        assert!(held.triangles.len() <= 8 && held.stopped == Stopped::Target, "{} {:?}", held.triangles.len(), held.stopped);
    }

    /// Within an area (the box's top), only the area is reduced: every triangle outside it comes
    /// out as it went in, corner for corner, and the area's own are fewer.
    #[test]
    fn only_the_area_is_reduced() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, size, 10);
        let scan = Scan::new(&positions, &triangles);
        let top = |t: &[u32; 3]| t.iter().all(|&v| (positions[v as usize].z - size.z).abs() < 1e-9);
        let area: Vec<bool> = triangles.iter().map(top).collect();
        let (inside, outside) = (area.iter().filter(|a| **a).count(), area.iter().filter(|a| !**a).count());
        let done = reduce_within(&scan, Settings { target: outside + 10, max_error: 0.0, keep_rim: true }, Some(&area)).unwrap();
        let key = |t: [Vec3; 3]| {
            let mut k: Vec<[u64; 3]> = t.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect();
            k.sort_unstable();
            k
        };
        let made: std::collections::HashSet<_> = done.triangles.iter().map(|t| key(t.map(|v| done.positions[v as usize]))).collect();
        let kept = triangles.iter().zip(&area).filter(|(_, a)| !**a).filter(|(t, _)| made.contains(&key(t.map(|v| positions[v as usize])))).count();
        assert_eq!(kept, outside, "every triangle outside the area as it was");
        let on_top = done.triangles.iter().filter(|t| t.iter().all(|&v| (done.positions[v as usize].z - size.z).abs() < 1e-9)).count();
        assert!(on_top < inside / 2, "the top reduced: {on_top} of {inside}");
        // With no area, the whole scan.
        assert_eq!(
            reduce_within(&scan, Settings { target: 100, max_error: 0.0, keep_rim: true }, None),
            reduce(&scan, Settings { target: 100, max_error: 0.0, keep_rim: true })
        );
    }

    /// An open cap of a sphere R 80, 100 mm across, in 60,000 triangles, reduced to a tenth of
    /// them. Measured both ways against the cap as it was (every point of the one against the
    /// other's triangles, the rim's included), nothing is farther than 0.02 mm (0.010 each way);
    /// and the error reported is of that size, not a rim's penalty and not nothing.
    #[test]
    fn a_cap_reduced_to_a_tenth_stays_within_two_hundredths_rim_included() {
        let (rings, spokes, radius) = (100u32, 300u32, 80.0f64);
        let open = (50.0 / radius).asin();
        let mut positions = vec![Vec3::new(0.0, 0.0, radius)];
        for i in 1..=rings {
            let down = open * f64::from(i) / f64::from(rings);
            for j in 0..spokes {
                let round = std::f64::consts::TAU * f64::from(j) / f64::from(spokes);
                positions.push(Vec3::new(radius * down.sin() * round.cos(), radius * down.sin() * round.sin(), radius * down.cos()));
            }
        }
        let at = |i: u32, j: u32| 1 + (i - 1) * spokes + j % spokes;
        let mut triangles = Vec::new();
        for j in 0..spokes {
            triangles.push([0, at(1, j), at(1, j + 1)]);
            for i in 1..rings {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let done = reduce(&scan, Settings { target: triangles.len() / 10, max_error: 0.0, keep_rim: true }).unwrap();
        assert!(done.triangles.len() <= triangles.len() / 10 && done.stopped == Stopped::Target, "{} {:?}", done.triangles.len(), done.stopped);
        // Every corner and every triangle's middle of the one, against the other.
        let sampled = |positions: &[Vec3], triangles: &[[u32; 3]]| -> Vec<Vec3> {
            let middles = triangles.iter().map(|t| (positions[t[0] as usize] + positions[t[1] as usize] + positions[t[2] as usize]) / 3.0);
            positions.iter().copied().chain(middles).collect()
        };
        let farthest = |points: &[Vec3], positions: &[Vec3], triangles: &[[u32; 3]]| {
            let mesh = solidcraft_geom::Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), ..Default::default() };
            let d = crate::deviation::measure(points, &mesh, 0.02, 10.0);
            assert_eq!(d.unmatched, 0);
            d.max_above.abs().max(d.max_below.abs())
        };
        let was_to_now = farthest(&sampled(&positions, &triangles), &done.positions, &done.triangles);
        let now_to_was = farthest(&sampled(&done.positions, &done.triangles), &positions, &triangles);
        assert!(was_to_now <= 0.02 && now_to_was <= 0.02, "{was_to_now} {now_to_was}");
        // The rim is where it was: every point of the old rim within 0.02 of the reduced cap is
        // part of the above; and the new rim's points are on the old rim's circle.
        let rim = |p: &Vec3| ((p.x.hypot(p.y) - 50.0).abs() < 1e-6) && (p.z - radius * open.cos()).abs() < 1e-6;
        assert!(done.positions.iter().filter(|p| rim(p)).count() >= 30, "the rim keeps its points");
        // The error reported is the worst collapse's root mean square, so a little under the
        // farthest point measured (0.0083 against 0.0101 here), and never a different size.
        let worst = was_to_now.max(now_to_was);
        assert!(done.largest_error <= worst && done.largest_error >= 0.75 * worst, "{} against {worst}", done.largest_error);
    }

    /// A curved surface stops where the error allows; a limit that allows nothing collapses nothing
    /// that bends.
    #[test]
    fn the_error_limit_stops_a_curved_surface() {
        let n = 40u32;
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (f64::from(i), f64::from(j));
                positions.push(Vec3::new(x, y, 0.02 * ((x - 20.0).powi(2) + (y - 20.0).powi(2))));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let loose = reduce(&scan, Settings { target: 0, max_error: 0.1, keep_rim: true }).unwrap();
        let tight = reduce(&scan, Settings { target: 0, max_error: 0.02, keep_rim: true }).unwrap();
        assert_eq!((loose.stopped, tight.stopped), (Stopped::Error, Stopped::Error));
        assert!(
            loose.triangles.len() < tight.triangles.len() && tight.triangles.len() < triangles.len(),
            "{} {}",
            loose.triangles.len(),
            tight.triangles.len()
        );
        assert!(loose.largest_error <= 0.1 && tight.largest_error <= 0.02);
        assert!(reduce(&scan, Settings { target: 0, max_error: 0.0, keep_rim: true }).is_none(), "no limit");
    }
}
