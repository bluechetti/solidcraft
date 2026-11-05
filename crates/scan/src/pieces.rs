//! The pieces of a scan: groups of triangles joined through their corners (corners at the same
//! place counted as one), largest first. A scan often carries specks and scraps not joined to the
//! part (dust, a fixture, a reflection); those far smaller than the largest piece float.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, UnionFind, welded};

/// The scan's pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Pieces {
    /// Each triangle's piece (`u32::MAX` for one with a corner that is not a finite point).
    pub of: Vec<u32>,
    /// Each piece's triangles and area (mm²), largest first.
    pub sizes: Vec<usize>,
    pub areas: Vec<f64>,
}

impl Pieces {
    /// Which pieces float: fewer triangles than `share` of the largest piece's, and never fewer
    /// than two, so a single triangle always counts. The largest never floats.
    pub fn floating(&self, share: f64) -> Vec<bool> {
        let largest = self.sizes.first().copied().unwrap_or(0) as f64;
        let below = (share.clamp(0.0, 1.0) * largest).max(2.0);
        self.sizes.iter().enumerate().map(|(i, &n)| i > 0 && (n as f64) < below).collect()
    }
}

/// The scan's pieces, largest first.
pub fn pieces(scan: &Scan) -> Pieces {
    let weld = welded(scan.positions);
    let mut sets = UnionFind::new(scan.positions.len());
    let usable: Vec<bool> = (0..scan.triangle_count()).map(|t| scan.corners(t).is_some()).collect();
    for (t, tri) in scan.triangles.iter().enumerate() {
        if usable[t] {
            let [a, b, c] = tri.map(|i| weld[i as usize] as usize);
            sets.join(a, b);
            sets.join(b, c);
        }
    }
    // Pieces numbered as first met, then renumbered largest first.
    let mut number = vec![u32::MAX; scan.positions.len()];
    let mut of = vec![u32::MAX; scan.triangle_count()];
    let (mut sizes, mut areas): (Vec<usize>, Vec<f64>) = (Vec::new(), Vec::new());
    for (t, tri) in scan.triangles.iter().enumerate() {
        let Some([a, b, c]) = scan.corners(t).filter(|_| usable[t]) else { continue };
        let root = sets.find(weld[tri[0] as usize] as usize);
        if number[root] == u32::MAX {
            number[root] = sizes.len() as u32;
            sizes.push(0);
            areas.push(0.0);
        }
        let k = number[root] as usize;
        of[t] = k as u32;
        sizes[k] += 1;
        areas[k] += (b - a).cross(c - a).len() / 2.0;
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&x, &y| sizes[y].cmp(&sizes[x]).then(x.cmp(&y)));
    let mut rank = vec![0u32; sizes.len()];
    for (r, &k) in order.iter().enumerate() {
        rank[k] = r as u32;
    }
    for p in of.iter_mut().filter(|p| **p != u32::MAX) {
        *p = rank[*p as usize];
    }
    Pieces { of, sizes: order.iter().map(|&k| sizes[k]).collect(), areas: order.iter().map(|&k| areas[k]).collect() }
}

/// The scan with only the triangles `keep` keeps, its unused points dropped.
pub fn keeping(scan: &Scan, keep: impl Fn(usize) -> bool) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut map = vec![u32::MAX; scan.positions.len()];
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for (t, tri) in scan.triangles.iter().enumerate() {
        if !keep(t) || scan.corners(t).is_none() {
            continue;
        }
        triangles.push(tri.map(|i| {
            let slot = &mut map[i as usize];
            if *slot == u32::MAX {
                *slot = positions.len() as u32;
                positions.push(scan.positions[i as usize]);
            }
            *slot
        }));
    }
    (positions, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A block with a scrap and a speck beside it: three pieces, largest first; the smaller float as
    /// the share rises, and keeping the rest leaves the block as it was.
    #[test]
    fn specks_beside_a_block_float() {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 6);
        let block = triangles.len();
        for (lo, n) in [(Vec3::new(50.0, 0.0, 0.0), 1u32), (Vec3::new(60.0, 0.0, 0.0), 2)] {
            let (p, t) = fine_cuboid(lo, lo + Vec3::new(1.0, 1.0, 1.0), n);
            let base = positions.len() as u32;
            positions.extend(p);
            triangles.extend(t.iter().map(|tri| tri.map(|i| i + base)));
        }
        let scan = Scan::new(&positions, &triangles);
        let found = pieces(&scan);
        assert_eq!(found.sizes, vec![block, 48, 12]);
        assert!((found.areas[0] - 2.0 * (600.0 + 300.0 + 200.0)).abs() < 1e-6);
        assert_eq!(found.floating(0.02), vec![false, false, false], "2 % of 216 is 4.3 triangles: none so small");
        assert_eq!(found.floating(0.1), vec![false, false, true], "at 10 % the speck of 12, not the scrap of 48");
        assert_eq!(found.floating(0.5), vec![false, true, true]);
        let floating = found.floating(0.5);
        let (kept_points, kept) = keeping(&scan, |t| !floating[found.of[t] as usize]);
        assert_eq!(kept.len(), block);
        assert!(kept_points.iter().all(|p| p.x <= 30.0));
    }
}
