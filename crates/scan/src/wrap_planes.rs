//! Wrap Scan's plane faces: where the scan is flat (Find All's planes), the patches laid there are
//! replaced by one plane face each, bounded by the very sides the patches round it share, so the
//! body is built in one go with no cutting or merging afterwards.
//!
//! 1. [`snapped`]: the scan's points in each plane's region, within the tolerance of the plane,
//!    moved onto it. Wrapped, that copy lays patches lying on the plane to rounding.
//! 2. [`planes`]: each set of connected patches whose every control point lies on one plane is a
//!    group; its control points are put exactly on the plane, in every patch that shares them
//!    (the side a group shares with a patch beyond it is the same four points in both), and the
//!    group's outline is traced from its patches' sides that no other patch of the group runs back
//!    along. The group becomes one plane face with those loops, the patches beyond keep theirs.
//!
//! The idea of wrapping a copy snapped onto the shapes is d0pawlus/SurfaceCraft's (its hybrid
//! wrap); there the groups were cut out of the built body afterwards. Here they never enter it.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::Scan;

/// A flat region of the scan: its plane (a point and a unit normal) and its triangles.
#[derive(Clone, Debug)]
pub struct PlaneRegion {
    pub point: Vec3,
    pub normal: Vec3,
    pub triangles: Vec<usize>,
}

/// One plane face of the wrap: its plane and its loops, each a run of cubic Bézier sides (four
/// control points, the end of one the start of the next), counter-clockwise about `normal` for
/// the outer loop, the largest first.
#[derive(Clone, Debug)]
pub struct PlaneFace {
    pub point: Vec3,
    pub normal: Vec3,
    pub loops: Vec<Vec<[Vec3; 4]>>,
    /// The patches it takes the place of.
    pub patches: usize,
}

/// A patch's control points lie on a plane when they are within this share of the scan's size of
/// it (rounding of the fit; a patch off the plane by anything real is far above it).
const ON_PLANE: f64 = 1e-9;

/// The scan's positions with each point of a plane's region that lies within `tolerance` of the
/// plane moved onto it. A point of two or three regions, within the tolerance of each (along a
/// crease, at a corner), goes onto their line or their point, by the least move, so the patches
/// on either side of the crease lie on their planes; planes within [`CREASE_SIN`] of parallel
/// count as one (the first).
pub fn snapped(scan: &Scan, regions: &[PlaneRegion], tolerance: f64) -> Vec<Vec3> {
    let units: Vec<Option<(Vec3, Vec3)>> = regions.iter().map(|r| r.normal.normalized().map(|n| (r.point, n))).collect();
    // Each point's regions, in order, within the tolerance of their planes.
    let mut of: Vec<Vec<usize>> = vec![Vec::new(); scan.positions.len()];
    for (k, r) in regions.iter().enumerate() {
        let Some((p0, n)) = units[k] else { continue };
        for &t in &r.triangles {
            let Some(tri) = scan.triangles.get(t) else { continue };
            for &v in tri {
                let Some((list, p)) = of.get_mut(v as usize).zip(scan.positions.get(v as usize)) else { continue };
                if !list.contains(&k) && (*p - p0).dot(n).abs() <= tolerance {
                    list.push(k);
                }
            }
        }
    }
    scan.positions
        .iter()
        .zip(&of)
        .map(|(&p, list)| {
            // The planes it goes onto: the first, then each other not near parallel to those taken.
            let mut planes: Vec<(Vec3, f64)> = Vec::new();
            for &k in list {
                let Some((p0, n)) = units[k] else { continue };
                let apart = planes.iter().all(|(m, _)| m.cross(n).len() > CREASE_SIN);
                if planes.len() < 3 && apart {
                    planes.push((n, n.dot(p0)));
                }
            }
            onto(p, &planes).unwrap_or(p)
        })
        .collect()
}

/// Two planes are one for snapping when their normals are within this sine (10°).
const CREASE_SIN: f64 = 0.1736;

/// The point nearest `p` on every plane given (n·x = d, unit n), one to three of them: `p` less
/// Nᵀ (N Nᵀ)⁻¹ (N p − d). `None` when the planes have no common point to rounding.
fn onto(p: Vec3, planes: &[(Vec3, f64)]) -> Option<Vec3> {
    let r: Vec<f64> = planes.iter().map(|(n, d)| n.dot(p) - d).collect();
    match planes {
        [] => Some(p),
        [(n, _)] => Some(p - *n * r[0]),
        [(a, _), (b, _)] => {
            let (g11, g12, g22) = (a.dot(*a), a.dot(*b), b.dot(*b));
            let det = g11 * g22 - g12 * g12;
            if !(det > 1e-12) {
                return None;
            }
            let (l1, l2) = ((g22 * r[0] - g12 * r[1]) / det, (g11 * r[1] - g12 * r[0]) / det);
            Some(p - *a * l1 - *b * l2)
        }
        [(a, da), (b, db), (c, dc)] => {
            // Three planes meet in one point: solve N x = d by Cramer's rule.
            let det = a.dot(b.cross(*c));
            if !(det.abs() > 1e-9) {
                return None;
            }
            Some((b.cross(*c) * *da + c.cross(*a) * *db + a.cross(*b) * *dc) / det)
        }
        _ => None,
    }
}

/// The four sides of a patch's 4 × 4 net (u fastest), counter-clockwise about its natural normal,
/// as indices into the net.
const SIDES: [[usize; 4]; 4] = [[0, 1, 2, 3], [3, 7, 11, 15], [15, 14, 13, 12], [12, 8, 4, 0]];

fn bits(p: Vec3) -> [u64; 3] {
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// The patches' nets, with the groups of patches lying on one of `regions`' planes taken out and
/// returned as plane faces. `size`: the scan's size. A group whose outline cannot be traced as
/// simple loops (two of its corners touching) keeps its patches.
pub fn planes(faces: &[NurbsSurface], regions: &[PlaneRegion], size: f64) -> (Vec<[Vec3; 16]>, Vec<PlaneFace>) {
    let mut nets: Vec<[Vec3; 16]> = faces
        .iter()
        .map(|f| {
            let pts = crate::surface_edit::points(f);
            std::array::from_fn(|k| pts.get(k).copied().unwrap_or(Vec3::ZERO))
        })
        .collect();
    let eps = ON_PLANE * size.max(1e-9);
    let units: Vec<Option<(Vec3, Vec3)>> = regions.iter().map(|r| r.normal.normalized().map(|n| (r.point, n))).collect();
    // The plane each patch lies on, if any.
    let on: Vec<Option<usize>> =
        nets.iter().map(|net| units.iter().position(|u| u.is_some_and(|(p, n)| net.iter().all(|q| (*q - p).dot(n).abs() <= eps)))).collect();
    // Patches sharing a side, by its two corners (either way round).
    let mut by_side: HashMap<([u64; 3], [u64; 3]), Vec<usize>> = HashMap::new();
    for (i, net) in nets.iter().enumerate() {
        for s in SIDES {
            let (a, b) = (bits(net[s[0]]), bits(net[s[3]]));
            by_side.entry(if a <= b { (a, b) } else { (b, a) }).or_default().push(i);
        }
    }
    // Groups: connected patches on one plane.
    let mut group = vec![usize::MAX; nets.len()];
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for start in 0..nets.len() {
        let Some(plane) = on[start] else { continue };
        if group[start] != usize::MAX {
            continue;
        }
        let g = groups.len();
        let mut members = vec![start];
        group[start] = g;
        let mut k = 0;
        while let Some(&i) = members.get(k) {
            k += 1;
            for s in SIDES {
                let (a, b) = (bits(nets[i][s[0]]), bits(nets[i][s[3]]));
                for &j in by_side.get(&if a <= b { (a, b) } else { (b, a) }).into_iter().flatten() {
                    if group[j] == usize::MAX && on[j] == Some(plane) {
                        group[j] = g;
                        members.push(j);
                    }
                }
            }
        }
        groups.push((plane, members));
    }
    let mut taken = vec![false; nets.len()];
    let mut out = Vec::new();
    for (plane, members) in groups {
        let Some((p0, n)) = units[plane] else { continue };
        // The group's control points onto the plane exactly, in every patch that has them.
        let mut moved: HashMap<[u64; 3], Vec3> = HashMap::new();
        for &i in &members {
            for q in nets[i] {
                moved.entry(bits(q)).or_insert_with(|| q - n * (q - p0).dot(n));
            }
        }
        // Its outline: the sides of its patches no patch of it runs back along.
        let mut sides: Vec<[Vec3; 4]> = Vec::new();
        let mut inner: HashMap<([u64; 3], [u64; 3]), usize> = HashMap::new();
        for &i in &members {
            for s in SIDES {
                let side = s.map(|k| nets[i][k]);
                *inner.entry((bits(side[0]), bits(side[3]))).or_default() += 1;
                sides.push(side);
            }
        }
        let rim: Vec<[Vec3; 4]> = sides.into_iter().filter(|s| !inner.contains_key(&(bits(s[3]), bits(s[0])))).collect();
        let Some(loops) = loops_of(&rim) else { continue };
        let loops: Vec<Vec<[Vec3; 4]>> =
            loops.into_iter().map(|l| l.into_iter().map(|s| s.map(|q| moved.get(&bits(q)).copied().unwrap_or(q))).collect()).collect();
        for net in nets.iter_mut() {
            for q in net.iter_mut() {
                if let Some(m) = moved.get(&bits(*q)) {
                    *q = *m;
                }
            }
        }
        for &i in &members {
            taken[i] = true;
        }
        out.push(PlaneFace { point: p0, normal: n, loops, patches: members.len() });
    }
    let kept = nets.into_iter().zip(taken).filter(|(_, t)| !t).map(|(n, _)| n).collect();
    (kept, out)
}

/// The sides of an outline joined into loops, the largest (by the area it encloses) first; `None`
/// when a corner starts two sides (the outline touches itself there) or a run does not close.
fn loops_of(rim: &[[Vec3; 4]]) -> Option<Vec<Vec<[Vec3; 4]>>> {
    let mut from: HashMap<[u64; 3], usize> = HashMap::new();
    for (k, s) in rim.iter().enumerate() {
        if from.insert(bits(s[0]), k).is_some() {
            return None;
        }
    }
    let mut used = vec![false; rim.len()];
    let mut loops: Vec<(f64, Vec<[Vec3; 4]>)> = Vec::new();
    for start in 0..rim.len() {
        if used[start] {
            continue;
        }
        let mut run = Vec::new();
        let mut k = start;
        while !used[k] {
            used[k] = true;
            run.push(rim[k]);
            k = *from.get(&bits(rim[k][3]))?;
        }
        if k != start {
            return None;
        }
        // Its enclosed area by Newell's sum over its corners.
        let mut area = Vec3::ZERO;
        for s in &run {
            area += s[0].cross(s[3]);
        }
        loops.push((area.len(), run));
    }
    loops.sort_by(|a, b| b.0.total_cmp(&a.0));
    Some(loops.into_iter().map(|(_, l)| l).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat patch over the quad a b c d, its net bilinear.
    fn flat(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> NurbsSurface {
        let net: Vec<Vec3> = (0..16)
            .map(|k| {
                let (u, v) = ((k % 4) as f64 / 3.0, (k / 4) as f64 / 3.0);
                a * ((1.0 - u) * (1.0 - v)) + b * (u * (1.0 - v)) + c * (u * v) + d * ((1.0 - u) * v)
            })
            .collect();
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        NurbsSurface::new([3, 3], [knots.clone(), knots], &net, &[1.0; 16]).expect("patch")
    }

    /// Four flat patches in a 2 × 2 square on z = 0 and one bent patch beside them: the four are
    /// one plane face with one loop of 8 sides (the square's outline, two sides to an edge of it),
    /// the bent one stays a patch, and the side it shares with the square is the same four points
    /// in it and in the face's loop.
    #[test]
    fn patches_on_a_plane_become_one_face_bounded_by_their_shared_sides() {
        let p = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let faces = vec![
            flat(p(0., 0., 0.), p(1., 0., 0.), p(1., 1., 0.), p(0., 1., 0.)),
            flat(p(1., 0., 0.), p(2., 0., 0.), p(2., 1., 0.), p(1., 1., 0.)),
            flat(p(0., 1., 0.), p(1., 1., 0.), p(1., 2., 0.), p(0., 2., 0.)),
            flat(p(1., 1., 0.), p(2., 1., 0.), p(2., 2., 0.), p(1., 2., 0.)),
            flat(p(2., 0., 0.), p(3., 0., 1.), p(3., 1., 1.), p(2., 1., 0.)),
        ];
        let region = PlaneRegion { point: Vec3::ZERO, normal: Vec3::Z, triangles: Vec::new() };
        let (kept, planes) = planes(&faces, &[region], 3.0);
        assert_eq!((kept.len(), planes.len()), (1, 1));
        let face = &planes[0];
        assert_eq!((face.patches, face.loops.len(), face.loops[0].len()), (4, 1, 8));
        // Counter-clockwise about +z: Newell's area is +4 along z.
        let mut area = Vec3::ZERO;
        for s in &face.loops[0] {
            area += s[0].cross(s[3]);
        }
        assert!((area.z / 2.0 - 4.0).abs() < 1e-12, "{area:?}");
        // The bent patch's side x = 2, y 0..1 is in the loop, run the other way.
        let shared = [kept[0][12], kept[0][8], kept[0][4], kept[0][0]];
        assert!(face.loops[0].iter().any(|s| s.iter().rev().zip(&shared).all(|(a, b)| a == b)), "{:?}", face.loops[0]);
    }

    /// A ring of eight flat patches round a hole: one face of two loops, the outer first.
    #[test]
    fn a_plane_group_round_a_hole_has_two_loops() {
        let p = |x: f64, y: f64| Vec3::new(x, y, 0.0);
        let mut faces = Vec::new();
        for i in 0..3 {
            for j in 0..3 {
                if (i, j) != (1, 1) {
                    let (x, y) = (f64::from(i), f64::from(j));
                    faces.push(flat(p(x, y), p(x + 1., y), p(x + 1., y + 1.), p(x, y + 1.)));
                }
            }
        }
        let region = PlaneRegion { point: Vec3::ZERO, normal: Vec3::Z, triangles: Vec::new() };
        let (kept, planes) = planes(&faces, &[region], 3.0);
        assert!(kept.is_empty());
        assert_eq!(planes[0].loops.iter().map(Vec::len).collect::<Vec<_>>(), vec![12, 4]);
    }

    /// A point near the crease of two planes (z = 0 and x = 0), within the tolerance of each, goes
    /// onto their line; one near three (and y = 0) onto their corner.
    #[test]
    fn a_point_by_a_crease_goes_onto_the_line_and_by_a_corner_onto_the_point() {
        let positions = [Vec3::new(0.03, 5.0, -0.04), Vec3::new(0.02, -0.01, 0.03), Vec3::new(9.0, 9.0, 9.0)];
        let triangles = [[0, 1, 2]];
        let scan = Scan::new(&positions, &triangles);
        let plane = |n: Vec3| PlaneRegion { point: Vec3::ZERO, normal: n, triangles: vec![0] };
        let out = snapped(&scan, &[plane(Vec3::Z), plane(Vec3::X), plane(Vec3::Y)], 0.1);
        assert!(out[0].dist(Vec3::new(0.0, 5.0, 0.0)) < 1e-12, "{:?}", out[0]);
        assert!(out[1].dist(Vec3::ZERO) < 1e-12, "{:?}", out[1]);
        assert_eq!(out[2], positions[2]);
    }

    #[test]
    fn snapping_moves_only_points_of_the_region_within_the_tolerance() {
        let positions = [Vec3::new(0.0, 0.0, 0.05), Vec3::new(1.0, 0.0, -0.05), Vec3::new(0.0, 1.0, 0.5), Vec3::new(5.0, 5.0, 0.01)];
        let triangles = [[0, 1, 2], [1, 3, 2]];
        let scan = Scan::new(&positions, &triangles);
        let region = PlaneRegion { point: Vec3::ZERO, normal: Vec3::Z, triangles: vec![0] };
        let out = snapped(&scan, &[region], 0.1);
        assert_eq!((out[0].z, out[1].z, out[2].z, out[3].z), (0.0, 0.0, 0.5, 0.01));
    }
}
