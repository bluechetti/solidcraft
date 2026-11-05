//! Repairing a scan's triangles, after SurfaceCraft's repair: copies of a triangle taken off, an
//! edge three or more faces meet along left with the two that continue each other best, and
//! slivers (three corners, next to no area) put right by joining their neighbours the other way
//! across. With `crossings`, faces that pass through others they share no edge with are taken off
//! with the ring of faces round them (which the fold has bent), and each hole so made is closed
//! with a smooth patch. (Equal points welded, triangles gone to a line or a point, and the winding
//! of each piece are put right when the scan is built, so they need no step here.)

use std::collections::{HashMap, HashSet};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, welded};

/// A repaired scan and what was done.
#[derive(Clone, Debug, PartialEq)]
pub struct Repaired {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
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
        self.duplicates + self.non_manifold + self.slivers + self.crossing_faces > 0
    }
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// A face is a sliver when its area is below this share of the square of its longest side.
const SLIVER: f64 = 1e-6;

/// Repairs the scan; with `crossings`, its faces that pass through others too.
pub fn repair(scan: &Scan, crossings: bool) -> Repaired {
    let weld = welded(scan.positions);
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

    // Edges three or more faces meet along: keep the pair that runs the edge opposite ways (the
    // winding agrees) and bends least between them, else the pair that bends least.
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(tris.len() * 3);
    for (t, tri) in tris.iter().enumerate().filter(|(t, _)| alive[*t]) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            edges.push((pair(a, b), t as u32));
        }
    }
    edges.sort_unstable();
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
    // and no edge twice.
    let mut owners: HashMap<u64, Vec<usize>> = HashMap::new();
    for (t, tri) in tris.iter().enumerate().filter(|(t, _)| alive[*t]) {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            owners.entry(pair(a, b)).or_default().push(t);
        }
    }
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
        let Some(across) = owners.get(&pair(a, b)).filter(|o| o.len() == 2).and_then(|o| o.iter().copied().find(|&u| u != t)) else { continue };
        if !alive[across] || touched[across] {
            continue;
        }
        // The far point of the face across, which runs the side b to a.
        let other = tris[across];
        let Some(&d) = other.iter().find(|&&v| v != a && v != b) else { continue };
        if c == d || owners.contains_key(&pair(c, d)) || !runs(&other, b, a) {
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
        owners.insert(pair(c, d), vec![t, across]);
        slivers += 1;
    }

    // Faces that pass through others: both of each pair go, and the ring round them.
    let (mut pairs, mut gone) = (0, Vec::new());
    if crossings {
        let live: Vec<usize> = (0..tris.len()).filter(|&t| alive[t]).collect();
        let found = crossing_faces(scan.positions, &tris, &live);
        pairs = found.1;
        if !found.0.is_empty() {
            let mut by_edge: HashMap<u64, Vec<usize>> = HashMap::new();
            for &t in &live {
                let tri = tris[t];
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    by_edge.entry(pair(a, b)).or_default().push(t);
                }
            }
            let mut take: HashSet<usize> = found.0.iter().copied().collect();
            for &t in &found.0 {
                let tri = tris[t];
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    take.extend(by_edge.get(&pair(a, b)).into_iter().flatten().copied());
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
    // The holes the crossings left, closed: the rings through a corner of a face taken off.
    let mut refilled = 0;
    if !gone.is_empty() {
        let bits = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        let corners: HashSet<[u64; 3]> = gone.iter().flat_map(|&t| tris[t]).map(|v| bits(at(v))).collect();
        let repaired = Scan::new(&positions, &triangles);
        let mut found = crate::holes::survey(&repaired);
        found.rings.retain(|r| r.points.iter().any(|&v| positions.get(v as usize).is_some_and(|p| corners.contains(&bits(*p)))));
        for ring in &mut found.rings {
            ring.rim = false;
        }
        if let Some(filled) = crate::holes::fill(&repaired, &found, usize::MAX, true) {
            refilled = filled.filled;
            positions = filled.positions;
            triangles = filled.triangles;
        }
    }
    Repaired { positions, triangles, duplicates, non_manifold, slivers, crossings: pairs, crossing_faces: gone.len(), refilled }
}

/// The faces among `live` (on welded points) that pass through another face they share no edge
/// with, and how many pairs cross. A grid of cells about a face across finds the pairs to test.
fn crossing_faces(positions: &[Vec3], tris: &[[u32; 3]], live: &[usize]) -> (Vec<usize>, usize) {
    if live.len() < 2 {
        return (Vec::new(), 0);
    }
    let at = |v: u32| positions[v as usize];
    let boxes: Vec<(Vec3, Vec3)> = live
        .iter()
        .map(|&t| {
            let [a, b, c] = tris[t].map(at);
            (a.min(b).min(c), a.max(b).max(c))
        })
        .collect();
    let (low, high) = boxes.iter().fold((boxes[0].0, boxes[0].1), |(lo, hi), (a, b)| (lo.min(*a), hi.max(*b)));
    let mean_side = boxes.iter().map(|(a, b)| (*b - *a).x.max((*b - *a).y).max((*b - *a).z)).sum::<f64>() / boxes.len() as f64;
    let extent = (high - low).x.max((high - low).y).max((high - low).z);
    if !(extent > 0.0 && extent.is_finite()) {
        return (Vec::new(), 0);
    }
    // About a face to a cell, and never so fine that a coordinate runs past its 21 bits.
    let cell = (mean_side * 1.2).max(extent / 1_000_000.0);
    let cell_of = |p: Vec3| {
        let g = (p - low) / cell;
        [g.x.max(0.0) as u64, g.y.max(0.0) as u64, g.z.max(0.0) as u64]
    };
    let pack = |c: [u64; 3]| (c[0] << 42) | (c[1] << 21) | c[2];
    let mut entries: Vec<(u64, u32)> = Vec::with_capacity(live.len() * 4);
    let mut big: Vec<u32> = Vec::new();
    for (i, (lo, hi)) in boxes.iter().enumerate() {
        let (a, b) = (cell_of(*lo), cell_of(*hi));
        if (b[0] - a[0] + 1) * (b[1] - a[1] + 1) * (b[2] - a[2] + 1) > 64 {
            big.push(i as u32);
            continue;
        }
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    entries.push((pack([x, y, z]), i as u32));
                }
            }
        }
    }
    entries.sort_unstable();
    let overlap = |i: usize, j: usize| {
        let ((a0, a1), (b0, b1)) = (boxes[i], boxes[j]);
        a0.x <= b1.x && a0.y <= b1.y && a0.z <= b1.z && b0.x <= a1.x && b0.y <= a1.y && b0.z <= a1.z
    };
    let mut hit = vec![false; live.len()];
    let mut pairs = 0;
    let mut test = |i: usize, j: usize, hit: &mut Vec<bool>| {
        if overlap(i, j) && faces_cross(tris[live[i]], tris[live[j]], positions) {
            pairs += 1;
            hit[i] = true;
            hit[j] = true;
        }
    };
    let mut start = 0;
    while start < entries.len() {
        let end = start + entries[start..].iter().take_while(|e| e.0 == entries[start].0).count();
        for x in start..end {
            for y in x + 1..end {
                let (i, j) = (entries[x].1 as usize, entries[y].1 as usize);
                // A pair sharing several cells is tested in one: where both boxes begin.
                if pack(cell_of(boxes[i].0.max(boxes[j].0))) == entries[start].0 {
                    test(i, j, &mut hit);
                }
            }
        }
        start = end;
    }
    // Faces too large for the grid are tried against every other.
    for &b in &big {
        for i in 0..live.len() {
            if i != b as usize && (!big.contains(&(i as u32)) || i > b as usize) {
                test(b as usize, i, &mut hit);
            }
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
    let shared: Vec<(usize, usize)> = (0..3).flat_map(|i| (0..3).map(move |j| (i, j))).filter(|&(i, j)| a[i] == b[j]).collect();
    let (pa, pb) = (a.map(|v| positions[v as usize]), b.map(|v| positions[v as usize]));
    match shared.as_slice() {
        [] => (0..3).any(|k| segment_crosses(pa[k], pa[(k + 1) % 3], pb)) || (0..3).any(|k| segment_crosses(pb[k], pb[(k + 1) % 3], pa)),
        [(i, j)] => segment_crosses(pa[(i + 1) % 3], pa[(i + 2) % 3], pb) || segment_crosses(pb[(j + 1) % 3], pb[(j + 2) % 3], pa),
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
        let weld = welded(&fixed.positions);
        let tris: Vec<[u32; 3]> = fixed.triangles.iter().map(|t| t.map(|v| weld[v as usize])).collect();
        assert_eq!(crossing_faces(&fixed.positions, &tris, &live).1, 0, "nothing crosses after");
        assert!(fixed.refilled >= 1, "the hole the crossing left is closed");
    }
}
