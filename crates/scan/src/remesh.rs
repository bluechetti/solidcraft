//! Remesh: a scan, or several passes of one part laid on each other, made again as one even
//! surface, after SurfaceCraft's surface fusion (Curless and Levoy's volumetric merging). Each
//! scan says, in a narrow band of grid points round its surface, how far each point is from it
//! (signed: positive on the side its faces point to); where scans overlap the readings are
//! averaged, and the surface is read off where the average is zero by surface nets (a vertex in
//! each cell the surface crosses, two triangles for each grid edge it crosses), optionally put
//! where the planes of a sharp cell meet (dual contouring), so a part's edges stay sharp. Pieces
//! far smaller than the rest (noise read as surface) go.

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
    /// The triangles of the small pieces taken away.
    pub dropped: usize,
}

/// Why no surface was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemeshError {
    /// The grid would be too large to hold at this cell.
    TooFine,
    /// No surface was read: the scans have no area, or nothing crossed the grid.
    Empty,
}

impl std::fmt::Display for RemeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RemeshError::TooFine => "the grid would be too fine to hold: use a larger cell",
            RemeshError::Empty => "no surface could be read off the scans at this cell",
        })
    }
}

/// How far from a scan's surface it speaks, in cells.
const BAND: i64 = 2;
/// The most grid points read.
const MOST_NODES: usize = 40_000_000;
/// The most scan vertices the error is measured at.
const MEASURED: usize = 200_000;
/// A piece with fewer than this share of the triangles goes.
const LEAST_PIECE: f64 = 0.002;

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
        let open = uses.into_iter().filter(|(_, n)| *n == 1).map(|(e, _)| e).collect();
        Source { tree: TriangleTree::new(&mesh), normals, weld, open, scan }
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
    let cell = settings.cell;
    if !(cell.is_finite() && cell > 0.0) || sources.iter().all(|s| s.triangle_count() == 0) {
        return Err(RemeshError::Empty);
    }
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
        nodes = dilated(&nodes, axis, BAND);
        if nodes.len() > MOST_NODES {
            return Err(RemeshError::TooFine);
        }
    }
    let prepared: Vec<Source> = sources.iter().map(|s| Source::new(*s)).collect();
    let reach = BAND as f64 * cell;
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
        if count > 0 { (sum / count as f64) as f32 } else { f32::NAN }
    });
    let index: HashMap<u64, u32> = nodes.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect();
    let value = |i: i64, j: i64, k: i64| index.get(&pack(i, j, k)).map(|&at| values[at as usize]).filter(|v| v.is_finite());

    // A vertex for each patch of surface in each cell it crosses: the cell's crossed edges joined
    // across its faces (a face crossed four times split by the sign at its middle, which both of
    // its cells read alike), so two sheets that pass through one cell are not glued together.
    let mut candidates: Vec<u64> = occupied.clone();
    for axis in 0..3 {
        candidates = dilated(&candidates, axis, 1);
    }
    let made: Vec<Vec<(u16, Vec3)>> = par::map(candidates.len(), |c| {
        let (i, j, k) = unpack(candidates[c]);
        let mut corner = [0.0f64; 8];
        for (m, slot) in corner.iter_mut().enumerate() {
            let m = m as i64;
            let Some(v) = value(i + (m & 1), j + ((m >> 1) & 1), k + ((m >> 2) & 1)) else { return Vec::new() };
            *slot = f64::from(v);
        }
        let crossed: Vec<usize> = (0..12)
            .filter(|&e| {
                let (from, to) = edge_ends(e);
                (corner[from] < 0.0) != (corner[to] < 0.0)
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
                if !settings.sharp {
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
    // Pieces far smaller than the rest go.
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
    let before = triangles.len();
    let kept: Vec<[u32; 3]> =
        triangles.into_iter().zip(&roots).filter(|(_, r)| sizes.get(r).copied().unwrap_or(0) >= least).map(|(t, _)| t).collect();
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
        let mended = crate::repair::repair(&Scan::new(&out_positions, &out_triangles));
        (mended.positions, mended.triangles)
    };
    let (new_rms, new_max, scan_rms, scan_p95, uncovered) = measure(&out_positions, &out_triangles, &prepared, cell);
    Ok(Remeshed { positions: out_positions, triangles: out_triangles, new_rms, new_max, scan_rms, scan_p95, uncovered, dropped })
}

/// How far the new surface is from the scans, both ways.
fn measure(positions: &[Vec3], triangles: &[[u32; 3]], sources: &[Source], cell: f64) -> (f64, f64, f64, f64, f64) {
    let sampled = crate::thinned(positions, MEASURED);
    let to_scans: Vec<f64> =
        par::map(sampled.len(), |i| sources.iter().filter_map(|s| s.tree.nearest(sampled[i]).map(|h| h.0)).fold(f64::INFINITY, f64::min));
    let rms = |d: &[f64]| (d.iter().filter(|x| x.is_finite()).map(|x| x * x).sum::<f64>() / d.len().max(1) as f64).sqrt();
    let new_max = to_scans.iter().copied().filter(|x| x.is_finite()).fold(0.0, f64::max);
    let mesh = solidcraft_geom::Mesh { positions: positions.to_vec(), triangles: triangles.to_vec(), ..Default::default() };
    let tree = TriangleTree::new(&mesh);
    let vertices: Vec<Vec3> = sources.iter().flat_map(|s| s.scan.points()).collect();
    let picked = crate::thinned(&vertices, MEASURED);
    let mut from_scans: Vec<f64> = par::map(picked.len(), |i| tree.nearest(picked[i]).map_or(f64::INFINITY, |h| h.0));
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
}
