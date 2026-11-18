//! A scan completed by its mirror image: every vertex is reflected in the mirror plane, and a
//! reflected triangle is kept where the scan has nothing (at least two of its corners and its middle
//! farther than the reach from every triangle of the scan). Triangles beside those that are only
//! near because the scan ends there (the nearest point of the scan is on one of its open edges) are
//! kept too, up to twelve steps out, so the piece runs up to the scan's edge instead of stopping a
//! reach short of it. Pieces of fewer than four triangles are dropped: there the two sides differ,
//! nothing is missing.
//!
//! Reflecting turns triangles inside out, so each is wound the other way round. A corner within
//! three-quarters of an edge of a vertex of the scan is put on that vertex, so the piece meets the
//! scan rather than merely coming close. The piece is its own mesh: a guess that the two sides are
//! alike, kept apart from what was scanned.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::Mesh;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::symmetry::Mirror;
use crate::{Scan, UnionFind, welded};

/// What completing a scan by its mirror image made: the added piece as a mesh of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorFill {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Separate pieces added, and their area in mm².
    pub pieces: usize,
    pub area: f64,
    /// Corners of the piece put on a vertex of the scan.
    pub joined: usize,
    /// How far from the scan a reflected triangle had to lie to count as missing from it.
    pub reach: f64,
}

/// One and a half typical edges, or three times the mismatch the plane was found with, whichever
/// is more.
pub fn automatic_reach(scan: &Scan, mirror: &Mirror) -> f64 {
    (1.5 * typical_edge(scan)).max(3.0 * mirror.rms).max(1e-6)
}

/// The side of the equilateral triangle of the scan's mean triangle area.
pub fn typical_edge(scan: &Scan) -> f64 {
    let (mut area, mut usable) = (0.0, 0usize);
    for t in 0..scan.triangle_count() {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let twice = (b - a).cross(c - a).len();
        if twice > 0.0 && twice.is_finite() {
            area += twice / 2.0;
            usable += 1;
        }
    }
    if usable > 0 { (4.0 * area / usable as f64 / 3.0f64.sqrt()).sqrt() } else { 0.0 }
}

/// The nearest point of the scan to a place: the triangle and the point.
struct Hit {
    triangle: usize,
    point: Vec3,
}

/// The reflected triangles that land where the scan has nothing, `None` when nothing is missing
/// (or the plane has no direction). `reach` defaults to [`automatic_reach`].
/// For a scan cut short across its mirror plane (one end never scanned), the plane to complete
/// it in: square to the plane of its open edge (the cut), at the place along that direction where
/// the largest share of the scan, reflected, lands back on the scan; each place judged only on
/// the reflections that land within the scan's own reach along the direction, since those past
/// it fall where the scan was never made. (Judged on the whole scan, every place matched about
/// half: T-LESS object 5 cut short at X = 15 scored 0.5 from X = -20 to 0; on the reach alone its
/// plane at X = 0 scores 0.87, the middle of what was scanned 0.53.) `None` for a scan with no open edge lying near a plane,
/// or no place that matches.
pub fn plane_across_cut(scan: &Scan) -> Option<Mirror> {
    let survey = crate::holes::survey(scan);
    // The longest open ring: the cut.
    let ring = survey.rings.iter().max_by(|a, b| a.perimeter.total_cmp(&b.perimeter))?;
    let edge: Vec<Vec3> = ring.points.iter().filter_map(|&i| scan.positions.get(i as usize).copied()).collect();
    if edge.len() < 8 {
        return None;
    }
    let mid = edge.iter().fold(Vec3::ZERO, |s, q| s + *q) / edge.len() as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for q in &edge {
        let d = *q - mid;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    let normal = axes[0].normalized()?;
    // An edge that does not lie near a plane is no cut.
    if !(values[0] < 0.05 * values[1]) {
        return None;
    }
    let points: Vec<Vec3> = crate::thinned(scan.positions, 4000);
    let along: Vec<f64> = points.iter().map(|q| q.dot(normal)).collect();
    let (lo, hi) = along.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    if !(hi > lo) {
        return None;
    }
    let tree = TriangleTree::new(&Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Mesh::default() });
    let near = 2.0 * typical_edge(scan).max(1e-6);
    // A place's share of the reflections in reach that land on the scan, and how far they land
    // from it on average.
    let score = |c: f64| -> (f64, f64) {
        let (mut landed, mut within, mut off) = (0usize, 0usize, 0.0);
        for (q, a) in points.iter().zip(&along) {
            let r = 2.0 * c - a;
            if r < lo || r > hi {
                continue;
            }
            within += 1;
            let reflected = *q + normal * (r - a);
            if let Some((d, _)) = tree.nearest(reflected).filter(|(d, _)| *d <= near) {
                landed += 1;
                off += d;
            }
        }
        // From a fifth of the scan at least: near either end only a sliver of it is in reach, and
        // any sliver matches.
        if within * 5 < points.len() || landed == 0 {
            return (0.0, f64::INFINITY);
        }
        (landed as f64 / within as f64, off / landed as f64)
    };
    // Whole steps across the scan, then finer round the best; of the places scoring near the best
    // share (it has a flat top), the one whose reflections land closest.
    let steps = 48;
    let step = (hi - lo) / steps as f64;
    let coarse = (1..steps).map(|k| lo + step * k as f64).map(|c| (c, score(c))).max_by(|a, b| a.1.0.total_cmp(&b.1.0))?;
    let fine: Vec<(f64, (f64, f64))> = (-20..=20).map(|k| coarse.0 + step * k as f64 / 10.0).map(|c| (c, score(c))).collect();
    let top = fine.iter().map(|f| f.1.0).fold(coarse.1.0, f64::max);
    if !(top > 0.0) {
        return None;
    }
    let best = fine.iter().filter(|f| f.1.0 >= 0.97 * top).min_by(|a, b| a.1.1.total_cmp(&b.1.1)).map_or(coarse.0, |f| f.0);
    let best = (top, best);
    let on = normal * best.1;
    crate::symmetry::measure(scan, on + (mid - normal * mid.dot(normal)), normal)
}

pub fn complete(scan: &Scan, mirror: &Mirror, reach: Option<f64>) -> Option<MirrorFill> {
    let normal = mirror.normal.normalized()?;
    let plane = Mirror { normal, ..*mirror };
    let reach = reach.filter(|r| r.is_finite() && *r > 0.0).unwrap_or_else(|| automatic_reach(scan, mirror));
    let reflected: Vec<Vec3> = scan.positions.iter().map(|p| plane.reflect(*p)).collect();
    // Only triangles with finite corners take part, as candidates and as the scan.
    let usable: Vec<usize> = (0..scan.triangle_count()).filter(|&t| scan.corners(t).is_some()).collect();
    if usable.is_empty() {
        return None;
    }
    let tree = TriangleTree::new(&Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Mesh::default() });
    let nearest = |p: Vec3| -> Option<Hit> {
        let (d, t) = tree.nearest(p)?;
        if !(d <= reach) {
            return None;
        }
        let [a, b, c] = scan.corners(t as usize)?;
        Some(Hit { triangle: t as usize, point: closest_on_triangle(p, a, b, c) })
    };
    let edge = typical_edge(scan);
    let join_reach = (0.75 * edge).min(reach);
    let corner = |t: usize, k: usize| scan.triangles.get(t).map_or(0, |tri| tri[k] as usize);

    // For each vertex, once: whether its reflection is far from the scan, and the scan's vertex to
    // put it on when it is close.
    let n = reflected.len();
    let mut far = vec![0u8; n]; // 0 not asked, 1 near, 2 far
    let mut join: Vec<Option<u32>> = vec![None; n];
    let mut is_far = |v: usize| -> bool {
        if far.get(v) == Some(&0) {
            match reflected.get(v).and_then(|p| nearest(*p).map(|h| (h, *p))) {
                Some((hit, p)) => {
                    far[v] = 1;
                    if hit.point.dist(p) <= join_reach {
                        let best = scan
                            .triangles
                            .get(hit.triangle)
                            .into_iter()
                            .flatten()
                            .filter_map(|&id| scan.positions.get(id as usize).map(|q| (id, q.dist(p))))
                            .min_by(|a, b| a.1.total_cmp(&b.1));
                        join[v] = best.filter(|(_, d)| *d <= join_reach).map(|(id, _)| id);
                    }
                }
                None => far[v] = 2,
            }
        }
        far.get(v) == Some(&2)
    };

    // The scan's edges with one face (vertices at the same place counted as one).
    let weld = welded(scan.positions);
    let key = |a: u32, b: u32| (u64::from(a.min(b)) << 32) | u64::from(a.max(b));
    let mut uses: HashMap<u64, u32> = HashMap::new();
    for &t in &usable {
        for k in 0..3 {
            let (a, b) = (weld[corner(t, k)], weld[corner(t, (k + 1) % 3)]);
            if a != b {
                *uses.entry(key(a, b)).or_insert(0) += 1;
            }
        }
    }
    let is_open = |a: usize, b: usize| uses.get(&key(weld[a], weld[b])) == Some(&1);
    // Whether the nearest point of the scan to `p` is on one of its open edges: `p` is off the end
    // of the scan, not over it.
    let tiny = 1e-4 * edge.max(1e-9) + 1e-9;
    let off_the_edge = |p: Vec3| -> bool {
        let Some(hit) = nearest(p) else { return false };
        (0..3).any(|k| {
            let (i, j) = (corner(hit.triangle, k), corner(hit.triangle, (k + 1) % 3));
            let (Some(&a), Some(&b)) = (scan.positions.get(i), scan.positions.get(j)) else { return false };
            if !is_open(i, j) {
                return false;
            }
            let ab = b - a;
            let length2 = ab.len2();
            let along = if length2 > 0.0 { ((hit.point - a).dot(ab) / length2).clamp(0.0, 1.0) } else { 0.0 };
            hit.point.dist(a + ab * along) <= tiny
        })
    };
    let middle = |t: usize| (reflected[corner(t, 0)] + reflected[corner(t, 1)] + reflected[corner(t, 2)]) / 3.0;

    // The reflected triangles that land where the scan has nothing.
    let mut is_kept = vec![false; scan.triangle_count()];
    let mut kept: Vec<usize> = Vec::new();
    for &t in &usable {
        let corners = (0..3).filter(|&k| is_far(corner(t, k))).count();
        if corners >= 2 && nearest(middle(t)).is_none() {
            is_kept[t] = true;
            kept.push(t);
        }
    }
    if kept.is_empty() {
        return None;
    }
    // Then those beside them that are only near because the scan ends there.
    let mut users: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &t in &usable {
        for k in 0..3 {
            users[corner(t, k)].push(t as u32);
        }
    }
    let mut frontier = kept.clone();
    for _ in 0..12 {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for &t in &frontier {
            for k in 0..3 {
                for &u in &users[corner(t, k)] {
                    let u = u as usize;
                    if !is_kept[u] && off_the_edge(middle(u)) {
                        is_kept[u] = true;
                        kept.push(u);
                        next.push(u);
                    }
                }
            }
        }
        frontier = next;
    }

    // Pieces: triangles that share a vertex; those of fewer than four are dropped.
    let mut sets = UnionFind::new(n);
    for &t in &kept {
        sets.join(corner(t, 0), corner(t, 1));
        sets.join(corner(t, 1), corner(t, 2));
    }
    let mut size: HashMap<usize, usize> = HashMap::new();
    for &t in &kept {
        *size.entry(sets.find(corner(t, 0))).or_insert(0) += 1;
    }
    let survivors: Vec<usize> = kept.into_iter().filter(|&t| size.get(&sets.find(corner(t, 0))).copied().unwrap_or(0) >= 4).collect();

    // The piece's own mesh: a joined corner at the scan's vertex, the rest where they were reflected.
    let mut positions: Vec<Vec3> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut map: HashMap<usize, u32> = HashMap::new();
    let mut at_scan: HashMap<u32, u32> = HashMap::new();
    let mut joined = 0usize;
    let mut area = 0.0;
    let mut pieces: Vec<usize> = Vec::new();
    for &t in &survivors {
        let mut ids = [0u32; 3];
        let mut owners = [None; 3];
        for (k, slot) in ids.iter_mut().enumerate() {
            let v = corner(t, k);
            *slot = match (map.get(&v), join[v]) {
                (Some(&i), _) => i,
                (None, Some(on)) => {
                    joined += 1;
                    let i = *at_scan.entry(on).or_insert_with(|| {
                        positions.push(scan.positions[on as usize]);
                        (positions.len() - 1) as u32
                    });
                    map.insert(v, i);
                    i
                }
                (None, None) => {
                    positions.push(reflected[v]);
                    let i = (positions.len() - 1) as u32;
                    map.insert(v, i);
                    i
                }
            };
            owners[k] = join[v];
        }
        let [a, b, c] = ids;
        // Two corners put on the same vertex of the scan: the triangle has gone to nothing.
        if a == b || b == c || c == a || (owners[0].is_some() && (owners[0] == owners[1] || owners[0] == owners[2])) {
            continue;
        }
        let (pa, pb, pc) = (positions[a as usize], positions[b as usize], positions[c as usize]);
        area += (pb - pa).cross(pc - pa).len() / 2.0;
        triangles.push([a, c, b]);
        pieces.push(sets.find(corner(t, 0)));
    }
    if triangles.is_empty() {
        return None;
    }
    pieces.sort_unstable();
    pieces.dedup();
    Some(MirrorFill { positions, triangles, pieces: pieces.len(), area, joined, reach })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere of radius `r` as a UV grid of `rings` × `segments`, poles shared.
    fn sphere(r: f64, rings: u32, segments: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = vec![Vec3::new(0.0, 0.0, r), Vec3::new(0.0, 0.0, -r)];
        for i in 1..rings {
            let polar = std::f64::consts::PI * f64::from(i) / f64::from(rings);
            for j in 0..segments {
                let around = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
                positions.push(Vec3::new(r * polar.sin() * around.cos(), r * polar.sin() * around.sin(), r * polar.cos()));
            }
        }
        let at = |i: u32, j: u32| 2 + (i - 1) * segments + j % segments;
        let mut triangles = Vec::new();
        for j in 0..segments {
            triangles.push([0, at(1, j), at(1, j + 1)]);
            triangles.push([1, at(rings - 1, j + 1), at(rings - 1, j)]);
            for i in 1..rings - 1 {
                triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (positions, triangles)
    }

    fn plane_x() -> Mirror {
        Mirror { point: Vec3::ZERO, normal: Vec3::X, rms: 0.0, coverage: 1.0, size: 40.0, spacing: 1.0 }
    }

    /// Half a sphere gets its other half back, in one piece of about the half's area, joined to the
    /// scan along the cut; a whole sphere is missing nothing.
    #[test]
    fn half_a_sphere_is_completed_by_its_mirror_image() {
        let (positions, triangles) = sphere(20.0, 24, 48);
        let half: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| t.iter().map(|&i| positions[i as usize].x).sum::<f64>() > 0.0).collect();
        let scan = Scan::new(&positions, &half);
        let fill = complete(&scan, &plane_x(), None).expect("the other half is missing");
        assert_eq!(fill.pieces, 1);
        let ratio = fill.triangles.len() as f64 / half.len() as f64;
        assert!(ratio > 0.8 && ratio < 1.2, "added {} to {}", fill.triangles.len(), half.len());
        let half_area = 2.0 * std::f64::consts::PI * 400.0;
        assert!((fill.area - half_area).abs() < 0.15 * half_area, "area {}", fill.area);
        assert!(fill.joined > 0, "joined to the scan along the cut");
        assert!(fill.positions.iter().all(|p| p.x < 1e-9), "all on the missing side");
        // Wound as the scan is: its faces point out of the sphere.
        let outward = fill.triangles.iter().filter(|t| {
            let [a, b, c] = t.map(|i| fill.positions[i as usize]);
            (b - a).cross(c - a).dot(a + b + c) > 0.0
        });
        assert_eq!(outward.count(), fill.triangles.len());

        let whole = Scan::new(&positions, &triangles);
        assert!(complete(&whole, &plane_x(), None).is_none(), "a whole sphere is missing nothing");
        assert!(complete(&scan, &Mirror { normal: Vec3::ZERO, ..plane_x() }, None).is_none(), "a plane with no direction");
    }

    /// An egg-shaped part, 80 x 40 x 24 mm, mirrors in all three planes; cut short across X (its
    /// +X end missing) it still mirrors whole front to back, but the plane to complete it in is the
    /// one across the cut, through the part's middle, and the fill lands on the missing end.
    #[test]
    fn the_plane_across_a_cut_is_found_through_the_parts_middle() {
        let (ball, triangles) = sphere(1.0, 48, 96);
        let positions: Vec<Vec3> = ball.iter().map(|p| Vec3::new(p.x * 40.0, p.y * 20.0, p.z * 12.0)).collect();
        let kept: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| t.iter().all(|&i| positions[i as usize].x < 15.0)).collect();
        let scan = Scan::new(&positions, &kept);
        let plane = plane_across_cut(&scan).expect("a plane across the cut");
        assert!(plane.normal.x.abs() > 0.99, "across the cut: {plane:?}");
        assert!(plane.point.dot(plane.normal).abs() < 1.0, "through the middle: {plane:?}");
        let fill = complete(&scan, &plane, None).expect("the end is missing");
        let middles: Vec<f64> = fill.triangles.iter().map(|t| t.iter().map(|&i| fill.positions[i as usize].x).sum::<f64>() / 3.0).collect();
        assert!(!middles.is_empty() && middles.iter().filter(|x| **x < 12.0).count() * 20 < middles.len(), "on the missing end");
        // A whole scan has no cut.
        assert!(plane_across_cut(&Scan::new(&positions, &triangles)).is_none());
    }

    #[test]
    fn the_typical_edge_of_an_equilateral_triangle_is_its_side() {
        let h = 3.0f64.sqrt() / 2.0;
        let positions = [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.5, h, 0.0)];
        assert!((typical_edge(&Scan::new(&positions, &[[0, 1, 2]])) - 1.0).abs() < 1e-9);
        assert_eq!(typical_edge(&Scan::new(&[], &[])), 0.0);
    }
}
