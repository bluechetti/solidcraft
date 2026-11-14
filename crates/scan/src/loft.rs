//! Loft It's reading: a part whose section changes smoothly from one end to the other (a blade, a
//! handle, a horn) is read as sections lofted. The direction it runs is the one two planes at its
//! ends face apart along, when Find All found such end faces; else the direction it is longest
//! along. The scan is sliced square to it at even steps from end to end; each slice must be one
//! closed loop, which is laid out from a matching start round the same way, so the loft through
//! them does not twist.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::reconstruct::Face;
use crate::{Scan, in_plane};

/// Directions this close are one direction (a cosine: about 2°).
const SAME_DIRECTION: f64 = 0.9994;
/// An end face lies this near the scan's end along its normal, as a share of the part's length.
const AT_THE_END: f64 = 0.01;
/// The slices nearest the ends are this share of the length in from them.
const END_MARGIN: f64 = 0.01;
/// The fewest sections and the most.
pub const LEAST_SECTIONS: usize = 2;
pub const MOST_SECTIONS: usize = 30;
/// Points each section is laid out with.
const POINTS: usize = 36;
/// A loop shorter than this share of the longest in its slice is a speck of noise.
const SPECK: f64 = 0.05;

/// One section: the plane it lies in (its x and y the same at every section) and its points in
/// that plane's coordinates, counter-clockwise, the first where the one before's first was.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub plane: Plane,
    pub points: Vec<Vec2>,
}

/// A part read as sections lofted along `direction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Loft {
    pub direction: Vec3,
    /// Whether the ends are faces Find All found (the loft then runs from one to the other).
    pub capped: bool,
    pub sections: Vec<Section>,
}

/// The direction two planes among `faces` face apart along, each at an end of the scan; `None`
/// without such a pair.
fn capped_direction(scan: &Scan, faces: &[Face]) -> Option<Vec3> {
    let planes: Vec<(Vec3, Vec3, f64)> = faces
        .iter()
        .filter_map(|f| match f.shape {
            Shape::Plane { point, normal } if f.area > 0.0 => Some((point, normal.normalized()?, f.area)),
            _ => None,
        })
        .collect();
    let mut best: Option<(f64, Vec3)> = None;
    for (i, a) in planes.iter().enumerate() {
        for b in &planes[i + 1..] {
            if a.1.dot(b.1) > -SAME_DIRECTION {
                continue;
            }
            let d = a.1;
            let (low, high) = scan.points().map(|p| p.dot(d)).fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| (l.min(x), h.max(x)));
            let length = high - low;
            let (la, lb) = (a.0.dot(d), b.0.dot(d));
            let ends = |x: f64| (x - low).abs() <= AT_THE_END * length || (high - x).abs() <= AT_THE_END * length;
            if length > 0.0 && ends(la) && ends(lb) && (la - lb).abs() >= 0.5 * length {
                let weight = a.2.min(b.2);
                if best.is_none_or(|(w, _)| weight > w) {
                    best = Some((weight, d));
                }
            }
        }
    }
    best.map(|(_, d)| d)
}

/// The loop's points evenly spaced along it, `count` of them from the point at `start`.
fn resampled(points: &[Vec2], start: usize, count: usize) -> Vec<Vec2> {
    let n = points.len();
    let ring: Vec<Vec2> = (0..=n).map(|k| points[(start + k) % n]).collect();
    let mut along = vec![0.0];
    for w in ring.windows(2) {
        along.push(along.last().copied().unwrap_or(0.0) + w[0].dist(w[1]));
    }
    let total = along.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(count);
    let mut j = 0;
    for k in 0..count {
        let t = total * k as f64 / count as f64;
        while j + 2 < along.len() && along[j + 1] < t {
            j += 1;
        }
        let span = along[j + 1] - along[j];
        let f = if span > 0.0 { (t - along[j]) / span } else { 0.0 };
        out.push(ring[j] + (ring[j + 1] - ring[j]) * f);
    }
    out
}

fn signed_area(points: &[Vec2]) -> f64 {
    let n = points.len();
    0.5 * (0..n).map(|k| points[k].x * points[(k + 1) % n].y - points[(k + 1) % n].x * points[k].y).sum::<f64>()
}

fn middle(points: &[Vec2]) -> Vec2 {
    points.iter().fold(Vec2::ZERO, |s, p| s + *p) / points.len().max(1) as f64
}

/// The scan read as `count` sections lofted (see the module); why not, when it cannot be.
pub fn read(scan: &Scan, faces: &[Face], count: usize) -> Result<Loft, String> {
    let count = count.clamp(LEAST_SECTIONS, MOST_SECTIONS);
    let points: Vec<Vec3> = crate::thinned(&scan.points().collect::<Vec<_>>(), 200_000);
    let (centre, axes, _) = crate::register::principal(&points).ok_or("the scan has no extent")?;
    let capped = capped_direction(scan, faces);
    let direction = capped.unwrap_or(axes[0]);
    let (low, high) = points.iter().map(|p| p.dot(direction)).fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| (l.min(x), h.max(x)));
    let length = high - low;
    if !(length > 0.0) {
        return Err("the scan has no length".into());
    }
    let (x, y) = in_plane(direction);
    let weld = crate::welded(scan.positions);
    let mut sections: Vec<Section> = Vec::with_capacity(count);
    let mut previous: Option<Vec2> = None;
    for k in 0..count {
        let share = k as f64 / (count - 1) as f64;
        let at = low + length * (END_MARGIN + (1.0 - 2.0 * END_MARGIN) * share);
        let origin = centre + direction * (at - centre.dot(direction));
        let plane = Plane { origin, x, y };
        let runs = crate::section::section_welded(scan, &weld, &plane);
        let length_of = |p: &[Vec2], closed: bool| {
            let n = p.len();
            (0..n.saturating_sub(usize::from(!closed))).map(|i| p[i].dist(p[(i + 1) % n])).sum::<f64>()
        };
        let longest = runs.iter().map(|r| length_of(&r.points, r.closed)).fold(0.0, f64::max);
        let kept: Vec<_> = runs.iter().filter(|r| length_of(&r.points, r.closed) > SPECK * longest).collect();
        let [only] = kept.as_slice() else {
            return Err(format!(
                "the slice {:.0}% along the part is {} loops, not one: the part is not one section swept smoothly along it",
                100.0 * share,
                kept.len()
            ));
        };
        if !only.closed || only.points.len() < 3 {
            return Err(format!("the slice {:.0}% along the part does not close round: fill the scan's holes first", 100.0 * share));
        }
        let mut ring = only.points.clone();
        if signed_area(&ring) < 0.0 {
            ring.reverse();
        }
        let m = middle(&ring);
        // Start where the one before started, as seen from the middle; the first at its furthest
        // point along x.
        let toward = previous.unwrap_or(Vec2::new(1.0, 0.0));
        let start = (0..ring.len()).max_by(|&a, &b| (ring[a] - m).dot(toward).total_cmp(&(ring[b] - m).dot(toward))).unwrap_or(0);
        let laid = resampled(&ring, start, POINTS);
        previous = (laid[0] - m).normalized();
        // The ends' sections lie at the end faces, when there are such.
        let plane = if capped.is_some() && (k == 0 || k + 1 == count) {
            let end = if k == 0 { low } else { high };
            Plane { origin: centre + direction * (end - centre.dot(direction)), x, y }
        } else {
            plane
        };
        sections.push(Section { plane, points: laid });
    }
    Ok(Loft { direction, capped: capped.is_some(), sections })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from `lo` to `hi`, as twelve triangles wound outward.
    fn cuboid(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let positions = (0..8).map(p).collect();
        let triangles =
            vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        (positions, triangles)
    }

    #[test]
    fn a_bar_is_lofted_along_its_length_between_its_end_faces() {
        let (positions, triangles) = cuboid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 6.0, 80.0));
        let scan = Scan::new(&positions, &triangles);
        let ends = [
            Face { shape: Shape::Plane { point: Vec3::new(5.0, 3.0, 0.0), normal: -Vec3::Z }, area: 60.0 },
            Face { shape: Shape::Plane { point: Vec3::new(5.0, 3.0, 80.0), normal: Vec3::Z }, area: 60.0 },
        ];
        let loft = read(&scan, &ends, 4).unwrap();
        assert!(loft.capped && loft.direction.z.abs() > 0.999, "{loft:?}");
        assert_eq!(loft.sections.len(), 4);
        let levels: Vec<f64> = loft.sections.iter().map(|s| s.plane.origin.z).collect();
        assert!((levels[0] - 0.0).abs() < 1e-9 || (levels[0] - 80.0).abs() < 1e-9, "{levels:?}");
        for s in &loft.sections {
            assert_eq!(s.points.len(), POINTS);
            assert!((signed_area(&s.points) - 60.0).abs() < 1.0, "{}", signed_area(&s.points));
        }
        // Without end faces, along its longest direction, 1% in from each end.
        let free = read(&scan, &[], 3).unwrap();
        assert!(!free.capped && free.direction.z.abs() > 0.999, "{free:?}");
    }

    #[test]
    fn two_bars_side_by_side_are_no_loft() {
        let (mut positions, mut triangles) = cuboid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 80.0));
        let (more, tris) = cuboid(Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 4.0, 80.0));
        triangles.extend(tris.iter().map(|t| t.map(|i| i + 8)));
        positions.extend(more);
        let scan = Scan::new(&positions, &triangles);
        assert!(read(&scan, &[], 3).is_err());
    }
}
