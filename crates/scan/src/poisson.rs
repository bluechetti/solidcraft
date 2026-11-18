//! A surface made from a point cloud, after SurfaceCraft's: screened Poisson reconstruction
//! (Kazhdan and Hoppe 2013), trimmed where the points are sparse.
//!
//! - **Normals.** Where the cloud states none, each point's normal is the direction its nearest
//!   neighbours vary least in (Hoppe et al. 1992), turned to agree with its neighbours along a
//!   minimum spanning tree of the neighbour graph weighted by how far from parallel the normals are,
//!   so the turning crosses flat parts first and folds last; then the whole is turned so most
//!   normals point away from the middle. Each point also stands for an area: its share of the disc
//!   its nearest neighbours cover.
//! - **The field.** The points' normals, each weighted by its area, make a vector field; the
//!   indicator χ is the one whose gradient best matches it while held near zero at the points (the
//!   screening, α = 4). It is solved on a grid of nodes over a cube a quarter larger than the cloud,
//!   `2^depth` cells across at the finest: the coarsest level (64 across) everywhere, each finer
//!   one only within two nodes of a cell that holds a point, its other nodes the coarser level's
//!   (a cascade), each by conjugate gradients with the diagonal as preconditioner.
//! - **The mesh.** Surface nets on the finest grid where χ equals its mean at the points, followed
//!   from the cells that hold points. Each vertex's density is how fully the points round it cover
//!   the surface (1 amid them, about ½ at a scanned patch's edge, 0 far off); vertices below the
//!   trim go, so the surface stops where the points do.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::par;

/// The finest grid's detail and how much is trimmed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The finest grid is `2^depth` cells across the cube round the cloud.
    pub depth: u32,
    /// Vertices whose density is below this go (0 keeps the closed surface).
    pub trim: f64,
}

pub const LEAST_DEPTH: u32 = 5;
pub const MOST_DEPTH: u32 = 10;
pub const DEFAULT_TRIM: f64 = 0.3;

impl Settings {
    /// The depth whose finest cells are about as wide as the points are apart (at most 9), and the
    /// default trim.
    pub fn suggested(extent: f64, spacing: f64) -> Settings {
        Self::suggested_for(extent, spacing, 0.0)
    }

    /// As [`Settings::suggested`], with cells no finer than [`LAYERED_CELLS`] times the layer of
    /// points is thick ([`layer_thickness`]): passes merged a millimetre apart then average into
    /// one surface, where cells finer than the gap made a sheet of each (the YCB mug: 110 pieces
    /// and four times its area at 0.56 mm cells, one mug at 2.2 mm). A clean pass, thinner than
    /// its spacing, is meshed as before.
    pub fn suggested_for(extent: f64, spacing: f64, thickness: f64) -> Settings {
        let by_spacing = (LEAST_DEPTH..=9).find(|&d| cell_size(extent, d) <= 1.5 * spacing).unwrap_or(9);
        let least = LAYERED_CELLS * thickness;
        let by_layer = (LEAST_DEPTH..=9).rev().find(|&d| cell_size(extent, d) >= least).unwrap_or(LEAST_DEPTH);
        Settings { depth: by_spacing.min(by_layer), trim: DEFAULT_TRIM }
    }
}

/// Cells at least this many times the thickness of the layer of points. Read off YCB's merged
/// clouds: the mug, hammer and drill each make one surface at 1.8, 2.3 and 3 times, and come out
/// layered at 1.1 to 1.5 times.
const LAYERED_CELLS: f64 = 1.75;

/// A layer of points thicker than this many times their spacing is not one surface's noise but
/// passes that do not lie on each other.
pub const LAYERED: f64 = 1.0;

/// The screening weight (the paper's α).
const SCREENING: f64 = 4.0;
/// The cube round the cloud, as a multiple of its largest side.
const ROOM: f64 = 1.25;
/// The coarsest level, solved everywhere.
const BASE_DEPTH: u32 = 6;
/// How many nodes round a cell that holds a point the finer levels solve.
const BAND: i64 = 2;
/// The most nodes a level may solve for, and vertices the surface may have.
const MOST_NODES: usize = 24_000_000;
const MOST_VERTICES: usize = 24_000_000;
/// The most points the distances are measured from.
const MEASURED: usize = 200_000;
/// Neighbours a normal is fitted to, and each point is joined to in the turning graph.
const FIT_NEIGHBOURS: usize = 16;
const GRAPH_NEIGHBOURS: usize = 8;

/// The finest cell of a cloud whose largest side is `extent`, at `depth`.
pub fn cell_size(extent: f64, depth: u32) -> f64 {
    extent.max(1e-9) * ROOM / f64::from(1u32 << depth.min(30))
}

/// How far the cloud's points are from the surface made of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distances {
    pub rms: f64,
    pub p95: f64,
    pub max: f64,
    pub measured: usize,
}

/// A surface made from a cloud.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub settings: Settings,
    /// The points given, and those the surface was solved from after thinning.
    pub points: usize,
    pub points_used: usize,
    /// Whether the normals were estimated (the cloud stated none).
    pub estimated_normals: bool,
    /// The finest cell, and the typical distance between the points.
    pub cell: f64,
    pub spacing: f64,
    /// The triangles the density trim took away.
    pub trimmed: usize,
    pub distances: Distances,
}

/// Why no surface was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoissonError {
    /// The grid would be too large to hold at this depth.
    TooFine,
    /// No surface was left, with why.
    Empty(&'static str),
}

impl std::fmt::Display for PoissonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PoissonError::TooFine => f.write_str("the surface would need too fine a grid to hold: lower the detail"),
            PoissonError::Empty(why) => f.write_str(why),
        }
    }
}

// -------------------------------------------------------------------------------------------
// Points by cells.

const AXIS_BITS: u32 = 21;
const AXIS_LIMIT: i64 = (1 << AXIS_BITS) - 1;

fn pack(i: i64, j: i64, k: i64) -> u64 {
    (i as u64) | ((j as u64) << AXIS_BITS) | ((k as u64) << (2 * AXIS_BITS))
}

/// Points sorted into cubic cells, for the nearest few of a place and those within a reach.
pub(crate) struct PointGrid<'a> {
    points: &'a [Vec3],
    lower: Vec3,
    cell: f64,
    keys: Vec<u64>,
    starts: Vec<u32>,
    order: Vec<u32>,
}

fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    let first = points.first().copied().unwrap_or(Vec3::ZERO);
    points.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
}

impl<'a> PointGrid<'a> {
    pub(crate) fn new(points: &'a [Vec3], cell: f64) -> Self {
        let (lower, upper) = bounds(points);
        let d = upper - lower;
        let extent = d.x.max(d.y).max(d.z);
        let cell = cell.max(extent / AXIS_LIMIT as f64 * 1.0001).max(1e-12);
        let mut keyed: Vec<(u64, u32)> = points
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let c = (*p - lower) / cell;
                (pack(c.x.floor() as i64, c.y.floor() as i64, c.z.floor() as i64), k as u32)
            })
            .collect();
        keyed.sort_unstable();
        let (mut keys, mut starts) = (Vec::new(), Vec::new());
        for (k, (key, _)) in keyed.iter().enumerate() {
            if keys.last() != Some(key) {
                keys.push(*key);
                starts.push(k as u32);
            }
        }
        starts.push(keyed.len() as u32);
        PointGrid { points, lower, cell, keys, starts, order: keyed.into_iter().map(|(_, k)| k).collect() }
    }

    /// A grid whose cells hold about `per_cell` points each, on a surface.
    pub(crate) fn sized_for(points: &'a [Vec3], per_cell: f64) -> Self {
        let (lower, upper) = bounds(points);
        let diagonal = (upper - lower).len().max(1e-9);
        let mut cell = diagonal / (points.len().max(1) as f64).sqrt() * per_cell.sqrt();
        let mut grid = Self::new(points, cell);
        for _ in 0..4 {
            let full = points.len() as f64 / grid.keys.len().max(1) as f64;
            if (full / per_cell - 1.0).abs() < 0.5 {
                break;
            }
            cell *= (per_cell / full).sqrt().clamp(0.25, 4.0);
            grid = Self::new(points, cell);
        }
        grid
    }

    fn cell_of(&self, p: Vec3) -> (i64, i64, i64) {
        let c = (p - self.lower) / self.cell;
        (c.x.floor() as i64, c.y.floor() as i64, c.z.floor() as i64)
    }

    fn points_in(&self, i: i64, j: i64, k: i64) -> &[u32] {
        if i < 0 || j < 0 || k < 0 || i > AXIS_LIMIT || j > AXIS_LIMIT || k > AXIS_LIMIT {
            return &[];
        }
        match self.keys.binary_search(&pack(i, j, k)) {
            Ok(at) => {
                let (a, b) = (self.starts[at] as usize, self.starts.get(at + 1).copied().unwrap_or(0) as usize);
                self.order.get(a..b).unwrap_or_default()
            }
            Err(_) => &[],
        }
    }

    /// The `count` points nearest `p` (but `skip`), nearest first, as (squared distance, point).
    pub(crate) fn nearest(&self, p: Vec3, count: usize, skip: Option<u32>, out: &mut Vec<(f64, u32)>) {
        out.clear();
        if count == 0 {
            return;
        }
        let (ci, cj, ck) = self.cell_of(p);
        for r in 0..48i64 {
            for di in -r..=r {
                for dj in -r..=r {
                    let edge = di.abs() == r || dj.abs() == r;
                    let mut dk = -r;
                    while dk <= r {
                        for &q in self.points_in(ci + di, cj + dj, ck + dk) {
                            if Some(q) == skip {
                                continue;
                            }
                            let d = (self.points[q as usize] - p).len2();
                            let worst = out.last().copied();
                            if out.len() < count || worst.is_some_and(|w| d < w.0 || d == w.0 && q < w.1) {
                                let at = out.partition_point(|&(e, i)| e < d || e == d && i < q);
                                out.insert(at, (d, q));
                                out.truncate(count);
                            }
                        }
                        // Inside the shell only its two faces along k are new.
                        dk += if edge { 1 } else { 2 * r.max(1) };
                    }
                }
            }
            let reach = r as f64 * self.cell;
            if out.len() == count && out.last().is_some_and(|w| w.0 <= reach * reach) {
                return;
            }
        }
    }

    /// Each point within `radius` of `p`, with its squared distance.
    pub(crate) fn within(&self, p: Vec3, radius: f64, mut visit: impl FnMut(u32, f64)) {
        let (ci, cj, ck) = self.cell_of(p);
        let span = (radius / self.cell).ceil().min(64.0) as i64;
        let limit = radius * radius;
        for di in -span..=span {
            for dj in -span..=span {
                for dk in -span..=span {
                    for &q in self.points_in(ci + di, cj + dj, ck + dk) {
                        let d = (self.points[q as usize] - p).len2();
                        if d <= limit {
                            visit(q, d);
                        }
                    }
                }
            }
        }
    }
}

/// The typical distance from a point to its nearest neighbour: the median over points spread
/// through the cloud.
pub fn typical_spacing(points: &[Vec3]) -> f64 {
    let grid = PointGrid::sized_for(points, 4.0);
    let stride = (points.len() / 4000).max(1);
    let picked: Vec<usize> = (0..points.len()).step_by(stride).collect();
    let mut distances: Vec<f64> = par::map(picked.len(), |i| {
        let k = picked[i];
        let mut out = Vec::with_capacity(1);
        grid.nearest(points[k], 1, Some(k as u32), &mut out);
        out.first().map(|&(d, _)| d.sqrt()).filter(|&d| d > 0.0)
    })
    .into_iter()
    .flatten()
    .collect();
    if distances.is_empty() {
        return 0.0;
    }
    distances.sort_by(f64::total_cmp);
    distances[distances.len() / 2]
}

/// How thick the layer of points is across the surface: the median, over points spread through
/// the cloud, of the root mean square distance of the points within `LAYER_REACH` spacings of one
/// from the plane fitted to them. A single clean pass reads its noise (a few hundredths of a mm);
/// passes merged without lying on each other read their gap (YCB's merged mug cloud: about 1 mm).
pub fn layer_thickness(points: &[Vec3], spacing: f64) -> f64 {
    if points.len() < 16 || !(spacing > 0.0 && spacing.is_finite()) {
        return 0.0;
    }
    let reach = LAYER_REACH * spacing;
    let grid = PointGrid::new(points, reach / 2.0);
    let stride = (points.len() / 2000).max(1);
    let picked: Vec<usize> = (0..points.len()).step_by(stride).collect();
    let mut spreads: Vec<f64> = par::map(picked.len(), |i| {
        let at = points[picked[i]];
        let mut near = Vec::new();
        grid.within(at, reach, |q, _| near.push(points[q as usize]));
        if near.len() < 12 {
            return None;
        }
        let middle = near.iter().fold(Vec3::ZERO, |s, q| s + *q) / near.len() as f64;
        let mut spread = [[0.0f64; 3]; 3];
        for q in &near {
            let d = [q.x - middle.x, q.y - middle.y, q.z - middle.z];
            for (row, di) in spread.iter_mut().zip(d) {
                for (cell, dj) in row.iter_mut().zip(d) {
                    *cell += di * dj;
                }
            }
        }
        let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
        let normal = axes[0].normalized()?;
        Some((near.iter().map(|q| (*q - middle).dot(normal).powi(2)).sum::<f64>() / near.len() as f64).sqrt())
    })
    .into_iter()
    .flatten()
    .collect();
    if spreads.is_empty() {
        return 0.0;
    }
    spreads.sort_by(f64::total_cmp);
    spreads[spreads.len() / 2]
}

/// How far round a point [`layer_thickness`] reads the layer, in spacings.
const LAYER_REACH: f64 = 8.0;

/// The points thinned to one a `cell`: each cell's mean, with its normals' mean.
fn thinned(points: &[Vec3], normals: Option<&[Vec3]>, cell: f64) -> (Vec<Vec3>, Option<Vec<Vec3>>) {
    let grid = PointGrid::new(points, cell);
    let cells = grid.keys.len();
    let mut out = Vec::with_capacity(cells);
    let mut out_normals = normals.map(|_| Vec::with_capacity(cells));
    for c in 0..cells {
        let members = grid.order.get(grid.starts[c] as usize..grid.starts[c + 1] as usize).unwrap_or_default();
        let sum = members.iter().fold(Vec3::ZERO, |s, &m| s + points[m as usize]);
        out.push(sum / members.len().max(1) as f64);
        if let (Some(list), Some(given)) = (out_normals.as_mut(), normals) {
            let n = members.iter().fold(Vec3::ZERO, |s, &m| s + given.get(m as usize).copied().unwrap_or(Vec3::ZERO));
            list.push(n.normalized().unwrap_or(Vec3::ZERO));
        }
    }
    (out, out_normals)
}

/// The least-spread direction of `p` and its neighbours.
fn fitted_normal(p: Vec3, near: &[(f64, u32)], points: &[Vec3]) -> Vec3 {
    if near.len() < 2 {
        return Vec3::Z;
    }
    let all: Vec<Vec3> = std::iter::once(p).chain(near.iter().map(|&(_, q)| points[q as usize])).collect();
    let mean = all.iter().fold(Vec3::ZERO, |s, q| s + *q) / all.len() as f64;
    let mut c = [[0.0f64; 3]; 3];
    for q in &all {
        let d = *q - mean;
        let v = [d.x, d.y, d.z];
        for (row, a) in c.iter_mut().zip(v) {
            for (cell, b) in row.iter_mut().zip(v) {
                *cell += a * b;
            }
        }
    }
    solidcraft_geom::inertia::principal(&c, 0.0).and_then(|(_, axes)| axes[0].normalized()).unwrap_or(Vec3::Z)
}

/// Each point's normal (stated, else fitted and turned along the spanning tree) and the area it
/// stands for.
fn estimate(points: &[Vec3], stated: Option<&[Vec3]>) -> (Vec<Vec3>, Vec<f64>) {
    let n = points.len();
    let grid = PointGrid::sized_for(points, 6.0);
    let found: Vec<(Vec3, f64, [u32; GRAPH_NEIGHBOURS])> = par::map(n, |k| {
        let mut near = Vec::with_capacity(FIT_NEIGHBOURS + 1);
        grid.nearest(points[k], FIT_NEIGHBOURS, Some(k as u32), &mut near);
        let r2 = near.last().map_or(0.0, |&(d, _)| d);
        let area = std::f64::consts::PI * r2 / near.len().max(1) as f64;
        let graph = std::array::from_fn(|g| near.get(g).map_or(u32::MAX, |&(_, q)| q));
        let given = stated.and_then(|s| s.get(k).copied()).unwrap_or(Vec3::ZERO);
        (if given == Vec3::ZERO { fitted_normal(points[k], &near, points) } else { given }, area, graph)
    });
    let mut normals: Vec<Vec3> = found.iter().map(|f| f.0).collect();
    let areas: Vec<f64> = found.iter().map(|f| f.1).collect();
    let graph: Vec<u32> = found.iter().flat_map(|f| f.2).collect();
    let fixed: Vec<bool> = (0..n).map(|k| stated.is_some_and(|s| s.get(k).is_some_and(|v| *v != Vec3::ZERO))).collect();
    orient(points, &mut normals, &graph, &fixed);
    if !fixed.iter().any(|f| *f) {
        // Most normals away from the middle: a closed part's outside, an open one's bulging side.
        let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / n.max(1) as f64;
        let outward: f64 = points.iter().zip(&normals).map(|(p, m)| (*p - middle).dot(*m)).sum();
        if outward < 0.0 {
            for m in &mut normals {
                *m = -*m;
            }
        }
    }
    (normals, areas)
}

/// The normals turned to agree along a minimum spanning tree of the neighbour graph (each point's
/// `GRAPH_NEIGHBOURS` both ways), weighted by how far from parallel they are; from the fixed
/// points, else from the point whose normal runs most nearly through the middle, turned outward.
fn orient(points: &[Vec3], normals: &mut [Vec3], graph: &[u32], fixed: &[bool]) {
    let n = points.len();
    let mut counts = vec![0u32; n + 1];
    for (k, &q) in graph.iter().enumerate() {
        if (q as usize) < n {
            counts[k / GRAPH_NEIGHBOURS] += 1;
            counts[q as usize] += 1;
        }
    }
    let mut starts = vec![0u32; n + 1];
    for k in 0..n {
        starts[k + 1] = starts[k] + counts[k];
    }
    let mut fill = starts.clone();
    let mut adjacent = vec![0u32; starts[n] as usize];
    for (k, &q) in graph.iter().enumerate() {
        if (q as usize) < n {
            let p = k / GRAPH_NEIGHBOURS;
            adjacent[fill[p] as usize] = q;
            fill[p] += 1;
            adjacent[fill[q as usize] as usize] = p as u32;
            fill[q as usize] += 1;
        }
    }
    let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / n.max(1) as f64;
    let mut seeds: Vec<(f64, u32)> = (0..n).map(|k| (-(points[k] - middle).dot(normals[k]).abs(), k as u32)).collect();
    seeds.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut seed_at = 0;
    let mut done = vec![false; n];
    let mut best = vec![f32::INFINITY; n];
    let mut parent = vec![u32::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    for k in (0..n).filter(|&k| fixed[k]) {
        best[k] = 0.0;
        heap.push(Reverse((0, k as u32)));
    }
    loop {
        let Some(Reverse((_, v))) = heap.pop() else {
            // A part no seed has reached yet.
            while seed_at < n && done[seeds[seed_at].1 as usize] {
                seed_at += 1;
            }
            if seed_at == n {
                break;
            }
            let k = seeds[seed_at].1 as usize;
            if (points[k] - middle).dot(normals[k]) < 0.0 {
                normals[k] = -normals[k];
            }
            best[k] = 0.0;
            parent[k] = u32::MAX;
            heap.push(Reverse((0, k as u32)));
            continue;
        };
        let v = v as usize;
        if done[v] {
            continue;
        }
        done[v] = true;
        if let Some(&u) = parent.get(v).filter(|&&u| (u as usize) < n)
            && !fixed[v]
            && normals[v].dot(normals[u as usize]) < 0.0
        {
            normals[v] = -normals[v];
        }
        for &q in &adjacent[starts[v] as usize..starts[v + 1] as usize] {
            let q = q as usize;
            if done[q] {
                continue;
            }
            let weight = (1.0 - normals[v].dot(normals[q]).abs()).max(0.0) as f32;
            if weight < best[q] {
                best[q] = weight;
                parent[q] = v as u32;
                heap.push(Reverse((weight.to_bits(), q as u32)));
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// The solve.

/// A point in the cube's units, its unit normal, and the area it stands for (in the cube's units
/// squared).
struct Sample {
    at: Vec3,
    normal: Vec3,
    area: f64,
}

fn key(i: i64, j: i64, k: i64) -> u64 {
    (i as u64) | ((j as u64) << 21) | ((k as u64) << 42)
}

fn unkey(key: u64) -> (i64, i64, i64) {
    ((key & 0x1f_ffff) as i64, ((key >> 21) & 0x1f_ffff) as i64, (key >> 42) as i64)
}

/// The field solved at one level: its nodes, `2^depth + 1` along each side, those solved and
/// their values.
struct Level {
    nodes: i64,
    index: HashMap<u64, u32>,
    values: Vec<f64>,
}

/// χ at every level, coarsest first.
struct Field {
    levels: Vec<Level>,
}

/// Reads χ anywhere, remembering the values of nodes no level solved for.
struct Evaluator<'a> {
    field: &'a Field,
    cache: Vec<HashMap<u64, f64>>,
}

impl<'a> Evaluator<'a> {
    fn new(field: &'a Field) -> Self {
        Evaluator { field, cache: (0..field.levels.len()).map(|_| HashMap::new()).collect() }
    }

    /// χ at node (i, j, k) of `level`: solved, or the coarser level's there.
    fn node(&mut self, level: usize, i: i64, j: i64, k: i64) -> f64 {
        let Some(l) = self.field.levels.get(level) else { return 0.0 };
        let key = key(i, j, k);
        if let Some(&at) = l.index.get(&key) {
            return l.values.get(at as usize).copied().unwrap_or(0.0);
        }
        if let Some(&v) = self.cache[level].get(&key) {
            return v;
        }
        let v = if level == 0 { 0.0 } else { self.level_at(level - 1, Vec3::new(i as f64, j as f64, k as f64) / 2.0) };
        self.cache[level].insert(key, v);
        v
    }

    /// χ at `u`, in `level`'s node units, by trilinear interpolation of its nodes.
    fn level_at(&mut self, level: usize, u: Vec3) -> f64 {
        let last = self.field.levels.get(level).map_or(0, |l| l.nodes - 2).max(0);
        let clamp = |x: f64| (x.floor() as i64).clamp(0, last);
        let c = [clamp(u.x), clamp(u.y), clamp(u.z)];
        let f = [u.x - c[0] as f64, u.y - c[1] as f64, u.z - c[2] as f64];
        let mut sum = 0.0;
        for corner in 0..8i64 {
            let (a, b, d) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let w = (if a == 1 { f[0] } else { 1.0 - f[0] }) * (if b == 1 { f[1] } else { 1.0 - f[1] }) * (if d == 1 { f[2] } else { 1.0 - f[2] });
            if w != 0.0 {
                sum += w * self.node(level, c[0] + a, c[1] + b, c[2] + d);
            }
        }
        sum
    }

    /// χ at `at` in the cube's units, from the finest level.
    fn at(&mut self, at: Vec3) -> f64 {
        let finest = self.field.levels.len().saturating_sub(1);
        let n = self.field.levels.get(finest).map_or(1, |l| l.nodes - 1) as f64;
        self.level_at(finest, at * n)
    }
}

/// The cell of `u` (in the cube's units) at `n` cells across.
fn cell_of(u: Vec3, n: i64) -> [i64; 3] {
    let c = |x: f64| ((x * n as f64).floor() as i64).clamp(0, n - 1);
    [c(u.x), c(u.y), c(u.z)]
}

/// The weight of corner `corner` of a cell at place `f` in it.
fn weight(corner: usize, f: [f64; 3]) -> f64 {
    (if corner & 1 == 1 { f[0] } else { 1.0 - f[0] })
        * (if corner & 2 == 2 { f[1] } else { 1.0 - f[1] })
        * (if corner & 4 == 4 { f[2] } else { 1.0 - f[2] })
}

/// The nodes a level solves for: every node at the coarsest; at a finer one, those within [`BAND`]
/// nodes of a cell that holds a point (sorted keys).
fn active_nodes(samples: &[Sample], depth: u32, everywhere: bool) -> Result<Vec<u64>, PoissonError> {
    let n = 1i64 << depth;
    if everywhere {
        let nodes = n + 1;
        let mut keys = Vec::with_capacity((nodes * nodes * nodes) as usize);
        for k in 0..nodes {
            for j in 0..nodes {
                for i in 0..nodes {
                    keys.push(key(i, j, k));
                }
            }
        }
        keys.sort_unstable();
        return Ok(keys);
    }
    let mut keys: Vec<u64> = samples
        .iter()
        .map(|s| {
            let [i, j, k] = cell_of(s.at, n);
            key(i, j, k)
        })
        .collect();
    keys.sort_unstable();
    keys.dedup();
    for axis in 0..3 {
        let mut grown = Vec::with_capacity(keys.len() * (2 * BAND as usize + 2));
        for &k in &keys {
            let (i, j, l) = unkey(k);
            for d in -BAND..=BAND + 1 {
                let mut c = [i, j, l];
                c[axis] += d;
                if (0..=n).contains(&c[axis]) {
                    grown.push(key(c[0], c[1], c[2]));
                }
            }
        }
        grown.sort_unstable();
        grown.dedup();
        if grown.len() > MOST_NODES {
            return Err(PoissonError::TooFine);
        }
        keys = grown;
    }
    Ok(keys)
}

/// A sample at one level: its cell's eight corner nodes and its place in the cell.
struct Placed {
    corners: [u32; 8],
    f: [f64; 3],
}

/// The level's matrix, as products: the graph Laplacian (Neumann at the cube's faces) plus the
/// screening at the samples.
struct System<'a> {
    neighbours: &'a [[u32; 6]],
    laplacian: &'a [f64],
    preconditioner: &'a [f64],
    placed: &'a [Placed],
    screening: &'a [f64],
    /// Each node's samples: (sample, its corner the node is).
    node_samples: &'a [Vec<(u32, u8)>],
}

impl System<'_> {
    /// A x.
    fn apply(&self, x: &[f64]) -> Vec<f64> {
        let at_samples: Vec<f64> = par::map(self.placed.len(), |s| {
            let p = &self.placed[s];
            (0..8).map(|c| weight(c, p.f) * x[p.corners[c] as usize]).sum()
        });
        par::map(x.len(), |at| {
            let sum: f64 = self.neighbours[at].iter().filter(|&&m| m != u32::MAX).map(|&m| x[m as usize]).sum();
            let screen: f64 = self.node_samples[at]
                .iter()
                .map(|&(s, corner)| self.screening[s as usize] * weight(corner as usize, self.placed[s as usize].f) * at_samples[s as usize])
                .sum();
            self.laplacian[at] * x[at] - sum + screen
        })
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(p, q)| p * q).sum()
}

/// A x = b from the x given, by conjugate gradients with the diagonal as preconditioner, until the
/// residual is `tolerance` of b's or `iterations` pass.
fn conjugate_gradients(system: &System<'_>, b: &[f64], x: &mut [f64], tolerance: f64, iterations: usize) {
    let ax = system.apply(x);
    let mut r: Vec<f64> = b.iter().zip(&ax).map(|(p, q)| p - q).collect();
    let goal = tolerance * tolerance * dot(b, b).max(1e-300);
    let precondition = |r: &[f64]| -> Vec<f64> { r.iter().zip(system.preconditioner).map(|(r, d)| if *d > 0.0 { r / d } else { *r }).collect() };
    let mut z = precondition(&r);
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    for _ in 0..iterations {
        if dot(&r, &r) <= goal {
            break;
        }
        let ap = system.apply(&p);
        let pap = dot(&p, &ap);
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rz / pap;
        for (xi, pi) in x.iter_mut().zip(&p) {
            *xi += alpha * pi;
        }
        for (ri, api) in r.iter_mut().zip(&ap) {
            *ri -= alpha * api;
        }
        z = precondition(&r);
        let next = dot(&r, &z);
        let beta = next / rz;
        rz = next;
        for (pi, zi) in p.iter_mut().zip(&z) {
            *pi = zi + beta * *pi;
        }
    }
}

/// Solves one level from the coarser ones.
fn solve_level(samples: &[Sample], depth: u32, everywhere: bool, coarser: &Field) -> Result<Level, PoissonError> {
    let n = 1i64 << depth;
    let keys = active_nodes(samples, depth, everywhere)?;
    let count = keys.len();
    let index: HashMap<u64, u32> = keys.iter().enumerate().map(|(k, &key)| (key, k as u32)).collect();
    let mut evaluator = (!everywhere).then(|| Evaluator::new(coarser));
    let mut coarse_at = |i: i64, j: i64, k: i64| -> f64 {
        match evaluator.as_mut() {
            Some(e) => {
                let last = e.field.levels.len().saturating_sub(1);
                e.level_at(last, Vec3::new(i as f64, j as f64, k as f64) / 2.0)
            }
            None => 0.0,
        }
    };
    let mut neighbours = vec![[u32::MAX; 6]; count];
    let mut diagonal = vec![0.0f64; count];
    let mut rhs = vec![0.0f64; count];
    let mut x = vec![0.0f64; count];
    for (at, &k) in keys.iter().enumerate() {
        let (i, j, l) = unkey(k);
        if !everywhere {
            x[at] = coarse_at(i, j, l);
        }
        for (slot, (di, dj, dl)) in [(-1, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)].into_iter().enumerate() {
            let (a, b, c) = (i + di, j + dj, l + dl);
            if !(0..=n).contains(&a) || !(0..=n).contains(&b) || !(0..=n).contains(&c) {
                continue;
            }
            diagonal[at] += 1.0;
            match index.get(&key(a, b, c)) {
                Some(&m) => neighbours[at][slot] = m,
                None => rhs[at] += coarse_at(a, b, c),
            }
        }
    }
    let h = 1.0 / n as f64;
    let mut placed = Vec::with_capacity(samples.len());
    for s in samples {
        let c = cell_of(s.at, n);
        let mut corners = [0u32; 8];
        for (corner, slot) in corners.iter_mut().enumerate() {
            let corner = corner as i64;
            *slot = *index
                .get(&key(c[0] + (corner & 1), c[1] + ((corner >> 1) & 1), c[2] + ((corner >> 2) & 1)))
                .ok_or(PoissonError::Empty("the grid lost a point's cell"))?;
        }
        let u = s.at * n as f64;
        placed.push(Placed { corners, f: [u.x - c[0] as f64, u.y - c[1] as f64, u.z - c[2] as f64] });
    }
    let mut spread = vec![Vec3::ZERO; count];
    let screening: Vec<f64> = samples.iter().map(|s| SCREENING * s.area / (h * h)).collect();
    let mut preconditioner = diagonal.clone();
    let mut node_samples: Vec<Vec<(u32, u8)>> = vec![Vec::new(); count];
    for (k, ((s, p), beta)) in samples.iter().zip(&placed).zip(&screening).enumerate() {
        for corner in 0..8 {
            let w = weight(corner, p.f);
            let node = p.corners[corner] as usize;
            spread[node] += s.normal * (s.area * w / (h * h));
            preconditioner[node] += beta * w * w;
            node_samples[node].push((k as u32, corner as u8));
        }
    }
    let get = |v: Vec3, axis: usize| [v.x, v.y, v.z][axis];
    for at in 0..count {
        let (i, j, l) = unkey(keys[at]);
        for (axis, place) in [i, j, l].into_iter().enumerate() {
            let mine = get(spread[at], axis);
            let other = |slot: usize| neighbours[at][slot];
            if place > 0 {
                let o = other(2 * axis);
                rhs[at] += (mine + if o == u32::MAX { 0.0 } else { get(spread[o as usize], axis) }) / 2.0;
            }
            if place < n {
                let o = other(2 * axis + 1);
                rhs[at] -= (mine + if o == u32::MAX { 0.0 } else { get(spread[o as usize], axis) }) / 2.0;
            }
        }
    }
    let system = System {
        neighbours: &neighbours,
        laplacian: &diagonal,
        preconditioner: &preconditioner,
        placed: &placed,
        screening: &screening,
        node_samples: &node_samples,
    };
    let (tolerance, iterations) = if everywhere { (1e-6, 600) } else { (1e-5, 200) };
    conjugate_gradients(&system, &rhs, &mut x, tolerance, iterations);
    Ok(Level { nodes: n + 1, index, values: x })
}

/// Surface nets on the finest level where χ equals `iso`, followed from the `seeds` cells across
/// every cell the surface crosses that `may_enter` allows (given the cell's centre in the cube's
/// units). The vertices (in the cube's units) and triangles, facing where χ rises.
fn surface_nets(
    field: &Field,
    iso: f64,
    depth: u32,
    seeds: &[[i64; 3]],
    may_enter: &dyn Fn(Vec3) -> bool,
) -> Result<(Vec<Vec3>, Vec<[u32; 3]>), PoissonError> {
    let n = 1i64 << depth;
    let finest = field.levels.len().saturating_sub(1);
    let mut evaluator = Evaluator::new(field);
    let mut value = |i: i64, j: i64, k: i64| evaluator.node(finest, i, j, k) - iso;
    let mut vertex_of: HashMap<u64, u32> = HashMap::new();
    let mut positions: Vec<Vec3> = Vec::new();
    let mut edges: Vec<(u64, u8, bool)> = Vec::new();
    let mut queued: HashSet<u64> = seeds.iter().map(|c| key(c[0], c[1], c[2])).collect();
    let mut queue: VecDeque<[i64; 3]> = seeds.iter().copied().collect();
    let inside = |c: [i64; 3]| c.iter().all(|&v| (0..n).contains(&v));
    while let Some(c) = queue.pop_front() {
        let mut corner = [0.0f64; 8];
        for (k, v) in corner.iter_mut().enumerate() {
            let k = k as i64;
            *v = value(c[0] + (k & 1), c[1] + ((k >> 1) & 1), c[2] + ((k >> 2) & 1));
        }
        let mut sum = [0.0f64; 3];
        let mut crossings = 0;
        for axis in 0..3usize {
            let bit = 1usize << axis;
            for k in (0..8usize).filter(|k| k & bit == 0) {
                let (a, b) = (corner[k], corner[k | bit]);
                if (a < 0.0) == (b < 0.0) {
                    continue;
                }
                let t = a / (a - b);
                let mut p = [(k & 1) as f64, ((k >> 1) & 1) as f64, ((k >> 2) & 1) as f64];
                p[axis] = t;
                for d in 0..3 {
                    sum[d] += p[d];
                }
                crossings += 1;
                let node = [c[0] + (k as i64 & 1), c[1] + ((k as i64 >> 1) & 1), c[2] + ((k as i64 >> 2) & 1)];
                if k == 0 {
                    edges.push((key(node[0], node[1], node[2]), axis as u8, a < 0.0));
                }
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                for (du, dv) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                    let mut other = node;
                    other[u] += du;
                    other[v] += dv;
                    if inside(other) && queued.insert(key(other[0], other[1], other[2])) {
                        let centre = (Vec3::new(other[0] as f64, other[1] as f64, other[2] as f64) + Vec3::new(0.5, 0.5, 0.5)) / n as f64;
                        if may_enter(centre) {
                            queue.push_back(other);
                        }
                    }
                }
            }
        }
        if crossings > 0 {
            if positions.len() >= MOST_VERTICES {
                return Err(PoissonError::TooFine);
            }
            vertex_of.insert(key(c[0], c[1], c[2]), positions.len() as u32);
            let local = Vec3::new(sum[0], sum[1], sum[2]) / f64::from(crossings);
            positions.push((Vec3::new(c[0] as f64, c[1] as f64, c[2] as f64) + local) / n as f64);
        }
    }
    let mut triangles = Vec::with_capacity(edges.len() * 2);
    for (edge, axis, rising) in edges {
        let (i, j, k) = unkey(edge);
        let node = [i, j, k];
        let axis = axis as usize;
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut quad = [0u32; 4];
        let mut whole = true;
        for (slot, (du, dv)) in [(-1, -1), (0, -1), (0, 0), (-1, 0)].into_iter().enumerate() {
            let mut cell = node;
            cell[u] += du;
            cell[v] += dv;
            match vertex_of.get(&key(cell[0], cell[1], cell[2])) {
                Some(&at) if inside(cell) => quad[slot] = at,
                _ => whole = false,
            }
        }
        if !whole {
            continue;
        }
        if !rising {
            quad.swap(1, 3);
        }
        let at = |q: u32| positions[q as usize];
        if at(quad[0]).dist(at(quad[2])) <= at(quad[1]).dist(at(quad[3])) {
            triangles.extend([[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]]);
        } else {
            triangles.extend([[quad[0], quad[1], quad[3]], [quad[1], quad[2], quad[3]]]);
        }
    }
    Ok((positions, triangles))
}

/// How far `points` are from the surface: every point, or [`MEASURED`] of them spread evenly.
fn distances(points: &[Vec3], positions: &[Vec3], triangles: &[[u32; 3]]) -> Distances {
    let mesh = solidcraft_geom::Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), ..Default::default() };
    let tree = solidcraft_geom::compare::TriangleTree::new(&mesh);
    let chosen = crate::thinned(points, MEASURED);
    let mut d: Vec<f64> = par::map(chosen.len(), |i| tree.nearest(chosen[i]).map_or(0.0, |h| h.0));
    let rms = (d.iter().map(|x| x * x).sum::<f64>() / d.len().max(1) as f64).sqrt();
    d.sort_by(f64::total_cmp);
    let p95 = d.get((d.len() * 95 / 100).min(d.len().saturating_sub(1))).copied().unwrap_or(0.0);
    Distances { rms, p95, max: d.last().copied().unwrap_or(0.0), measured: d.len() }
}

/// The surface of `points` (with their `normals`, when the cloud states them) at `settings`.
pub fn reconstruct(points: &[Vec3], normals: Option<&[Vec3]>, settings: &Settings) -> Result<Surface, PoissonError> {
    let depth = settings.depth.clamp(LEAST_DEPTH, MOST_DEPTH);
    let points: Vec<Vec3> = points.iter().copied().filter(|p| p.is_finite()).collect();
    if points.len() < 16 {
        return Err(PoissonError::Empty("the cloud has too few points to make a surface of"));
    }
    let normals = normals.filter(|n| n.len() == points.len());
    let (lower, upper) = bounds(&points);
    let d = upper - lower;
    let extent = d.x.max(d.y).max(d.z).max(1e-9);
    let side = extent * ROOM;
    let corner = (lower + upper) / 2.0 - Vec3::new(side, side, side) / 2.0;
    let cell = side / f64::from(1u32 << depth);

    let (thin, stated) = thinned(&points, normals, cell / 2.0);
    let (normals_used, areas) = estimate(&thin, stated.as_deref());
    let spacing = typical_spacing(&points);
    let samples: Vec<Sample> = thin
        .iter()
        .zip(&normals_used)
        .zip(&areas)
        .map(|((p, n), a)| Sample { at: (*p - corner) / side, normal: *n, area: *a / (side * side) })
        .collect();

    let mut field = Field { levels: Vec::new() };
    let base = BASE_DEPTH.min(depth);
    for level in base..=depth {
        crate::progress::stage("solving the surface", (level - base) as usize, (depth - base + 1) as usize);
        let solved = solve_level(&samples, level, level == base, &field)?;
        field.levels.push(solved);
    }
    let iso = {
        let mut e = Evaluator::new(&field);
        let (mut sum, mut weight) = (0.0, 0.0);
        for s in &samples {
            sum += s.area * e.at(s.at);
            weight += s.area;
        }
        if weight > 0.0 { sum / weight } else { 0.0 }
    };

    let reach = 4.0 * cell.max(spacing);
    let point_grid = PointGrid::new(&thin, reach);
    let n = 1i64 << depth;
    let mut seeds: Vec<[i64; 3]> = samples.iter().map(|s| cell_of(s.at, n)).collect();
    seeds.sort_unstable();
    seeds.dedup();
    let prune = settings.trim > 0.0;
    let near_points = |centre: Vec3| -> bool {
        let mut any = false;
        point_grid.within(corner + centre * side, reach + cell, |_, _| any = true);
        any
    };
    let (unit_positions, triangles) = surface_nets(&field, iso, depth, &seeds, &|c: Vec3| !prune || near_points(c))?;
    if triangles.is_empty() {
        return Err(PoissonError::Empty("no surface could be made through the points"));
    }
    let positions: Vec<Vec3> = unit_positions.iter().map(|u| corner + *u * side).collect();
    let norm = 3.0 / (std::f64::consts::PI * reach * reach);
    let density: Vec<f64> = par::map(positions.len(), |i| {
        let mut sum = 0.0;
        point_grid.within(positions[i], reach, |q, d2| {
            let t = 1.0 - d2 / (reach * reach);
            sum += areas[q as usize] * t * t;
        });
        sum * norm
    });
    let keep: Vec<bool> = density.iter().map(|&d| d >= settings.trim).collect();
    let mut number = vec![u32::MAX; positions.len()];
    let (mut kept_positions, mut kept) = (Vec::new(), Vec::with_capacity(triangles.len()));
    let mut trimmed = 0;
    for t in &triangles {
        if t.iter().all(|&v| keep[v as usize]) {
            let mut out = [0u32; 3];
            for (slot, &v) in out.iter_mut().zip(t) {
                if number[v as usize] == u32::MAX {
                    number[v as usize] = kept_positions.len() as u32;
                    kept_positions.push(positions[v as usize]);
                }
                *slot = number[v as usize];
            }
            kept.push(out);
        } else {
            trimmed += 1;
        }
    }
    if kept.is_empty() {
        return Err(PoissonError::Empty("the density trim left no surface: lower the trim"));
    }
    let distances = distances(&points, &kept_positions, &kept);
    Ok(Surface {
        positions: kept_positions,
        triangles: kept,
        settings: Settings { depth, trim: settings.trim },
        points: points.len(),
        points_used: thin.len(),
        estimated_normals: normals.is_none(),
        cell,
        spacing,
        trimmed,
        distances,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points spread evenly over a sphere (a Fibonacci lattice).
    fn sphere(count: usize, radius: f64, centre: Vec3) -> Vec<Vec3> {
        let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
        (0..count)
            .map(|k| {
                let z = 1.0 - 2.0 * (k as f64 + 0.5) / count as f64;
                let r = (1.0 - z * z).sqrt();
                let a = golden * k as f64;
                centre + Vec3::new(r * a.cos(), r * a.sin(), z) * radius
            })
            .collect()
    }

    fn volume(positions: &[Vec3], triangles: &[[u32; 3]]) -> f64 {
        triangles.iter().map(|t| positions[t[0] as usize].dot(positions[t[1] as usize].cross(positions[t[2] as usize])) / 6.0).sum()
    }

    /// A sphere's points, no normals stated: a closed surface (no trim) about the sphere's volume,
    /// facing out, the points on it.
    #[test]
    fn a_sphere_of_points_is_a_closed_sphere() {
        let points = sphere(20_000, 25.0, Vec3::new(10.0, -4.0, 7.0));
        let made = reconstruct(&points, None, &Settings { depth: 7, trim: 0.0 }).unwrap();
        let v = volume(&made.positions, &made.triangles);
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 25.0f64.powi(3);
        assert!((v - exact).abs() < 0.02 * exact, "{v} against {exact}");
        assert!(made.distances.rms < 0.15, "{:?}", made.distances);
        assert!(made.estimated_normals);
    }

    /// Three passes of one sphere merged without lying on each other (each moved 1 mm another way)
    /// read as a layer about as thick as their gap, far more than their spacing, and are meshed with
    /// cells no finer than that: one closed surface through the layer, not a sheet for each pass.
    /// One pass alone reads thin, and keeps cells as fine as its spacing.
    #[test]
    fn passes_merged_apart_read_as_a_thick_layer_and_mesh_as_one_surface() {
        let one = sphere(20_000, 25.0, Vec3::ZERO);
        let spacing = typical_spacing(&one);
        let thin = layer_thickness(&one, spacing);
        assert!(thin < 0.3 * spacing, "one pass is thin (its curve over the reach read): {thin} (spacing {spacing})");
        let mut merged = Vec::new();
        for shift in [Vec3::new(1.0, 0.0, 0.0), Vec3::new(-0.5, 0.87, 0.0), Vec3::new(-0.5, -0.87, 0.0)] {
            merged.extend(sphere(20_000, 25.0, shift));
        }
        let spacing = typical_spacing(&merged);
        let thick = layer_thickness(&merged, spacing);
        assert!(thick > LAYERED * spacing && thick > 0.3, "the passes read as a layer: {thick} (spacing {spacing})");
        assert_eq!(Settings::suggested_for(50.0, 0.4, 0.0).depth, Settings::suggested(50.0, 0.4).depth, "a thin layer changes nothing");
        let settings = Settings::suggested_for(51.0, spacing, thick);
        assert!(cell_size(51.0, settings.depth) >= LAYERED_CELLS * thick, "cells no finer than the layer: depth {}", settings.depth);
        let made = reconstruct(&merged, None, &Settings { trim: 0.0, ..settings }).unwrap();
        let v = volume(&made.positions, &made.triangles);
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 25.0f64.powi(3);
        assert!((v - exact).abs() < 0.1 * exact, "one sphere through the layer: {v} against {exact}");
    }

    /// Half a sphere of points, trimmed: the surface stops near where the points do and is not
    /// closed over the open half.
    #[test]
    fn half_a_sphere_is_trimmed_where_the_points_stop() {
        let points: Vec<Vec3> = sphere(20_000, 20.0, Vec3::ZERO).into_iter().filter(|p| p.z > 0.0).collect();
        let made = reconstruct(&points, None, &Settings { depth: 7, trim: DEFAULT_TRIM }).unwrap();
        let lowest = made.positions.iter().map(|p| p.z).fold(f64::INFINITY, f64::min);
        assert!(lowest > -2.0, "nothing far below the points: {lowest}");
        assert!(made.trimmed > 0 && made.distances.p95 < 0.3, "{} {:?}", made.trimmed, made.distances);
    }

    #[test]
    fn too_few_points_are_refused() {
        assert!(matches!(reconstruct(&[Vec3::ZERO; 4], None, &Settings { depth: 6, trim: 0.0 }), Err(PoissonError::Empty(_))));
    }
}
