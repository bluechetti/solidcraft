//! Surface the scanner never saw: the triangles of a scan that lie away from anything the scanner
//! recorded. Hole fills, Remove and Fill, a mirror fill and a surface made from points all add
//! surface where there was no scan; a fit or a deviation map that leans on it is reading made-up
//! surface, and should say so.
//!
//! It is read from the geometry, not from what each tool says it added, so it holds whatever
//! chain of tools made the scan: a triangle of the scan as it is now is unseen when its middle is
//! farther than `reach` (about one and a half of the scanned triangles' sides) from the scan as it
//! came in, or, for a surface made from points, from every point. Something taken off that stood
//! less than that off the surface under it (a sticker) leaves a fill that reads as seen.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::curve::Projector;

/// How many of the scanned triangles' typical sides a triangle's middle may lie from the scan and
/// still be read as seen.
pub const REACH_EDGES: f64 = 1.5;
/// The most patches a reading lists, largest first.
pub const MOST_PATCHES: usize = 20;

/// One joined patch of unseen surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    /// The area-weighted middle of its triangles, mm.
    pub centre: Vec3,
    pub area: f64,
    pub triangles: usize,
}

/// What of a scan the scanner never saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unseen {
    /// One flag a triangle of the scan.
    pub flags: Vec<bool>,
    /// Their area and the whole scan's, mm².
    pub area: f64,
    pub total: f64,
    /// The largest joined patches, largest first (at most [`MOST_PATCHES`]).
    pub patches: Vec<Patch>,
}

impl Unseen {
    /// The share of the scan's area that is unseen, 0 to 1.
    pub fn share(&self) -> f64 {
        if self.total > 0.0 { self.area / self.total } else { 0.0 }
    }

    /// The share of the area of the triangles `region` (indices into the scan) that is unseen.
    pub fn share_of(&self, scan: &Scan, region: &[usize]) -> f64 {
        let (mut unseen, mut all) = (0.0, 0.0);
        for &t in region {
            let a = area(scan, t);
            all += a;
            if self.flags.get(t).copied().unwrap_or(false) {
                unseen += a;
            }
        }
        if all > 0.0 { unseen / all } else { 0.0 }
    }

    fn of(scan: &Scan, flags: Vec<bool>) -> Unseen {
        let total = (0..scan.triangle_count()).map(|t| area(scan, t)).sum();
        let area = (0..scan.triangle_count()).filter(|&t| flags[t]).map(|t| area(scan, t)).sum();
        let patches = patches(scan, &flags);
        Unseen { flags, area, total, patches }
    }
}

fn area(scan: &Scan, t: usize) -> f64 {
    scan.corners(t).map_or(0.0, |[a, b, c]| 0.5 * (b - a).cross(c - a).len())
}

fn middle(scan: &Scan, t: usize) -> Option<Vec3> {
    scan.corners(t).map(|[a, b, c]| (a + b + c) / 3.0)
}

/// Every triangle unseen: a mirror fill, made of the scan's reflection where it had none.
pub fn all(scan: &Scan) -> Unseen {
    Unseen::of(scan, vec![true; scan.triangle_count()])
}

/// The triangles of `now` farther than `reach` from `seen` (the scan as it came in, in the same
/// frame); `reach` defaults to [`REACH_EDGES`] of `seen`'s typical side.
pub fn against_scan(now: &Scan, seen: &Scan, reach: Option<f64>) -> Unseen {
    let on = Projector::new(seen);
    let reach = reach.unwrap_or(REACH_EDGES * on.edge).max(1e-9);
    let flags = crate::par::map(now.triangle_count(), |t| middle(now, t).is_some_and(|m| on.nearest(m).is_none_or(|(_, d)| d > reach)));
    Unseen::of(now, flags)
}

/// The triangles of `now` with no point of `points` within `reach` of their middle: a surface made
/// from a cloud, where the cloud had nothing.
pub fn against_points(now: &Scan, points: &[Vec3], reach: f64) -> Unseen {
    let reach = reach.max(1e-9);
    let cell = |p: Vec3| [(p.x / reach).floor() as i64, (p.y / reach).floor() as i64, (p.z / reach).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    for (i, p) in points.iter().enumerate().filter(|(_, p)| p.is_finite()) {
        grid.entry(cell(*p)).or_default().push(i as u32);
    }
    let near = |m: Vec3| {
        let [x, y, z] = cell(m);
        (-1..=1).any(|i| {
            (-1..=1).any(|j| {
                (-1..=1).any(|k| {
                    grid.get(&[x + i, y + j, z + k])
                        .is_some_and(|list| list.iter().any(|&q| points.get(q as usize).is_some_and(|p| p.dist(m) <= reach)))
                })
            })
        })
    };
    let flags = crate::par::map(now.triangle_count(), |t| middle(now, t).is_some_and(|m| !near(m)));
    Unseen::of(now, flags)
}

/// The unseen triangles joined through their corners into patches, the largest first.
fn patches(scan: &Scan, flags: &[bool]) -> Vec<Patch> {
    let weld = crate::welded(scan.positions);
    // Union-find over the corners of the unseen triangles.
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn root(parent: &mut HashMap<u32, u32>, mut v: u32) -> u32 {
        while let Some(&p) = parent.get(&v) {
            if p == v {
                break;
            }
            let grand = parent.get(&p).copied().unwrap_or(p);
            parent.insert(v, grand);
            v = p;
        }
        v
    }
    let corner = |i: u32| weld.get(i as usize).copied().unwrap_or(i);
    for (t, tri) in scan.triangles.iter().enumerate() {
        if !flags.get(t).copied().unwrap_or(false) {
            continue;
        }
        let [a, b, c] = tri.map(corner);
        for v in [a, b, c] {
            parent.entry(v).or_insert(v);
        }
        for v in [b, c] {
            let (ra, rv) = (root(&mut parent, a), root(&mut parent, v));
            if ra != rv {
                parent.insert(rv, ra);
            }
        }
    }
    let mut found: HashMap<u32, (Vec3, f64, usize)> = HashMap::new();
    for (t, tri) in scan.triangles.iter().enumerate() {
        if !flags.get(t).copied().unwrap_or(false) {
            continue;
        }
        let Some(m) = middle(scan, t) else { continue };
        let a = area(scan, t);
        let r = root(&mut parent, corner(tri[0]));
        let entry = found.entry(r).or_insert((Vec3::ZERO, 0.0, 0));
        entry.0 += m * a;
        entry.1 += a;
        entry.2 += 1;
    }
    let mut out: Vec<Patch> =
        found.into_values().map(|(sum, area, triangles)| Patch { centre: if area > 0.0 { sum / area } else { sum }, area, triangles }).collect();
    out.sort_by(|a, b| b.area.total_cmp(&a.area));
    out.truncate(MOST_PATCHES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat sheet of 1 mm cells, `n` across, with the cells `skip` left out.
    fn sheet(n: u32, skip: impl Fn(u32, u32) -> bool) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                p.push(Vec3::new(f64::from(i), f64::from(j), 0.0));
            }
        }
        let at = |i: u32, j: u32| j * (n + 1) + i;
        let mut t = Vec::new();
        for j in 0..n {
            for i in 0..n {
                if skip(i, j) {
                    continue;
                }
                t.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                t.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (p, t)
    }

    /// A hole 6 mm across closed by Fill Holes: the patch's middle reads as unseen, the sheet round
    /// it as seen, one patch about where the hole was.
    #[test]
    fn a_filled_hole_reads_as_unseen_and_the_rest_as_seen() {
        let hole = |i: u32, j: u32| (7..13).contains(&i) && (7..13).contains(&j);
        let (p, t) = sheet(20, hole);
        let seen = Scan::new(&p, &t);
        let survey = crate::holes::survey(&seen);
        let filled = crate::holes::fill(&seen, &survey, 300, true).expect("filled");
        let now = Scan::new(&filled.positions, &filled.triangles);
        let unseen = against_scan(&now, &seen, None);
        // Nothing of the sheet as scanned is unseen.
        assert!(unseen.flags[..t.len()].iter().all(|f| !f), "the scanned triangles are seen");
        assert!(unseen.area > 0.0 && unseen.area < 36.0, "part of the 36 mm² hole, its middle: {}", unseen.area);
        let patch = &unseen.patches[0];
        assert_eq!(unseen.patches.len(), 1);
        assert!(patch.centre.dist(Vec3::new(10.0, 10.0, 0.0)) < 0.5, "{patch:?}");
        assert!((unseen.share() - unseen.area / 400.0).abs() < 1e-9);
        // The scan as it came in, against itself: nothing unseen.
        assert_eq!(against_scan(&seen, &seen, None).area, 0.0);
    }

    /// A surface made from points reads as unseen where the points stop.
    #[test]
    fn a_surface_made_from_points_is_unseen_where_there_were_none() {
        let (p, t) = sheet(20, |_, _| false);
        let now = Scan::new(&p, &t);
        // Points over the left half only, every 0.5 mm.
        let points: Vec<Vec3> = (0..=20).flat_map(|i| (0..=40).map(move |j| Vec3::new(f64::from(i) * 0.5, f64::from(j) * 0.5, 0.0))).collect();
        let unseen = against_points(&now, &points, 1.0);
        let share = unseen.share();
        assert!((0.4..0.6).contains(&share), "about the right half: {share}");
        let region: Vec<usize> = (0..t.len()).filter(|&k| now.corners(k).is_some_and(|c| c.iter().all(|q| q.x <= 8.0))).collect();
        assert_eq!(unseen.share_of(&now, &region), 0.0, "the left part is seen");
        assert_eq!(all(&now).share(), 1.0);
    }
}
