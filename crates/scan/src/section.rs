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

/// How many steps the stretch of the axis that closes a half-section is walked in, so that a
/// fitter of the outline sees it as the straight run it is.
const AXIS_STEPS: usize = 12;

/// One side of a cut through a turned part's axis: the parts of `runs` (in a plane whose X runs
/// along the axis, Y the distance from it) with `side` × y at least 0, each as points and whether
/// it is closed. A loop that crosses the axis is cut there and closed along it, from where it
/// comes back to the axis to where it left (a solid part's half profile, ready to revolve); a loop
/// wholly on the side is kept as it is (a ring's section); an open run gives its stretches on the
/// side, closed along the axis when both ends come down to it (within `reach`: a scan's last
/// triangles stop short of it), else left open.
pub fn half(runs: &[Run], side: f64, reach: f64) -> Vec<(Vec<Vec2>, bool)> {
    let side = if side < 0.0 { -1.0 } else { 1.0 };
    let mut out = Vec::new();
    // A stretch closed along the axis: the axis walked from its last point back to its first.
    let along_axis = |mut points: Vec<Vec2>| {
        let (Some(&first), Some(&last)) = (points.first(), points.last()) else { return points };
        for i in 1..AXIS_STEPS {
            points.push(Vec2::new(last.x + (first.x - last.x) * i as f64 / AXIS_STEPS as f64, 0.0));
        }
        points
    };
    let crossing = |p: Vec2, q: Vec2| Vec2::new(p.x + (q.x - p.x) * (p.y / (p.y - q.y)), 0.0);
    for run in runs {
        let points: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p.x, side * p.y)).collect();
        let n = points.len();
        if n < 2 {
            continue;
        }
        if run.closed {
            if points.iter().all(|p| p.y >= 0.0) {
                out.push((points, true));
                continue;
            }
            // From a point below the axis, round once: each stretch above it, crossing to crossing.
            let Some(start) = points.iter().position(|p| p.y < 0.0) else { continue };
            let mut kept: Vec<Vec2> = Vec::new();
            for k in 0..n {
                let (p, q) = (points[(start + k) % n], points[(start + k + 1) % n]);
                if p.y >= 0.0 {
                    kept.push(p);
                }
                if (p.y >= 0.0) != (q.y >= 0.0) {
                    kept.push(crossing(p, q));
                    if p.y >= 0.0 {
                        if kept.len() >= 3 {
                            out.push((along_axis(std::mem::take(&mut kept)), true));
                        }
                        kept.clear();
                    }
                }
            }
            continue;
        }
        let mut kept: Vec<Vec2> = Vec::new();
        let mut finish = |mut kept: Vec<Vec2>| {
            if kept.len() < 2 {
                return;
            }
            // An end that stops within reach of the axis is taken down to it.
            let last = kept.len() - 1;
            for end in [0, last] {
                if kept[end].y > 0.0 && kept[end].y <= reach {
                    kept[end].y = 0.0;
                }
            }
            if kept[0].y == 0.0 && kept[last].y == 0.0 && kept.len() >= 3 {
                out.push((along_axis(kept), true));
            } else {
                out.push((kept, false));
            }
        };
        for k in 0..n {
            let p = points[k];
            if p.y >= 0.0 {
                kept.push(p);
            }
            let Some(&q) = points.get(k + 1) else { continue };
            if (p.y >= 0.0) != (q.y >= 0.0) {
                kept.push(crossing(p, q));
                if p.y >= 0.0 {
                    finish(std::mem::take(&mut kept));
                }
            }
        }
        finish(kept);
    }
    out
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
    /// A cup's section through its axis (one loop round the thickness of its wall and bottom,
    /// crossing the axis twice): one side of it is the wall's half, closed along the axis through
    /// the bottom's thickness. A ring's section (two loops, neither on the axis) keeps the loop on
    /// that side. An open run that comes down to the axis at both ends is closed along it; one
    /// that does not is left open.
    #[test]
    fn one_side_of_a_cut_through_an_axis_is_closed_along_it() {
        use super::*;
        let loop_of = |points: &[[f64; 2]]| Run {
            points: points.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
            triangles: vec![0; points.len()],
            closed: true,
        };
        // Height along x, radius y: bottom from x 0 to 3 thick, wall from radius 27 to 30, 60 high.
        let cup = loop_of(&[[0.0, -30.0], [0.0, 30.0], [60.0, 30.0], [60.0, 27.0], [3.0, 27.0], [3.0, -27.0], [60.0, -27.0], [60.0, -30.0]]);
        for side in [1.0, -1.0] {
            let found = half(std::slice::from_ref(&cup), side, 0.1);
            assert_eq!(found.len(), 1, "{found:?}");
            let (points, closed) = &found[0];
            assert!(*closed && points.iter().all(|p| p.y >= 0.0), "{points:?}");
            // The wall's corners are all there, and the axis from the inside of the bottom to its outside.
            for corner in [[0.0, 30.0], [60.0, 30.0], [60.0, 27.0], [3.0, 27.0], [3.0, 0.0], [0.0, 0.0]] {
                assert!(points.iter().any(|p| p.dist(Vec2::new(corner[0], corner[1])) < 1e-9), "{corner:?} of {points:?}");
            }
            let on_axis = points.iter().filter(|p| p.y == 0.0).count();
            assert!(on_axis > 2, "the axis is walked, not jumped: {points:?}");
            let twice_area: f64 = (0..points.len()).map(|i| points[i].cross(points[(i + 1) % points.len()])).sum();
            assert!((twice_area.abs() / 2.0 - (60.0 * 3.0 + 3.0 * 27.0)).abs() < 1e-9, "the wall's half section: {twice_area}");
        }
        let ring =
            [loop_of(&[[0.0, 10.0], [5.0, 10.0], [5.0, 14.0], [0.0, 14.0]]), loop_of(&[[0.0, -10.0], [0.0, -14.0], [5.0, -14.0], [5.0, -10.0]])];
        let found = half(&ring, 1.0, 0.1);
        assert!(found.len() == 1 && found[0].1 && found[0].0.len() == 4 && found[0].0.iter().all(|p| p.y >= 10.0), "{found:?}");
        // A cone's open skin: from near the axis out and back down to near it.
        let open = |points: &[[f64; 2]]| Run {
            points: points.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
            triangles: vec![0; points.len()],
            closed: false,
        };
        let skin = open(&[[0.0, 0.05], [0.0, 20.0], [30.0, 10.0], [30.0, 0.02]]);
        let found = half(std::slice::from_ref(&skin), 1.0, 0.1);
        assert!(found.len() == 1 && found[0].1 && found[0].0[0] == Vec2::new(0.0, 0.0), "taken down to the axis and closed: {found:?}");
        let short = open(&[[0.0, 5.0], [0.0, 20.0], [30.0, 10.0]]);
        let found = half(std::slice::from_ref(&short), 1.0, 0.1);
        assert!(found.len() == 1 && !found[0].1 && found[0].0.len() == 3, "{found:?}");
    }

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
