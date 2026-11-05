//! Where a plane cuts the scan: each triangle the plane passes through gives a segment between the
//! two edges it crosses, and the segments are joined through those edges (points at one place
//! counted as one) into runs, closed where they come back round. The runs are in the plane's own
//! coordinates, to sketch from.

use std::collections::HashMap;

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::{Scan, welded};

/// One run of the cut, in the plane's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub points: Vec<Vec2>,
    /// It comes back round to where it began (the first point is not repeated).
    pub closed: bool,
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// The runs where `plane` cuts the scan, longest first.
pub fn section(scan: &Scan, plane: &Plane) -> Vec<Run> {
    let weld = welded(scan.positions);
    let height = |v: u32| {
        let h = plane.height(scan.positions[v as usize]);
        // A point on the plane counts as just above it, so each crossing is on an edge between sides.
        if h == 0.0 { f64::MIN_POSITIVE } else { h }
    };
    let crossing = |a: u32, b: u32| -> Vec3 {
        let (ha, hb) = (height(a), height(b));
        let (pa, pb) = (scan.positions[a as usize], scan.positions[b as usize]);
        pa + (pb - pa) * (ha / (ha - hb))
    };
    // Each crossed edge, where it is crossed, and the (at most two) edges joined to it by a triangle.
    let mut at: HashMap<u64, Vec3> = HashMap::new();
    let mut joined: HashMap<u64, Vec<u64>> = HashMap::new();
    for t in 0..scan.triangle_count() {
        if scan.corners(t).is_none() {
            continue;
        }
        let [a, b, c] = scan.triangles[t].map(|i| weld[i as usize]);
        if a == b || b == c || c == a {
            continue;
        }
        let crossed: Vec<(u32, u32)> = [(a, b), (b, c), (c, a)].into_iter().filter(|&(p, q)| (height(p) > 0.0) != (height(q) > 0.0)).collect();
        let [(p1, q1), (p2, q2)] = crossed[..] else { continue };
        let (k1, k2) = (pair(p1, q1), pair(p2, q2));
        at.entry(k1).or_insert_with(|| crossing(p1.min(q1), p1.max(q1)));
        at.entry(k2).or_insert_with(|| crossing(p2.min(q2), p2.max(q2)));
        joined.entry(k1).or_default().push(k2);
        joined.entry(k2).or_default().push(k1);
    }
    // Walk the joins: from an end (an edge with one join) first, so open runs come out whole,
    // then round what is left, which closes.
    let mut used: HashMap<u64, bool> = HashMap::new();
    let mut runs = Vec::new();
    let mut starts: Vec<u64> = joined.iter().filter(|(_, j)| j.len() == 1).map(|(k, _)| *k).collect();
    starts.sort_unstable();
    let mut rest: Vec<u64> = joined.keys().copied().collect();
    rest.sort_unstable();
    for first in starts.into_iter().chain(rest) {
        if used.get(&first).copied().unwrap_or(false) {
            continue;
        }
        let mut keys = vec![first];
        used.insert(first, true);
        let mut current = first;
        let closed = loop {
            let next = joined.get(&current).and_then(|j| j.iter().copied().find(|k| !used.get(k).copied().unwrap_or(false)));
            match next {
                Some(k) => {
                    used.insert(k, true);
                    keys.push(k);
                    current = k;
                }
                None => break keys.len() > 2 && joined.get(&current).is_some_and(|j| j.contains(&first)),
            }
        };
        let points: Vec<Vec2> = keys.iter().filter_map(|k| at.get(k)).map(|p| plane.to_local(*p)).collect();
        if points.len() >= 2 {
            runs.push(Run { points, closed });
        }
    }
    let length = |r: &Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    runs.sort_by(|a, b| length(b).total_cmp(&length(a)));
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A box cut half way up is one closed run round its four sides, in the plane's coordinates.
    #[test]
    fn a_box_cut_across_is_one_closed_square() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 4);
        let plane = Plane::from_normal(Vec3::new(0.0, 0.0, 5.0), Vec3::Z).unwrap();
        let runs = section(&Scan::new(&positions, &triangles), &plane);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].closed);
        let length: f64 = runs[0].points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() + runs[0].points[0].dist(*runs[0].points.last().unwrap());
        assert!((length - 100.0).abs() < 1e-9, "the box's perimeter: {length}");
        let (lo, hi) = runs[0].points.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
            (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
        });
        assert!(lo.dist(Vec2::new(0.0, 0.0)) < 1e-9 && hi.dist(Vec2::new(30.0, 20.0)) < 1e-9, "{lo:?} {hi:?}");
        let above = Plane::from_normal(Vec3::new(0.0, 0.0, 50.0), Vec3::Z).unwrap();
        assert!(section(&Scan::new(&positions, &triangles), &above).is_empty());
    }
}
