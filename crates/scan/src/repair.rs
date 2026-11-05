//! Repairing a scan's triangles, after SurfaceCraft's repair: copies of a triangle taken off, an
//! edge three or more faces meet along left with the two that continue each other best, and
//! slivers (three corners, next to no area) put right by joining their neighbours the other way
//! across. (Equal points welded, triangles gone to a line or a point, and the winding of each piece
//! are put right when the scan is built, so they need no step here.)

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
}

impl Repaired {
    /// Whether anything was done.
    pub fn changed(&self) -> bool {
        self.duplicates + self.non_manifold + self.slivers > 0
    }
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// A face is a sliver when its area is below this share of the square of its longest side.
const SLIVER: f64 = 1e-6;

/// Repairs the scan.
pub fn repair(scan: &Scan) -> Repaired {
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

    // The repaired scan: its points as they were, those no longer used dropped.
    let kept: Vec<[u32; 3]> = tris.iter().zip(&alive).filter(|(_, a)| **a).map(|(t, _)| *t).collect();
    let (positions, triangles) = crate::pieces::keeping(&Scan::new(scan.positions, &kept), |_| true);
    Repaired { positions, triangles, duplicates, non_manifold, slivers }
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
        let fixed = repair(&Scan::new(&positions, &triangles));
        assert_eq!((fixed.duplicates, fixed.non_manifold), (1, 1), "{fixed:?}");
        assert_eq!(fixed.triangles.len(), 4);
        assert!(fixed.positions.iter().all(|p| p.z == 0.0), "the fin's top is gone");

        // A sliver: a point a hair off the middle of a side, its triangle next to flat.
        let positions = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(1.0, 1e-7, 0.0), Vec3::new(1.0, -1.0, 0.0)];
        let triangles = vec![[0, 2, 1], [0, 1, 3]];
        let fixed = repair(&Scan::new(&positions, &triangles));
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
        let clean = repair(&Scan::new(&positions[..], &[[0, 1, 3]]));
        assert!(!clean.changed());
    }
}
