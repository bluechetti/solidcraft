//! Remesh: a scan, or several passes of one part laid on each other, made again as one even
//! surface, after SurfaceCraft's surface fusion (Curless and Levoy's volumetric merging). Each
//! scan says, in a narrow band of grid points round its surface, how far each point is from it
//! (signed: positive on the side its faces point to); where scans overlap the readings are
//! averaged, and the surface is read off where the average is zero by surface nets (a vertex in
//! each cell the surface crosses, two triangles for each grid edge it crosses), optionally put
//! where the planes of a sharp cell meet (dual contouring), so a part's edges stay sharp. A cell
//! a scan leaves partly unread (past its open edge) keeps the edges it does read, so a scan's rim
//! and a strip a few cells wide keep their surface. Small pieces that do not lie on a scan (a few
//! faces read a cell or more off it, where readings at the band's edge disagree) go; a small piece
//! on a scan (a fragment of it, a strip cut off past a gap) stays.

use std::collections::{HashMap, HashSet};

use solidcore_record::Vec3;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::{Scan, UnionFind, par};

/// How a scan is made again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The grid's cell, about the new mesh's edge, in mm.
    pub cell: f64,
    /// Put each vertex where the planes of its cell meet, so sharp edges and corners stay sharp.
    pub sharp: bool,
}

/// A scan made again, and how far it is from what it was made of.
#[derive(Clone, Debug, PartialEq)]
pub struct Remeshed {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// The new vertices' distance from the nearest scan: root mean square and largest, in mm.
    pub new_rms: f64,
    pub new_max: f64,
    /// The scans' vertices' distance from the new surface: root mean square and 95th percentile.
    pub scan_rms: f64,
    pub scan_p95: f64,
    /// The share of the scans' vertices more than two cells from the new surface (what it left out).
    pub uncovered: f64,
    /// The triangles taken away: those of small pieces not on a scan, and those two vertices at one
    /// place left with no area.
    pub dropped: usize,
}

/// Why no surface was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemeshError {
    /// The grid would be too large to hold at this cell.
    TooFine,
    /// No surface was read: the scans have no area, or nothing crossed the grid.
    Empty,
    /// A shell's two skins have open rims that do not pair up to be joined.
    Rims,
}

impl std::fmt::Display for RemeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RemeshError::TooFine => "the grid would be too fine to hold: use a larger cell",
            RemeshError::Empty => "no surface could be read off the scans at this cell",
            RemeshError::Rims => {
                "the shell's outside and inside have open rims that do not pair up: fill the scan's holes first, or give a thinner wall"
            }
        })
    }
}

/// How far from a scan's surface it speaks, in cells.
const BAND: i64 = 2;
/// The most grid points read.
const MOST_NODES: usize = 40_000_000;
/// The farthest an offset reaches, in cells: past it the band is mostly empty space.
const MOST_OFFSET_CELLS: f64 = 200.0;
/// The most scan vertices the error is measured at.
const MEASURED: usize = 200_000;
/// A piece with fewer than this share of the triangles is looked at: it stays only if it lies on a
/// scan (its points, the middle of them, within [`ON_SCAN`] of one).
const LEAST_PIECE: f64 = 0.002;
/// How near a scan a small piece's points must lie, in cells, to be the scan's own surface.
const ON_SCAN: f64 = 0.5;
/// The most points of a small piece looked at.
const PIECE_SAMPLE: usize = 32;

/// The middle length of a scan's edges: a cell that keeps its detail.
pub fn typical_edge(scan: &Scan) -> f64 {
    let step = (scan.triangle_count() / 20_000).max(1);
    let mut lengths: Vec<f64> = (0..scan.triangle_count())
        .step_by(step)
        .filter_map(|t| scan.corners(t))
        .flat_map(|[a, b, c]| [a.dist(b), b.dist(c), c.dist(a)])
        .filter(|l| l.is_finite() && *l > 0.0)
        .collect();
    if lengths.is_empty() {
        return 0.0;
    }
    lengths.sort_by(f64::total_cmp);
    lengths[lengths.len() / 2]
}

/// A scan as the grid reads it: its nearest points and its vertices' directions.
struct Source<'a> {
    scan: Scan<'a>,
    tree: TriangleTree,
    normals: Vec<Vec3>,
    /// Each position's first copy at the same place, and the scan's open edges (used by one
    /// triangle) between such copies, lowest first.
    weld: Vec<u32>,
    open: HashSet<(u32, u32)>,
    /// The ends of those open edges.
    rim: HashSet<u32>,
}

impl<'a> Source<'a> {
    fn new(scan: Scan<'a>) -> Self {
        let mesh = solidcraft_geom::Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Default::default() };
        let mut normals = vec![Vec3::ZERO; scan.positions.len()];
        for (t, tri) in scan.triangles.iter().enumerate() {
            let Some([a, b, c]) = scan.corners(t) else { continue };
            let n = (b - a).cross(c - a);
            for &v in tri {
                if let Some(slot) = normals.get_mut(v as usize) {
                    *slot += n;
                }
            }
        }
        for n in &mut normals {
            *n = n.normalized().unwrap_or(Vec3::ZERO);
        }
        let weld = crate::welded(scan.positions);
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in scan.triangles {
            let [a, b, c] = tri.map(|v| weld.get(v as usize).copied().unwrap_or(v));
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *uses.entry((p.min(q), p.max(q))).or_default() += 1;
            }
        }
        let open: HashSet<(u32, u32)> = uses.into_iter().filter(|(_, n)| *n == 1).map(|(e, _)| e).collect();
        let rim = open.iter().flat_map(|&(a, b)| [a, b]).collect();
        Source { tree: TriangleTree::new(&mesh), normals, weld, open, rim, scan }
    }

    /// The point of the surface nearest `p` within `reach`: its distance, where it is, and the
    /// face's direction there.
    fn nearest_on(&self, p: Vec3, reach: f64) -> Option<(f64, Vec3, Vec3)> {
        let (distance, t) = self.tree.nearest(p)?;
        if !(distance <= reach) {
            return None;
        }
        let [a, b, c] = self.scan.corners(t as usize)?;
        Some((distance, closest_on_triangle(p, a, b, c), (b - a).cross(c - a).normalized()?))
    }

    /// Whether the scan's nearest point to `p` is on its rim (an open edge, or a vertex one ends at):
    /// `p` lies past where the scan was scanned.
    fn past_rim(&self, p: Vec3) -> bool {
        if self.open.is_empty() {
            return false;
        }
        let Some((_, t)) = self.tree.nearest(p) else { return false };
        let (Some([a, b, c]), Some(tri)) = (self.scan.corners(t as usize), self.scan.triangles.get(t as usize)) else { return false };
        let q = closest_on_triangle(p, a, b, c);
        let near = 1e-9 * (1.0 + a.dist(b).max(b.dist(c)).max(c.dist(a)));
        let corners = [a, b, c];
        let weld = |v: u32| self.weld.get(v as usize).copied().unwrap_or(v);
        for k in 0..3 {
            let (x, y) = (k, (k + 1) % 3);
            let (u, v) = (corners[x], corners[y]);
            let along = (q - u).dot(v - u) / (v - u).dot(v - u).max(1e-300);
            let foot = u + (v - u) * along.clamp(0.0, 1.0);
            if q.dist(foot) > near {
                continue;
            }
            // On this edge: an open one, or at a corner of it some open edge ends at.
            if self.is_open(tri[x], tri[y]) {
                return true;
            }
            for (w, at) in [(tri[x], u), (tri[y], v)] {
                let w = weld(w);
                if q.dist(at) <= near && self.rim.contains(&w) {
                    return true;
                }
            }
        }
        false
    }

    /// Whether the edge from vertex `a` to `b` is one of the scan's open edges.
    fn is_open(&self, a: u32, b: u32) -> bool {
        let weld = |v: u32| self.weld.get(v as usize).copied().unwrap_or(v);
        let (a, b) = (weld(a), weld(b));
        !self.open.is_empty() && self.open.contains(&(a.min(b), a.max(b)))
    }

    /// The signed distance of `p` from the surface (positive where its faces point), with the
    /// surface's direction there, when the surface is within `reach`.
    fn reading(&self, p: Vec3, reach: f64) -> Option<(f64, Vec3)> {
        let (distance, t) = self.tree.nearest(p)?;
        if !(distance <= reach) {
            return None;
        }
        let [a, b, c] = self.scan.corners(t as usize)?;
        let q = closest_on_triangle(p, a, b, c);
        // The vertices' directions at the nearest point, blended: a smooth way out at an edge or a
        // corner, where the face's own is wrong for points round it.
        let (v0, v1) = ((b - a), (c - a));
        let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
        let v2 = q - a;
        let (d20, d21) = (v2.dot(v0), v2.dot(v1));
        let den = d00 * d11 - d01 * d01;
        let (wb, wc) = if den.abs() > 1e-300 { ((d11 * d20 - d01 * d21) / den, (d00 * d21 - d01 * d20) / den) } else { (0.0, 0.0) };
        let tri = self.scan.triangles.get(t as usize)?;
        // Nearest a scan's open edge, the point is beyond where it was scanned: no reading of it.
        let wa = 1.0 - wb - wc;
        for (w, (x, y)) in [(wa, (1, 2)), (wb, (2, 0)), (wc, (0, 1))] {
            if w <= 1e-9 && self.is_open(tri[x], tri[y]) {
                return None;
            }
        }
        let n = |k: usize| self.normals.get(tri[k] as usize).copied().unwrap_or(Vec3::ZERO);
        let face = v0.cross(v1).normalized()?;
        let smooth = (n(0) * (1.0 - wb - wc) + n(1) * wb + n(2) * wc).normalized().unwrap_or(face);
        let sign = if (p - q).dot(smooth) < 0.0 { -1.0 } else { 1.0 };
        Some((sign * distance, face))
    }
}

const BIAS: i64 = 1 << 20;

fn pack(i: i64, j: i64, k: i64) -> u64 {
    ((i + BIAS) as u64) | (((j + BIAS) as u64) << 21) | (((k + BIAS) as u64) << 42)
}

fn unpack(key: u64) -> (i64, i64, i64) {
    ((key & 0x1f_ffff) as i64 - BIAS, ((key >> 21) & 0x1f_ffff) as i64 - BIAS, (key >> 42) as i64 - BIAS)
}

/// The keys `keys` grown by `radius` along `axis`.
fn dilated(keys: &[u64], axis: usize, radius: i64) -> Vec<u64> {
    let mut grown = Vec::with_capacity(keys.len() * (2 * radius as usize + 1));
    for &k in keys {
        let (i, j, l) = unpack(k);
        for d in -radius..=radius {
            let mut c = [i, j, l];
            c[axis] += d;
            grown.push(pack(c[0], c[1], c[2]));
        }
    }
    grown.sort_unstable();
    grown.dedup();
    grown
}

/// A cell's 12 edges: four along each axis, each from a corner with that axis's bit clear (corner
/// `m` is +x when `m & 1`, +y `m & 2`, +z `m & 4`).
fn edge_index(axis: usize, start: usize) -> usize {
    let bit = 1 << axis;
    let others: Vec<usize> = (0..8).filter(|m| m & bit == 0).collect();
    4 * axis + others.iter().position(|&m| m == start).unwrap_or(0)
}

/// The corners edge `e` joins.
fn edge_ends(e: usize) -> (usize, usize) {
    let axis = e / 4;
    let bit = 1 << axis;
    let start = (0..8).filter(|m| m & bit == 0).nth(e % 4).unwrap_or(0);
    (start, start | bit)
}

/// A cell's six faces (two square to each axis): their corners, and their edges.
fn face_corners(face: usize) -> [usize; 4] {
    let (axis, side) = (face / 2, face % 2);
    let mut out = [0usize; 4];
    let mut n = 0;
    for m in 0..8 {
        if (m >> axis) & 1 == side && n < 4 {
            out[n] = m;
            n += 1;
        }
    }
    out
}

fn face_edges(face: usize) -> Vec<usize> {
    let corners = face_corners(face);
    (0..12)
        .filter(|&e| {
            let (a, b) = edge_ends(e);
            corners.contains(&a) && corners.contains(&b)
        })
        .collect()
}

/// The point where planes (through `points` with the unit `normals`) meet, least squares: only
/// the directions they pin are moved from their mean (a flat cell's along it, a creased one's
/// along the crease, stay at the mean).
fn meeting(points: &[Vec3], normals: &[Vec3]) -> Option<Vec3> {
    let mean = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len().max(1) as f64;
    let (mut a, mut b) = ([[0.0f64; 3]; 3], Vec3::ZERO);
    for (p, n) in points.iter().zip(normals) {
        let nv = [n.x, n.y, n.z];
        b += *n * n.dot(*p - mean);
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += nv[i] * nv[j];
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&a, 0.0)?;
    let largest = values.iter().copied().fold(0.0, f64::max);
    if !(largest > 0.0) {
        return Some(mean);
    }
    let mut x = Vec3::ZERO;
    for (value, axis) in values.iter().zip(axes) {
        if *value > 0.1 * largest
            && let Some(axis) = axis.normalized()
        {
            x += axis * (axis.dot(b) / value);
        }
    }
    Some(mean + x)
}

/// `sources` made again as one surface at `settings`.
pub fn remesh(sources: &[Scan], settings: &Settings) -> Result<Remeshed, RemeshError> {
    remesh_at(sources, settings, 0.0)
}

/// The scans made again `offset` off their surface (positive the way their faces point, out of
/// a closed part; negative into it): the surface where the averaged distance is `offset`, read as
/// [`remesh`] reads the one where it is 0, the band of grid points widened by the offset. A
/// distance can't cross itself, so an offset into a hollow or round a tight bend comes out whole
/// where moving each point along its normal would fold. Sharp edges are not kept sharp (an edge
/// grown outward is rounded, as an offset of it is); a scan's open rim is offset as a rim. The
/// errors reported are how far the new surface is from `|offset|` off the scans.
pub fn remesh_at(sources: &[Scan], settings: &Settings, offset: f64) -> Result<Remeshed, RemeshError> {
    let cell = settings.cell;
    if !(cell.is_finite() && cell > 0.0) || !offset.is_finite() || sources.iter().all(|s| s.triangle_count() == 0) {
        return Err(RemeshError::Empty);
    }
    // How many cells the offset reaches past the band round the scans.
    let extra = (offset.abs() / cell).ceil();
    if extra > MOST_OFFSET_CELLS {
        return Err(RemeshError::TooFine);
    }
    let extra = extra as i64;
    let sharp = settings.sharp && offset == 0.0;
    // The cells the scans' surfaces pass through: each triangle's points at half a cell apart,
    // along two of its sides. How many that is is known first, so a cell far too fine is refused
    // before anything is read.
    let cell_of = |p: Vec3| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
    let steps = |a: Vec3, b: Vec3, c: Vec3| (((b - a).len() / (0.5 * cell)).ceil().max(1.0), ((c - a).len() / (0.5 * cell)).ceil().max(1.0));
    let samples: f64 = sources
        .iter()
        .flat_map(|s| (0..s.triangle_count()).filter_map(|t| s.corners(t)))
        .map(|[a, b, c]| {
            let (n, m) = steps(a, b, c);
            (n + 1.0) * (m + 1.0) / 2.0 + 1.0
        })
        .sum();
    if samples > 8.0 * MOST_NODES as f64 {
        return Err(RemeshError::TooFine);
    }
    let mut occupied: Vec<u64> = Vec::new();
    for scan in sources {
        for t in 0..scan.triangle_count() {
            let Some([a, b, c]) = scan.corners(t) else { continue };
            let (n, m) = steps(a, b, c);
            let (n, m) = (n as usize, m as usize);
            for i in 0..=n {
                let u = i as f64 / n as f64;
                for j in 0..=m {
                    let v = j as f64 / m as f64;
                    if u + v > 1.0 + 1e-9 {
                        break;
                    }
                    let (x, y, z) = cell_of(a + (b - a) * u + (c - a) * v);
                    occupied.push(pack(x, y, z));
                }
            }
        }
        occupied.sort_unstable();
        occupied.dedup();
    }
    // The grid points read: the corners of those cells and round them.
    let mut nodes = occupied.clone();
    for axis in 0..3 {
        nodes = dilated(&nodes, axis, BAND + extra);
        if nodes.len() > MOST_NODES {
            return Err(RemeshError::TooFine);
        }
    }
    let prepared: Vec<Source> = sources.iter().map(|s| Source::new(*s)).collect();
    let reach = (BAND + extra) as f64 * cell;
    let point = |k: u64| {
        let (i, j, l) = unpack(k);
        Vec3::new(i as f64 * cell, j as f64 * cell, l as f64 * cell)
    };
    // Each grid point's distance: the scans' readings averaged.
    let values: Vec<f32> = par::map(nodes.len(), |n| {
        let p = point(nodes[n]);
        let (mut sum, mut count) = (0.0, 0usize);
        for s in &prepared {
            if let Some((d, _)) = s.reading(p, reach) {
                sum += d;
                count += 1;
            }
        }
        if count > 0 { (sum / count as f64 - offset) as f32 } else { f32::NAN }
    });
    let index: HashMap<u64, u32> = nodes.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect();
    let value = |i: i64, j: i64, k: i64| index.get(&pack(i, j, k)).map(|&at| values[at as usize]).filter(|v| v.is_finite());

    // A vertex for each patch of surface in each cell it crosses: the cell's crossed edges joined
    // across its faces (a face crossed four times split by the sign at its middle, which both of
    // its cells read alike), so two sheets that pass through one cell are not glued together.
    let mut candidates: Vec<u64> = occupied.clone();
    for axis in 0..3 {
        candidates = dilated(&candidates, axis, 1 + extra);
    }
    let made: Vec<Vec<(u16, Vec3)>> = par::map(candidates.len(), |c| {
        let (i, j, k) = unpack(candidates[c]);
        // A corner no scan reads (past a scan's open edge) leaves its edges out, not the cell: the
        // cell's other edges still say where the surface crosses it, so a scan's rim and a strip a
        // few cells wide keep their surface (as SurfaceCraft's fusion reads them).
        let mut corner = [0.0f64; 8];
        let mut read = [false; 8];
        for m in 0..8usize {
            let n = m as i64;
            if let Some(v) = value(i + (n & 1), j + ((n >> 1) & 1), k + ((n >> 2) & 1)) {
                corner[m] = f64::from(v);
                read[m] = true;
            }
        }
        let crossed: Vec<usize> = (0..12)
            .filter(|&e| {
                let (from, to) = edge_ends(e);
                read[from] && read[to] && (corner[from] < 0.0) != (corner[to] < 0.0)
            })
            .collect();
        if crossed.is_empty() {
            return Vec::new();
        }
        let mut joined = UnionFind::new(12);
        for face in 0..6 {
            let on: Vec<usize> = crossed.iter().copied().filter(|&e| face_edges(face).contains(&e)).collect();
            match on.as_slice() {
                [x, y] => joined.join(*x, *y),
                [_, _, _, _] => {
                    // Each corner of the face's lone sign keeps its two edges.
                    let corners = face_corners(face);
                    let middle: f64 = corners.iter().map(|&m| corner[m]).sum::<f64>() / 4.0;
                    for &m in &corners {
                        if (corner[m] < 0.0) == (middle < 0.0) {
                            continue;
                        }
                        let mine: Vec<usize> = on
                            .iter()
                            .copied()
                            .filter(|&e| {
                                let (from, to) = edge_ends(e);
                                from == m || to == m
                            })
                            .collect();
                        if let [x, y] = mine.as_slice() {
                            joined.join(*x, *y);
                        }
                    }
                }
                _ => {}
            }
        }
        let mut patches: Vec<(usize, u16, Vec<Vec3>)> = Vec::new();
        for &e in &crossed {
            let root = joined.find(e);
            let (from, to) = edge_ends(e);
            let t = corner[from] / (corner[from] - corner[to]);
            let place = |m: usize| [(m & 1) as f64, ((m >> 1) & 1) as f64, ((m >> 2) & 1) as f64];
            let (p, q) = (place(from), place(to));
            let at = Vec3::new(
                (i as f64 + p[0] + (q[0] - p[0]) * t) * cell,
                (j as f64 + p[1] + (q[1] - p[1]) * t) * cell,
                (k as f64 + p[2] + (q[2] - p[2]) * t) * cell,
            );
            match patches.iter_mut().find(|patch| patch.0 == root) {
                Some(patch) => {
                    patch.1 |= 1 << e;
                    patch.2.push(at);
                }
                None => patches.push((root, 1 << e, vec![at])),
            }
        }
        // A little past the cell: an edge or a corner just beyond it is still its patch's.
        let slack = Vec3::new(0.5 * cell, 0.5 * cell, 0.5 * cell);
        let lo = Vec3::new(i as f64 * cell, j as f64 * cell, k as f64 * cell) - slack;
        let hi = lo + Vec3::new(cell, cell, cell) + slack * 2.0;
        patches
            .into_iter()
            .map(|(_, mask, crossings)| {
                let mean = crossings.iter().fold(Vec3::ZERO, |s, p| s + *p) / crossings.len() as f64;
                if !sharp {
                    return (mask, mean);
                }
                // Where the planes of the surfaces at the crossings meet, kept within the cell: each
                // crossing put on the nearest scan first (the distance is not straight along a grid
                // edge near a sharp edge, so the one read off it lies a little off).
                let on: Vec<(Vec3, Vec3)> = crossings
                    .iter()
                    .filter_map(|q| {
                        prepared.iter().filter_map(|s| s.nearest_on(*q, reach)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, at, n)| (at, n))
                    })
                    .collect();
                if on.len() < crossings.len() {
                    return (mask, mean);
                }
                let (points, normals): (Vec<Vec3>, Vec<Vec3>) = on.into_iter().unzip();
                let at = meeting(&points, &normals).unwrap_or(mean);
                let inside = at.x >= lo.x && at.y >= lo.y && at.z >= lo.z && at.x <= hi.x && at.y <= hi.y && at.z <= hi.z;
                (mask, if inside { at } else { mean })
            })
            .collect()
    });
    let mut vertex_of: HashMap<u64, Vec<(u16, u32)>> = HashMap::new();
    let mut positions = Vec::new();
    for (c, patches) in candidates.iter().zip(&made) {
        for &(mask, at) in patches {
            vertex_of.entry(*c).or_default().push((mask, positions.len() as u32));
            positions.push(at);
        }
    }
    // Two triangles for each grid edge the surface crosses, facing where the distance rises, each
    // corner the vertex of the patch the edge is in, in each of the four cells round it.
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for &key in &nodes {
        let (i, j, l) = unpack(key);
        let Some(a) = value(i, j, l) else { continue };
        for axis in 0..3usize {
            let mut to = [i, j, l];
            to[axis] += 1;
            let Some(b) = value(to[0], to[1], to[2]) else { continue };
            if (a < 0.0) == (b < 0.0) {
                continue;
            }
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut quad = [0u32; 4];
            let mut whole = true;
            for (slot, (du, dv)) in [(-1i64, -1i64), (0, -1), (0, 0), (-1, 0)].into_iter().enumerate() {
                let mut c = [i, j, l];
                c[u] += du;
                c[v] += dv;
                // The edge within that cell: along `axis`, from its corner at the edge's start.
                let mut bits = [0usize; 3];
                bits[u] = usize::from(du == -1);
                bits[v] = usize::from(dv == -1);
                let start = bits[0] | (bits[1] << 1) | (bits[2] << 2);
                let local = edge_index(axis, start);
                match vertex_of.get(&pack(c[0], c[1], c[2])).and_then(|ps| ps.iter().find(|(mask, _)| mask & (1 << local) != 0)) {
                    Some(&(_, at)) => quad[slot] = at,
                    None => whole = false,
                }
            }
            if !whole {
                continue;
            }
            if a >= 0.0 {
                quad.swap(1, 3);
            }
            let p = |q: u32| positions[q as usize];
            if p(quad[0]).dist(p(quad[2])) <= p(quad[1]).dist(p(quad[3])) {
                triangles.extend([[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]]);
            } else {
                triangles.extend([[quad[0], quad[1], quad[3]], [quad[1], quad[2], quad[3]]]);
            }
        }
    }
    if triangles.is_empty() {
        return Err(RemeshError::Empty);
    }
    // Small pieces that are not on a scan go: noise read as surface, a cell or more off it.
    let mut pieces = UnionFind::new(positions.len());
    for t in &triangles {
        pieces.join(t[0] as usize, t[1] as usize);
        pieces.join(t[1] as usize, t[2] as usize);
    }
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    let roots: Vec<usize> = triangles.iter().map(|t| pieces.find(t[0] as usize)).collect();
    for r in &roots {
        *sizes.entry(*r).or_default() += 1;
    }
    let least = (LEAST_PIECE * triangles.len() as f64).ceil() as usize;
    let mut sampled: HashMap<usize, Vec<u32>> = HashMap::new();
    for (t, r) in triangles.iter().zip(&roots) {
        if sizes.get(r).copied().unwrap_or(0) < least {
            let at = sampled.entry(*r).or_default();
            for &v in t {
                if at.len() < PIECE_SAMPLE && !at.contains(&v) {
                    at.push(v);
                }
            }
        }
    }
    let on_scan: HashSet<usize> = sampled
        .into_iter()
        .filter(|(_, vertices)| {
            let mut gaps: Vec<f64> = vertices
                .iter()
                .filter_map(|&v| prepared.iter().filter_map(|s| s.tree.nearest(*positions.get(v as usize)?)).map(|(d, _)| d).min_by(f64::total_cmp))
                .map(|d| (d - offset.abs()).abs())
                .collect();
            gaps.sort_by(f64::total_cmp);
            gaps.get(gaps.len() / 2).is_some_and(|middle| *middle <= ON_SCAN * cell)
        })
        .map(|(r, _)| r)
        .collect();
    let before = triangles.len();
    let kept: Vec<[u32; 3]> = triangles
        .into_iter()
        .zip(&roots)
        .filter(|(_, r)| sizes.get(r).copied().unwrap_or(0) >= least || on_scan.contains(r))
        .map(|(t, _)| t)
        .collect();
    // Vertices put at one place (two cells' patches at one corner) are one, and the triangles
    // that leaves with two corners together are none.
    let step = 1e-7 * cell;
    let mut at_place: HashMap<[i64; 3], u32> = HashMap::new();
    let same: Vec<u32> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| *at_place.entry([(p.x / step).round() as i64, (p.y / step).round() as i64, (p.z / step).round() as i64]).or_insert(i as u32))
        .collect();
    let kept: Vec<[u32; 3]> =
        kept.into_iter().map(|t| t.map(|v| same[v as usize])).filter(|t| t[0] != t[1] && t[1] != t[2] && t[2] != t[0]).collect();
    let dropped = before - kept.len();
    // Numbered again without the vertices no triangle uses.
    let mut number = vec![u32::MAX; positions.len()];
    let mut out_positions = Vec::new();
    let out_triangles: Vec<[u32; 3]> = kept
        .iter()
        .map(|t| {
            t.map(|v| {
                if number[v as usize] == u32::MAX {
                    number[v as usize] = out_positions.len() as u32;
                    out_positions.push(positions[v as usize]);
                }
                number[v as usize]
            })
        })
        .collect();
    // An edge three or more faces met along (two sheets read into one where scans overlap) put
    // right: its extra faces taken off.
    let (out_positions, out_triangles) = if is_manifold(&out_triangles) {
        (out_positions, out_triangles)
    } else {
        let mended = crate::repair::repair(&Scan::new(&out_positions, &out_triangles), false);
        (mended.positions, mended.triangles)
    };
    // An offset's rim taken out to where the scan's rim moved by the offset is: the grid reads
    // nothing past a scan's open edge, so the surface stops a cell short of it.
    let (out_positions, out_triangles) =
        if offset != 0.0 { out_to_rims(out_positions, out_triangles, &prepared, offset, cell) } else { (out_positions, out_triangles) };
    let (new_rms, new_max, scan_rms, scan_p95, uncovered) = measure(&out_positions, &out_triangles, &prepared, cell, offset.abs());
    Ok(Remeshed { positions: out_positions, triangles: out_triangles, new_rms, new_max, scan_rms, scan_p95, uncovered, dropped })
}

/// How far an open loop of the new surface may lie from a scan's rim moved by the offset, in cells,
/// for the strip between them to be made.
const RIM_REACH: f64 = 2.5;
/// The scans made again as a hollow part `wall` thick: the surface `offset` off them (as
/// [`remesh_at`]) and, inside it, the surface `wall` farther in, facing inward, the two joined by
/// a strip along each pair of open rims (an open scan's rim, a socket's or a cover's edge), so the
/// result is closed and can be printed. The errors reported are the outside's.
pub fn shell(sources: &[Scan], settings: &Settings, offset: f64, wall: f64) -> Result<Remeshed, RemeshError> {
    if !(wall.is_finite() && wall > 0.0) {
        return Err(RemeshError::Empty);
    }
    let outside = remesh_at(sources, settings, offset)?;
    let inside = remesh_at(sources, &Settings { sharp: false, ..*settings }, offset - wall)?;
    let base = outside.positions.len() as u32;
    let mut positions = outside.positions.clone();
    positions.extend_from_slice(&inside.positions);
    let mut triangles = outside.triangles.clone();
    // The inside faces into the hollow: each of its triangles turned over.
    triangles.extend(inside.triangles.iter().map(|&[a, b, c]| [a + base, c + base, b + base]));
    let rims = |tris: &[[u32; 3]]| {
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for &[a, b, c] in tris {
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *uses.entry((p.min(q), p.max(q))).or_insert(0) += 1;
            }
        }
        let open: Vec<(u32, u32)> =
            tris.iter().flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)]).filter(|&(p, q)| uses.get(&(p.min(q), p.max(q))) == Some(&1)).collect();
        loops_of(&open)
    };
    let (outer, inner) = (rims(&outside.triangles), rims(&triangles[outside.triangles.len()..]));
    if outer.len() != inner.len() {
        return Err(RemeshError::Rims);
    }
    let at = |v: u32| positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let middle = |ring: &[u32]| ring.iter().fold(Vec3::ZERO, |s, &v| s + at(v)) / ring.len().max(1) as f64;
    let mut taken = vec![false; inner.len()];
    let mut strips = Vec::new();
    for ring in &outer {
        let m = middle(ring);
        let Some(k) = (0..inner.len()).filter(|&k| !taken[k]).min_by(|&a, &b| middle(&inner[a]).dist(m).total_cmp(&middle(&inner[b]).dist(m))) else {
            return Err(RemeshError::Rims);
        };
        taken[k] = true;
        strips.extend(zipped(ring, &inner[k], &at));
    }
    triangles.extend(strips);
    Ok(Remeshed { positions, triangles, ..outside })
}

/// Triangles joining two loops of points round one rim, `a` and `b` (each a loop of vertex
/// numbers): each step goes on along the loop whose next diagonal is shorter, so the strip
/// neither twists nor folds. `b` is walked the way `a` runs, from its point nearest `a`'s start.
fn zipped(a: &[u32], b: &[u32], at: &dyn Fn(u32) -> Vec3) -> Vec<[u32; 3]> {
    let (n, m) = (a.len(), b.len());
    if n < 2 || m < 2 {
        return Vec::new();
    }
    let start = (0..m).min_by(|&x, &y| at(b[x]).dist(at(a[0])).total_cmp(&at(b[y]).dist(at(a[0])))).unwrap_or(0);
    let along = at(a[1]) - at(a[0]);
    let forward = (at(b[(start + 1) % m]) - at(b[start])).dot(along) >= (at(b[(start + m - 1) % m]) - at(b[start])).dot(along);
    let bv = |j: usize| if forward { b[(start + j) % m] } else { b[(start + m - j % m) % m] };
    let av = |i: usize| a[i % n];
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    while i < n || j < m {
        let step_a = i < n && (j >= m || at(av(i + 1)).dist(at(bv(j))) <= at(av(i)).dist(at(bv(j + 1))));
        // Each runs `a`'s rim edge the other way from the face it ends (`a`'s loops run as its
        // faces do), and `b`'s as walked (against its faces, `b` being turned inward).
        if step_a {
            out.push([av(i + 1), av(i), bv(j)]);
            i += 1;
        } else {
            out.push([bv(j), bv(j + 1), av(i)]);
            j += 1;
        }
    }
    out
}

/// The most pairs of points a loop of the new surface and a rim are compared at: past it the loop
/// is left where it ends.
const MOST_RIM_PAIRS: usize = 50_000_000;

/// Closed loops of directed edges, each edge `(a, b)` once, chained from `a` to `b`; a vertex with
/// more than one edge leaving it, or a chain that does not close, is left out.
fn loops_of(edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut next: HashMap<u32, u32> = HashMap::new();
    let mut forked: HashSet<u32> = HashSet::new();
    for &(a, b) in edges {
        if next.insert(a, b).is_some() {
            forked.insert(a);
        }
    }
    let mut seen: HashSet<u32> = HashSet::new();
    let mut out = Vec::new();
    for &(start, _) in edges {
        if seen.contains(&start) || forked.contains(&start) {
            continue;
        }
        let mut chain = vec![start];
        seen.insert(start);
        let mut at = start;
        let closed = loop {
            let Some(&b) = next.get(&at) else { break false };
            if b == start {
                break true;
            }
            if forked.contains(&b) || !seen.insert(b) {
                break false;
            }
            chain.push(b);
            at = b;
        };
        if closed && chain.len() >= 3 {
            out.push(chain);
        }
    }
    out
}

/// Each scan's rims (its open edges' loops) moved by `offset` along the scan's vertex normals:
/// where the offset surface's rims belong.
fn moved_rims(sources: &[Source], offset: f64) -> Vec<Vec<Vec3>> {
    let mut out = Vec::new();
    for src in sources {
        if src.open.is_empty() {
            continue;
        }
        let weld = |v: u32| src.weld.get(v as usize).copied().unwrap_or(v);
        let mut normal: HashMap<u32, Vec3> = HashMap::new();
        let mut edges = Vec::new();
        for (t, tri) in src.scan.triangles.iter().enumerate() {
            let Some([a, b, c]) = src.scan.corners(t) else { continue };
            let n = (b - a).cross(c - a);
            let w = tri.map(weld);
            for k in 0..3 {
                *normal.entry(w[k]).or_insert(Vec3::ZERO) += n;
                let (p, q) = (w[k], w[(k + 1) % 3]);
                if src.open.contains(&(p.min(q), p.max(q))) {
                    edges.push((p, q));
                }
            }
        }
        for chain in loops_of(&edges) {
            let rim: Vec<Vec3> = chain
                .iter()
                .filter_map(|&v| {
                    let p = *src.scan.positions.get(v as usize)?;
                    Some(p + normal.get(&v).and_then(|n| n.normalized()).unwrap_or(Vec3::ZERO) * offset)
                })
                .collect();
            if rim.len() == chain.len() {
                out.push(rim);
            }
        }
    }
    out
}

/// The surface's open loops each joined by a strip to the scan rim moved by `offset` that it
/// stops short of (within [`RIM_REACH`] cells on average), so an offset reaches the scan's rims;
/// a loop no such rim lies along is left as it is.
fn out_to_rims(mut positions: Vec<Vec3>, mut triangles: Vec<[u32; 3]>, sources: &[Source], offset: f64, cell: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let rims = moved_rims(sources, offset);
    if rims.is_empty() {
        return (positions, triangles);
    }
    // What lies past a rim goes first (a vertex of a cell the rim crosses, half a cell beyond it,
    // read where the nearest scan is its open edge): the strip takes the surface out to the rim.
    let past: Vec<bool> = positions
        .iter()
        .map(|p| {
            sources
                .iter()
                .filter_map(|s| s.tree.nearest(*p).map(|(d, _)| (d, s)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .is_some_and(|(_, s)| s.past_rim(*p))
        })
        .collect();
    triangles.retain(|t| t.iter().all(|&v| !past[v as usize]));
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let open: Vec<(u32, u32)> = triangles
        .iter()
        .flat_map(|t| (0..3).map(move |k| (t[k], t[(k + 1) % 3])))
        .filter(|(a, b)| uses.get(&((*a).min(*b), (*a).max(*b))) == Some(&1))
        .collect();
    let nearest = |p: Vec3, rim: &[Vec3]| -> (usize, f64) {
        rim.iter().enumerate().map(|(j, q)| (j, p.dist(*q))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((0, f64::INFINITY))
    };
    for chain in loops_of(&open) {
        let Some(rim) = rims
            .iter()
            .filter(|rim| chain.len() * rim.len() <= MOST_RIM_PAIRS)
            .map(|rim| (rim, chain.iter().map(|&v| nearest(positions[v as usize], rim).1).sum::<f64>() / chain.len() as f64))
            .filter(|(_, mean)| *mean <= RIM_REACH * cell)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(rim, _)| rim)
        else {
            continue;
        };
        // The rim run the way the loop runs, from the point of it nearest the loop's first.
        let (n, m) = (chain.len(), rim.len());
        let at: Vec<Vec3> = chain.iter().map(|&v| positions[v as usize]).collect();
        let p = |i: usize| at[i % n];
        let (j0, _) = nearest(p(0), rim);
        let ahead = (p(1) - p(0)).dot(rim[(j0 + 1) % m] - rim[(j0 + m - 1) % m]) >= 0.0;
        let q = |j: usize| if ahead { rim[(j0 + j) % m] } else { rim[(j0 + m - j % m) % m] };
        let base = positions.len() as u32;
        positions.extend((0..m).map(q));
        let qi = |j: usize| base + (j % m) as u32;
        // Zipped: the loop's edges each taken the other way round by a triangle, and the rim's.
        let (mut i, mut j) = (0usize, 0usize);
        while i < n || j < m {
            let take_loop = j >= m || (i < n && p(i + 1).dist(q(j)) <= p(i).dist(q(j + 1)));
            if take_loop {
                triangles.push([chain[(i + 1) % n], chain[i % n], qi(j)]);
                i += 1;
            } else {
                triangles.push([chain[i % n], qi(j), qi(j + 1)]);
                j += 1;
            }
        }
    }
    // Numbered again without the vertices no triangle uses now.
    let mut number = vec![u32::MAX; positions.len()];
    let mut kept = Vec::new();
    for t in &mut triangles {
        for v in t.iter_mut() {
            if number[*v as usize] == u32::MAX {
                number[*v as usize] = kept.len() as u32;
                kept.push(positions[*v as usize]);
            }
            *v = number[*v as usize];
        }
    }
    (kept, triangles)
}

/// How far the new surface is from `off` off the scans, both ways.
fn measure(positions: &[Vec3], triangles: &[[u32; 3]], sources: &[Source], cell: f64, off: f64) -> (f64, f64, f64, f64, f64) {
    let sampled = crate::thinned(positions, MEASURED);
    let to_scans: Vec<f64> = par::map(sampled.len(), |i| {
        (sources.iter().filter_map(|s| s.tree.nearest(sampled[i]).map(|h| h.0)).fold(f64::INFINITY, f64::min) - off).abs()
    });
    let rms = |d: &[f64]| (d.iter().filter(|x| x.is_finite()).map(|x| x * x).sum::<f64>() / d.len().max(1) as f64).sqrt();
    let new_max = to_scans.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    let mesh = solidcraft_geom::Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), ..Default::default() };
    let tree = TriangleTree::new(&mesh);
    let vertices: Vec<Vec3> = sources.iter().flat_map(|s| s.scan.points()).collect();
    let picked = crate::thinned(&vertices, MEASURED);
    let mut from_scans: Vec<f64> = par::map(picked.len(), |i| tree.nearest(picked[i]).map_or(f64::INFINITY, |h| (h.0 - off).abs()));
    let uncovered = from_scans.iter().filter(|d| **d > 2.0 * cell).count() as f64 / from_scans.len().max(1) as f64;
    from_scans.sort_by(f64::total_cmp);
    let p95 = from_scans.get((from_scans.len() * 95 / 100).min(from_scans.len().saturating_sub(1))).copied().unwrap_or(0.0);
    let covered: Vec<f64> = from_scans.iter().copied().filter(|d| *d <= 2.0 * cell).collect();
    (rms(&to_scans), new_max, rms(&covered), p95, uncovered)
}

/// How many triangles each edge joins: those with one, and those with more than two.
fn edge_uses(triangles: &[[u32; 3]]) -> (usize, usize) {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for t in triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    (edges.values().filter(|&&n| n == 1).count(), edges.values().filter(|&&n| n > 2).count())
}

/// Whether no edge joins more than two triangles.
pub fn is_manifold(triangles: &[[u32; 3]]) -> bool {
    edge_uses(triangles).1 == 0
}

/// Whether the mesh's edges each join two triangles (a closed surface).
pub fn is_closed(triangles: &[[u32; 3]]) -> bool {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for t in triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let seen: HashSet<u32> = edges.values().copied().collect();
    seen == HashSet::from([2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, revolved, steps, turned};

    /// A ball R 20 made a shell 2 thick, and a bowl (the ball's lower half, open at the top) the
    /// same: both closed and every edge shared by two faces, the ball's two skins about the volume
    /// between spheres of 20 and 18, the bowl's about half of it, the strip at its rim closing it.
    #[test]
    fn a_ball_and_an_open_bowl_are_made_hollow_and_closed() {
        let volume = |p: &[Vec3], t: &[[u32; 3]]| t.iter().map(|&[a, b, c]| p[a as usize].dot(p[b as usize].cross(p[c as usize]))).sum::<f64>() / 6.0;
        let between = 4.0 / 3.0 * std::f64::consts::PI * (20f64.powi(3) - 18f64.powi(3));
        let settings = Settings { cell: 0.8, sharp: false };
        for (half, share) in [(false, 1.0), (true, 0.5)] {
            let last = if half { 16 } else { 32 };
            // Profile as (along the axis, radius), from the bottom pole.
            let profile: Vec<[f64; 2]> = (0..=last)
                .map(|k| {
                    let t = std::f64::consts::PI * f64::from(k) / 32.0;
                    [-20.0 * t.cos(), 20.0 * t.sin()]
                })
                .collect();
            let (positions, triangles) = revolved(&profile, 96, false);
            let made = shell(&[Scan::new(&positions, &triangles)], &settings, 0.0, 2.0).expect("a shell");
            assert!(is_closed(&made.triangles) && is_manifold(&made.triangles), "closed and manifold (half {half})");
            let v = volume(&made.positions, &made.triangles).abs();
            assert!((v - share * between).abs() < 0.04 * share * between, "half {half}: {v} against {}", share * between);
        }
    }

    fn volume(positions: &[Vec3], triangles: &[[u32; 3]]) -> f64 {
        triangles.iter().map(|t| positions[t[0] as usize].dot(positions[t[1] as usize].cross(positions[t[2] as usize])) / 6.0).sum()
    }

    /// A box made again with sharp edges keeps its volume and its corners; smoothly, its edges round.
    #[test]
    fn a_box_remeshed_sharp_keeps_its_edges() {
        let (positions, triangles) = fine_cuboid(Vec3::new(0.3, 0.1, 0.2), Vec3::new(30.3, 20.1, 10.2), 6);
        let axis = Vec3::new(0.2, 0.3, 0.9).normalized().unwrap();
        let moved = turned(&positions, axis, 0.4, Vec3::ZERO);
        let scan = Scan::new(&moved, &triangles);
        let sharp = remesh(&[scan], &Settings { cell: 0.5, sharp: true }).unwrap();
        assert!(is_closed(&sharp.triangles), "a closed surface");
        let v = volume(&sharp.positions, &sharp.triangles);
        assert!((v - 6000.0).abs() < 0.003 * 6000.0, "{v}");
        // Every vertex on the box but at a corner or two, which a vertex near its own cell reaches
        // only as near as that.
        let src = Source::new(scan);
        let off: Vec<f64> = sharp.positions.iter().filter_map(|p| src.tree.nearest(*p).map(|h| h.0)).collect();
        let near = off.iter().filter(|d| **d <= 0.02).count() as f64 / off.len() as f64;
        assert!(near > 0.999 && sharp.new_max < 0.25, "{near} within 0.02, {} at most", sharp.new_max);
        let smooth = remesh(&[scan], &Settings { cell: 0.5, sharp: false }).unwrap();
        assert!(smooth.new_max > sharp.new_max, "smooth edges stand off the box's own: {} {}", smooth.new_max, sharp.new_max);
    }

    /// A box offset outward is the box grown round, its edges quarter cylinders and its corners
    /// eighths of a sphere of the offset; inward it is the box shrunk, edges still sharp: each
    /// closed, its volume as those shapes', and its points the offset off the scan.
    #[test]
    fn a_box_offset_out_is_rounded_and_in_is_a_smaller_box() {
        let (positions, triangles) = fine_cuboid(Vec3::new(0.3, 0.1, 0.2), Vec3::new(30.3, 20.1, 10.2), 6);
        let axis = Vec3::new(0.2, 0.3, 0.9).normalized().unwrap();
        let moved = turned(&positions, axis, 0.4, Vec3::ZERO);
        let scan = Scan::new(&moved, &triangles);
        let d = 2.0;
        let out = remesh_at(&[scan], &Settings { cell: 0.4, sharp: false }, d).unwrap();
        assert!(is_closed(&out.triangles));
        let area = 2.0 * (30.0 * 20.0 + 30.0 * 10.0 + 20.0 * 10.0);
        let edges = 4.0 * (30.0 + 20.0 + 10.0);
        let grown = 6000.0 + area * d + edges * std::f64::consts::PI * d * d / 4.0 + 4.0 / 3.0 * std::f64::consts::PI * d.powi(3);
        let v = volume(&out.positions, &out.triangles);
        assert!((v - grown).abs() < 0.01 * grown, "grown round: {v} against {grown}");
        assert!(out.new_rms < 0.05 && out.new_max < 0.2, "{d} mm off the box: {} {}", out.new_rms, out.new_max);
        let inner = remesh_at(&[scan], &Settings { cell: 0.4, sharp: false }, -d).unwrap();
        assert!(is_closed(&inner.triangles));
        let shrunk = 26.0 * 16.0 * 6.0;
        let v = volume(&inner.positions, &inner.triangles);
        assert!((v - shrunk).abs() < 0.02 * shrunk, "shrunk: {v} against {shrunk}");
        // Sharp is not kept on an offset, and hostile offsets are refused.
        assert_eq!(remesh_at(&[scan], &Settings { cell: 0.4, sharp: true }, d).unwrap().positions.len(), out.positions.len());
        assert_eq!(remesh_at(&[scan], &Settings { cell: 0.4, sharp: false }, f64::NAN), Err(RemeshError::Empty));
        assert_eq!(remesh_at(&[scan], &Settings { cell: 0.01, sharp: false }, 50.0), Err(RemeshError::TooFine));
    }

    /// Two passes of a shaft, each of half of it overlapping in the middle, fused into one closed
    /// shaft.
    #[test]
    fn two_overlapping_passes_are_fused_into_one_surface() {
        let mut profile = vec![[0.0, 0.0]];
        profile.extend(steps([0.0, 1.0], [0.0, 10.0], 9));
        profile.extend(steps([0.0, 10.0], [40.0, 10.0], 80).into_iter().skip(1));
        profile.extend(steps([40.0, 10.0], [40.0, 1.0], 9).into_iter().skip(1));
        profile.push([40.0, 0.0]);
        let (positions, triangles) = revolved(&profile, 120, false);
        let part =
            |keep: &dyn Fn(Vec3) -> bool| triangles.iter().copied().filter(|t| t.iter().all(|&v| keep(positions[v as usize]))).collect::<Vec<_>>();
        let (low, high) = (part(&|p| p.z <= 26.0), part(&|p| p.z >= 14.0));
        let fused = remesh(&[Scan::new(&positions, &low), Scan::new(&positions, &high)], &Settings { cell: 0.5, sharp: true }).unwrap();
        // Where the passes overlap, two readings of one sheet may touch: the surface is kept
        // manifold, open at most at a few edges.
        let (open, more) = edge_uses(&fused.triangles);
        assert!(more == 0 && open <= 12, "{open} open, {more} of three or more");
        let v = volume(&fused.positions, &fused.triangles);
        let exact = std::f64::consts::PI * 100.0 * 40.0;
        assert!((v - exact).abs() < 0.01 * exact, "{v} against {exact}");
        assert!(fused.uncovered < 0.001 && fused.scan_rms < 0.05, "{fused:?}");
    }

    #[test]
    fn a_cell_too_fine_or_no_scan_is_refused() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(500.0, 500.0, 500.0), 2);
        assert_eq!(remesh(&[Scan::new(&positions, &triangles)], &Settings { cell: 0.01, sharp: false }), Err(RemeshError::TooFine));
        assert_eq!(remesh(&[Scan::new(&positions, &[])], &Settings { cell: 1.0, sharp: false }), Err(RemeshError::Empty));
    }

    /// A flat sheet `n` × `n` squares of `side` mm, from `at`, tilted off the grid's axes.
    fn sheet(at: [f64; 2], n: u32, side: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.push(Vec3::new(at[0] + side * f64::from(i), at[1] + side * f64::from(j), 0.0));
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
        (turned(&positions, Vec3::new(0.3, -0.5, 0.8).normalized().unwrap(), 0.4, Vec3::new(1.3, -0.7, 2.1)), triangles)
    }

    /// The farthest any of the scan's points is from the surface made of it.
    fn farthest(positions: &[Vec3], made: &Remeshed) -> f64 {
        let mesh = solidcraft_geom::Mesh { positions: made.positions.clone(), triangles: made.triangles.clone(), ..Default::default() };
        let tree = TriangleTree::new(&mesh);
        positions.iter().map(|p| tree.nearest(*p).map_or(f64::INFINITY, |(d, _)| d)).fold(0.0, f64::max)
    }

    /// An open sheet made again reaches near its rim: a cell some of whose corners lie past the rim
    /// (no reading there) still keeps the edges it does read, so no point of the scan is left as much
    /// as 1.7 cells from the new surface (a cell only all of whose corners are read left 2.2), and the
    /// sheet stays one piece.
    #[test]
    fn an_open_sheet_keeps_its_rim() {
        let (positions, triangles) = sheet([0.0, 0.0], 60, 0.5);
        let made = remesh(&[Scan::new(&positions, &triangles)], &Settings { cell: 0.5, sharp: false }).unwrap();
        let far = farthest(&positions, &made);
        assert!(far < 0.85, "the rim's points are {far} mm from the surface");
        assert_eq!(made.uncovered, 0.0);
        let mut pieces = crate::UnionFind::new(made.positions.len());
        for t in &made.triangles {
            pieces.join(t[0] as usize, t[1] as usize);
            pieces.join(t[1] as usize, t[2] as usize);
        }
        let roots: HashSet<usize> = made.triangles.iter().map(|t| pieces.find(t[0] as usize)).collect();
        assert_eq!(roots.len(), 1);
    }

    /// An open tube offset in and out: the new surface reaches both of the tube's open ends (each
    /// rim moved by the offset, not a cell short of it), its points the offset off the tube, all of it
    /// one piece, every rim edge on one triangle and every other on two.
    #[test]
    fn an_offset_reaches_the_scans_open_rims() {
        let (r, h, around, along) = (10.0, 30.0, 96u32, 60u32);
        let mut positions = Vec::new();
        for j in 0..=along {
            for i in 0..around {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(around);
                positions.push(Vec3::new(r * a.cos(), r * a.sin(), h * f64::from(j) / f64::from(along)));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..along {
            for i in 0..around {
                let (a, b) = (j * around + i, j * around + (i + 1) % around);
                triangles.push([a, b, b + around]);
                triangles.push([a, b + around, a + around]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        for d in [-1.0, 1.0] {
            let made = remesh_at(&[scan], &Settings { cell: 0.5, sharp: false }, d).unwrap();
            let (lo, hi) = made.positions.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.z), hi.max(p.z)));
            assert!(lo.abs() < 1e-6 && (hi - h).abs() < 1e-6, "offset {d}: from {lo} to {hi}, the tube's ends");
            let worst = made.positions.iter().map(|p| ((p.x * p.x + p.y * p.y).sqrt() - (r + d)).abs()).fold(0.0, f64::max);
            assert!(worst < 0.15, "offset {d}: {worst} off the radius {}", r + d);
            let (open, many) = edge_uses(&made.triangles);
            assert_eq!(many, 0, "offset {d}: no edge of three faces");
            assert_eq!(open, 2 * around as usize, "offset {d}: only the two rims are open");
            let mut pieces = crate::UnionFind::new(made.positions.len());
            for t in &made.triangles {
                pieces.join(t[0] as usize, t[1] as usize);
                pieces.join(t[1] as usize, t[2] as usize);
            }
            let roots: HashSet<usize> = made.triangles.iter().map(|t| pieces.find(t[0] as usize)).collect();
            assert_eq!(roots.len(), 1, "offset {d}: one piece");
        }
    }

    /// A small fragment of the scan apart from the rest (far fewer triangles than a five-hundredth of
    /// it) is the scan's own surface: it stays.
    #[test]
    fn a_small_fragment_of_the_scan_stays() {
        let (mut positions, mut triangles) = sheet([0.0, 0.0], 200, 0.5);
        let (bit, bit_triangles) = sheet([110.0, 40.0], 8, 0.5);
        let base = positions.len() as u32;
        positions.extend(&bit);
        triangles.extend(bit_triangles.iter().map(|t| t.map(|v| v + base)));
        let made = remesh(&[Scan::new(&positions, &triangles)], &Settings { cell: 0.5, sharp: false }).unwrap();
        assert!(farthest(&bit, &made) < 0.75, "the fragment is kept: {}", farthest(&bit, &made));
        assert!(made.dropped < 50, "{}", made.dropped);
    }
}
