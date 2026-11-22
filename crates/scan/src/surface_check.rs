//! Surface Check: what is wrong with a B-spline surface before it is taken as a face of the part,
//! found on a grid of its places (after SurfaceCraft's Surface Check). It looks for:
//!
//! - **folds**: across a cell of the grid the way the surface faces turns back on itself, so the
//!   surface doubles over there;
//! - **pinches**: a place where the surface narrows to no area (its directions along u and v
//!   lie in one line, or one of them is nothing) that is not a corner of its parameter square;
//! - **crossings**: two parts of the surface that are not neighbours passing through each other;
//!
//! and measures how square its parameters are (the least angle between its directions along u and
//! v) and how evenly they cover it (the stretch: the square root of the largest area a step of the
//! parameters covers over the least).
//!
//! The grid samples the exact surface; a trimmed face checks only the cells whose middle is on it.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::surface_edit::{frame, points};

/// Cells each way a check uses by default, and the fewest and most it takes.
pub const GRID: usize = 40;
pub const MIN_GRID: usize = 4;
pub const MAX_GRID: usize = 200;

/// The most trouble places a check keeps (the counts go on past it).
pub const MOST_PLACES: usize = 256;

/// A place's area below this share of the middle one is a pinch.
const PINCH: f64 = 1e-3;

/// What is wrong at a place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trouble {
    Fold,
    Pinch,
    Crossing,
}

impl Trouble {
    /// Its name in a reply.
    pub fn name(self) -> &'static str {
        match self {
            Trouble::Fold => "fold",
            Trouble::Pinch => "pinch",
            Trouble::Crossing => "crossing",
        }
    }
}

/// What a check found.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    /// Cells of the grid that fold, that pinch, and pairs of the grid's triangles that cross.
    pub folds: usize,
    pub pinches: usize,
    pub crossings: usize,
    /// Cells checked (those on the face).
    pub cells: usize,
    /// The least angle between the directions along u and v, in degrees (90 is square).
    pub least_angle: f64,
    /// How unevenly the parameters cover the surface: 1 is even.
    pub stretch: f64,
    /// Where the trouble is, at most [`MOST_PLACES`] of it: the middle of a cell that folds, a
    /// pinched place, the middle of a triangle that another crosses.
    pub places: Vec<(Trouble, Vec3)>,
}

impl Check {
    /// No folds, no pinches, no crossings.
    pub fn is_clean(&self) -> bool {
        self.folds == 0 && self.pinches == 0 && self.crossings == 0
    }

    fn note(&mut self, trouble: Trouble, at: Vec3) {
        if self.places.len() < MOST_PLACES {
            self.places.push((trouble, at));
        }
    }
}

/// `surface` checked on a `grid` × `grid` grid of cells over `area` (its parameters
/// `[[u0, u1], [v0, v1]]`, kept inside its domain), over the cells whose middle `include` says is
/// on the face. `None` when the surface cannot be evaluated or the area is empty.
pub fn check(surface: &NurbsSurface, grid: usize, area: [[f64; 2]; 2], include: &dyn Fn(f64, f64) -> bool) -> Option<Check> {
    let n = grid.clamp(MIN_GRID, MAX_GRID);
    let [[du0, du1], [dv0, dv1]] = surface.domain();
    let [[u0, u1], [v0, v1]] = area;
    let (u0, u1, v0, v1) = (u0.max(du0), u1.min(du1), v0.max(dv0), v1.min(dv1));
    if !(u1 > u0 && v1 > v0) {
        return None;
    }
    let net = points(surface);
    let at = |i: usize, j: usize| i + j * (n + 1);
    let param = |i: f64, j: f64| (u0 + (u1 - u0) * i / n as f64, v0 + (v1 - v0) * j / n as f64);
    let mut place = Vec::with_capacity((n + 1) * (n + 1));
    let mut su = Vec::with_capacity((n + 1) * (n + 1));
    let mut sv = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let (u, v) = param(i as f64, j as f64);
            let (s, du, dv) = frame(surface, &net, u, v)?;
            if !(s.is_finite() && du.is_finite() && dv.is_finite()) {
                return None;
            }
            place.push(s);
            su.push(du);
            sv.push(dv);
        }
    }
    let cells: Vec<(usize, usize)> = (0..n)
        .flat_map(|j| (0..n).map(move |i| (i, j)))
        .filter(|&(i, j)| {
            let (u, v) = param(i as f64 + 0.5, j as f64 + 0.5);
            include(u, v)
        })
        .collect();
    let mut found = Check { folds: 0, pinches: 0, crossings: 0, cells: cells.len(), least_angle: 90.0, stretch: 1.0, places: Vec::new() };
    if cells.is_empty() {
        return Some(found);
    }
    // How much area a step of the parameters covers at each place.
    let area_at: Vec<f64> = su.iter().zip(&sv).map(|(a, b)| a.cross(*b).len()).collect();
    // A corner of the square, where two sides meet, has no area of its own to lose: a corner, not
    // a pinch.
    let corners = [at(0, 0), at(n, 0), at(0, n), at(n, n)];
    let mut used: Vec<usize> =
        cells.iter().flat_map(|&(i, j)| [at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)]).filter(|k| !corners.contains(k)).collect();
    used.sort_unstable();
    used.dedup();
    let mut areas: Vec<f64> = used.iter().map(|&k| area_at[k]).collect();
    areas.sort_by(f64::total_cmp);
    let Some(middle) = areas.get(areas.len() / 2).copied().filter(|m| *m > 0.0) else {
        // Nothing has any area: the surface is all pinch.
        found.pinches = cells.len();
        found.least_angle = 0.0;
        found.stretch = f64::INFINITY;
        if let Some(&(i, j)) = cells.first() {
            found.note(Trouble::Pinch, place[at(i, j)]);
        }
        return Some(found);
    };
    for &k in &used {
        let (a, b) = (su[k].len(), sv[k].len());
        if a > 0.0 && b > 0.0 {
            found.least_angle = found.least_angle.min((area_at[k] / (a * b)).min(1.0).asin().to_degrees());
        }
    }
    let thin = PINCH * middle;
    for &(i, j) in &cells {
        let round = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
        if let Some(&c) = round.iter().find(|c| !corners.contains(c) && area_at[**c] < thin) {
            found.pinches += 1;
            found.note(Trouble::Pinch, place[c]);
            continue;
        }
        // Across a side of a cell the way the surface faces turns smoothly unless it folds there.
        let facing: Vec<Vec3> = round.iter().filter(|c| area_at[**c] > thin).map(|&c| su[c].cross(sv[c]) * (1.0 / area_at[c])).collect();
        if (0..facing.len()).any(|m| facing[m].dot(facing[(m + 1) % facing.len()]) < 0.0) {
            found.folds += 1;
            let mid = round.iter().fold(Vec3::ZERO, |s, c| s + place[*c]) * 0.25;
            found.note(Trouble::Fold, mid);
        }
    }
    let (Some(least), Some(most)) = (areas.first(), areas.last()) else { return Some(found) };
    found.stretch = (most / least.max(1e-300)).sqrt();
    let (count, at) = crossings(&place, n, &cells);
    found.crossings = count;
    for m in at {
        found.note(Trouble::Crossing, m);
    }
    Some(found)
}

/// The surface checked over its whole domain.
pub fn check_whole(surface: &NurbsSurface, grid: usize) -> Option<Check> {
    check(surface, grid, surface.domain(), &|_, _| true)
}

/// How a surface a fit made is wrong: its folds, pinches and crossings on the default grid (none
/// when it cannot be evaluated).
pub fn trouble(surface: &NurbsSurface) -> [usize; 3] {
    check_whole(surface, GRID).map_or([0; 3], |c| [c.folds, c.pinches, c.crossings])
}

/// The pairs of the cells' triangles (two a cell) that share no corner and pass through each other:
/// each triangle's sides tried against the other among the triangles near it (a grid of buckets as
/// big as the largest triangle). How many, and the middles of the first [`MOST_PLACES`].
fn crossings(place: &[Vec3], n: usize, cells: &[(usize, usize)]) -> (usize, Vec<Vec3>) {
    let at = |i: usize, j: usize| i + j * (n + 1);
    let triangles: Vec<[usize; 3]> =
        cells.iter().flat_map(|&(i, j)| [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]]).collect();
    if triangles.len() <= 2 {
        return (0, Vec::new());
    }
    let size = triangles.iter().flat_map(|t| (0..3).map(move |k| place[t[k]].dist(place[t[(k + 1) % 3]]))).fold(0.0, f64::max);
    if !(size > 0.0 && size.is_finite()) {
        return (0, Vec::new());
    }
    let middle = |t: &[usize; 3]| (place[t[0]] + place[t[1]] + place[t[2]]) * (1.0 / 3.0);
    // Saturating: a place far out lands in the outermost bucket, which is only slower.
    let key = |p: Vec3| [(p.x / size).floor() as i64, (p.y / size).floor() as i64, (p.z / size).floor() as i64];
    let mut buckets: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (index, t) in triangles.iter().enumerate() {
        buckets.entry(key(middle(t))).or_default().push(index);
    }
    let tiny = 1e-14 * size * size * size;
    let segment_crosses = |a: Vec3, b: Vec3, t: &[usize; 3]| -> bool {
        let p0 = place[t[0]];
        let (e1, e2) = (place[t[1]] - p0, place[t[2]] - p0);
        let d = b - a;
        let h = d.cross(e2);
        let det = e1.dot(h);
        if !(det.abs() > tiny) {
            return false;
        }
        let s = a - p0;
        let f = 1.0 / det;
        let u = f * s.dot(h);
        if !(u > 1e-9 && u < 1.0 - 1e-9) {
            return false;
        }
        let q = s.cross(e1);
        let v = f * d.dot(q);
        if !(v > 1e-9 && u + v < 1.0 - 1e-9) {
            return false;
        }
        let w = f * e2.dot(q);
        w > 1e-9 && w < 1.0 - 1e-9
    };
    let mut out = Vec::new();
    let mut count = 0;
    for (index, t) in triangles.iter().enumerate() {
        let [x, y, z] = key(middle(t));
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(others) = buckets.get(&[x.saturating_add(dx), y.saturating_add(dy), z.saturating_add(dz)]) else { continue };
                    for &other in others.iter().filter(|o| **o > index) {
                        let o = &triangles[other];
                        // Neighbours share a corner, and touch there: not a crossing.
                        if t.iter().any(|c| o.contains(c)) {
                            continue;
                        }
                        let crosses = (0..3).any(|k| segment_crosses(place[t[k]], place[t[(k + 1) % 3]], o))
                            || (0..3).any(|k| segment_crosses(place[o[k]], place[o[(k + 1) % 3]], t));
                        if crosses {
                            count += 1;
                            if out.len() < MOST_PLACES {
                                out.push(middle(t));
                            }
                        }
                    }
                }
            }
        }
    }
    (count, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bicubic surface over `n` × `n` points from `f(i, j)`, clamped knots on [0, 1].
    fn surface(n: usize, f: impl Fn(usize, usize) -> Vec3) -> NurbsSurface {
        let mut knots = vec![0.0; 4];
        knots.extend((1..n - 3).map(|k| k as f64 / (n - 3) as f64));
        knots.extend([1.0; 4]);
        let pts: Vec<Vec3> = (0..n).flat_map(|j| (0..n).map(move |i| (i, j))).map(|(i, j)| f(i, j)).collect();
        NurbsSurface::new([3, 3], [knots.clone(), knots], &pts, &vec![1.0; n * n]).unwrap()
    }

    #[test]
    fn a_gentle_dome_is_clean_square_and_even() {
        let s = surface(6, |i, j| {
            let (x, y) = (i as f64 * 10.0, j as f64 * 10.0);
            Vec3::new(x, y, 5.0 - ((x - 25.0).powi(2) + (y - 25.0).powi(2)) / 200.0)
        });
        let c = check_whole(&s, GRID).unwrap();
        assert!(c.is_clean(), "{c:?}");
        assert_eq!(c.cells, GRID * GRID);
        assert!(c.least_angle > 70.0, "{}", c.least_angle);
        // Clamped knots run faster at the sides than in the middle: about 3 times each way.
        assert!(c.stretch >= 1.0 && c.stretch < 3.5, "{}", c.stretch);
        assert!(c.places.is_empty());
    }

    #[test]
    fn a_point_pulled_back_through_its_neighbours_folds_the_surface() {
        // A flat net with one middle column dragged far back past the next one along u.
        let s = surface(6, |i, j| {
            let x = if i == 3 { 0.0 } else { i as f64 * 10.0 };
            Vec3::new(x, j as f64 * 10.0, 0.0)
        });
        let c = check_whole(&s, GRID).unwrap();
        assert!(c.folds > 0, "{c:?}");
        assert!(!c.is_clean());
        let (kind, at) = c.places[0];
        assert_eq!(kind, Trouble::Fold);
        assert!((0.0..=50.0).contains(&at.x) && (0.0..=50.0).contains(&at.y), "{at:?}");
    }

    #[test]
    fn a_side_drawn_to_one_place_pinches_there() {
        // The first row of points all at one place: the side v = 0 is a point, a pole.
        let s = surface(6, |i, j| if j == 0 { Vec3::new(25.0, 0.0, 0.0) } else { Vec3::new(i as f64 * 10.0, j as f64 * 10.0, 0.0) });
        let c = check_whole(&s, GRID).unwrap();
        assert_eq!(c.pinches, GRID, "every cell along that side: {c:?}");
        assert!(!c.is_clean());
        assert!(c.places.iter().all(|(t, at)| *t == Trouble::Pinch && at.dist(Vec3::new(25.0, 0.0, 0.0)) < 1e-9), "{:?}", c.places);
    }

    #[test]
    fn a_sheet_that_passes_through_itself_crosses() {
        // A figure of eight drawn out along z: the sheet passes through itself along a line
        // through the middle, square across, with no fold or pinch.
        let s = surface(16, |i, j| {
            let a = 0.25 + i as f64 / 15.0 * (std::f64::consts::TAU - 0.5);
            Vec3::new(30.0 * a.cos(), 15.0 * (2.0 * a).sin(), j as f64 * 6.0)
        });
        let c = check_whole(&s, GRID).unwrap();
        assert!(c.crossings > 0, "{c:?}");
        assert_eq!((c.folds, c.pinches), (0, 0), "{c:?}");
        let (kind, at) = c.places[0];
        assert_eq!(kind, Trouble::Crossing);
        assert!(at.x.abs() < 3.0 && at.y.abs() < 3.0, "near the middle: {at:?}");
    }

    #[test]
    fn only_the_cells_on_the_face_are_checked() {
        let s = surface(6, |i, j| {
            let x = if i == 3 { 0.0 } else { i as f64 * 10.0 };
            Vec3::new(x, j as f64 * 10.0, 0.0)
        });
        // The fold is in the middle of u; a face on the first fifth of it misses it.
        let c = check(&s, GRID, s.domain(), &|u, _| u < 0.2).unwrap();
        assert_eq!(c.cells, 8 * GRID);
        assert!(c.is_clean(), "{c:?}");
        let none = check(&s, GRID, s.domain(), &|_, _| false).unwrap();
        assert_eq!(none.cells, 0);
        assert!(none.is_clean());
    }

    #[test]
    fn hostile_areas_and_grids_never_panic() {
        let s = surface(4, |i, j| Vec3::new(i as f64, j as f64, 0.0));
        assert!(check(&s, 0, [[0.5, 0.5], [0.0, 1.0]], &|_, _| true).is_none(), "an empty area");
        assert!(check(&s, usize::MAX, [[f64::NAN, 1.0], [0.0, 1.0]], &|_, _| true).is_some(), "a side not a number: the domain's");
        assert!(check(&s, usize::MAX, [[-1e300, 1e300], [f64::NEG_INFINITY, f64::INFINITY]], &|_, _| true).is_some(), "kept in the domain");
        // A net all at one place has no area anywhere.
        let point = surface(4, |_, _| Vec3::new(1.0, 2.0, 3.0));
        let c = check_whole(&point, 8).unwrap();
        assert!(c.pinches > 0 && !c.is_clean(), "{c:?}");
    }
}
