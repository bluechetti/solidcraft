//! A scan's outline seen along a plane's normal: the boundary of everything its triangles cover
//! on the plane. A section at one height misses whatever is wider elsewhere (a hammer's claw), and
//! the mesh's own contour (the edges between triangles facing toward and away) breaks into
//! hundreds of pieces and loops on a bumpy scan. The cover has neither fault: every triangle is
//! laid on a grid of cells on the plane, and the outline runs round the cells covered, so no part
//! of the scan lies outside it. Holes right through the part are outlines of their own.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::section::Run;
use crate::wall_outline::{boundaries, filled};

/// The most cells the grid has: the cell asked for grows for a scan too large for it.
const MOST_CELLS: f64 = 16.0e6;
/// A hole through the part smaller than this many cells is a gap in the scan, not a hole.
const LEAST_HOLE: usize = 24;

/// The outlines of what `scan` covers of `plane`, seen along its normal, as closed runs in the
/// plane's coordinates: the outer boundary of each piece and each hole through it of at least
/// [`LEAST_HOLE`] cells, on a grid of `cell` (grown when the scan would need more than
/// [`MOST_CELLS`]). Each run goes along cell sides, so it lies up to a cell outside what the scan
/// covers and never inside it. The cell used comes back with them; nothing when the scan is empty.
pub fn outline(scan: &Scan, plane: &Plane, cell: f64) -> (Vec<Run>, f64) {
    let flat = |p: Vec3| {
        let d = p - plane.origin;
        Vec2::new(d.dot(plane.x), d.dot(plane.y))
    };
    let mut low = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut high = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for t in 0..scan.triangle_count() {
        let Some(corners) = scan.corners(t) else { continue };
        for q in corners.map(flat) {
            if q.x.is_finite() && q.y.is_finite() {
                low = Vec2::new(low.x.min(q.x), low.y.min(q.y));
                high = Vec2::new(high.x.max(q.x), high.y.max(q.y));
            }
        }
    }
    let size = high - low;
    if !(size.x >= 0.0 && size.y >= 0.0 && cell > 0.0 && cell.is_finite()) {
        return (Vec::new(), cell);
    }
    let cell = cell.max((size.x * size.y / MOST_CELLS).sqrt()).max(1e-9);
    // A cell of margin all round, so the outside is one piece.
    let (w, h) = ((size.x / cell).ceil() as usize + 3, (size.y / cell).ceil() as usize + 3);
    let corner = low - Vec2::new(cell, cell);
    let middle = |i: usize, j: usize| corner + Vec2::new((i as f64 + 0.5) * cell, (j as f64 + 0.5) * cell);
    let index = |v: f64, most: usize| ((v / cell).floor().max(0.0) as usize).min(most - 1);
    let mut covered = vec![false; w * h];
    for t in 0..scan.triangle_count() {
        let Some(corners) = scan.corners(t) else { continue };
        let [a, b, c] = corners.map(flat);
        if ![a, b, c].iter().all(|q| q.x.is_finite() && q.y.is_finite()) {
            continue;
        }
        let (lo, hi) =
            (Vec2::new(a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y)) - corner, Vec2::new(a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y)) - corner);
        let (i0, i1, j0, j1) = (index(lo.x, w), index(hi.x, w), index(lo.y, h), index(hi.y, h));
        for j in j0..=j1 {
            for i in i0..=i1 {
                if covered[j * w + i] {
                    continue;
                }
                // A cell whose middle the triangle covers, or that one of its sides passes through
                // (a triangle seen edge-on is a line, and a small one misses every middle).
                let p = middle(i, j);
                if inside(p, a, b, c) || [(a, b), (b, c), (c, a)].iter().any(|&(u, v)| to_segment(p, u, v) <= 0.5 * cell) {
                    covered[j * w + i] = true;
                }
            }
        }
    }
    // Gaps too small to be holes through the part are covered over.
    let solid = filled(&covered, w, h);
    let mut seen = covered.clone();
    for start in 0..w * h {
        if seen[start] || !solid[start] {
            continue;
        }
        let mut gap = vec![start];
        seen[start] = true;
        let mut k = 0;
        while k < gap.len() {
            let c = gap[k];
            k += 1;
            let (i, j) = (c % w, c / w);
            let near = [(i > 0).then(|| c - 1), (i + 1 < w).then(|| c + 1), (j > 0).then(|| c - w), (j + 1 < h).then(|| c + w)];
            for d in near.into_iter().flatten() {
                if !seen[d] && solid[d] {
                    seen[d] = true;
                    gap.push(d);
                }
            }
        }
        if gap.len() < LEAST_HOLE {
            for c in gap {
                covered[c] = true;
            }
        }
    }
    let mut runs: Vec<Run> = boundaries(&covered, w, h)
        .into_iter()
        .map(|ring| {
            let points: Vec<Vec2> = ring.into_iter().map(|(i, j)| corner + Vec2::new(i as f64 * cell, j as f64 * cell)).collect();
            Run { triangles: vec![0; points.len()], points, closed: true }
        })
        .collect();
    // The largest first: the part's own outline before its holes and any scraps beside it.
    let area = |r: &Run| (0..r.points.len()).map(|i| r.points[i].cross(r.points[(i + 1) % r.points.len()])).sum::<f64>().abs();
    runs.sort_by(|a, b| area(b).total_cmp(&area(a)));
    (runs, cell)
}

/// How far the farthest of `points` (in the plane's coordinates) lies outside the closed `ring`:
/// 0 when every one is inside it or on it.
pub fn farthest_outside(ring: &[Vec2], points: &[Vec2]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    points
        .iter()
        .filter(|p| !crate::profile::contains(ring, **p))
        .map(|p| (0..n).map(|i| to_segment(*p, ring[i], ring[(i + 1) % n])).fold(f64::INFINITY, f64::min))
        .fold(0.0, f64::max)
}

/// Whether `p` lies inside the triangle `a`, `b`, `c` (either way round).
fn inside(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (d1, d2, d3) = ((b - a).cross(p - a), (c - b).cross(p - b), (a - c).cross(p - c));
    (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
}

fn to_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A block 60 × 20 × 10 with a boss 20 × 30 standing on its middle, wider than the block: seen
    /// from above its outline is a plus, the boss's width where a cut through the block alone
    /// would be straight. One outline, no point of the scan outside it, and within a cell of the
    /// true shape. A plate with a hole through it gives two: the plate and the hole.
    #[test]
    fn a_scans_outline_from_above_is_all_it_covers_and_holds_every_point() {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::new(-30.0, -10.0, 0.0), Vec3::new(30.0, 10.0, 10.0), 12);
        let (boss, boss_triangles) = fine_cuboid(Vec3::new(-10.0, -15.0, 10.0), Vec3::new(10.0, 15.0, 16.0), 8);
        let shift = positions.len() as u32;
        positions.extend(boss);
        triangles.extend(boss_triangles.into_iter().map(|t| t.map(|i| i + shift)));
        let scan = Scan::new(&positions, &triangles);
        let plane = Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y };
        let (runs, cell) = outline(&scan, &plane, 0.25);
        assert_eq!((runs.len(), cell), (1, 0.25), "one outline at the cell asked for");
        let ring = &runs[0].points;
        let flat: Vec<Vec2> = positions.iter().map(|p| Vec2::new(p.x, p.y)).collect();
        assert_eq!(farthest_outside(ring, &flat), 0.0, "every point of the scan is inside its outline");
        let twice: f64 = (0..ring.len()).map(|i| ring[i].cross(ring[(i + 1) % ring.len()])).sum();
        let plus = 60.0 * 20.0 + 2.0 * 20.0 * 5.0;
        assert!(twice > 0.0 && (twice / 2.0 - plus).abs() < 0.05 * plus, "{} against {plus}", twice / 2.0);
        assert!(ring.iter().any(|p| p.y > 14.9) && ring.iter().any(|p| p.y < -14.9), "the boss's width, which a cut through the block misses");
        assert!(ring.iter().all(|p| p.x.abs() <= 30.0 + 2.0 * cell && p.y.abs() <= 15.0 + 2.0 * cell), "within a cell or two of the part");

        // A plate 40 × 40 with a 10 × 10 hole through its middle: four slabs round it.
        let mut plate: (Vec<Vec3>, Vec<[u32; 3]>) = (Vec::new(), Vec::new());
        for (lo, hi) in [((-20.0, -20.0), (20.0, -5.0)), ((-20.0, 5.0), (20.0, 20.0)), ((-20.0, -5.0), (-5.0, 5.0)), ((5.0, -5.0), (20.0, 5.0))] {
            let (p, t) = fine_cuboid(Vec3::new(lo.0, lo.1, 0.0), Vec3::new(hi.0, hi.1, 4.0), 6);
            let shift = plate.0.len() as u32;
            plate.0.extend(p);
            plate.1.extend(t.into_iter().map(|t| t.map(|i| i + shift)));
        }
        let (runs, _) = outline(&Scan::new(&plate.0, &plate.1), &plane, 0.25);
        assert_eq!(runs.len(), 2, "the plate and the hole through it");
        let area = |r: &Run| (0..r.points.len()).map(|i| r.points[i].cross(r.points[(i + 1) % r.points.len()])).sum::<f64>() / 2.0;
        assert!((area(&runs[0]) - 1600.0).abs() < 80.0 && (area(&runs[1]) + 100.0).abs() < 15.0, "{} {}", area(&runs[0]), area(&runs[1]));
    }
}
