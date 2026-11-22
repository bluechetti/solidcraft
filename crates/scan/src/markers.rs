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
    // By default the fit's, and room for the scan's triangles: a coarse scan's triangle cuts
    // inside the ball by about edge² / radius (at 1 mm triangles on a 6 mm ball, one marker of four
    // was found), at most a tenth of the radius.
    let coarse = (crate::mirror_fill::typical_edge(scan).powi(2) / radius).min(0.1 * radius);
    let tolerance = tolerance.filter(|t| t.is_finite() && *t > 0.0).unwrap_or_else(|| (3.0 * fitted.rms).max(0.03 * radius).max(coarse));
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
    let middle = |t: usize| scan.corners(t).map(|[a, b, c]| (a + b + c) / 3.0);
    let cut_by = |t: usize, among: &[Vec3]| middle(t).is_some_and(|q| among.iter().any(|m| q.dist(*m) <= reach));
    // A marker on the scan's edge: cut out, its hole would open into the scan's rim, which is not
    // filled (a bite out of the edge was left). It is pressed down instead, onto the floor round
    // it, and no hole is made.
    let open = |t: usize| cut_by(t, centres);
    let (trial, trial_triangles) = crate::pieces::keeping(scan, |t| !open(t));
    let trial_scan = Scan::new(&trial, &trial_triangles);
    let rims: Vec<Vec3> = crate::holes::survey(&trial_scan)
        .rings
        .iter()
        .filter(|r| r.rim)
        .flat_map(|r| r.points.iter().filter_map(|&i| trial.get(i as usize).copied()))
        .collect();
    let (pressed, cut): (Vec<Vec3>, Vec<Vec3>) = centres.iter().partition(|m| rims.iter().any(|q| q.dist(**m) <= 1.6 * radius));
    let flattened = pressed_down(scan, &pressed, radius);
    let base_positions: Vec<Vec3> = flattened.as_ref().map_or_else(|| scan.positions.to_vec(), |f| f.clone());
    let base = Scan::new(&base_positions, scan.triangles);
    let flat_count = if flattened.is_some() { pressed.len() } else { 0 };
    // A triangle goes when its middle is within the band, or, one smaller than the marker, any
    // corner is on the ball: on a coarse scan a triangle up a ball's side has its middle out past
    // the band. (A large one is the floor's: a CAD mesh's face fans out from round each marker,
    // and taking those took the whole face.)
    let goes = |t: usize| {
        base.corners(t).is_some_and(|[a, b, c]| {
            let q = (a + b + c) / 3.0;
            let small = a.dist(b).max(b.dist(c)).max(c.dist(a)) < radius;
            cut.iter().any(|m| q.dist(*m) <= reach || (small && [a, b, c].iter().any(|p| p.dist(*m) <= 1.1 * radius)))
        })
    };
    let removed = (0..base.triangle_count()).filter(|&t| goes(t)).count();
    if removed == 0 {
        return flattened.map(|positions| Removed { positions, triangles: scan.triangles.to_vec(), removed: 0, filled: flat_count, failed: 0 });
    }
    let scan = &base;
    let centres = &cut[..];
    let (positions, triangles) = crate::pieces::keeping(scan, |t| !goes(t));
    let left = Scan::new(&positions, &triangles);
    let survey = crate::holes::survey(&left);
    // Only the holes the markers left: a ring passing within the band of a marker.
    let near = |r: &crate::holes::Ring| {
        !r.rim && r.points.iter().filter_map(|&i| positions.get(i as usize)).any(|p| centres.iter().any(|m| p.dist(*m) <= 1.6 * radius))
    };
    let rings: Vec<&crate::holes::Ring> = survey.rings.iter().filter(|r| near(r)).collect();
    if rings.is_empty() {
        return Some(Removed { positions, triangles, removed, filled: flat_count, failed: 0 });
    }
    match crate::holes::fill_rings(&left, &rings, fair) {
        Some(done) => {
            Some(Removed { positions: done.positions, triangles: done.triangles, removed, filled: done.filled + flat_count, failed: done.failed })
        }
        None => Some(Removed { failed: rings.len(), positions, triangles, removed, filled: flat_count }),
    }
}

/// The scan's points with each marker at `centres` pressed down onto the floor round it: the
/// points within its reach moved square onto the plane fitted to the points in a band round it
/// (out to twice its radius). `None` when there is none to press, or no floor round any.
fn pressed_down(scan: &Scan, centres: &[Vec3], radius: f64) -> Option<Vec<Vec3>> {
    if centres.is_empty() {
        return None;
    }
    let reach = 1.15 * radius;
    let mut out = scan.positions.to_vec();
    let mut any = false;
    for m in centres {
        let band: Vec<Vec3> = scan.positions.iter().copied().filter(|q| (1.3 * radius..=2.2 * radius).contains(&q.dist(*m))).collect();
        if band.len() < 6 {
            continue;
        }
        let mid = band.iter().fold(Vec3::ZERO, |s, q| s + *q) / band.len() as f64;
        let mut spread = [[0.0f64; 3]; 3];
        for q in &band {
            let d = *q - mid;
            let d = [d.x, d.y, d.z];
            for (row, di) in spread.iter_mut().zip(d) {
                for (cell, dj) in row.iter_mut().zip(d) {
                    *cell += di * dj;
                }
            }
        }
        let Some((_, axes)) = solidcraft_geom::inertia::principal(&spread, 0.0) else { continue };
        let Some(normal) = axes[0].normalized() else { continue };
        for q in out.iter_mut().filter(|q| q.dist(*m) <= reach) {
            *q = *q - normal * (*q - mid).dot(normal);
        }
        any = true;
    }
    any.then_some(out)
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

/// A triangle's middle, outward direction, area and corners.
struct Facet {
    middle: Vec3,
    normal: Vec3,
    area: f64,
    corners: [Vec3; 3],
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
                Some(Facet { middle: (a + b + c) / 3.0, normal, area, corners: [a, b, c] })
            })
            .collect();
        Facets { all }
    }

    /// The triangles on the sphere at `c` of `radius`: each corner within the tolerance of it, and
    /// facing out of it. By the corners, not the middle: a large flat triangle (a CAD mesh's face,
    /// a reduced scan's floor) whose middle grazes a sphere would count its whole area, and on a
    /// plate of them every flat face read as a marker (44 found among 4).
    fn on(&self, c: Vec3, radius: f64, tolerance: f64) -> impl Iterator<Item = &Facet> {
        self.all.iter().filter(move |f| {
            let v = f.middle - c;
            let d = v.len();
            d > 0.0
                && (d - radius).abs() <= tolerance
                && f.corners.iter().all(|p| (p.dist(c) - radius).abs() <= tolerance)
                && f.normal.dot(v / d) >= MAX_LEAN_COS
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

    /// A floor of a few large triangles (as a CAD mesh's face is, or a reduced scan's floor) with
    /// four markers on it: only the four are found, not one for each large triangle whose middle
    /// grazes a sphere of their size.
    #[test]
    fn a_floor_of_large_triangles_is_no_marker() {
        let (mut p, mut t) = (Vec::new(), Vec::new());
        floor(100.0, 4, &mut p, &mut t);
        let at = [Vec3::new(20.0, 20.0, 0.0), Vec3::new(80.0, 20.0, 0.0), Vec3::new(80.0, 80.0, 0.0), Vec3::new(20.0, 80.0, 0.0)];
        for c in at {
            ball(c, 3.0, &mut p, &mut t);
        }
        let scan = Scan::new(&p, &t);
        let found = find(&scan, Vec3::new(20.0, 20.0, 3.0), None).unwrap();
        assert_eq!(found.centres.len(), 4, "{found:?}");
    }

    /// A floor modelled as CAD meshes it, large triangles fanning out from a ring round the
    /// marker: only the marker goes, and its hole is closed; the floor's large triangles stay
    /// (taking every one with a corner on the ball took the whole face, and its fill sagged).
    #[test]
    fn a_marker_on_a_fanned_floor_takes_only_itself() {
        let (mut p, mut t) = (Vec::new(), Vec::new());
        let c = Vec3::new(50.0, 50.0, 0.0);
        let n = 24u32;
        for k in 0..n {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(n);
            p.push(c + Vec3::new(a.cos(), a.sin(), 0.0) * 3.0);
        }
        let corners = [Vec3::new(100.0, 100.0, 0.0), Vec3::new(0.0, 100.0, 0.0), Vec3::new(0.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0)];
        let first = p.len() as u32;
        p.extend(corners);
        // Each ring segment to the corner of its quarter (the corners at 45°, 135°, 225° and 315°),
        // and each pair of corners to the ring point between them, all facing up.
        for k in 0..n {
            let q = k * 4 / n;
            t.push([(k + 1) % n, k, first + q]);
        }
        for q in 0..4u32 {
            t.push([((q + 1) * n / 4) % n, first + q, first + (q + 1) % 4]);
        }
        let up = |tri: &[u32; 3]| (p[tri[1] as usize] - p[tri[0] as usize]).cross(p[tri[2] as usize] - p[tri[0] as usize]).z > 0.0;
        assert!(t.iter().all(up), "the floor faces up");
        let floor_triangles = t.len();
        ball(c, 3.0, &mut p, &mut t);
        let scan = Scan::new(&p, &t);
        let done = remove(&scan, &[c], 3.0, true).unwrap();
        let left = Scan::new(&done.positions, &done.triangles);
        let big = (0..left.triangle_count()).filter_map(|i| left.corners(i)).filter(|[a, b, c]| a.dist(*b).max(b.dist(*c)) > 20.0).count();
        assert!(big >= floor_triangles - 2, "the floor's large triangles stay: {big} of {floor_triangles}");
        assert!(left.points().all(|q| q.z.abs() < 1e-6), "the marker gone, the patch flat");
        let down = (0..left.triangle_count()).filter_map(|i| left.corners(i)).filter(|[a, b, c]| (*b - *a).cross(*c - *a).z < 0.0).count();
        assert_eq!(down, 0, "every triangle faces up: none of the patch folded over");
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

    /// A scan of a floor with markers stuck on it, as a scanner sees it: one surface, the balls'
    /// tops merged into the floor (a cliff where a ball meets it), its points jittered and noisy.
    fn scanned_floor(size: f64, spacing: f64, markers: &[Vec3], radius: f64, seed: u64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut rnd = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f64 / (1u64 << 31) as f64) - 0.5
        };
        let n = (size / spacing) as u32;
        let mut p = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                let (x, y) = (f64::from(i) * spacing + 0.35 * spacing * rnd(), f64::from(j) * spacing + 0.35 * spacing * rnd());
                let mut z = 0.03 * rnd();
                for m in markers {
                    let d = ((x - m.x).powi(2) + (y - m.y).powi(2)).sqrt();
                    if d < radius {
                        z = z.max(m.z + (radius * radius - d * d).sqrt() + 0.02 * rnd());
                    }
                }
                p.push(Vec3::new(x, y, z));
            }
        }
        let at = |i: u32, j: u32| i * (n + 1) + j;
        let mut t = Vec::new();
        for i in 0..n {
            for j in 0..n {
                if (i + j) % 2 == 0 {
                    t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                } else {
                    t.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
                    t.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
        (p, t)
    }

    /// Markers on a scan as a scanner sees them (merged into a noisy, jittered floor), one of
    /// them over the scan's edge, at fine and coarse spacing: all four are found and taken off and
    /// no hole is left; the one on the edge is pressed down onto the floor, not cut into a bite out
    /// of the rim.
    #[test]
    fn markers_on_a_scanned_floor_leave_no_holes() {
        for seed in 0..4u64 {
            for spacing in [0.3, 0.6, 1.0] {
                let markers = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(25.0, 12.0, -1.5), Vec3::new(15.0, 27.0, 0.0), Vec3::new(3.5, 20.0, 0.0)];
                let (p, t) = scanned_floor(30.0, spacing, &markers, 3.0, seed);
                let scan = Scan::new(&p, &t);
                let found = find(&scan, Vec3::new(10.0, 10.0, 3.0), None).unwrap();
                assert_eq!(found.centres.len(), 4, "seed {seed}, {spacing} mm: {found:?}");
                let done = remove(&scan, &found.centres, found.radius, true).unwrap();
                assert_eq!((done.filled, done.failed), (4, 0), "seed {seed}, {spacing} mm");
                let left = Scan::new(&done.positions, &done.triangles);
                let survey = crate::holes::survey(&left);
                assert_eq!(survey.rings.iter().filter(|r| !r.rim).count(), 0, "no hole left: seed {seed}, {spacing} mm");
                let high: Vec<Vec3> = left.points().filter(|q| q.z >= 0.4).collect();
                assert!(high.is_empty(), "nothing of the markers left: seed {seed}, {spacing} mm: {high:?} centres {:?}", found.centres);
            }
        }
    }
}
