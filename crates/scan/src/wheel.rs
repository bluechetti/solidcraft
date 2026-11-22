//! A wheel's openings between its spokes, read off the scan seen along the wheel's axis: where the
//! scan, looked through along the axis, has nothing between the hub and the rim. Reconstruct turns
//! a spoked wheel as a whole (the rim and hub with the openings filled); one opening's outline cut
//! through it and patterned round the axis gives the spokes back.

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::reconstruct::Axis;

/// One opening: its outline in the plane square to the axis (`x` and `y` there, about the axis),
/// and how far the scan reaches along the axis either way of the axis's point.
#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub outline: Vec<Vec2>,
    pub x: Vec3,
    pub y: Vec3,
    pub front: f64,
    pub back: f64,
    /// How many openings alike the scan has.
    pub alike: usize,
}

/// Cells across the wheel's diameter the scan is looked through on.
const CELLS: usize = 480;
/// An opening covers at least this share of the wheel's disc ...
const LEAST_OPENING: f64 = 0.004;
/// ... and openings alike are within this share of each other's area.
const ALIKE_AREA: f64 = 0.3;

/// The largest of the openings between a wheel's spokes, when the scan seen along `axis` has at
/// least `count` alike, each enclosed (not the outside, not the bore at the middle). `None` when it
/// has fewer, or the scan is empty.
pub fn opening(scan: &Scan, axis: &Axis, count: usize) -> Option<Opening> {
    let direction = axis.direction.normalized()?;
    let (x, y) = crate::in_plane(direction);
    let flat = |p: Vec3| {
        let d = p - axis.point;
        (Vec2::new(d.dot(x), d.dot(y)), d.dot(direction))
    };
    let mut reach = 0.0_f64;
    let (mut front, mut back) = (f64::NEG_INFINITY, f64::INFINITY);
    for &p in scan.positions {
        let (q, t) = flat(p);
        reach = reach.max(q.len());
        front = front.max(t);
        back = back.min(t);
    }
    if !(reach > 0.0) || count < 2 {
        return None;
    }
    // The grid: CELLS + 2 across, a cell of margin round the wheel.
    let n = CELLS + 2;
    let cell = 2.0 * reach / CELLS as f64;
    let at = |i: usize, j: usize| Vec2::new((i as f64 - 1.0 + 0.5) * cell - reach, (j as f64 - 1.0 + 0.5) * cell - reach);
    let index = |v: f64| ((v + reach) / cell + 1.0).floor();
    let mut full = vec![false; n * n];
    for t in 0..scan.triangle_count() {
        let Some(corners) = scan.corners(t) else { continue };
        let [a, b, c] = corners.map(|p| flat(p).0);
        let (lo_x, hi_x) = (a.x.min(b.x).min(c.x), a.x.max(b.x).max(c.x));
        let (lo_y, hi_y) = (a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y));
        let clamp = |v: f64| v.clamp(0.0, (n - 1) as f64) as usize;
        let (i0, i1, j0, j1) = (clamp(index(lo_x)), clamp(index(hi_x)), clamp(index(lo_y)), clamp(index(hi_y)));
        for j in j0..=j1 {
            for i in i0..=i1 {
                // A cell the triangle covers, or one its edges pass within half a cell of (a
                // triangle seen edge-on, along the barrel, is a thin line).
                let p = at(i, j);
                if inside(p, a, b, c) || [(a, b), (b, c), (c, a)].iter().any(|&(u, v)| to_segment(p, u, v) <= 0.5 * cell) {
                    full[j * n + i] = true;
                }
            }
        }
    }
    // The empty cells in pieces; the outside (touching the grid's edge) and the bore (holding the
    // middle) are no openings.
    let mut piece = vec![usize::MAX; n * n];
    let mut pieces: Vec<Vec<usize>> = Vec::new();
    for start in 0..n * n {
        if full[start] || piece[start] != usize::MAX {
            continue;
        }
        let id = pieces.len();
        let mut cells = vec![start];
        piece[start] = id;
        let mut k = 0;
        while k < cells.len() {
            let c = cells[k];
            k += 1;
            let (i, j) = (c % n, c / n);
            let near = [(i > 0).then(|| c - 1), (i + 1 < n).then(|| c + 1), (j > 0).then(|| c - n), (j + 1 < n).then(|| c + n)];
            for d in near.into_iter().flatten() {
                if !full[d] && piece[d] == usize::MAX {
                    piece[d] = id;
                    cells.push(d);
                }
            }
        }
        pieces.push(cells);
    }
    let middle = (n / 2) * n + n / 2;
    let edge = |c: usize| {
        let (i, j) = (c % n, c / n);
        i == 0 || j == 0 || i + 1 == n || j + 1 == n
    };
    let disc = (CELLS * CELLS) as f64 * std::f64::consts::FRAC_PI_4;
    let mut openings: Vec<&Vec<usize>> = pieces
        .iter()
        .filter(|cells| !cells.contains(&middle) && !cells.iter().any(|&c| edge(c)) && cells.len() as f64 >= LEAST_OPENING * disc)
        .collect();
    openings.sort_by_key(|cells| std::cmp::Reverse(cells.len()));
    let largest = openings.first()?;
    let alike = openings.iter().filter(|cells| (cells.len() as f64) >= (1.0 - ALIKE_AREA) * largest.len() as f64).count();
    if alike < count {
        return None;
    }
    let outline = trace(largest, n, cell, reach)?;
    Some(Opening { outline, x, y, front, back, alike })
}

/// Whether `p` lies inside the triangle `a`, `b`, `c` (either way round).
fn inside(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (d1, d2, d3) = ((b - a).cross(p - a), (c - b).cross(p - b), (a - c).cross(p - c));
    (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
}

fn to_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// The outline round `cells` (a piece of the grid), along the cells' sides, its corners in the
/// plane: the longest loop (the piece's outer edge).
fn trace(cells: &[usize], n: usize, cell: f64, reach: f64) -> Option<Vec<Vec2>> {
    let member: std::collections::HashSet<usize> = cells.iter().copied().collect();
    // Each side between a member and a non-member, run with the member on its left: from grid
    // corner to grid corner (corner (i, j) is the cell's lower left).
    let mut next: std::collections::HashMap<(usize, usize), (usize, usize)> = std::collections::HashMap::new();
    for &c in cells {
        let (i, j) = (c % n, c / n);
        let out = |di: i64, dj: i64| {
            let (a, b) = (i as i64 + di, j as i64 + dj);
            a < 0 || b < 0 || a >= n as i64 || b >= n as i64 || !member.contains(&(b as usize * n + a as usize))
        };
        if out(0, -1) {
            next.insert((i, j), (i + 1, j));
        }
        if out(1, 0) {
            next.insert((i + 1, j), (i + 1, j + 1));
        }
        if out(0, 1) {
            next.insert((i + 1, j + 1), (i, j + 1));
        }
        if out(-1, 0) {
            next.insert((i, j + 1), (i, j));
        }
    }
    let mut loops: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut left = next.clone();
    while let Some((&start, _)) = left.iter().next() {
        let mut ring = vec![start];
        let mut here = start;
        while let Some(to) = left.remove(&here) {
            if to == start {
                break;
            }
            ring.push(to);
            here = to;
        }
        loops.push(ring);
    }
    let ring = loops.into_iter().max_by_key(Vec::len)?;
    let corner = |(i, j): (usize, usize)| Vec2::new((i as f64 - 1.0) * cell - reach, (j as f64 - 1.0) * cell - reach);
    // The staircase's straight runs kept as their ends only.
    let count = ring.len();
    let points: Vec<Vec2> = (0..count)
        .filter(|&k| {
            let (p, q, r) = (ring[(k + count - 1) % count], ring[k], ring[(k + 1) % count]);
            (q.0 as i64 - p.0 as i64, q.1 as i64 - p.1 as i64) != (r.0 as i64 - q.0 as i64, r.1 as i64 - q.1 as i64)
        })
        .map(|k| corner(ring[k]))
        .collect();
    (points.len() >= 4).then_some(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat wheel face: a ring of rim and a hub joined by four spokes, as triangles in the XY
    /// plane, looked at along Z: four openings alike, the outline of one between two spokes.
    #[test]
    fn the_openings_between_four_spokes_are_found() {
        let mut positions: Vec<Vec3> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let k = positions.len() as u32;
            positions.extend([a, b, c, d]);
            triangles.extend([[k, k + 1, k + 2], [k, k + 2, k + 3]]);
        };
        let polar = |r: f64, deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            Vec3::new(r * c, r * s, 0.0)
        };
        // The rim (R 90 to 100) and the hub (R 15 to 30), in 72 steps round.
        for k in 0..72 {
            let (a, b) = (5.0 * f64::from(k), 5.0 * f64::from(k + 1));
            quad(polar(90.0, a), polar(100.0, a), polar(100.0, b), polar(90.0, b));
            quad(polar(15.0, a), polar(30.0, a), polar(30.0, b), polar(15.0, b));
        }
        // Four spokes 12 wide from hub to rim.
        for deg in [0.0, 90.0, 180.0, 270.0] {
            let (s, c) = f64::to_radians(deg).sin_cos();
            let (along, side) = (Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0));
            quad(along * 29.0 - side * 6.0, along * 91.0 - side * 6.0, along * 91.0 + side * 6.0, along * 29.0 + side * 6.0);
        }
        let scan = Scan::new(&positions, &triangles);
        let axis = Axis { point: Vec3::ZERO, direction: Vec3::Z };
        let found = opening(&scan, &axis, 4).expect("four openings");
        assert_eq!(found.alike, 4);
        // The outline lies between the hub and the rim, and between two spokes.
        let radii: Vec<f64> = found.outline.iter().map(|p| p.len()).collect();
        assert!(radii.iter().all(|r| *r > 28.0 && *r < 92.0), "{radii:?}");
        let middle = found.outline.iter().fold(Vec2::ZERO, |s, p| s + *p) / found.outline.len() as f64;
        assert!(middle.len() > 40.0 && middle.len() < 75.0, "{middle:?}");
        // Five spokes asked of a wheel with four openings: none.
        assert!(opening(&scan, &axis, 5).is_none());
    }
}
