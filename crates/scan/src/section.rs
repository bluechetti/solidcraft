//! Where a plane cuts the scan: each triangle the plane passes through gives a segment between the
//! two edges it crosses, and the segments are joined through those edges (points at one place
//! counted as one) into runs, closed where they come back round. The runs are in the plane's own
//! coordinates, to sketch from.
//!
//! Sections along a curve (SurfaceCraft's Sections Along Curve): planes square to a path at even
//! steps along it, from end to end of an open path or round a closed one, so a bent part (a
//! handle, a pipe, a rib) gets cross sections that follow it. Each plane's axes are carried along
//! the path from the one before (turned with the path, never spun about it), so sections lofted
//! in order do not twist. Of each cut the loop nearest the path is kept, with the loops inside it
//! (a pipe's bore), or every loop.

use std::collections::HashMap;

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::{Scan, par, welded};

/// One run of the cut, in the plane's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub points: Vec<Vec2>,
    /// For each point, a triangle of the scan the run passes through there (the one it comes in
    /// by, for the first point the one it leaves by): what part of the scan the point is on.
    pub triangles: Vec<usize>,
    /// It comes back round to where it began (the first point is not repeated).
    pub closed: bool,
}

fn pair(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// The runs where `plane` cuts the scan, longest first.
pub fn section(scan: &Scan, plane: &Plane) -> Vec<Run> {
    cut(scan, &welded(scan.positions), plane)
}

/// [`section`] with the scan's points already welded (`weld`, as [`crate::welded`] gives them),
/// for many planes through one scan.
pub(crate) fn section_welded(scan: &Scan, weld: &[u32], plane: &Plane) -> Vec<Run> {
    cut(scan, weld, plane)
}

/// [`section`] with the scan's points already welded.
fn cut(scan: &Scan, weld: &[u32], plane: &Plane) -> Vec<Run> {
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
    let mut joined: HashMap<u64, Vec<(u64, usize)>> = HashMap::new();
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
        joined.entry(k1).or_default().push((k2, t));
        joined.entry(k2).or_default().push((k1, t));
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
        let mut keys = vec![(first, joined.get(&first).and_then(|j| j.first()).map_or(0, |j| j.1))];
        used.insert(first, true);
        let mut current = first;
        let closed = loop {
            let next = joined.get(&current).and_then(|j| j.iter().copied().find(|(k, _)| !used.get(k).copied().unwrap_or(false)));
            match next {
                Some((k, t)) => {
                    used.insert(k, true);
                    keys.push((k, t));
                    current = k;
                }
                None => break keys.len() > 2 && joined.get(&current).is_some_and(|j| j.iter().any(|(k, _)| *k == first)),
            }
        };
        let (points, triangles): (Vec<Vec2>, Vec<usize>) = keys.iter().filter_map(|(k, t)| at.get(k).map(|p| (plane.to_local(*p), *t))).unzip();
        if points.len() >= 2 {
            runs.push(Run { points, triangles, closed });
        }
    }
    let length = |r: &Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    runs.sort_by(|a, b| length(b).total_cmp(&length(a)));
    runs
}

/// A place along a path where a section is cut: the plane square to the path there, its origin
/// on the path and its axes carried along from the station before.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub plane: Plane,
    /// How far along the path, in mm.
    pub along: f64,
}

impl Station {
    /// The way the path goes here: the plane's normal.
    pub fn tangent(&self) -> Vec3 {
        self.plane.normal()
    }
}

/// Whether a path comes back round to where it began.
pub fn is_closed(path: &[Vec3]) -> bool {
    let length: f64 = path.windows(2).map(|w| w[0].dist(w[1])).sum();
    path.len() >= 4 && path.first().zip(path.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6 * length.max(1.0))
}

/// `count` stations evenly along `path` by length: from end to end of an open path (its middle
/// for one), round a closed one from its start. None for a path with no length.
pub fn stations(path: &[Vec3], count: usize) -> Vec<Station> {
    let mut along = vec![0.0];
    for w in path.windows(2) {
        along.push(along[along.len() - 1] + w[0].dist(w[1]));
    }
    let length = along[along.len() - 1];
    if !(length > 0.0) || count == 0 || path.len() < 2 {
        return Vec::new();
    }
    let closed = is_closed(path);
    // The direction at a distance along: the segment's there, or across a joint, both sides'.
    let tangent_at = |k: usize| -> Option<Vec3> {
        let d = path[k + 1] - path[k];
        d.normalized()
    };
    let mut stations: Vec<Station> = Vec::with_capacity(count);
    let mut x: Option<Vec3> = None;
    let mut segment = 0;
    for i in 0..count {
        let s = if closed {
            length * i as f64 / count as f64
        } else if count == 1 {
            length / 2.0
        } else {
            length * i as f64 / (count - 1) as f64
        };
        while segment + 2 < along.len() && along[segment + 1] < s {
            segment += 1;
        }
        let span = along[segment + 1] - along[segment];
        let t = if span > 0.0 { ((s - along[segment]) / span).clamp(0.0, 1.0) } else { 0.0 };
        let point = path[segment] + (path[segment + 1] - path[segment]) * t;
        // On a joint, square to the segments either side.
        let here = (segment..path.len() - 1).find_map(tangent_at).or_else(|| (0..segment).rev().find_map(tangent_at));
        let beside = if t >= 1.0 && segment + 2 < path.len() {
            (segment + 1..path.len() - 1).find_map(tangent_at)
        } else if t <= 0.0 && segment > 0 {
            (0..segment).rev().find_map(tangent_at)
        } else if t <= 0.0 && closed {
            (0..path.len() - 1).rev().find_map(tangent_at)
        } else {
            None
        };
        let tangent = match (here, beside) {
            (Some(h), Some(b)) => (h + b).normalized().unwrap_or(h),
            (Some(h), None) => h,
            (None, _) => continue,
        };
        // The x axis: the last one with its part along the new tangent taken away, else any
        // direction square to the tangent.
        let carried = x.and_then(|x| (x - tangent * x.dot(tangent)).normalized());
        let any = || {
            let pick = if tangent.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            (pick - tangent * pick.dot(tangent)).normalized()
        };
        let Some(axis) = carried.or_else(any) else { continue };
        let Some(y) = tangent.cross(axis).normalized() else { continue };
        x = Some(axis);
        stations.push(Station { plane: Plane { origin: point, x: axis, y }, along: s });
    }
    stations
}

/// One section along a path: its station and the runs of the cut kept, in its plane's
/// coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Along {
    pub station: Station,
    pub runs: Vec<Run>,
}

/// Whether `q` is inside the closed `ring` (even-odd).
fn inside(q: Vec2, ring: &[Vec2]) -> bool {
    let mut odd = false;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (b.x - a.x) * (q.y - a.y) / (b.y - a.y) {
            odd = !odd;
        }
    }
    odd
}

fn area(r: &Run) -> f64 {
    let n = r.points.len();
    (0..n).map(|k| r.points[k].x * r.points[(k + 1) % n].y - r.points[(k + 1) % n].x * r.points[k].y).sum::<f64>().abs() / 2.0
}

/// Of a section's runs, the loop round the path's point (the plane's origin) with the loops inside
/// it: inside an odd number of loops the point is in the material and the innermost is its
/// outline; inside an even number it is in a hole (a pipe's bore), whose outline is the next one
/// out. Round none, the run nearest the point within `reach`.
fn nearest(runs: Vec<Run>, reach: f64) -> Vec<Run> {
    let centre = Vec2::new(0.0, 0.0);
    let mut round: Vec<usize> = (0..runs.len()).filter(|&k| runs[k].closed && inside(centre, &runs[k].points)).collect();
    round.sort_by(|&a, &b| area(&runs[a]).total_cmp(&area(&runs[b])));
    let outline = match round.len() {
        0 => {
            let away = |r: &Run| r.points.iter().map(|p| p.dist(centre)).fold(f64::INFINITY, f64::min);
            let near = (0..runs.len()).map(|k| (k, away(&runs[k]))).filter(|(_, d)| *d <= reach).min_by(|a, b| a.1.total_cmp(&b.1));
            match near {
                Some((k, _)) => k,
                None => return Vec::new(),
            }
        }
        n if n % 2 == 1 => round[0],
        _ => round[1],
    };
    let outer = &runs[outline];
    let holes = (0..runs.len())
        .filter(|&k| k != outline && outer.closed && runs[k].closed && runs[k].points.first().is_some_and(|q| inside(*q, &outer.points)));
    std::iter::once(outline).chain(holes).map(|k| runs[k].clone()).collect()
}

/// The sections of the scan square to `path` at `count` stations along it: of each cut the loop
/// round the path, with the loops inside it, or the run nearest the path within `reach`
/// ([`nearest`]); or, with `every`, every run the plane cuts. A station whose plane cuts nothing
/// kept has no runs.
pub fn along(scan: &Scan, path: &[Vec3], count: usize, every: bool, reach: f64) -> Vec<Along> {
    let stations = stations(path, count);
    if stations.is_empty() {
        return Vec::new();
    }
    let weld = welded(scan.positions);
    par::map(stations.len(), |i| {
        let station = stations[i];
        let mut runs = cut(scan, &weld, &station.plane);
        if !every {
            runs = nearest(runs, reach);
        }
        Along { station, runs }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, revolved};

    /// A ring (torus) about Z: its core a circle of radius 30, its tube of radius 5.
    fn ring() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let tube: Vec<[f64; 2]> = (0..48)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / 48.0;
                [5.0 * a.sin(), 30.0 + 5.0 * a.cos()]
            })
            .collect();
        revolved(&tube, 96, true)
    }

    fn core(n: usize) -> Vec<Vec3> {
        (0..=n)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / n as f64;
                Vec3::new(30.0 * a.cos(), 30.0 * a.sin(), 0.0)
            })
            .collect()
    }

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

    /// Sections along the core of a ring are each the tube's circle, round the core, in a plane
    /// through the ring's axis; the planes' axes turn with the core, never about it.
    #[test]
    fn sections_along_a_rings_core_are_its_tube() {
        let (positions, triangles) = ring();
        let path = core(200);
        assert!(is_closed(&path));
        let made = along(&Scan::new(&positions, &triangles), &path, 8, false, 10.0);
        assert_eq!(made.len(), 8, "round the closed core");
        for a in &made {
            assert_eq!(a.runs.len(), 1, "one loop at {}", a.station.along);
            let run = &a.runs[0];
            assert!(run.closed);
            let worst = run.points.iter().map(|p| (p.len() - 5.0).abs()).fold(0.0, f64::max);
            assert!(worst < 0.1, "a circle of radius 5 round the core: {worst}");
            // The plane holds the axis: its normal is square to Z, its x stays level or upright.
            assert!(a.station.tangent().z.abs() < 1e-9);
            assert!(a.station.plane.origin.dist(Vec3::new(a.station.plane.origin.x, a.station.plane.origin.y, 0.0)) < 1e-9);
            assert!(a.station.plane.x.z.abs() < 1e-6 || a.station.plane.x.z.abs() > 1.0 - 1e-6, "no twist: {:?}", a.station.plane.x);
        }
    }

    /// A pipe cut along its bore keeps the wall and the bore as one section; a path off to the
    /// side within reach keeps the nearest wall; a path far away cuts nothing kept, but every
    /// loop is still there to ask for.
    #[test]
    fn a_pipe_keeps_its_bore_and_a_far_path_keeps_nothing() {
        // A pipe up Z: outside radius 10, bore 6, from 0 to 40.
        let profile = [[0.0, 6.0], [0.0, 10.0], [40.0, 10.0], [40.0, 6.0]];
        let (positions, triangles) = revolved(&profile, 64, true);
        let scan = Scan::new(&positions, &triangles);
        let up: Vec<Vec3> = vec![Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 45.0)];
        let made = along(&scan, &up, 4, false, 5.0);
        let cut: Vec<&Along> = made.iter().filter(|a| !a.runs.is_empty()).collect();
        assert_eq!(cut.len(), 2, "the two stations inside the pipe's length");
        for a in cut {
            let mut radii: Vec<f64> = a.runs.iter().map(|r| r.points.iter().map(|p| p.len()).sum::<f64>() / r.points.len() as f64).collect();
            radii.sort_by(f64::total_cmp);
            assert_eq!(radii.len(), 2, "the wall and its bore");
            assert!((radii[0] - 6.0).abs() < 0.1 && (radii[1] - 10.0).abs() < 0.1, "{radii:?}");
        }
        let far: Vec<Vec3> = vec![Vec3::new(100.0, 0.0, 10.0), Vec3::new(100.0, 0.0, 30.0)];
        assert!(along(&scan, &far, 3, false, 5.0).iter().all(|a| a.runs.is_empty()));
        // Square to Z beside the pipe, every loop.
        let beside: Vec<Vec3> = vec![Vec3::new(30.0, 0.0, 10.0), Vec3::new(30.0, 0.0, 30.0)];
        assert!(along(&scan, &beside, 3, true, 5.0).iter().all(|a| a.runs.len() == 2));
    }

    /// A path with no length, or no stations asked for, gives none; a bent path's stations are
    /// square to it either side of the bend and their axes carried round it.
    #[test]
    fn stations_follow_a_bent_path() {
        assert!(stations(&[Vec3::ZERO, Vec3::ZERO], 4).is_empty());
        assert!(stations(&[Vec3::ZERO, Vec3::X], 0).is_empty());
        let bent = [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0)];
        let made = stations(&bent, 5);
        assert_eq!(made.len(), 5);
        assert!(made[0].tangent().dist(Vec3::X) < 1e-9 && made[4].tangent().dist(Vec3::Y) < 1e-9);
        assert!(made[2].tangent().dist(Vec3::new(1.0, 1.0, 0.0).normalized().unwrap()) < 1e-9, "square to both at the bend");
        for st in &made {
            assert!(st.plane.x.dot(st.tangent()).abs() < 1e-9 && (st.plane.x.len() - 1.0).abs() < 1e-9);
            assert!(st.plane.x.z.abs() < 1e-9 || st.plane.x.z.abs() > 1.0 - 1e-9, "turned with the path in its plane");
        }
        assert!((made[4].along - 20.0).abs() < 1e-9 && made[0].along == 0.0);
    }
}
