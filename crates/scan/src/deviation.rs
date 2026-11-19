//! How far a scan is from the model built from it: each scan point's signed distance to the
//! model's surface (outside positive), and what the distances add up to. The model is given as a
//! fine mesh of its surface, wound outward, as Compare Bodies measures one.

use solidcore_record::Vec3;
use solidcraft_geom::Mesh;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

/// Each scan point's distance from the model and what they add up to.
#[derive(Clone, Debug, PartialEq)]
pub struct Deviation {
    /// Per scan point, in mm: outside the model positive, inside negative; NaN where the point
    /// is farther than the reach from the model (a part of the scan the model leaves out).
    pub values: Vec<f32>,
    /// Over the points within reach: root mean square, mean, and the largest above and below.
    pub rms: f64,
    pub mean: f64,
    pub max_above: f64,
    pub max_below: f64,
    /// Which point is the largest above and which the largest below (`None` when no measured point
    /// is on that side).
    pub max_above_at: Option<usize>,
    pub max_below_at: Option<usize>,
    /// The share of the points within reach that are within the tolerance, 0 to 1.
    pub within: f64,
    /// How many points were within reach, and how many were not.
    pub measured: usize,
    pub unmatched: usize,
}

/// The deviation of `points` from `model`. Points farther than `reach` from it are left out of the
/// numbers; `tolerance` is what counts as on it.
pub fn measure(points: &[Vec3], model: &Mesh, tolerance: f64, reach: f64) -> Deviation {
    let tree = TriangleTree::new(model);
    // A whole scan's points (tens of millions when a selection reads every triangle) are measured
    // across the threads; each point on its own, so the order and the answer do not change.
    let values: Vec<f32> = crate::par::map(points.len(), |i| {
        let Some(&p) = points.get(i) else { return f32::NAN };
        let Some((distance, t)) = tree.nearest(p) else { return f32::NAN };
        if !(distance <= reach) {
            return f32::NAN;
        }
        let sign = model
            .triangles
            .get(t as usize)
            .and_then(|&[a, b, c]| Some([*model.positions.get(a as usize)?, *model.positions.get(b as usize)?, *model.positions.get(c as usize)?]))
            .and_then(|[a, b, c]| {
                let normal = (b - a).cross(c - a).normalized()?;
                let off = p - closest_on_triangle(p, a, b, c);
                Some(if off.dot(normal) < 0.0 { -1.0 } else { 1.0 })
            })
            .unwrap_or(1.0);
        (sign * distance) as f32
    });
    let measured: Vec<f64> = values.iter().filter(|v| v.is_finite()).map(|&v| f64::from(v)).collect();
    let n = measured.len().max(1) as f64;
    let extreme = |side: f32| {
        values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite() && **v * side > 0.0)
            .max_by(|(_, a), (_, b)| (**a * side).total_cmp(&(**b * side)))
            .map(|(i, _)| i)
    };
    let (max_above_at, max_below_at) = (extreme(1.0), extreme(-1.0));
    Deviation {
        rms: (measured.iter().map(|v| v * v).sum::<f64>() / n).sqrt(),
        mean: measured.iter().sum::<f64>() / n,
        max_above: measured.iter().copied().fold(0.0, f64::max),
        max_below: measured.iter().copied().fold(0.0, f64::min),
        max_above_at,
        max_below_at,
        within: measured.iter().filter(|v| v.abs() <= tolerance).count() as f64 / n,
        measured: measured.len(),
        unmatched: values.len() - measured.len(),
        values,
    }
}

/// A scan point and a model face are the same side of the part when their normals are within this
/// of each other (a cosine: 45° from the middle of one of the 26 ways the faces are sorted by).
const SAME_WAY: f64 = 0.707;

/// Each scan point's distance for measuring what was worn off or added: as `values` (from
/// [`measure`]: the nearest of the model's surface), but for a point beyond `tolerance`, to the
/// nearest model face that faces the way the scan does there. Beside a wall or a step the nearest
/// surface is the wall, not the face the material left: a dent 1.5 deep half a millimetre from a
/// slot's side is half a millimetre from the model, and its volume read by that came out a sixth
/// short. So is a point next along the scan to one found beyond it this way, and so on outward (the
/// dent's floor right by the wall is within the tolerance of the wall). A point with no such face
/// within `reach` keeps its value. Both scan and model are wound outward.
pub fn facing(positions: &[Vec3], triangles: &[[u32; 3]], model: &Mesh, values: &[f32], tolerance: f64, reach: f64) -> Vec<f32> {
    let n = positions.len().min(values.len());
    let beyond = |v: f32| v.is_finite() && f64::from(v.abs()) > tolerance;
    if !values[..n].iter().any(|v| beyond(*v)) {
        return values.to_vec();
    }
    // The scan's normal at each point: its triangles', by their areas.
    let mut normals = vec![Vec3::ZERO; n];
    for t in triangles {
        let [a, b, c] = t.map(|i| i as usize);
        if a < n && b < n && c < n {
            let cross = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
            if cross.is_finite() {
                normals[a] += cross;
                normals[b] += cross;
                normals[c] += cross;
            }
        }
    }
    // The model's faces sorted by which of 26 ways they face (along the axes, the edges and the
    // corners of a cube), each way a tree of its own.
    let ways: Vec<Vec3> = (-1..=1)
        .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| Vec3::new(f64::from(x), f64::from(y), f64::from(z)))))
        .filter_map(|v| v.normalized())
        .collect();
    let corner = |i: u32| model.positions.get(i as usize).copied();
    let face = |t: &[u32; 3]| Some([corner(t[0])?, corner(t[1])?, corner(t[2])?]);
    let mut sorted: Vec<Vec<[u32; 3]>> = vec![Vec::new(); ways.len()];
    for t in &model.triangles {
        let Some(normal) = face(t).and_then(|[a, b, c]| (b - a).cross(c - a).normalized()) else { continue };
        let Some(way) = (0..ways.len()).max_by(|&i, &j| ways[i].dot(normal).total_cmp(&ways[j].dot(normal))) else { continue };
        sorted[way].push(*t);
    }
    let trees: Vec<Option<TriangleTree>> =
        sorted.iter().map(|faces| (!faces.is_empty()).then(|| TriangleTree::of(&model.positions, faces))).collect();
    let faced = |i: usize| {
        let value = values[i];
        let Some(normal) = normals[i].normalized() else { return value };
        let p = positions[i];
        let mut best: Option<(f64, f64)> = None;
        for (way, tree) in ways.iter().zip(&trees).filter(|(way, _)| way.dot(normal) >= SAME_WAY) {
            let (Some(tree), Some(faces)) = (tree, sorted.get(ways.iter().position(|w| w == way).unwrap_or(usize::MAX))) else { continue };
            let Some((distance, t)) = tree.nearest(p) else { continue };
            if !(distance <= reach) || best.is_some_and(|(d, _)| d <= distance) {
                continue;
            }
            // Above or below that face.
            let side = faces
                .get(t as usize)
                .and_then(face)
                .and_then(|[a, b, c]| Some(((b - a).cross(c - a).normalized()?, closest_on_triangle(p, a, b, c))))
                .map_or(1.0, |(m, foot)| if (p - foot).dot(m) < 0.0 { -1.0 } else { 1.0 });
            best = Some((distance, side));
        }
        best.map_or(value, |(distance, side)| (side * distance) as f32)
    };
    // The points beyond the tolerance first, then round after round the points next to one that
    // came out beyond it.
    let mut out = values.to_vec();
    let mut done = vec![false; n];
    let mut front: Vec<usize> = (0..n).filter(|&i| beyond(values[i])).collect();
    front.iter().for_each(|&i| done[i] = true);
    let mut beside: Vec<Vec<u32>> = Vec::new();
    while !front.is_empty() {
        let found = crate::par::map(front.len(), |k| faced(front[k]));
        if beside.is_empty() {
            beside = vec![Vec::new(); n];
            for t in triangles {
                let [a, b, c] = t.map(|i| i as usize);
                if a < n && b < n && c < n {
                    beside[a].extend([t[1], t[2]]);
                    beside[b].extend([t[0], t[2]]);
                    beside[c].extend([t[0], t[1]]);
                }
            }
        }
        let mut next = Vec::new();
        for (&i, value) in front.iter().zip(found) {
            out[i] = value;
            if beyond(value) {
                for &j in &beside[i] {
                    if !std::mem::replace(&mut done[j as usize], true) {
                        next.push(j as usize);
                    }
                }
            }
        }
        front = next;
    }
    out
}

/// A patch of the scan beyond the tolerance on one side of the model: a dent, a worn place, a
/// lump of extra material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Inside the model (material missing: wear, a dent) or outside it (material added).
    pub below: bool,
    /// The area of the model it lies over (mm²) and the volume between it and the model (mm³):
    /// each scan point's distance times the area it stands for.
    pub area: f64,
    pub volume: f64,
    /// Its largest distance from the model (mm, unsigned) and the scan point where.
    pub deepest: f64,
    pub at: usize,
}

/// What of the scan lies beyond the tolerance: areas (mm²) and volumes (mm³) above and below the
/// model, and the regions they fall into, the largest volume first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beyond {
    pub area_above: f64,
    pub area_below: f64,
    pub volume_above: f64,
    pub volume_below: f64,
    pub regions: Vec<Region>,
}

/// What of the scan (`positions`, `triangles`) lies beyond `tolerance` of the model, from each
/// point's distance (`values`, as [`measure`] gives them for the scan's points). Each point stands
/// for a third of the area of its triangles as the model sees them (a triangle leaning against the
/// model's surface counts by what it covers of it), and the volume between scan and model there is
/// that area times its distance. Neither scan nor model needs to be closed. A region is points beyond the tolerance on one side, joined along the
/// scan's edges.
pub fn beyond(positions: &[Vec3], triangles: &[[u32; 3]], values: &[f32], tolerance: f64) -> Beyond {
    let n = positions.len().min(values.len());
    let mut area = vec![0.0f64; n];
    for t in triangles {
        let [a, b, c] = t.map(|i| i as usize);
        if a >= n || b >= n || c >= n {
            continue;
        }
        let (u, v) = (positions[b] - positions[a], positions[c] - positions[a]);
        let cross = u.cross(v);
        let twice = cross.len();
        if !(twice > 0.0 && twice.is_finite()) {
            continue;
        }
        // The triangle's area as the model sees it: a triangle leaning against the model's
        // surface (a dent's wall) covers less of it than its own area, by the cosine of the lean.
        // The distance changes along the triangle at the sine of that lean, so its slope across
        // the triangle (from the three corners' distances) gives the cosine.
        let (da, db, dc) = (f64::from(values[a]), f64::from(values[b]), f64::from(values[c]));
        let facing = if da.is_finite() && db.is_finite() && dc.is_finite() {
            let normal = cross / twice;
            let slope = (normal.cross(v) * (db - da) + u.cross(normal) * (dc - da)) / twice;
            (1.0 - slope.len2().min(1.0)).sqrt()
        } else {
            1.0
        };
        let third = facing * twice / 6.0;
        area[a] += third;
        area[b] += third;
        area[c] += third;
    }
    // Which side each point is beyond the tolerance on: 1 above, -1 below, 0 within or unmeasured.
    let side: Vec<i8> = values[..n]
        .iter()
        .map(|&v| {
            let v = f64::from(v);
            if !v.is_finite() || v.abs() <= tolerance {
                0
            } else if v > 0.0 {
                1
            } else {
                -1
            }
        })
        .collect();
    // Points of one side joined along the scan's edges are one region.
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    for t in triangles {
        let [a, b, c] = *t;
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let (i, j) = (x as usize, y as usize);
            if i < n && j < n && side[i] != 0 && side[i] == side[j] {
                let (ri, rj) = (root(&mut parent, x), root(&mut parent, y));
                if ri != rj {
                    parent[ri.max(rj) as usize] = ri.min(rj);
                }
            }
        }
    }
    let mut out = Beyond::default();
    let mut regions: std::collections::HashMap<u32, Region> = std::collections::HashMap::new();
    for i in 0..n {
        if side[i] == 0 {
            continue;
        }
        let (distance, below) = (f64::from(values[i]).abs(), side[i] < 0);
        let volume = area[i] * distance;
        if below {
            out.area_below += area[i];
            out.volume_below += volume;
        } else {
            out.area_above += area[i];
            out.volume_above += volume;
        }
        let region = regions.entry(root(&mut parent, i as u32)).or_insert(Region { below, area: 0.0, volume: 0.0, deepest: 0.0, at: i });
        region.area += area[i];
        region.volume += volume;
        if distance > region.deepest {
            (region.deepest, region.at) = (distance, i);
        }
    }
    out.regions = regions.into_values().collect();
    out.regions.sort_by(|a, b| b.volume.total_cmp(&a.volume).then(a.at.cmp(&b.at)));
    out
}

/// Where a deviation sits on the colour scale, -1 to 1: 0 within the tolerance, rising to ±1 at the
/// limit and held there beyond it; `None` for a point the model leaves out.
pub fn scale(value: f32, tolerance: f64, limit: f64) -> Option<f64> {
    let v = f64::from(value);
    if !v.is_finite() {
        return None;
    }
    if v.abs() <= tolerance {
        return Some(0.0);
    }
    let span = (limit - tolerance).max(1e-12);
    Some(v.signum() * ((v.abs() - tolerance) / span).min(1.0))
}

/// The share of the scan the automatic colour range covers: the distance below which this share
/// of the measured points lie, so a few stray points do not stretch the scale for all the others.
pub const AUTO_SHARE: f64 = 0.99;

/// The distance (unsigned, mm) that `share` (0 to 1) of the measured `values` lie within; `None`
/// when none was measured.
pub fn spread(values: &[f32], share: f64) -> Option<f64> {
    let mut d: Vec<f32> = values.iter().filter(|v| v.is_finite()).map(|v| v.abs()).collect();
    if d.is_empty() || !share.is_finite() {
        return None;
    }
    let k = ((share.clamp(0.0, 1.0) * d.len() as f64).ceil() as usize).clamp(1, d.len()) - 1;
    let (_, v, _) = d.select_nth_unstable_by(k, f32::total_cmp);
    Some(f64::from(*v))
}

/// A place on the colour scale (-1 to 1) put on one of `bands` steps each side of within, the step
/// it reaches into; 0 bands leaves it as it is (a smooth scale). Within stays within.
pub fn banded(s: f64, bands: u32) -> f64 {
    if bands == 0 || s == 0.0 || !s.is_finite() {
        return s;
    }
    let n = f64::from(bands);
    s.signum() * ((s.abs() * n).ceil() / n).min(1.0)
}

#[cfg(test)]
mod tests {
    /// A floor with a slot beside it (the slot's wall goes down from the floor's edge), and a scan
    /// of it worn 2 deep in a strip along that edge, rising back to the floor. The nearest of the model to the worn
    /// points by the edge is the slot's wall, half a millimetre away, so the volume read by the
    /// nearest surface is far short; read to the face the scan faces (the floor) it is the strip's.
    #[test]
    fn wear_beside_a_wall_is_measured_to_the_face_it_left() {
        use super::*;
        let model = Mesh {
            positions: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(20.0, 0.0, 0.0),
                Vec3::new(20.0, 20.0, 0.0),
                Vec3::new(0.0, 20.0, 0.0),
                Vec3::new(0.0, 0.0, -10.0),
                Vec3::new(0.0, 20.0, -10.0),
            ],
            // The floor facing up, and the slot's wall at x = 0 facing the slot (-x).
            triangles: vec![[0, 1, 2], [0, 2, 3], [0, 3, 5], [0, 5, 4]],
            ..Mesh::default()
        };
        let cells = 80u32;
        let at = |i: u32, j: u32| {
            let (x, y) = (0.25 + 19.5 * f64::from(i) / f64::from(cells), 0.25 + 19.5 * f64::from(j) / f64::from(cells));
            // Worn 2 deep by the slot, rising back to the floor between x = 3 and x = 6.
            Vec3::new(x, y, -2.0 * ((6.0 - x) / 3.0).clamp(0.0, 1.0))
        };
        let index = |i: u32, j: u32| i * (cells + 1) + j;
        let positions: Vec<Vec3> = (0..=cells).flat_map(|i| (0..=cells).map(move |j| at(i, j))).collect();
        let mut triangles = Vec::new();
        for i in 0..cells {
            for j in 0..cells {
                triangles.push([index(i, j), index(i + 1, j), index(i + 1, j + 1)]);
                triangles.push([index(i, j), index(i + 1, j + 1), index(i, j + 1)]);
            }
        }
        let d = measure(&positions, &model, 0.05, 10.0);
        let by_nearest = beyond(&positions, &triangles, &d.values, 0.05).volume_below;
        let faced = facing(&positions, &triangles, &model, &d.values, 0.05, 10.0);
        let by_face = beyond(&positions, &triangles, &faced, 0.05).volume_below;
        // What was worn off: under each cell of the floor, its area times its corners' mean depth.
        let cell = (19.5 / f64::from(cells)).powi(2);
        let worn: f64 = (0..cells)
            .flat_map(|i| (0..cells).map(move |j| (i, j)))
            .map(|(i, j)| -cell * (at(i, j).z + at(i + 1, j).z + at(i + 1, j + 1).z + at(i, j + 1).z) / 4.0)
            .sum();
        assert!((by_face - worn).abs() < 0.03 * worn, "{by_face} against the {worn} worn off");
        assert!(by_nearest < 0.85 * worn, "the wall is nearer than the floor by the edge: {by_nearest} of {worn}");
        // Away from any wall the two are one: those points keep their values.
        assert!(positions.iter().zip(d.values.iter().zip(&faced)).filter(|(p, _)| p.x > 3.0).all(|(_, (a, b))| a == b));
    }

    /// A flat 40 × 40 scan with a dent 1.5 deep over a round patch of radius 8, against the flat
    /// model: the volume lost is the dent's (π R² D / 2 for its parabolic bowl), one region below
    /// the model, deepest at its middle; nothing above. The scan is open, and so is the model.
    #[test]
    fn a_dents_volume_and_region_are_read_off_an_open_scan() {
        use super::*;
        let (n, size, radius, depth) = (160u32, 40.0, 8.0, 1.5);
        let at = |i: u32, j: u32| {
            let (x, y) = (size * f64::from(i) / f64::from(n) - size / 2.0, size * f64::from(j) / f64::from(n) - size / 2.0);
            let r = x.hypot(y) / radius;
            Vec3::new(x, y, if r < 1.0 { -depth * (1.0 - r * r) } else { 0.0 })
        };
        let index = |i: u32, j: u32| i * (n + 1) + j;
        let positions: Vec<Vec3> = (0..=n).flat_map(|i| (0..=n).map(move |j| at(i, j))).collect();
        let mut triangles = Vec::new();
        for i in 0..n {
            for j in 0..n {
                triangles.push([index(i, j), index(i + 1, j), index(i + 1, j + 1)]);
                triangles.push([index(i, j), index(i + 1, j + 1), index(i, j + 1)]);
            }
        }
        let half = size / 2.0;
        let model = Mesh {
            positions: vec![Vec3::new(-half, -half, 0.0), Vec3::new(half, -half, 0.0), Vec3::new(half, half, 0.0), Vec3::new(-half, half, 0.0)],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            ..Mesh::default()
        };
        let d = measure(&positions, &model, 0.05, 10.0);
        let found = beyond(&positions, &triangles, &d.values, 0.05);
        let expected = std::f64::consts::PI * radius * radius * depth / 2.0;
        assert!((found.volume_below - expected).abs() < 0.02 * expected, "{} against {expected}", found.volume_below);
        assert!(found.volume_above == 0.0 && found.area_above == 0.0, "{found:?}");
        assert_eq!(found.regions.len(), 1, "{:?}", found.regions);
        let dent = found.regions[0];
        assert!(dent.below && (dent.deepest - depth).abs() < 1e-6 && positions[dent.at].x.hypot(positions[dent.at].y) < 1e-9, "{dent:?}");
        let patch = std::f64::consts::PI * radius * radius;
        assert!((dent.area - patch).abs() < 0.1 * patch && (dent.volume - found.volume_below).abs() < 1e-9, "{dent:?}");
        // Two dents apart are two regions, the larger volume first.
        let mut twice = positions.clone();
        for p in &mut twice {
            let r = (p.x - 14.0).hypot(p.y - 14.0) / 3.0;
            if r < 1.0 {
                p.z = -0.6 * (1.0 - r * r);
            }
        }
        let d = measure(&twice, &model, 0.05, 10.0);
        let found = beyond(&twice, &triangles, &d.values, 0.05);
        assert_eq!(found.regions.len(), 2, "{:?}", found.regions);
        assert!(found.regions[0].volume > found.regions[1].volume && (found.regions[1].deepest - 0.6).abs() < 0.02, "{:?}", found.regions);
    }

    use super::*;

    /// A closed box mesh from `lo` to `hi`, wound outward.
    fn box_mesh(lo: Vec3, hi: Vec3) -> Mesh {
        let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let triangles =
            vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        Mesh { positions: (0..8).map(p).collect(), triangles, ..Mesh::default() }
    }

    #[test]
    fn points_outside_are_above_inside_below_and_far_ones_left_out() {
        let model = box_mesh(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let points = [Vec3::new(5.0, 5.0, 10.2), Vec3::new(5.0, 5.0, 9.95), Vec3::new(5.0, 5.0, 10.05), Vec3::new(5.0, 5.0, 40.0)];
        let d = measure(&points, &model, 0.1, 5.0);
        assert!((d.values[0] - 0.2).abs() < 1e-6 && (d.values[1] + 0.05).abs() < 1e-6, "{:?}", d.values);
        assert!(d.values[3].is_nan(), "beyond the reach");
        assert_eq!((d.measured, d.unmatched), (3, 1));
        assert!((d.max_above - 0.2).abs() < 1e-6 && (d.max_below + 0.05).abs() < 1e-6);
        assert!((d.within - 2.0 / 3.0).abs() < 1e-9, "the 0.05 below and the 0.05 above are within 0.1");
        assert_eq!(scale(0.05, 0.1, 0.5), Some(0.0));
        assert!(scale(0.3, 0.1, 0.5).is_some_and(|s| (s - 0.5).abs() < 1e-6));
        assert_eq!(scale(-0.9, 0.1, 0.5), Some(-1.0));
        assert_eq!(scale(f32::NAN, 0.1, 0.5), None);
        assert_eq!((d.max_above_at, d.max_below_at), (Some(0), Some(1)), "the 0.2 above and the 0.05 below");
    }

    #[test]
    fn the_spread_leaves_out_the_stray_few() {
        // 99 points 0.01 … 0.99 mm off and one stray 50 mm: 99 % of them lie within 0.99.
        let mut v: Vec<f32> = (1..=99).map(|i| i as f32 / 100.0 * if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        v.push(50.0);
        v.push(f32::NAN);
        assert!((spread(&v, AUTO_SHARE).unwrap() - 0.99).abs() < 1e-6);
        assert_eq!(spread(&v, 1.0), Some(50.0));
        assert_eq!(spread(&[f32::NAN], 0.5), None);
        assert_eq!(spread(&[], 0.5), None);
        assert_eq!(spread(&[0.3], f64::NAN), None);
    }

    #[test]
    fn bands_step_the_scale_and_leave_within_alone() {
        assert_eq!(banded(0.0, 4), 0.0);
        assert_eq!(banded(0.1, 4), 0.25);
        assert_eq!(banded(0.25, 4), 0.25);
        assert_eq!(banded(-0.26, 4), -0.5);
        assert_eq!(banded(1.0, 4), 1.0);
        assert_eq!(banded(0.37, 0), 0.37, "no bands: smooth");
        assert!(banded(f64::NAN, 3).is_nan());
    }
}
