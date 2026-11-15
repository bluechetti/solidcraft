//! Reference markers on a scan: the small spheres a structured-light or photogrammetry scan is
//! laid together by, which the part does not have. The one clicked is fitted as a sphere; the
//! others of its radius are found by voting (each triangle, stepped one radius in along its
//! normal, votes for a centre: the Hough transform for spheres), and each candidate is kept only
//! when the scan holds a cap of that sphere about as large as a share of the clicked one's. A round
//! of the same radius along an edge spreads its votes along a line, so it does not pass.
//!
//! [`remove`] takes each marker's cap off with a thin band round it and closes the holes it leaves
//! with [`crate::holes`]' patches, so the surface the marker stood on carries on under it.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::fit::{self, Kind, Shape};

/// The markers found: their radius and centres, the clicked one first.
#[derive(Clone, Debug, PartialEq)]
pub struct Markers {
    pub radius: f64,
    pub centres: Vec<Vec3>,
    /// How far a point may be off a marker's sphere and still be on it, mm.
    pub tolerance: f64,
}

/// Why no markers were found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerError {
    /// The click is not on the scan.
    NotOnTheScan,
    /// The scan there is not a sphere.
    NotASphere,
}

impl std::fmt::Display for MarkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MarkerError::NotOnTheScan => "click a marker on the scan",
            MarkerError::NotASphere => "the scan there is not a sphere: click the middle of a marker",
        })
    }
}

/// A candidate needs at least this share of the clicked marker's cap to be a marker.
const LEAST_SHARE: f64 = 0.4;

/// How far a triangle's direction may lean off the sphere's there and still be on it.
const MAX_LEAN_COS: f64 = 0.906; // cos 25°

/// The markers like the one at `at`: its sphere fitted there, then every other cap of that radius.
/// `tolerance` (mm) is how far a point may be off the sphere (default: the fit's).
pub fn find(scan: &Scan, at: Vec3, tolerance: Option<f64>) -> Result<Markers, MarkerError> {
    let seed = crate::base::nearest_triangle(scan, at).ok_or(MarkerError::NotOnTheScan)?;
    let fitted = fit::fit(scan, seed, Kind::Sphere, tolerance).map_err(|_| MarkerError::NotASphere)?;
    let Shape::Sphere { center, radius } = fitted.shape else { return Err(MarkerError::NotASphere) };
    if !(radius.is_finite() && radius > 0.0) {
        return Err(MarkerError::NotASphere);
    }
    let tolerance = tolerance.filter(|t| t.is_finite() && *t > 0.0).unwrap_or_else(|| (3.0 * fitted.rms).max(0.03 * radius));
    let facets = Facets::of(scan);
    let clicked = facets.cap(center, radius, tolerance);
    if clicked <= 0.0 {
        return Err(MarkerError::NotASphere);
    }
    // Votes, weighted by area, in cells half a radius across.
    let cell = radius / 2.0;
    let key = |c: Vec3| [(c.x / cell).floor() as i64, (c.y / cell).floor() as i64, (c.z / cell).floor() as i64];
    let mut votes: HashMap<[i64; 3], (Vec3, f64)> = HashMap::new();
    for f in &facets.all {
        let c = f.middle - f.normal * radius;
        let v = votes.entry(key(c)).or_insert((Vec3::ZERO, 0.0));
        v.0 += c * f.area;
        v.1 += f.area;
    }
    let mut cells: Vec<(Vec3, f64)> = votes.into_values().filter(|(_, w)| *w > 0.0).map(|(s, w)| (s / w, w)).collect();
    cells.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut centres = vec![center];
    for (start, weight) in cells {
        // A cell with too few votes cannot hold a marker's cap; nor can any after it.
        if weight < 0.25 * LEAST_SHARE * clicked {
            break;
        }
        if centres.iter().any(|c| c.dist(start) < 1.5 * radius) {
            continue;
        }
        let Some(c) = facets.settle(start, radius, tolerance) else { continue };
        if centres.iter().any(|k| k.dist(c) < 1.5 * radius) {
            continue;
        }
        if facets.cap(c, radius, tolerance) >= LEAST_SHARE * clicked {
            centres.push(c);
        }
    }
    Ok(Markers { radius, centres, tolerance })
}

/// The scan with the markers at `centres` (of `radius`) taken off and the holes they leave closed
/// (`fair`: shaped to carry the surface across): the positions, the triangles, how many triangles
/// went and how many holes were closed.
pub fn remove(scan: &Scan, centres: &[Vec3], radius: f64, fair: bool) -> Option<Removed> {
    if !(radius.is_finite() && radius > 0.0) || centres.is_empty() {
        return None;
    }
    // The cap and a thin band round it, where the marker meets what it stands on.
    let reach = 1.15 * radius;
    let goes = |t: usize| scan.corners(t).is_some_and(|[a, b, c]| centres.iter().any(|m| ((a + b + c) / 3.0).dist(*m) <= reach));
    let removed = (0..scan.triangle_count()).filter(|&t| goes(t)).count();
    if removed == 0 {
        return None;
    }
    let (positions, triangles) = crate::pieces::keeping(scan, |t| !goes(t));
    let left = Scan::new(&positions, &triangles);
    let survey = crate::holes::survey(&left);
    // Only the holes the markers left: a ring passing within the band of a marker.
    let near = |r: &crate::holes::Ring| {
        !r.rim && r.points.iter().filter_map(|&i| positions.get(i as usize)).any(|p| centres.iter().any(|m| p.dist(*m) <= 1.6 * radius))
    };
    let rings: Vec<&crate::holes::Ring> = survey.rings.iter().filter(|r| near(r)).collect();
    if rings.is_empty() {
        return Some(Removed { positions, triangles, removed, filled: 0, failed: 0 });
    }
    match crate::holes::fill_rings(&left, &rings, fair) {
        Some(done) => Some(Removed { positions: done.positions, triangles: done.triangles, removed, filled: done.filled, failed: done.failed }),
        None => Some(Removed { failed: rings.len(), positions, triangles, removed, filled: 0 }),
    }
}

/// A scan with its markers taken off.
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Triangles taken off with the markers.
    pub removed: usize,
    /// Holes closed, and holes that could not be.
    pub filled: usize,
    pub failed: usize,
}

/// A triangle's middle, outward direction and area.
struct Facet {
    middle: Vec3,
    normal: Vec3,
    area: f64,
}

struct Facets {
    all: Vec<Facet>,
}

impl Facets {
    fn of(scan: &Scan) -> Facets {
        let all = (0..scan.triangle_count())
            .filter_map(|t| {
                let [a, b, c] = scan.corners(t)?;
                let n = (b - a).cross(c - a);
                let area = 0.5 * n.len();
                let normal = n.normalized()?;
                Some(Facet { middle: (a + b + c) / 3.0, normal, area })
            })
            .collect();
        Facets { all }
    }

    /// The triangles on the sphere at `c` of `radius`: within the tolerance of it, facing out of it.
    fn on(&self, c: Vec3, radius: f64, tolerance: f64) -> impl Iterator<Item = &Facet> {
        self.all.iter().filter(move |f| {
            let v = f.middle - c;
            let d = v.len();
            d > 0.0 && (d - radius).abs() <= tolerance && f.normal.dot(v / d) >= MAX_LEAN_COS
        })
    }

    /// The area of the scan on that sphere.
    fn cap(&self, c: Vec3, radius: f64, tolerance: f64) -> f64 {
        self.on(c, radius, tolerance).map(|f| f.area).sum()
    }

    /// A centre moved from `start` until the triangles on its sphere (the radius held) agree with
    /// it: each pulls it to one radius in from itself, weighted by area. `None` when nothing is on it.
    fn settle(&self, start: Vec3, radius: f64, tolerance: f64) -> Option<Vec3> {
        let mut c = start;
        // Wide at first (a vote's centre can be a fraction of a radius off), then the tolerance.
        for reach in [0.5 * radius, 0.25 * radius, tolerance, tolerance, tolerance] {
            let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
            for f in self.on(c, radius, reach.max(tolerance)) {
                let Some(out) = (f.middle - c).normalized() else { continue };
                sum += (f.middle - out * radius) * f.area;
                weight += f.area;
            }
            if weight <= 0.0 {
                return None;
            }
            c = sum / weight;
        }
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere of `radius` at `c` as a closed mesh, wound outward.
    fn ball(c: Vec3, radius: f64, p: &mut Vec<Vec3>, t: &mut Vec<[u32; 3]>) {
        let (rings, segments) = (24u32, 48u32);
        let base = p.len() as u32;
        for i in 0..=rings {
            let phi = std::f64::consts::PI * f64::from(i) / f64::from(rings);
            for j in 0..segments {
                let th = 2.0 * std::f64::consts::PI * f64::from(j) / f64::from(segments);
                p.push(c + Vec3::new(phi.sin() * th.cos(), phi.sin() * th.sin(), phi.cos()) * radius);
            }
        }
        let at = |i: u32, j: u32| base + i * segments + j % segments;
        for i in 0..rings {
            for j in 0..segments {
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }

    /// A flat grid on z = 0 from 0 to `size`, facing up.
    fn floor(size: f64, n: u32, p: &mut Vec<Vec3>, t: &mut Vec<[u32; 3]>) {
        let base = p.len() as u32;
        for i in 0..=n {
            for j in 0..=n {
                p.push(Vec3::new(size * f64::from(i) / f64::from(n), size * f64::from(j) / f64::from(n), 0.0));
            }
        }
        let at = |i: u32, j: u32| base + i * (n + 1) + j;
        for i in 0..n {
            for j in 0..n {
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }

    /// A floor with three markers of radius 3 sunk half into it and one ball of radius 6: the one
    /// clicked finds the other two of its size, not the larger ball.
    #[test]
    fn a_clicked_marker_finds_the_others_of_its_size() {
        let (mut p, mut t) = (Vec::new(), Vec::new());
        floor(100.0, 100, &mut p, &mut t);
        for c in [Vec3::new(20.0, 20.0, 0.0), Vec3::new(70.0, 30.0, 0.0), Vec3::new(40.0, 80.0, 0.0)] {
            ball(c, 3.0, &mut p, &mut t);
        }
        ball(Vec3::new(80.0, 80.0, 0.0), 6.0, &mut p, &mut t);
        let scan = Scan::new(&p, &t);
        let found = find(&scan, Vec3::new(20.0, 20.0, 3.0), None).unwrap();
        assert!((found.radius - 3.0).abs() < 0.05, "{found:?}");
        assert_eq!(found.centres.len(), 3, "the three markers, not the larger ball: {found:?}");
        for want in [Vec3::new(70.0, 30.0, 0.0), Vec3::new(40.0, 80.0, 0.0)] {
            assert!(found.centres.iter().any(|c| c.dist(want) < 0.1), "{want:?} in {found:?}");
        }
        assert_eq!(find(&scan, Vec3::new(50.0, 50.0, 0.0), None), Err(MarkerError::NotASphere), "the floor is no sphere");
    }

    /// The markers taken off: their triangles go, the holes they cut in the floor are closed flat,
    /// and the larger ball is left.
    #[test]
    fn removed_markers_leave_the_floor_closed() {
        let (mut p, mut t) = (Vec::new(), Vec::new());
        floor(100.0, 100, &mut p, &mut t);
        let markers = [Vec3::new(20.0, 20.0, 0.0), Vec3::new(70.0, 30.0, 0.0)];
        for c in markers {
            ball(c, 3.0, &mut p, &mut t);
        }
        let scan = Scan::new(&p, &t);
        let done = remove(&scan, &markers, 3.0, true).unwrap();
        assert_eq!((done.filled, done.failed), (2, 0), "a hole round each marker, closed");
        let left = Scan::new(&done.positions, &done.triangles);
        assert!(left.points().all(|q| q.z.abs() < 1e-6), "nothing of the markers left; the patches flat");
        let rings = crate::holes::survey(&left);
        assert!(rings.rings.iter().all(|r| r.rim), "no hole left, only the floor's rim");
        assert!(remove(&scan, &[], 3.0, true).is_none());
        assert!(remove(&scan, &markers, f64::NAN, true).is_none());
    }
}
