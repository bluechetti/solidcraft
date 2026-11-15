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
