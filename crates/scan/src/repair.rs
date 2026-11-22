//! Repairing a scan's triangles, after SurfaceCraft's repair: copies of a triangle taken off, an
//! edge three or more faces meet along left with the two that continue each other best, and
//! slivers (three corners, next to no area) put right by joining their neighbours the other way
//! across. With `crossings`, faces that pass through others they share no edge with are taken off
//! with the ring of faces round them (which the fold has bent), and each hole so made is closed
//! with a smooth patch. Points a hair apart (a file's two copies of one point, differing in their
//! last digits) are joined first, closing the crack between them. (Equal points welded, triangles
//! gone to a line or a point, and the winding of each piece are put right when the scan is built,
//! so they need no step here.)

use std::collections::{HashMap, HashSet};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, welded_within};

/// A repaired scan and what was done.
#[derive(Clone, Debug, PartialEq)]
pub struct Repaired {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Points joined to another a hair away, closing the cracks between them.
    pub joined: usize,
    /// Copies of another triangle taken off.
    pub duplicates: usize,
    /// Faces taken off edges that three or more met along.
    pub non_manifold: usize,
    /// Slivers put right.
    pub slivers: usize,
    /// Pairs of faces found passing through each other, the faces taken off for them (with the
    /// ring round them), and the holes that left closed again.
    pub crossings: usize,
    pub crossing_faces: usize,
    pub refilled: usize,
}

impl Repaired {
    /// Whether anything was done.
    pub fn changed(&self) -> bool {
        self.joined + self.duplicates + self.non_manifold + self.slivers + self.crossing_faces > 0
    }
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// A face is a sliver when its area is below this share of the square of its longest side.
const SLIVER: f64 = 1e-6;

/// Repairs the scan; with `crossings`, its faces that pass through others too.
pub fn repair(scan: &Scan, crossings: bool) -> Repaired {
    let (weld, joined) = welded_within(scan.positions);
    let at = |v: u32| scan.positions[v as usize];
    // The triangles on welded points; those gone to a line or a point are not triangles.
    let mut tris: Vec<[u32; 3]> = (0..scan.triangle_count())
        .filter_map(|t| {
            scan.corners(t)?;
            let tri = scan.triangles.get(t)?.map(|i| weld[i as usize]);
            (tri[0] != tri[1] && tri[1] != tri[2] && tri[2] != tri[0]).then_some(tri)
        })
        .collect();
    let mut alive = vec![true; tris.len()];

    // Copies: the same three points, either way round.
    let mut seen: HashSet<[u32; 3]> = HashSet::with_capacity(tris.len());
    let mut duplicates = 0;
    for (t, tri) in tris.iter().enumerate() {
        let mut k = *tri;
        k.sort_unstable();
        if !seen.insert(k) {
            alive[t] = false;
            duplicates += 1;
        }
    }
    drop(seen);

    // Edges three or more faces meet along: keep the pair that runs the edge opposite ways (the
    // winding agrees) and bends least between them, else the pair that bends least.
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(tris.len() * 3);
    for (t, tri) in tris.iter().enumerate().filter(|(t, _)| alive[*t]) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            edges.push((pair(a, b), t as u32));
        }
    }
    crate::par::sort_unstable(&mut edges);
    let normal = |tri: &[u32; 3]| (at(tri[1]) - at(tri[0])).cross(at(tri[2]) - at(tri[0])).normalized().unwrap_or(Vec3::ZERO);
    let runs = |tri: &[u32; 3], a: u32, b: u32| (0..3).any(|k| tri[k] == a && tri[(k + 1) % 3] == b);
    let mut non_manifold = 0;
    let mut start = 0;
    while start < edges.len() {
        let end = start + edges[start..].iter().take_while(|e| e.0 == edges[start].0).count();
        let faces: Vec<usize> = edges[start..end].iter().map(|e| e.1 as usize).filter(|&t| alive[t]).collect();
        if faces.len() > 2 {
            let (a, b) = ((edges[start].0 >> 32) as u32, (edges[start].0 & 0xFFFF_FFFF) as u32);
            let mut best: Option<(bool, f64, usize, usize)> = None;
            for (i, &f) in faces.iter().enumerate() {
                for &g in &faces[i + 1..] {
                    let opposite = runs(&tris[f], a, b) != runs(&tris[g], a, b);
                    // Agreeing winding, the two normals alike when the surface goes straight on.
                    let bend = if opposite { normal(&tris[f]).dot(normal(&tris[g])) } else { -normal(&tris[f]).dot(normal(&tris[g])) };
                    if best.is_none_or(|(o, d, _, _)| (opposite, bend) > (o, d)) {
                        best = Some((opposite, bend, f, g));
                    }
                }
            }
            if let Some((_, _, f, g)) = best {
                for &t in &faces {
                    if t != f && t != g {
                        alive[t] = false;
                        non_manifold += 1;
                    }
                }
            }
        }
        start = end;
    }

    // Slivers: flip the longest side with the face across it, when that gives two proper faces
    // and no edge twice. The faces along a side are read from the sorted sides (those still
    // there), and from the sides flips have made: a map of each side's faces took gigabytes
    // for a scan of millions of triangles.
    let mut made: HashMap<u64, [usize; 2]> = HashMap::new();
    let along = |key: u64, alive: &[bool]| -> Vec<usize> {
        let from = edges.partition_point(|e| e.0 < key);
        edges[from..].iter().take_while(|e| e.0 == key).map(|e| e.1 as usize).filter(|&t| alive[t]).collect()
    };
    let area = |tri: &[u32; 3]| (at(tri[1]) - at(tri[0])).cross(at(tri[2]) - at(tri[0])).len() / 2.0;
    let mut slivers = 0;
    let mut touched = vec![false; tris.len()];
    for t in 0..tris.len() {
        if !alive[t] || touched[t] {
            continue;
        }
        let tri = tris[t];
        let sides = [(tri[0], tri[1], tri[2]), (tri[1], tri[2], tri[0]), (tri[2], tri[0], tri[1])];
        let Some(&(a, b, c)) = sides.iter().max_by(|x, y| at(x.0).dist(at(x.1)).total_cmp(&at(y.0).dist(at(y.1)))) else { continue };
        let longest = at(a).dist(at(b));
        if !(area(&tri) < SLIVER * longest * longest) {
            continue;
        }
        let owners = made.get(&pair(a, b)).map_or_else(|| along(pair(a, b), &alive), |o| o.to_vec());
        let Some(across) = Some(owners).filter(|o| o.len() == 2).and_then(|o| o.iter().copied().find(|&u| u != t)) else { continue };
        if !alive[across] || touched[across] {
            continue;
        }
        // The far point of the face across, which runs the side b to a.
        let other = tris[across];
        let Some(&d) = other.iter().find(|&&v| v != a && v != b) else { continue };
        if c == d || made.contains_key(&pair(c, d)) || !along(pair(c, d), &alive).is_empty() || !runs(&other, b, a) {
            continue;
        }
        let (first, second) = ([c, a, d], [d, b, c]);
        let reference = normal(&other);
        if !(area(&first) > 0.0 && area(&second) > 0.0 && normal(&first).dot(reference) > 0.0 && normal(&second).dot(reference) > 0.0) {
            continue;
        }
        tris[t] = first;
        tris[across] = second;
        touched[t] = true;
        touched[across] = true;
        made.insert(pair(c, d), [t, across]);
        slivers += 1;
    }
    drop((made, edges));

    // Faces that pass through others: both of each pair go, and the ring round them.
    let (mut pairs, mut gone) = (0, Vec::new());
    if crossings {
        let live: Vec<usize> = (0..tris.len()).filter(|&t| alive[t]).collect();
        let found = crossing_faces(scan.positions, &tris, &live);
        pairs = found.1;
        if !found.0.is_empty() {
            // The faces along each side, as sorted sides (a map of lists took gigabytes).
            let mut sides: Vec<(u64, u32)> = live
                .iter()
                .flat_map(|&t| {
                    let tri = tris[t];
                    [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])].map(|(a, b)| (pair(a, b), t as u32))
                })
                .collect();
            crate::par::sort_unstable(&mut sides);
            let mut take: HashSet<usize> = found.0.iter().copied().collect();
            for &t in &found.0 {
                let tri = tris[t];
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    let key = pair(a, b);
                    let from = sides.partition_point(|e| e.0 < key);
                    take.extend(sides[from..].iter().take_while(|e| e.0 == key).map(|e| e.1 as usize));
                }
            }
            gone = take.into_iter().filter(|&t| alive[t]).collect();
            gone.sort_unstable();
            for &t in &gone {
                alive[t] = false;
            }
        }
    }

    // The repaired scan: its points as they were, those no longer used dropped.
    let kept: Vec<[u32; 3]> = tris.iter().zip(&alive).filter(|(_, a)| **a).map(|(t, _)| *t).collect();
    let (mut positions, mut triangles) = crate::pieces::keeping(&Scan::new(scan.positions, &kept), |_| true);
    // The holes the crossings left, closed: the rings every point of which is a corner of a face
    // taken off. A ring that also runs where the scan was already open is a hole the scanner left,
    // which taking the faces off only widened: closing it would lay an invented surface over a
    // patch the scanner never saw (a plate's corner rounded off, a hole's wall closed over), and
    // what a scan is read for must be what was scanned. That is Fill Holes', when asked.
    let mut refilled = 0;
    if !gone.is_empty() {
        let bits = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        let corners: HashSet<[u64; 3]> = gone.iter().flat_map(|&t| tris[t]).map(|v| bits(at(v))).collect();
        let repaired = Scan::new(&positions, &triangles);
        let mut found = crate::holes::survey(&repaired);
        found.rings.retain(|r| r.points.iter().all(|&v| positions.get(v as usize).is_some_and(|p| corners.contains(&bits(*p)))));
        for ring in &mut found.rings {
            ring.rim = false;
        }
        if let Some(filled) = crate::holes::fill(&repaired, &found, usize::MAX, true) {
            refilled = filled.filled;
            positions = filled.positions;
            triangles = filled.triangles;
        }
    }
    Repaired { positions, triangles, joined, duplicates, non_manifold, slivers, crossings: pairs, crossing_faces: gone.len(), refilled }
}

/// How many of `scan`'s faces pass through another face (on points welded as Repair Mesh welds
/// them), changing nothing: the faces Repair Mesh would take out and fill round.
pub fn crossing_count(scan: &Scan) -> usize {
    crossing_count_welded(scan, &welded_within(scan.positions).0)
}

/// [`crossing_count`] on points already welded as Repair Mesh welds them (`weld`, as
/// [`welded_within`] gives it): a caller that has welded them does not weld them again.
pub(crate) fn crossing_count_welded(scan: &Scan, weld: &[u32]) -> usize {
    let tris: Vec<[u32; 3]> = (0..scan.triangle_count())
        .filter_map(|t| {
            scan.corners(t)?;
            let tri = scan.triangles.get(t)?.map(|i| weld[i as usize]);
            (tri[0] != tri[1] && tri[1] != tri[2] && tri[2] != tri[0]).then_some(tri)
        })
        .collect();
    let live: Vec<usize> = (0..tris.len()).collect();
    crossing_faces(scan.positions, &tris, &live).0.len()
}

/// The faces among `live` (on welded points) that pass through another face they share no edge
/// with, and how many pairs cross. A grid of cells about a face across finds the pairs to test.
fn crossing_faces(positions: &[Vec3], tris: &[[u32; 3]], live: &[usize]) -> (Vec<usize>, usize) {
    crossing_faces_in(positions, tris, live, SLAB_FACES)
}

/// The grid is listed a slab of cells at a time (along x), each slab with about this many faces:
/// every face in every cell its box covers, all at once, was 130 bytes a face (3 GB for a scan of
/// 23 million faces, 12 GB for 92 million: the most memory Survey Scan took).
const SLAB_FACES: usize = 4_000_000;

/// [`crossing_faces`], the grid listed in slabs of about `per_slab` faces. Each cell lies in one
/// slab and each pair is tried in one cell (where both boxes begin), so the slabs find what the
/// whole grid does.
fn crossing_faces_in(positions: &[Vec3], tris: &[[u32; 3]], live: &[usize], per_slab: usize) -> (Vec<usize>, usize) {
    if live.len() < 2 {
        return (Vec::new(), 0);
    }
    let at = |v: u32| positions[v as usize];
    // Each face's box in single precision, rounded outward: a box a hair larger only tries a pair
    // more, never one fewer.
    let down = |x: f64| {
        let f = x as f32;
        if f64::from(f) > x { f.next_down() } else { f }
    };
    let up = |x: f64| {
        let f = x as f32;
        if f64::from(f) < x { f.next_up() } else { f }
    };
    let boxes: Vec<([f32; 3], [f32; 3])> = live
        .iter()
        .map(|&t| {
            let [a, b, c] = tris[t].map(at);
            let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));
            ([down(lo.x), down(lo.y), down(lo.z)], [up(hi.x), up(hi.y), up(hi.z)])
        })
        .collect();
    let v = |p: [f32; 3]| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
    let (low, high) = boxes.iter().fold((v(boxes[0].0), v(boxes[0].1)), |(lo, hi), (a, b)| (lo.min(v(*a)), hi.max(v(*b))));
    let mean_side = boxes.iter().map(|(a, b)| (v(*b) - v(*a)).x.max((v(*b) - v(*a)).y).max((v(*b) - v(*a)).z)).sum::<f64>() / boxes.len() as f64;
    let extent = (high - low).x.max((high - low).y).max((high - low).z);
    if !(extent > 0.0 && extent.is_finite()) {
        return (Vec::new(), 0);
    }
    // About a face to a cell, and never so fine that a coordinate runs past its 21 bits.
    let cell = (mean_side * 1.2).max(extent / 1_000_000.0);
    let cell_of = |p: [f32; 3]| {
        let g = (v(p) - low) / cell;
        [g.x.max(0.0) as u64, g.y.max(0.0) as u64, g.z.max(0.0) as u64]
    };
    let pack = |c: [u64; 3]| (c[0] << 42) | (c[1] << 21) | c[2];
    // Each face's cells, worked out when wanted (kept for every face, they were 48 bytes a face).
    let range = |i: usize| boxes.get(i).map_or(([0; 3], [0; 3]), |(lo, hi)| (cell_of(*lo), cell_of(*hi)));
    let is_big: Vec<bool> = (0..boxes.len())
        .map(|i| {
            let (a, b) = range(i);
            (b[0] - a[0] + 1) * (b[1] - a[1] + 1) * (b[2] - a[2] + 1) > 64
        })
        .collect();
    let big: Vec<u32> = (0..live.len()).filter(|&i| is_big[i]).map(|i| i as u32).collect();
    // Each face's first and last column of cells along x: what each slab asks of every face.
    let spans: Vec<[u32; 2]> = (0..boxes.len())
        .map(|i| {
            let (a, b) = range(i);
            [u32::try_from(a[0]).unwrap_or(u32::MAX), u32::try_from(b[0]).unwrap_or(u32::MAX)]
        })
        .collect();
    let overlap = |i: usize, j: usize| {
        let ((a0, a1), (b0, b1)) = (boxes[i], boxes[j]);
        (0..3).all(|k| a0[k] <= b1[k] && b0[k] <= a1[k])
    };
    let crossing = |i: usize, j: usize| overlap(i, j) && faces_cross(tris[live[i]], tris[live[j]], positions);
    // A large face is tried against the faces in the cells its box covers, when those are few
    // against all the faces; else against every face.
    let near_path: Vec<bool> = big
        .iter()
        .map(|&b| {
            let (lo, hi) = range(b as usize);
            (hi[0] - lo[0] + 1).saturating_mul(hi[1] - lo[1] + 1).saturating_mul(64) < live.len() as u64
        })
        .collect();
    let mut near: Vec<Vec<u32>> = vec![Vec::new(); big.len()];
    let columns = cell_of(boxes.iter().fold(boxes[0].1, |m, (_, b)| [m[0].max(b[0]), m[1].max(b[1]), m[2].max(b[2])]))[0] + 1;
    let slabs = (live.len().div_ceil(per_slab.max(1)) as u64).clamp(1, columns.max(1));
    let width = columns.div_ceil(slabs).max(1);
    let mut found: Vec<(Vec<u32>, usize)> = Vec::new();
    for slab in 0..slabs {
        let (x0, x1) = (slab * width, ((slab + 1) * width).min(columns));
        if x0 >= x1 {
            continue;
        }
        let mut entries: Vec<(u64, u32)> = Vec::new();
        for (i, &[from, to]) in spans.iter().enumerate() {
            if is_big[i] || u64::from(to) < x0 || u64::from(from) >= x1 {
                continue;
            }
            let (a, b) = range(i);
            for x in a[0].max(x0)..=b[0].min(x1 - 1) {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        entries.push((pack([x, y, z]), i as u32));
                    }
                }
            }
        }
        crate::par::sort_unstable(&mut entries);
        // Each cell's faces tried against each other, the cells in runs side by side on the
        // threads: each run gives the faces it found crossing (both of each pair) and how many pairs.
        let mut cells = Vec::new();
        let mut start = 0;
        while start < entries.len() {
            let end = start + entries[start..].iter().take_while(|e| e.0 == entries[start].0).count();
            cells.push((start, end));
            start = end;
        }
        let run = cells.len().div_ceil(1024).max(1);
        found.extend(crate::par::map(cells.len().div_ceil(run), |r| {
            let (mut hits, mut pairs) = (Vec::new(), 0usize);
            for &(start, end) in cells.iter().skip(r * run).take(run) {
                for x in start..end {
                    for y in x + 1..end {
                        let (i, j) = (entries[x].1 as usize, entries[y].1 as usize);
                        // A pair sharing several cells is tried in one: where both boxes begin.
                        let begin = [0, 1, 2].map(|k| boxes[i].0[k].max(boxes[j].0[k]));
                        if pack(cell_of(begin)) == entries[start].0 && crossing(i, j) {
                            pairs += 1;
                            hits.extend([i as u32, j as u32]);
                        }
                    }
                }
            }
            (hits, pairs)
        }));
        // The large faces' neighbours in this slab's cells.
        for ((&b, list), &by_cells) in big.iter().zip(near.iter_mut()).zip(&near_path) {
            if !by_cells {
                continue;
            }
            let (lo, hi) = range(b as usize);
            for x in lo[0].max(x0)..=hi[0].min(x1 - 1) {
                if lo[0] > x1 - 1 || hi[0] < x0 {
                    break;
                }
                for y in lo[1]..=hi[1] {
                    let from = entries.partition_point(|e| e.0 < pack([x, y, lo[2]]));
                    list.extend(entries[from..].iter().take_while(|e| e.0 <= pack([x, y, hi[2]])).map(|e| e.1));
                }
            }
        }
    }
    // Faces too large for the grid are tried against every face whose box meets theirs: those
    // in the cells their box covers, else, when that is more cells than there are faces, against
    // every face; and against each other, each pair once.
    let found_big = crate::par::map(big.len(), |n| {
        let b = big[n] as usize;
        let (mut hits, mut pairs) = (Vec::new(), 0usize);
        let mut try_one = |i: usize| {
            if crossing(b, i) {
                pairs += 1;
                hits.extend([b as u32, i as u32]);
            }
        };
        if near_path[n] {
            let mut list = near[n].clone();
            list.sort_unstable();
            list.dedup();
            list.into_iter().for_each(|i| try_one(i as usize));
        } else {
            (0..live.len()).filter(|&i| !is_big[i]).for_each(&mut try_one);
        }
        big.iter().map(|&c| c as usize).filter(|&c| c > b).for_each(&mut try_one);
        (hits, pairs)
    });
    let mut hit = vec![false; live.len()];
    let mut pairs = 0;
    for (hits, n) in found.into_iter().chain(found_big) {
        pairs += n;
        for i in hits {
            hit[i as usize] = true;
        }
    }
    ((0..live.len()).filter(|&i| hit[i]).map(|i| live[i]).collect(), pairs)
}

/// How far inside a face, as a share of it, a crossing has to be to count.
const INSIDE: f64 = 1e-7;

/// Whether two faces on welded points pass through each other. Faces sharing an edge are
/// neighbours; faces sharing a corner can cross only through their edges away from it. Faces
/// lying in one plane are not found.
fn faces_cross(a: [u32; 3], b: [u32; 3], positions: &[Vec3]) -> bool {
    // The corners they share, counted, and the last one (no list made: this runs for every
    // pair of faces near each other).
    let (mut count, mut corner) = (0, (0usize, 0usize));
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            if x == y {
                count += 1;
                corner = (i, j);
            }
        }
    }
    let (pa, pb) = (a.map(|v| positions[v as usize]), b.map(|v| positions[v as usize]));
    let (i, j) = corner;
    match count {
        0 => (0..3).any(|k| segment_crosses(pa[k], pa[(k + 1) % 3], pb)) || (0..3).any(|k| segment_crosses(pb[k], pb[(k + 1) % 3], pa)),
        1 => segment_crosses(pa[(i + 1) % 3], pa[(i + 2) % 3], pb) || segment_crosses(pb[(j + 1) % 3], pb[(j + 2) % 3], pa),
        _ => false,
    }
}

/// Whether the segment from `p` to `q` passes through the inside of the triangle `t` (its ends
/// and the triangle's edges left out; a segment in the triangle's plane is no crossing).
fn segment_crosses(p: Vec3, q: Vec3, t: [Vec3; 3]) -> bool {
    let direction = q - p;
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let h = direction.cross(e2);
    let determinant = e1.dot(h);
    let scale = direction.len() * e1.len() * e2.len();
    if !(determinant.abs() > 1e-12 * scale && scale > 0.0) {
        return false;
    }
    let f = 1.0 / determinant;
    let s = p - t[0];
    let u = f * s.dot(h);
    if !(u > INSIDE && u < 1.0 - INSIDE) {
        return false;
    }
    let r = s.cross(e1);
    let v = f * direction.dot(r);
    if !(v > INSIDE && u + v < 1.0 - INSIDE) {
        return false;
    }
    let along = f * e2.dot(r);
    along > INSIDE && along < 1.0 - INSIDE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100 mm plate of two halves whose shared column of points the file wrote twice, the second
    /// copies a hundred-millionth of a millimetre off (as 32-bit rounding leaves them): an open
    /// crack down the middle until Repair joins the copies, and none after; Survey counts the
    /// crack as edges to repair, not as a hole.
    #[test]
    fn points_a_hair_apart_are_joined_and_their_crack_closed() {
        let n = 10usize;
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for (half, x0) in [(0usize, 0.0f64), (1, 50.0)] {
            let base = positions.len() as u32;
            for y in 0..=n {
                for x in 0..=n {
                    let shift = if half == 1 && x == 0 { 1e-8 } else { 0.0 };
                    positions.push(Vec3::new(x0 + 5.0 * x as f64 + shift, 10.0 * y as f64, 0.0));
                }
            }
            let v = |x: usize, y: usize| base + (y * (n + 1) + x) as u32;
            for y in 0..n {
                for x in 0..n {
                    triangles.push([v(x, y), v(x + 1, y), v(x + 1, y + 1)]);
                    triangles.push([v(x, y), v(x + 1, y + 1), v(x, y + 1)]);
                }
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let before = crate::survey::look(&scan, 0.01).unwrap();
        assert_eq!((before.holes, before.crack_edges), (0, 2 * n), "{before:?}");
        let fixed = repair(&scan, false);
        assert_eq!(fixed.joined, n + 1);
        assert!(fixed.changed());
        let after = crate::holes::survey(&Scan::new(&fixed.positions, &fixed.triangles));
        assert_eq!((after.rings.len(), after.unfillable_edges), (1, 0), "only the plate's own rim: {after:?}");
    }

    /// A strip of two squares with a copy of a triangle, a fin standing up from the middle edge,
    /// and a sliver: the copy and the fin go, the sliver is flipped away.
    #[test]
    fn copies_fins_and_slivers_are_put_right() {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(1.0, 0.5, 1.0), // the fin's top
        ];
        let triangles = vec![[0, 1, 4], [0, 4, 3], [1, 2, 5], [1, 5, 4], [4, 0, 1], [1, 4, 6]];
        let fixed = repair(&Scan::new(&positions, &triangles), false);
        assert_eq!((fixed.duplicates, fixed.non_manifold), (1, 1), "{fixed:?}");
        assert_eq!(fixed.triangles.len(), 4);
        assert!(fixed.positions.iter().all(|p| p.z == 0.0), "the fin's top is gone");

        // A sliver: a point a hair off the middle of a side, its triangle next to flat.
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 1e-7, 0.0), Vec3::new(1.0, -1.0, 0.0)];
        let triangles = vec![[0, 2, 1], [0, 1, 3]];
        let fixed = repair(&Scan::new(&positions, &triangles), false);
        assert_eq!(fixed.slivers, 1, "{fixed:?}");
        let smallest = fixed
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| fixed.positions[i as usize]);
                (b - a).cross(c - a).len() / 2.0
            })
            .fold(f64::INFINITY, f64::min);
        assert!(smallest > 1e-6, "no sliver left: {smallest}");
        let clean = repair(&Scan::new(&positions[..], &[[0, 1, 3]]), true);
        assert!(!clean.changed());
    }

    /// A fine sheet with a fold pushed through it: a strip of the sheet bent back down through
    /// the faces beside it. The crossing faces and the ring round them go and the hole is closed;
    /// nothing crosses after.
    #[test]
    fn faces_passing_through_others_go_and_the_hole_is_closed() {
        let n = 24u32;
        let mut positions: Vec<Vec3> = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (f64::from(i), f64::from(j));
                // A tongue of the sheet between x 10 and 14 dips through: its middle row folded
                // down under its neighbours and back up through them.
                let z = if (10..=14).contains(&i) && (8..=16).contains(&j) { if j % 2 == 0 { -0.8 } else { 0.8 } } else { 0.0 };
                positions.push(Vec3::new(x, y, z));
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
        // A flat patch through the zig-zag, its own faces: the fold's faces pass through it.
        let base = positions.len() as u32;
        positions.extend([Vec3::new(10.5, 8.5, 0.05), Vec3::new(13.5, 8.5, 0.05), Vec3::new(13.5, 15.5, -0.05), Vec3::new(10.5, 15.5, -0.05)]);
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);
        let scan = Scan::new(&positions, &triangles);
        let plain = repair(&scan, false);
        assert_eq!(plain.crossings, 0, "not looked for unless asked");
        let fixed = repair(&scan, true);
        assert!(fixed.crossings > 0 && fixed.crossing_faces > 0, "{:?}", (fixed.crossings, fixed.crossing_faces));
        let live: Vec<usize> = (0..fixed.triangles.len()).collect();
        let weld = crate::welded(&fixed.positions);
        let tris: Vec<[u32; 3]> = fixed.triangles.iter().map(|t| t.map(|v| weld[v as usize])).collect();
        assert_eq!(crossing_faces(&fixed.positions, &tris, &live).1, 0, "nothing crosses after");
        assert!(fixed.refilled >= 1, "the hole the crossing left is closed");

        // The same sheet with a patch the scanner missed right beside the fold: taking the fold
        // off joins its hole to that patch, and the patch, never scanned, stays open (a plate's
        // corner was rounded off by a surface laid over it). Before, it was closed over.
        let missed = |t: &[u32; 3]| {
            let middle = t.iter().fold(Vec3::ZERO, |s, &v| s + positions[v as usize]) / 3.0;
            (15.0..20.0).contains(&middle.x) && (9.0..15.0).contains(&middle.y)
        };
        let open: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| !missed(t)).collect();
        let fixed = repair(&Scan::new(&positions, &open), true);
        assert!(fixed.crossing_faces > 0, "the fold is taken off");
        let laid_over = fixed
            .triangles
            .iter()
            .filter(|t| {
                let middle = t.iter().fold(Vec3::ZERO, |s, &v| s + fixed.positions[v as usize]) / 3.0;
                (16.0..19.0).contains(&middle.x) && (10.0..14.0).contains(&middle.y)
            })
            .count();
        assert_eq!(laid_over, 0, "nothing laid over the patch the scanner missed");
    }

    /// The faces found crossing, among them faces too large for the grid (needles through the
    /// sheet, found through the cells they cover; a wide slanted face, tried against every face;
    /// large faces through each other), are those a test of every pair finds, as many pairs.
    #[test]
    fn large_faces_cross_what_every_pair_tested_finds() {
        let n = 30u32;
        let mut positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(f64::from(i), f64::from(j), 0.0))).collect();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                tris.push([k, k + 1, k + n + 2]);
                tris.push([k, k + n + 2, k + n + 1]);
            }
        }
        let mut add = |ps: [Vec3; 3]| {
            let base = positions.len() as u32;
            positions.extend(ps);
            tris.push([base, base + 1, base + 2]);
        };
        for (x, y) in [(3.3, 4.6), (12.7, 20.2), (25.1, 7.9)] {
            add([Vec3::new(x, y, -60.0), Vec3::new(x + 0.3, y, 60.0), Vec3::new(x, y + 0.3, 60.0)]);
        }
        add([Vec3::new(-2.0, -2.0, -1.0), Vec3::new(32.0, 5.0, 1.0), Vec3::new(5.0, 32.0, 0.5)]);
        add([Vec3::new(3.0, 4.0, 5.0), Vec3::new(3.6, 4.0, 5.0), Vec3::new(3.3, 5.2, 5.0)]);
        add([Vec3::new(12.0, 20.0, -8.0), Vec3::new(14.0, 21.0, -8.0), Vec3::new(13.0, 19.0, -8.0)]);
        let live: Vec<usize> = (0..tris.len()).collect();
        let (found, pairs) = crossing_faces(&positions, &tris, &live);
        let boxes: Vec<(Vec3, Vec3)> =
            tris.iter().map(|t| t.map(|v| positions[v as usize])).map(|[a, b, c]| (a.min(b).min(c), a.max(b).max(c))).collect();
        let overlap = |i: usize, j: usize| {
            let ((a0, a1), (b0, b1)) = (boxes[i], boxes[j]);
            a0.x <= b1.x && a0.y <= b1.y && a0.z <= b1.z && b0.x <= a1.x && b0.y <= a1.y && b0.z <= a1.z
        };
        let (mut hit, mut every) = (vec![false; tris.len()], 0);
        for i in 0..tris.len() {
            for j in i + 1..tris.len() {
                if overlap(i, j) && faces_cross(tris[i], tris[j], &positions) {
                    every += 1;
                    hit[i] = true;
                    hit[j] = true;
                }
            }
        }
        let want: Vec<usize> = (0..tris.len()).filter(|&i| hit[i]).collect();
        let large = (tris.len() - 6..tris.len()).filter(|&i| hit[i]).count();
        assert!(large >= 4 && every > 10, "the large faces cross: {large} of them, {every} pairs");
        assert_eq!((found, pairs), (want.clone(), every));
        // The grid listed in slabs of a few dozen faces (as a scan of millions is): the same.
        for per_slab in [1, 37, 500] {
            assert_eq!(crossing_faces_in(&positions, &tris, &live, per_slab), (want.clone(), every), "slabs of {per_slab}");
        }
    }
}
