//! A scan's long edges cut until none is longer than a length, by longest-edge bisection with
//! its propagation path (Rivara 1984, 1997): a triangle is cut only across its own longest edge,
//! and before a triangle's longest edge is cut, a neighbour whose longest edge is a different one
//! is cut first, so that the triangles still meet edge to edge. Cutting only longest edges keeps
//! the triangles' shapes (no angle falls below half the smallest one the scan had), so the number
//! of triangles stays about the area over the length squared. Cutting an edge in a triangle where
//! it is not the longest instead makes thinner and thinner slivers whose long edges never shrink,
//! and the count runs away.
//!
//! The new points lie on the scan's triangles, so the surface is the same; only its triangles
//! are finer. For the tools whose rules count triangles or read a patch of them, which a CAD
//! model's long slivers or a thinned scan's coarse triangles would starve.

use std::cmp::Ordering;
use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;

/// A scan cut finer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Finer {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Each triangle's triangle of the scan it was cut from.
    pub parent: Vec<u32>,
}

impl Finer {
    pub(crate) fn scan(&self) -> Scan<'_> {
        Scan::new(&self.positions, &self.triangles)
    }
}

/// The edge from `a` to `b`, either way round.
fn key(a: u32, b: u32) -> u64 {
    (u64::from(a.min(b)) << 32) | u64::from(a.max(b))
}

/// An edge of a triangle: its length and its corners as the triangle winds.
#[derive(Clone, Copy, Debug)]
struct Edge {
    length: f64,
    from: u32,
    to: u32,
}

impl Edge {
    fn key(&self) -> u64 {
        key(self.from, self.to)
    }

    /// Edges in one order the whole mesh agrees on: by length, then by corners. A path that
    /// always moves to a longer edge in this order cannot come back on itself.
    fn order(&self, other: &Edge) -> Ordering {
        self.length.total_cmp(&other.length).then(self.key().cmp(&other.key()))
    }
}

/// The longest edge of `tri`, or `None` when it names a missing or non-finite point or repeats a
/// corner: such a triangle is never cut.
fn longest_edge(positions: &[Vec3], tri: &[u32; 3]) -> Option<Edge> {
    if tri[0] == tri[1] || tri[1] == tri[2] || tri[2] == tri[0] {
        return None;
    }
    let at = |v: u32| positions.get(v as usize).copied().filter(|p| p.is_finite());
    let corners = [at(tri[0])?, at(tri[1])?, at(tri[2])?];
    (0..3).map(|k| Edge { length: corners[k].dist(corners[(k + 1) % 3]), from: tri[k], to: tri[(k + 1) % 3] }).max_by(Edge::order)
}

/// About how many triangles cutting `scan` to `longest` makes: a well-shaped triangle about
/// twice its area over a cut one's, a sliver about its perimeter over the length.
fn expected_count(scan: &Scan, longest: f64) -> f64 {
    let cut_area = 0.25 * 3f64.sqrt() * longest * longest;
    (0..scan.triangle_count())
        .map(|t| {
            let Some([a, b, c]) = scan.corners(t) else { return 1.0 };
            let perimeter = a.dist(b) + b.dist(c) + c.dist(a);
            let area = (b - a).cross(c - a).len() / 2.0;
            (2.0 * area / cut_area).max(perimeter / longest).max(1.0)
        })
        .sum()
}

/// `scan` with its edges cut until none is longer than `longest`, or `None` when that would make
/// more than `most` triangles (or `longest` is not a length). Triangles that name a missing or
/// non-finite point or repeat a corner are left as they are.
pub(crate) fn finer(scan: &Scan, longest: f64, most: usize) -> Option<Finer> {
    if !(longest > 0.0) || expected_count(scan, longest) > most as f64 {
        return None;
    }
    let mut positions = scan.positions.to_vec();
    // A triangle naming a point past the end would name a cut's new point: it names none.
    let named = positions.len();
    let mut triangles: Vec<[u32; 3]> =
        scan.triangles.iter().map(|t| if t.iter().all(|&v| (v as usize) < named) { *t } else { [u32::MAX; 3] }).collect();
    let mut parent: Vec<u32> = (0..triangles.len() as u32).collect();
    // The triangles that may be cut, each on the list of its edges; the halves of one are cut in
    // its place.
    let mut work: Vec<u32> = (0..triangles.len() as u32).filter(|&t| longest_edge(&positions, &triangles[t as usize]).is_some()).collect();
    let mut on: HashMap<u64, Vec<u32>> = HashMap::with_capacity(work.len() * 3 / 2);
    for &i in &work {
        let t = triangles[i as usize];
        for k in 0..3 {
            on.entry(key(t[k], t[(k + 1) % 3])).or_default().push(i);
        }
    }
    work.reverse();
    while let Some(t) = work.pop() {
        // Cut until the triangle's own longest edge is short enough. Every cut is on an edge at
        // least as long as that one, so a triangle once finished is never cut again.
        while let Some(edge) = longest_edge(&positions, &triangles[t as usize]).filter(|e| e.length > longest) {
            let terminal = terminal_edge(&positions, &triangles, &on, t, edge)?;
            // A cut that cuts nothing would come round again for ever.
            if bisect(&mut positions, &mut triangles, &mut parent, &mut on, &mut work, terminal) == 0 {
                return None;
            }
            // However the estimate went, never more than asked for.
            if triangles.len() > most {
                return None;
            }
        }
    }
    Some(Finer { positions, triangles, parent })
}

/// The end of `t`'s longest-edge path: from `t` across its longest edge to the neighbour, as long
/// as that neighbour's longest edge is a longer one, until an edge is the longest of every
/// triangle on it. `None` only if the path would not end (it always does: each step is to a
/// longer edge in [`Edge::order`]).
fn terminal_edge(positions: &[Vec3], triangles: &[[u32; 3]], on: &HashMap<u64, Vec<u32>>, t: u32, edge: Edge) -> Option<Edge> {
    let (mut at, mut edge) = (t, edge);
    for _ in 0..=triangles.len() {
        let next = on.get(&edge.key()).and_then(|list| {
            list.iter().copied().filter(|&n| n != at).find_map(|n| {
                let theirs = longest_edge(positions, triangles.get(n as usize)?)?;
                (theirs.key() != edge.key()).then_some((n, theirs))
            })
        });
        match next {
            Some((n, theirs)) => (at, edge) = (n, theirs),
            None => return Some(edge),
        }
    }
    None
}

/// `edge` cut at its middle in every triangle on it, each into two that wind as it did. Returns
/// how many triangles it cut.
fn bisect(
    positions: &mut Vec<Vec3>,
    triangles: &mut Vec<[u32; 3]>,
    parent: &mut Vec<u32>,
    on: &mut HashMap<u64, Vec<u32>>,
    work: &mut Vec<u32>,
    edge: Edge,
) -> usize {
    let Some(sharing) = on.remove(&edge.key()) else { return 0 };
    let (Some(&a), Some(&b)) = (positions.get(edge.from as usize), positions.get(edge.to as usize)) else { return 0 };
    let before = triangles.len();
    let middle = positions.len() as u32;
    positions.push((a + b) / 2.0);
    for s in sharing {
        let Some(&old) = triangles.get(s as usize) else { continue };
        // Turned so the cut edge runs from its first corner, as the triangle winds.
        let Some(r) = (0..3).find(|&r| key(old[r], old[(r + 1) % 3]) == edge.key()) else { continue };
        let (p, q, c) = (old[r], old[(r + 1) % 3], old[(r + 2) % 3]);
        let new = triangles.len() as u32;
        // The other two edges now belong to one half each; the edge to the middle to both.
        if let Some(list) = on.get_mut(&key(q, c)) {
            for slot in list.iter_mut().filter(|x| **x == s) {
                *slot = new;
            }
        }
        on.entry(key(p, middle)).or_default().push(s);
        on.entry(key(middle, q)).or_default().push(new);
        on.entry(key(middle, c)).or_default().extend([s, new]);
        triangles[s as usize] = [p, middle, c];
        triangles.push([middle, q, c]);
        parent.push(parent.get(s as usize).copied().unwrap_or(s));
        work.push(new);
    }
    triangles.len() - before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::edge_neighbours;

    fn area(s: &Scan) -> f64 {
        (0..s.triangle_count()).filter_map(|t| s.corners(t)).map(|[a, b, c]| (b - a).cross(c - a).len() / 2.0).sum()
    }

    /// `cut` is `scan` cut to `longest`: the same surface (every piece in its parent's plane and
    /// wound as it, the area the same), every edge at most `longest`, closed still (every edge on
    /// two triangles).
    fn same_surface_closed(scan: &Scan, cut_up: &Finer, longest: f64) {
        let cut = cut_up.scan();
        assert!((area(&cut) - area(scan)).abs() < 1e-9 * area(scan).max(1.0), "{} {}", area(&cut), area(scan));
        for (t, &parent) in cut_up.parent.iter().enumerate().take(cut.triangle_count()) {
            let [a, b, c] = cut.corners(t).unwrap();
            assert!(a.dist(b) <= longest && b.dist(c) <= longest && c.dist(a) <= longest);
            let [p, q, r] = scan.corners(parent as usize).unwrap();
            let n = (q - p).cross(r - p).normalized().unwrap();
            assert!([a, b, c].iter().all(|x| (*x - p).dot(n).abs() < 1e-9), "in its parent's plane");
            assert!((b - a).cross(c - a).normalized().unwrap().dot(n) > 0.999, "wound as its parent");
        }
        let neighbours = edge_neighbours(&cut);
        assert!((0..cut.triangle_count()).all(|t| neighbours.of(&cut, t).count() == 3), "every edge meets another triangle's");
    }

    /// A box of 12 long triangles cut to edges of at most 2. Cutting edges that are not a
    /// triangle's longest ran away here (tens of millions of triangles); longest-edge cuts make
    /// about the box's area over a cut triangle's.
    #[test]
    fn a_box_cut_finer_is_the_same_box_closed() {
        let (positions, triangles) = crate::test_meshes::cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0));
        let scan = Scan::new(&positions, &triangles);
        let cut_up = finer(&scan, 2.0, 1_000_000).unwrap();
        let count = cut_up.triangles.len();
        assert!(count > 500 && (count as f64) < 4.0 * expected_count(&scan, 2.0), "{count} vs {}", expected_count(&scan, 2.0));
        same_surface_closed(&scan, &cut_up, 2.0);
        assert_eq!(finer(&scan, 0.01, 1000), None, "too many");
        assert_eq!(finer(&scan, 2.0, count - 1), None, "one fewer than it takes");
        assert_eq!(finer(&scan, f64::NAN, 1000), None);
    }

    /// A CAD model's cylinder, slivers about as long against their width as T-LESS's models
    /// have (down the side) and fans of slivers on the ends.
    #[test]
    fn a_cylinder_of_slivers_cut_finer_keeps_its_count_down() {
        let (positions, triangles) = crate::test_meshes::cad_cylinder(10.0, 30.0, 48);
        let scan = Scan::new(&positions, &triangles);
        let cut_up = finer(&scan, 1.0, 2_000_000).unwrap();
        let count = cut_up.triangles.len();
        assert!((count as f64) < 4.0 * expected_count(&scan, 1.0), "{count} vs {}", expected_count(&scan, 1.0));
        same_surface_closed(&scan, &cut_up, 1.0);
    }

    /// Hostile input: non-finite points, missing points and repeated corners are left, and the
    /// rest is still cut.
    #[test]
    fn broken_triangles_are_left_and_the_rest_cut() {
        let positions =
            vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0), Vec3::new(f64::INFINITY, 0.0, 0.0), Vec3::new(f64::NAN, 1.0, 1.0)];
        let triangles = vec![[0, 1, 2], [0, 1, 3], [1, 2, 4], [0, 0, 1], [0, 1, 99]];
        let scan = Scan::new(&positions, &triangles);
        let cut_up = finer(&scan, 1.0, 100_000).unwrap();
        let cut = cut_up.scan();
        let pieces = (0..cut.triangle_count()).filter(|&t| cut_up.parent[t] == 0).count();
        assert!(pieces > 50, "{pieces}");
        for t in (0..cut.triangle_count()).filter(|&t| cut_up.parent[t] == 0) {
            let [a, b, c] = cut.corners(t).unwrap();
            assert!(a.dist(b) <= 1.0 && b.dist(c) <= 1.0 && c.dist(a) <= 1.0);
        }
        assert!((area(&cut) - area(&scan)).abs() < 1e-9, "{} {}", area(&cut), area(&scan));
    }
}
