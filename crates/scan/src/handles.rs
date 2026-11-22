//! Closing a scan's small handles: the tunnels a scanner makes where the two sides of a thin wall come
//! closer than its resolution (a sponge of them on a casting's fins), and small bridges. They stall a
//! reduction (a collapse that would close a tunnel is refused, so the region stays at full
//! resolution) and so cost Wrap Scan its patches.
//!
//! Method (topological noise removal after Guskov and Wood, 2001; adapted from d0pawlus/SurfaceCraft's
//! `geom/src/handles`): around seeds a quarter of a scale apart along the surface, the geodesic balls
//! one scale across are looked at, from [`SMALLEST_EDGES`] median edges up to the largest size, each
//! scale √2 times the one before. A connected part of a ball that is not a disk (genus above zero, or
//! several boundary loops joined outside it) is cut out and each of its loops filled with the fork's
//! hole fill ([`crate::holes`]); it closes `genus + loops − 1` handles. Nothing else moves. A part is
//! refused when its loops are not joined outside it (cutting it would split the surface), when it
//! touches the scan's rim or an edge of three triangles, when cutting it leaves a piece loose, when
//! a hole stays open, when more than a quarter of a fill lies farther than the tolerance from what it
//! replaced (a cap over the mouth of a real opening), or when what it removed is not covered by the
//! result (more than a quarter of the points sampled over it farther than the tolerance, or any
//! farther than a quarter of the ball). A real hole through a wall narrower than the largest size is
//! a tunnel too and is closed; a wider one is kept.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap};

use solidcore_record::Vec3;
use solidcraft_geom::compare::TriangleTree;
use solidcraft_geom::prelude::*;

use crate::holes::{self, Ring};
use crate::{Scan, UnionFind, welded};

/// The smallest scale, in median edges: a ball narrower holds no ring of triangles round a tunnel.
pub const SMALLEST_EDGES: f64 = 4.0;
/// The largest size when none is given, in median edges (d0's default).
pub const DEFAULT_SIZE_EDGES: f64 = 12.0;
/// Each scale is this much larger than the one before.
const SCALE_STEP: f64 = std::f64::consts::SQRT_2;
/// Passes per scale at most (each from another first seed); two in a row that cut nothing end it.
const PASSES: usize = 6;
/// Attempts per pass: the candidates that failed are left out and the rest cut again.
const ATTEMPTS: usize = 5;
/// A fill or what was removed with more than this share of its points farther than the tolerance
/// from the other is refused.
const FAR_SHARE: f64 = 0.25;
/// The lattice sampled over a removed triangle has at most this many steps along an edge.
const MAX_LATTICE: usize = 8;

/// How [`close`] works.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Handles in patches of the surface at most this wide (mm) are closed.
    pub max_size: f64,
    /// How far a fill may stray from what it replaces, and what was removed from the result (mm).
    pub tolerance: f64,
}

/// The scan with its small handles closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Closed {
    /// The scan's points (welded: one per place) and the fills' new ones; the triangles left and
    /// the fills'. Points no triangle uses any more stay.
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Handles before and after, from each piece's Euler characteristic and loops.
    pub handles_before: usize,
    pub handles_after: usize,
    /// Parts cut out and filled, and the area they removed (mm²).
    pub parts: usize,
    pub removed_area: f64,
}

/// Handles of a mesh: over its pieces, (2 − χ − loops) / 2 (none for a piece that is a disk or a
/// sphere with holes).
pub fn handles(positions: &[Vec3], triangles: &[[u32; 3]]) -> usize {
    let n = positions.len();
    let mut sets = UnionFind::new(n);
    let edges = edge_counts(triangles);
    for t in triangles {
        sets.join(t[0] as usize, t[1] as usize);
        sets.join(t[1] as usize, t[2] as usize);
    }
    // Per piece: vertices, edges, faces, and its boundary edges' loops.
    let mut used = vec![false; n];
    triangles.iter().flatten().for_each(|&v| used[v as usize] = true);
    let mut chi: HashMap<usize, i64> = HashMap::new();
    for v in (0..n).filter(|&v| used[v]) {
        *chi.entry(sets.find(v)).or_default() += 1;
    }
    let mut rim = UnionFind::new(n);
    let mut on_rim = vec![false; n];
    for &((a, b), c) in &edges {
        *chi.entry(sets.find(a as usize)).or_default() -= 1;
        if c == 1 {
            rim.join(a as usize, b as usize);
            on_rim[a as usize] = true;
            on_rim[b as usize] = true;
        }
    }
    for t in triangles {
        *chi.entry(sets.find(t[0] as usize)).or_default() += 1;
    }
    let mut loops: HashMap<usize, std::collections::HashSet<usize>> = HashMap::new();
    for v in (0..n).filter(|&v| on_rim[v]) {
        let piece = sets.find(v);
        let l = rim.find(v);
        loops.entry(piece).or_default().insert(l);
    }
    chi.iter().map(|(p, &x)| ((2 - x - loops.get(p).map_or(0, |l| l.len() as i64)).max(0) / 2) as usize).sum()
}

/// The median length of the mesh's edges (0 with none): [`DEFAULT_SIZE_EDGES`] of it is the size
/// to close handles up to when none is given.
pub fn median_edge(positions: &[Vec3], triangles: &[[u32; 3]]) -> f64 {
    let mut lengths: Vec<f64> =
        edge_counts(triangles).iter().filter_map(|((a, b), _)| Some(positions.get(*a as usize)?.dist(*positions.get(*b as usize)?))).collect();
    if lengths.is_empty() {
        return 0.0;
    }
    let k = lengths.len() / 2;
    *lengths.select_nth_unstable_by(k, f64::total_cmp).1
}

/// Every edge (low, high) of the triangles, sorted, with how many triangles run along it.
fn edge_counts(triangles: &[[u32; 3]]) -> Vec<((u32, u32), u32)> {
    let mut e: Vec<(u32, u32)> = triangles.iter().flat_map(|t| (0..3).map(move |k| (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3])))).collect();
    crate::par::sort_unstable(&mut e);
    let mut out: Vec<((u32, u32), u32)> = Vec::with_capacity(e.len() / 2 + 1);
    for x in e {
        match out.last_mut() {
            Some((y, c)) if *y == x => *c += 1,
            _ => out.push((x, 1)),
        }
    }
    out
}

/// Lists per vertex, packed.
struct Csr {
    start: Vec<usize>,
    items: Vec<u32>,
}

impl Csr {
    fn new(n: usize, pairs: &[(u32, u32)]) -> Self {
        let mut start = vec![0usize; n + 1];
        for &(v, _) in pairs {
            start[v as usize + 1] += 1;
        }
        for i in 1..start.len() {
            start[i] += start[i - 1];
        }
        let mut fill = start.clone();
        let mut items = vec![0u32; pairs.len()];
        for &(v, x) in pairs {
            items[fill[v as usize]] = x;
            fill[v as usize] += 1;
        }
        Csr { start, items }
    }

    fn of(&self, v: u32) -> &[u32] {
        match (self.start.get(v as usize), self.start.get(v as usize + 1)) {
            (Some(&a), Some(&b)) => self.items.get(a..b).unwrap_or(&[]),
            _ => &[],
        }
    }
}

/// A mesh's adjacency: each vertex's neighbours and triangles, and the vertices not to be touched
/// (on the rim or on an edge of three or more triangles).
struct Topo {
    nbr: Csr,
    tri: Csr,
    fixed: Vec<bool>,
    used: Vec<bool>,
}

impl Topo {
    fn new(n: usize, triangles: &[[u32; 3]]) -> Topo {
        let edges = edge_counts(triangles);
        let mut fixed = vec![false; n];
        let mut used = vec![false; n];
        let mut pairs = Vec::with_capacity(edges.len() * 2);
        for &((a, b), c) in &edges {
            pairs.extend([(a, b), (b, a)]);
            for v in [a, b] {
                used[v as usize] = true;
                if c != 2 {
                    fixed[v as usize] = true;
                }
            }
        }
        let corners: Vec<(u32, u32)> = triangles.iter().enumerate().flat_map(|(i, t)| t.map(|v| (v, i as u32))).collect();
        Topo { nbr: Csr::new(n, &pairs), tri: Csr::new(n, &corners), fixed, used }
    }
}

#[derive(PartialEq)]
struct Front(f64, u32);
impl Eq for Front {}
impl PartialOrd for Front {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Front {
    /// Nearest first; ties by vertex.
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

/// The vertices within `reach` of `seed` along the edges, with their distances, sorted by vertex.
fn reach_of(positions: &[Vec3], topo: &Topo, seed: u32, reach: f64) -> Vec<(u32, f64)> {
    let mut dist: HashMap<u32, f64> = HashMap::new();
    let mut heap = BinaryHeap::new();
    dist.insert(seed, 0.0);
    heap.push(Front(0.0, seed));
    while let Some(Front(d, x)) = heap.pop() {
        if d > dist.get(&x).copied().unwrap_or(f64::INFINITY) {
            continue;
        }
        for &w in topo.nbr.of(x) {
            let nd = d + positions[x as usize].dist(positions[w as usize]);
            if nd <= reach && nd < dist.get(&w).copied().unwrap_or(f64::INFINITY) {
                dist.insert(w, nd);
                heap.push(Front(nd, w));
            }
        }
    }
    let mut out: Vec<(u32, f64)> = dist.into_iter().collect();
    out.sort_unstable_by_key(|x| x.0);
    out
}

/// Seeds `spacing` apart along the surface, from vertex `first` on (and round).
fn seeds(positions: &[Vec3], topo: &Topo, spacing: f64, first: usize) -> Vec<u32> {
    let n = positions.len();
    let mut covered = vec![false; n];
    let mut out = Vec::new();
    for v in (first.min(n)..n).chain(0..first.min(n)) {
        if covered[v] || !topo.used[v] {
            continue;
        }
        out.push(v as u32);
        for (x, _) in reach_of(positions, topo, v as u32, spacing) {
            covered[x as usize] = true;
        }
    }
    out
}

/// A connected part of a ball that is not a disk.
#[derive(Clone, Debug)]
struct Candidate {
    /// Its triangles and vertices, ascending.
    triangles: Vec<u32>,
    vertices: Vec<u32>,
    genus: usize,
    /// One vertex of each boundary loop.
    loops: Vec<u32>,
}

impl Candidate {
    fn handles(&self) -> usize {
        self.genus + self.loops.len().saturating_sub(1)
    }
}

/// The candidates among the connected parts of the triangles of the ball of radius `radius` round
/// `seed`.
fn ball_candidates(positions: &[Vec3], triangles: &[[u32; 3]], topo: &Topo, seed: u32, radius: f64) -> Vec<Candidate> {
    let inside = reach_of(positions, topo, seed, radius);
    let is_in = |v: u32| inside.binary_search_by_key(&v, |x| x.0).is_ok();
    let mut tris: Vec<u32> =
        inside.iter().flat_map(|&(v, _)| topo.tri.of(v).iter().copied()).filter(|&t| triangles[t as usize].iter().all(|&v| is_in(v))).collect();
    tris.sort_unstable();
    tris.dedup();
    // Its connected parts (through shared vertices).
    let mut verts: Vec<u32> = tris.iter().flat_map(|&t| triangles[t as usize]).collect();
    verts.sort_unstable();
    verts.dedup();
    let local = |v: u32| verts.binary_search(&v).unwrap_or(0);
    let mut sets = UnionFind::new(verts.len());
    for &t in &tris {
        let [a, b, c] = triangles[t as usize];
        sets.join(local(a), local(b));
        sets.join(local(b), local(c));
    }
    let mut parts: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for &t in &tris {
        parts.entry(sets.find(local(triangles[t as usize][0]))).or_default().push(t);
    }
    parts.into_values().filter_map(|part| judge(triangles, topo, part)).collect()
}

/// A part of a ball as a candidate: not a disk, touching no fixed vertex, not pinched to the rest of
/// the mesh at a vertex off its boundary, and not a whole closed piece.
fn judge(triangles: &[[u32; 3]], topo: &Topo, tris: Vec<u32>) -> Option<Candidate> {
    let own: Vec<[u32; 3]> = tris.iter().map(|&t| triangles[t as usize]).collect();
    let edges = edge_counts(&own);
    let mut verts: Vec<u32> = own.iter().flatten().copied().collect();
    verts.sort_unstable();
    let count_in = |v: u32| verts.partition_point(|&x| x <= v) - verts.partition_point(|&x| x < v);
    let mut unique = verts.clone();
    unique.dedup();
    if unique.iter().any(|&v| topo.fixed[v as usize]) {
        return None;
    }
    let boundary: Vec<(u32, u32)> = edges.iter().filter(|(_, c)| *c == 1).map(|(e, _)| *e).collect();
    let mut on_boundary: Vec<u32> = boundary.iter().flat_map(|&(a, b)| [a, b]).collect();
    on_boundary.sort_unstable();
    // A vertex on two of its loops (a pinch on its boundary) would merge them in the count.
    if on_boundary.chunk_by(|a, b| a == b).any(|run| run.len() != 2) {
        return None;
    }
    on_boundary.dedup();
    if unique.iter().any(|&v| on_boundary.binary_search(&v).is_err() && count_in(v) != topo.tri.of(v).len()) {
        return None;
    }
    // Its loops: the boundary's connected runs.
    let local = |v: u32| unique.binary_search(&v).unwrap_or(0);
    let mut runs = UnionFind::new(unique.len());
    for &(a, b) in &boundary {
        runs.join(local(a), local(b));
    }
    let mut loops: BTreeMap<usize, u32> = BTreeMap::new();
    for &v in &on_boundary {
        let e = loops.entry(runs.find(local(v))).or_insert(v);
        *e = (*e).min(v);
    }
    let loops: Vec<u32> = loops.into_values().collect();
    let chi = unique.len() as i64 - edges.len() as i64 + own.len() as i64;
    let twice = 2 - chi - loops.len() as i64;
    if twice < 0 || twice % 2 != 0 || loops.is_empty() {
        return None;
    }
    let c = Candidate { triangles: tris, vertices: unique, genus: (twice / 2) as usize, loops };
    (c.handles() > 0).then_some(c)
}

/// Candidates that are apart, smallest first: one is kept when none of its vertices is a vertex of a
/// kept one or a neighbour of one, so the holes they leave do not touch.
fn pick(topo: &Topo, mut cands: Vec<Candidate>, n: usize) -> Vec<Candidate> {
    cands.sort_by_key(|c| (c.triangles.len(), c.triangles.first().copied()));
    let mut claimed = vec![false; n];
    let mut out = Vec::new();
    for c in cands {
        if c.vertices.iter().any(|&v| claimed[v as usize]) {
            continue;
        }
        for &v in &c.vertices {
            claimed[v as usize] = true;
            for &w in topo.nbr.of(v) {
                claimed[w as usize] = true;
            }
        }
        out.push(c);
    }
    out
}

/// Points over a triangle: a lattice of steps no longer than `spacing` (at most [`MAX_LATTICE`] along
/// an edge), corners and edge middles among them, and its centre.
fn samples(t: [Vec3; 3], spacing: f64) -> Vec<Vec3> {
    let [a, b, c] = t;
    let longest = a.dist(b).max(b.dist(c)).max(c.dist(a));
    let steps = if spacing > 0.0 && longest.is_finite() { ((longest / spacing).ceil() as usize).clamp(2, MAX_LATTICE) } else { 2 };
    let n = steps as f64;
    let mut out = Vec::with_capacity((steps + 1) * (steps + 2) / 2 + 1);
    for i in 0..=steps {
        for j in 0..=steps - i {
            out.push(a + (b - a) * (i as f64 / n) + (c - a) * (j as f64 / n));
        }
    }
    out.push((a + b + c) / 3.0);
    out
}

/// The candidates (indices into `cands`, those `live`) cut out and filled, or why not: `Ok` the new
/// triangles and positions, `Err` the candidates that failed.
fn attempt(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    cands: &[Candidate],
    live: &[bool],
    (tolerance, far_limit): (f64, f64),
) -> Result<(Vec<Vec3>, Vec<[u32; 3]>), Vec<usize>> {
    let mut owner = vec![u32::MAX; positions.len()];
    let mut dead = vec![false; triangles.len()];
    for (g, c) in cands.iter().enumerate().filter(|(g, _)| live[*g]) {
        for &t in &c.triangles {
            dead[t as usize] = true;
        }
        for &v in &c.vertices {
            owner[v as usize] = g as u32;
        }
    }
    let kept: Vec<[u32; 3]> = triangles.iter().zip(&dead).filter(|(_, d)| !**d).map(|(t, _)| *t).collect();
    // Each candidate's loops as rings, from its own triangles' boundary edges as they ran them (the
    // way a triangle closing the hole runs them).
    let mut rings: Vec<(usize, Ring)> = Vec::new();
    for (g, c) in cands.iter().enumerate().filter(|(g, _)| live[*g]) {
        let own: Vec<[u32; 3]> = c.triangles.iter().map(|&t| triangles[t as usize]).collect();
        let open: std::collections::HashSet<(u32, u32)> = edge_counts(&own).into_iter().filter(|(_, n)| *n == 1).map(|(e, _)| e).collect();
        let mut next: HashMap<u32, u32> = HashMap::new();
        for t in &own {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if open.contains(&(a.min(b), a.max(b))) {
                    next.insert(a, b);
                }
            }
        }
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &start in &c.loops {
            let mut points = Vec::new();
            let mut v = start;
            while seen.insert(v) {
                points.push(v);
                match next.get(&v) {
                    Some(&w) => v = w,
                    None => break,
                }
            }
            if v != start || points.len() < 3 {
                return Err(vec![g]);
            }
            let perimeter = (0..points.len()).map(|k| positions[points[k] as usize].dist(positions[points[(k + 1) % points.len()] as usize])).sum();
            rings.push((g, Ring { points, perimeter, rim: false }));
        }
    }
    let cut = Scan::new(positions, &kept);
    let chosen: Vec<&Ring> = rings.iter().map(|(_, r)| r).collect();
    let Some(filled) = holes::fill_rings(&cut, &chosen, true) else { return Err((0..cands.len()).filter(|&g| live[g]).collect()) };
    let mut failed: Vec<usize> = Vec::new();
    // A ring left open: its candidate failed (left in the rings' order, each with its centre).
    for un in &filled.left {
        if let Some((g, _)) = rings.iter().find(|(_, r)| {
            let n = r.points.len().max(1) as f64;
            (r.points.iter().fold(Vec3::ZERO, |s, &v| s + positions[v as usize]) / n).dist(un.at) < 1e-12
        }) {
            failed.push(*g);
        }
    }
    // Each fill's points (its corners and centres) against the triangles it replaced: a fill mostly
    // off them is a cap over a real opening.
    let fills = &filled.triangles[kept.len()..];
    let fill_patch = {
        let mut sets = UnionFind::new(fills.len());
        let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
        for (i, t) in fills.iter().enumerate() {
            for k in 0..3 {
                let e = (t[k].min(t[(k + 1) % 3]), t[k].max(t[(k + 1) % 3]));
                if let Some(&j) = by_edge.get(&e) {
                    sets.join(i, j);
                } else {
                    by_edge.insert(e, i);
                }
            }
        }
        (0..fills.len()).map(|i| sets.find(i)).collect::<Vec<_>>()
    };
    let removed: Vec<[u32; 3]> = triangles.iter().zip(&dead).filter(|(_, d)| **d).map(|(t, _)| *t).collect();
    if !removed.is_empty() && !fills.is_empty() {
        let tree = TriangleTree::of(positions, &removed);
        let mut counts: HashMap<usize, (usize, usize, Option<u32>)> = HashMap::new();
        for (t, &p) in fills.iter().zip(&fill_patch) {
            let pts = t.map(|v| filled.positions[v as usize]);
            let e = counts.entry(p).or_insert((0, 0, None));
            for q in pts.iter().copied().chain(std::iter::once((pts[0] + pts[1] + pts[2]) / 3.0)) {
                e.0 += 1;
                e.1 += usize::from(tree.nearest(q).is_none_or(|(d, _)| d > tolerance));
            }
            if e.2.is_none() {
                e.2 = t.iter().find_map(|&v| owner.get(v as usize).copied().filter(|&o| o != u32::MAX));
            }
        }
        for (all, far, g) in counts.into_values() {
            if let Some(g) = g
                && far as f64 > FAR_SHARE * all as f64
            {
                failed.push(g as usize);
            }
        }
    }
    // What each candidate removed must be covered by the result near it (its fill and the
    // triangles left round its loops).
    let near: Vec<[u32; 3]> = filled
        .triangles
        .iter()
        .enumerate()
        .filter(|(i, t)| *i >= kept.len() || t.iter().any(|&v| owner.get(v as usize).is_some_and(|&o| o != u32::MAX)))
        .map(|(_, t)| *t)
        .collect();
    if !near.is_empty() {
        let tree = TriangleTree::of(&filled.positions, &near);
        for (g, c) in cands.iter().enumerate().filter(|(g, _)| live[*g]) {
            let pts: Vec<Vec3> =
                c.triangles.iter().flat_map(|&t| samples(triangles[t as usize].map(|v| positions[v as usize]), 2.0 * tolerance)).collect();
            let d: Vec<f64> = pts.iter().map(|&q| tree.nearest(q).map_or(f64::INFINITY, |(d, _)| d)).collect();
            let far = d.iter().filter(|&&x| x > tolerance).count();
            if far as f64 > FAR_SHARE * pts.len() as f64 || d.iter().any(|&x| x > far_limit) {
                failed.push(g);
            }
        }
    }
    failed.sort_unstable();
    failed.dedup();
    if failed.is_empty() { Ok((filled.positions, filled.triangles)) } else { Err(failed) }
}

/// The candidates whose loops the triangles left do not join (cutting them would split the surface).
fn splitting(n: usize, triangles: &[[u32; 3]], cands: &[Candidate], live: &[bool]) -> Vec<usize> {
    let mut dead = vec![false; triangles.len()];
    for (_, c) in cands.iter().enumerate().filter(|(g, _)| live[*g]) {
        for &t in &c.triangles {
            dead[t as usize] = true;
        }
    }
    let mut sets = UnionFind::new(n);
    for (t, tri) in triangles.iter().enumerate().filter(|(t, _)| !dead[*t]) {
        let _ = t;
        sets.join(tri[0] as usize, tri[1] as usize);
        sets.join(tri[1] as usize, tri[2] as usize);
    }
    cands
        .iter()
        .enumerate()
        .filter(|(g, c)| live[*g] && c.loops.windows(2).any(|w| sets.find(w[0] as usize) != sets.find(w[1] as usize)))
        .map(|(g, _)| g)
        .collect()
}

/// The scan with its handles in patches at most `settings.max_size` wide closed; `None` when the
/// settings are out of range or nothing was closed.
pub fn close(scan: &Scan, settings: &Settings) -> Option<Closed> {
    if !(settings.max_size.is_finite() && settings.max_size > 0.0 && settings.tolerance.is_finite() && settings.tolerance > 0.0) {
        return None;
    }
    let weld = welded(scan.positions);
    let mut positions = scan.positions.to_vec();
    let mut triangles: Vec<[u32; 3]> = (0..scan.triangle_count())
        .filter_map(|t| {
            scan.corners(t)?;
            let [a, b, c] = scan.triangles.get(t)?.map(|i| weld[i as usize]);
            (a != b && b != c && c != a).then_some([a, b, c])
        })
        .collect();
    let before = handles(&positions, &triangles);
    if before == 0 {
        return None;
    }
    let median = median_edge(&positions, &triangles);
    let max_size = settings.max_size.min(scan.extent());
    let mut scales = Vec::new();
    let mut r = SMALLEST_EDGES * median;
    while r.is_finite() && r > 0.0 && r < max_size && scales.len() < 64 {
        scales.push(r);
        r *= SCALE_STEP;
    }
    if SMALLEST_EDGES * median <= max_size {
        scales.push(max_size);
    }
    let (mut parts, mut closed_handles, mut removed_area) = (0usize, 0usize, 0.0);
    for scale in scales {
        if closed_handles >= before {
            break;
        }
        let mut idle = 0;
        for pass in 0..PASSES {
            let topo = Topo::new(positions.len(), &triangles);
            let first = pass * positions.len() / PASSES;
            let centres = seeds(&positions, &topo, 0.25 * scale, first);
            let found: Vec<Candidate> = crate::par::map(centres.len(), |i| ball_candidates(&positions, &triangles, &topo, centres[i], 0.5 * scale))
                .into_iter()
                .flatten()
                .collect();
            let cands = pick(&topo, found, positions.len());
            let mut live = vec![true; cands.len()];
            for g in splitting(positions.len(), &triangles, &cands, &live) {
                live[g] = false;
            }
            let mut cut = false;
            for _ in 0..ATTEMPTS {
                if !live.iter().any(|&l| l) {
                    break;
                }
                match attempt(&positions, &triangles, &cands, &live, (settings.tolerance, (0.25 * scale).max(settings.tolerance))) {
                    Ok((p, t)) => {
                        for (_, c) in cands.iter().enumerate().filter(|(g, _)| live[*g]) {
                            parts += 1;
                            closed_handles += c.handles();
                            removed_area += c
                                .triangles
                                .iter()
                                .map(|&t| {
                                    let [a, b, c] = triangles[t as usize].map(|v| positions[v as usize]);
                                    (b - a).cross(c - a).len() / 2.0
                                })
                                .sum::<f64>();
                        }
                        (positions, triangles) = (p, t);
                        cut = true;
                        break;
                    }
                    Err(failed) => failed.into_iter().for_each(|g| live[g] = false),
                }
            }
            idle = if cut { 0 } else { idle + 1 };
            if idle >= 2 {
                break;
            }
        }
    }
    if parts == 0 {
        return None;
    }
    let after = handles(&positions, &triangles);
    Some(Closed { positions, triangles, handles_before: before, handles_after: after, parts, removed_area })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A torus (R 20, r 6) as a mesh of `n` × `m` quads, each two triangles.
    fn torus(n: u32, m: u32, big: f64, small: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for i in 0..n {
            for j in 0..m {
                let (u, v) = (std::f64::consts::TAU * f64::from(i) / f64::from(n), std::f64::consts::TAU * f64::from(j) / f64::from(m));
                p.push(Vec3::new((big + small * v.cos()) * u.cos(), (big + small * v.cos()) * u.sin(), small * v.sin()));
            }
        }
        let at = |i: u32, j: u32| (i % n) * m + (j % m);
        let mut t = Vec::new();
        for i in 0..n {
            for j in 0..m {
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (p, t)
    }

    #[test]
    fn a_torus_has_one_handle_and_a_cube_none() {
        let (p, t) = torus(24, 12, 20.0, 6.0);
        assert_eq!(handles(&p, &t), 1);
        let cube = [
            Vec3::new(0., 0., 0.),
            Vec3::new(1., 0., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(0., 1., 0.),
            Vec3::new(0., 0., 1.),
            Vec3::new(1., 0., 1.),
            Vec3::new(1., 1., 1.),
            Vec3::new(0., 1., 1.),
        ];
        let faces =
            [[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
        assert_eq!(handles(&cube, &faces), 0);
    }

    /// The surface of the cells (i, j, k) of a grid of `h` mm cells that `filled` says are solid, each
    /// square between a solid cell and an empty one two triangles, counter-clockwise seen from the
    /// empty side.
    fn voxels(n: [i32; 3], h: f64, filled: impl Fn(i32, i32, i32) -> bool) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let id = |i: i32, j: i32, k: i32| ((i * (n[1] + 1) + j) * (n[2] + 1) + k) as u32;
        let positions: Vec<Vec3> = (0..=n[0])
            .flat_map(|i| (0..=n[1]).flat_map(move |j| (0..=n[2]).map(move |k| Vec3::new(f64::from(i) * h, f64::from(j) * h, f64::from(k) * h))))
            .collect();
        let solid = |i: i32, j: i32, k: i32| (0..n[0]).contains(&i) && (0..n[1]).contains(&j) && (0..n[2]).contains(&k) && filled(i, j, k);
        let mut triangles = Vec::new();
        for i in 0..n[0] {
            for j in 0..n[1] {
                for k in 0..n[2] {
                    if !solid(i, j, k) {
                        continue;
                    }
                    // Each axis both ways: the square's corners, counter-clockwise about its normal.
                    let faces: [((i32, i32, i32), [(i32, i32, i32); 4]); 6] = [
                        ((1, 0, 0), [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)]),
                        ((-1, 0, 0), [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)]),
                        ((0, 1, 0), [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)]),
                        ((0, -1, 0), [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)]),
                        ((0, 0, 1), [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]),
                        ((0, 0, -1), [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)]),
                    ];
                    for ((di, dj, dk), q) in faces {
                        if solid(i + di, j + dj, k + dk) {
                            continue;
                        }
                        let c = q.map(|(a, b, c)| id(i + a, j + b, k + c));
                        triangles.push([c[0], c[1], c[2]]);
                        triangles.push([c[0], c[2], c[3]]);
                    }
                }
            }
        }
        (positions, triangles)
    }

    /// A plate 20 × 20 × 1 mm with a 1 × 1 mm hole through its middle (cells of 0.25 mm): one handle,
    /// a tunnel through a thin wall. With handles up to 5 mm closed, the hole's lining is cut out and
    /// its two mouths filled: no handle left, the surface closed, the area taken a few mm² (the
    /// lining's 4 mm² and the collars round the mouths). Up to 2 mm, no ball goes round the hole
    /// (half its 4 mm round is 2 mm): it is kept.
    #[test]
    fn a_tunnel_through_a_thin_plate_is_closed_and_kept_below_its_size() {
        let (p, t) = voxels([80, 80, 4], 0.25, |i, j, _| !((38..42).contains(&i) && (38..42).contains(&j)));
        assert_eq!(handles(&p, &t), 1);
        let scan = Scan::new(&p, &t);
        let closed = close(&scan, &Settings { max_size: 5.0, tolerance: 0.6 }).expect("closes the tunnel");
        assert_eq!((closed.handles_before, closed.handles_after, closed.parts), (1, 0, 1));
        let open = edge_counts(&closed.triangles).iter().filter(|(_, c)| *c != 2).count();
        assert_eq!(open, 0, "the result is closed");
        assert!((4.0..60.0).contains(&closed.removed_area), "removed {} mm²", closed.removed_area);
        assert!(close(&scan, &Settings { max_size: 2.0, tolerance: 0.6 }).is_none());
    }

    #[test]
    fn nothing_to_close_or_bad_settings_give_none() {
        let (p, t) = torus(24, 12, 20.0, 6.0);
        let scan = Scan::new(&p, &t);
        assert!(close(&scan, &Settings { max_size: f64::NAN, tolerance: 0.1 }).is_none());
        assert!(close(&scan, &Settings { max_size: 1.0, tolerance: -1.0 }).is_none());
    }
}
