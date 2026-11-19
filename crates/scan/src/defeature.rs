//! Remove and Fill: part of a scan taken off (a stamp, a boss, a sprue stub, a sticker) and the
//! hole it leaves closed with a patch that carries the surface round it across. Only the rings the
//! removal opened are closed: holes the scan already had stay open, unless the removal ran into one
//! (then the two are one hole, closed together). A removal that runs into the scan's own edge makes
//! a bite out of the rim, which is no hole: it is left open and counted.

use std::collections::HashSet;

use solidcore_record::Vec3;

use crate::Scan;

/// The scan with the part taken off and its holes closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Defeatured {
    /// The scan's points and triangles as they are now (the patches' after the scan's).
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Triangles taken off.
    pub removed: usize,
    /// Holes the removal opened that were closed, and those that could not be.
    pub filled: usize,
    pub failed: usize,
    /// Triangles the patches added.
    pub added_triangles: usize,
    /// Places where the removal ran into the scan's own edge: left open.
    pub open_to_edge: usize,
}

/// A point's bits, to know it again among the points left.
fn key(p: Vec3) -> [u64; 3] {
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// Takes off the triangles `chosen` marks (one flag a triangle of the scan) and closes the holes
/// that leaves, shaped to the surface when `fair`. `None` when nothing is chosen, or everything is
/// (nothing would be left to close a hole in).
pub fn remove_and_fill(scan: &Scan, chosen: &[bool], fair: bool) -> Option<Defeatured> {
    let goes = |t: usize| chosen.get(t).copied().unwrap_or(false);
    let removed = (0..scan.triangle_count()).filter(|&t| goes(t) && scan.corners(t).is_some()).count();
    if removed == 0 {
        return None;
    }
    // The points round what goes: a ring through any of them was opened by the removal.
    let mut touched: HashSet<[u64; 3]> = HashSet::new();
    for t in (0..scan.triangle_count()).filter(|&t| goes(t)) {
        if let Some(corners) = scan.corners(t) {
            touched.extend(corners.iter().map(|p| key(*p)));
        }
    }
    let (positions, triangles) = crate::pieces::keeping(scan, |t| !goes(t));
    if triangles.is_empty() {
        return None;
    }
    let left = Scan::new(&positions, &triangles);
    let survey = crate::holes::survey(&left);
    let opened = |r: &crate::holes::Ring| r.points.iter().filter_map(|&i| positions.get(i as usize)).any(|p| touched.contains(&key(*p)));
    let rings: Vec<&crate::holes::Ring> = survey.rings.iter().filter(|r| !r.rim && opened(r)).collect();
    let open_to_edge = survey.rings.iter().filter(|r| r.rim && opened(r)).count();
    let unchanged = |positions: Vec<Vec3>, triangles: Vec<[u32; 3]>, failed: usize| Defeatured {
        positions,
        triangles,
        removed,
        filled: 0,
        failed,
        added_triangles: 0,
        open_to_edge,
    };
    if rings.is_empty() {
        return Some(unchanged(positions, triangles, 0));
    }
    let count = rings.len();
    match crate::holes::fill_rings(&left, &rings, fair) {
        Some(done) => Some(Defeatured {
            positions: done.positions,
            triangles: done.triangles,
            removed,
            filled: done.filled,
            failed: done.failed,
            added_triangles: done.added_triangles,
            open_to_edge,
        }),
        None => Some(unchanged(positions, triangles, count)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plate `n` × `n` cells of 1 mm on z = 0, a bump of `height` raised round its middle
    /// (within 3 mm of it), and a hole of its own (the cell at (2, 2) left out).
    fn plate(n: u32, height: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let middle = f64::from(n) / 2.0;
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (f64::from(i), f64::from(j));
                let r = ((x - middle).powi(2) + (y - middle).powi(2)).sqrt();
                let z = if r < 3.0 { height * (1.0 - r / 3.0) } else { 0.0 };
                positions.push(Vec3::new(x, y, z));
            }
        }
        let at = |i: u32, j: u32| j * (n + 1) + i;
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                if (i, j) == (2, 2) {
                    continue;
                }
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (positions, triangles)
    }

    /// The triangles with a corner within `reach` of (x, y) in plan.
    fn round(scan: &Scan, x: f64, y: f64, reach: f64) -> Vec<bool> {
        (0..scan.triangle_count())
            .map(|t| scan.corners(t).is_some_and(|c| c.iter().any(|p| ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt() < reach)))
            .collect()
    }

    /// The bump taken off and its hole closed flat with the plate round it; the plate's own hole
    /// stays open.
    #[test]
    fn a_bump_goes_and_the_plate_is_closed_flat_where_it_was() {
        let (p, t) = plate(20, 2.0);
        let scan = Scan::new(&p, &t);
        let chosen = round(&scan, 10.0, 10.0, 3.5);
        let done = remove_and_fill(&scan, &chosen, true).expect("removed");
        assert_eq!(done.removed, chosen.iter().filter(|c| **c).count());
        assert_eq!((done.filled, done.failed, done.open_to_edge), (1, 0, 0));
        assert!(done.added_triangles > 0);
        // Nothing stands off the plate any more: the patch carries z = 0 across (to rounding).
        let highest = done.positions.iter().map(|q| q.z.abs()).fold(0.0, f64::max);
        assert!(highest < 1e-6, "{highest}");
        // The plate's own hole is still there: one ring that is no rim.
        let after = Scan::new(&done.positions, &done.triangles);
        let rings = crate::holes::survey(&after).rings;
        assert_eq!(rings.iter().filter(|r| !r.rim).count(), 1, "the hole the plate had is left open");
    }

    /// A removal that reaches the plate's edge bites into its rim: nothing to close, said so.
    #[test]
    fn a_bite_out_of_the_edge_is_left_open() {
        let (p, t) = plate(20, 0.0);
        let scan = Scan::new(&p, &t);
        let chosen = round(&scan, 0.0, 10.0, 2.5);
        let done = remove_and_fill(&scan, &chosen, true).expect("removed");
        assert_eq!((done.filled, done.failed, done.open_to_edge), (0, 0, 1));
        assert_eq!(done.triangles.len(), t.len() - done.removed);
    }

    /// Nothing chosen, or everything: nothing to do.
    #[test]
    fn nothing_or_everything_chosen_does_nothing() {
        let (p, t) = plate(6, 0.0);
        let scan = Scan::new(&p, &t);
        assert!(remove_and_fill(&scan, &vec![false; t.len()], true).is_none());
        assert!(remove_and_fill(&scan, &vec![true; t.len()], true).is_none());
        assert!(remove_and_fill(&scan, &[], true).is_none());
    }
}
