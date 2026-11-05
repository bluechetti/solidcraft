//! Scan to CAD (#8615): the maths of the Scan workspace, on a scanned mesh as shared positions and
//! triangles in millimetres, Z up. It knows nothing of documents or commands; the engine's `scan.*`
//! commands hand it a mesh body's triangles and turn its answers into ordinary features (a move is
//! a Move). Ported from SurfaceCraft, the same author's scan-to-CAD app.
//!
//! - [`base`]: the ways a scan could stand on the ground (its broad flat faces, the poses it would
//!   rest in, its own axes) and a flat face grown from a clicked triangle.
//! - [`chain`]: a run of a section as lines and arcs, each fitted to its points (the chain fitter).
//! - [`curve`]: a curve drawn on the scan through points clicked on it, kept as a 3D sketch spline.
//! - [`deviation`]: how far the scan is from the model built from it.
//! - [`fit`]: a plane or a cylinder fitted to the part of the scan round a picked point.
//! - [`free_form`]: a Free Form: a net over four corners clicked on the scan, fitted to it.
//! - [`holes`]: the holes in a scan and the smooth patches that close them.
//! - [`mirror_fill`]: a scan completed by its mirror image where it has nothing.
//! - [`net`]: a Free Form's net changed as a whole: split, copied out at an edge, doubled, creased.
//! - [`pieces`]: the pieces of a scan, and those small enough to float.
//! - [`place`]: rigid moves that stand a scan on a base, rest it on the grid, centre it over the
//!   origin or lay it along its own axes.
//! - [`reduce`]: fewer triangles for a scan, cheapest collapses first (quadric errors).
//! - [`repair`]: copies, edges of three or more faces and slivers put right.
//! - [`section`]: where a plane cuts the scan, as runs of points in the plane.
//! - [`surface`]: a smooth B-spline surface fitted to the part of the scan round a picked point.
//! - [`surface_edit`]: a surface's control points moved and the rest fitted to the scan again.
//! - [`symmetry`]: the plane a scan is a mirror image about, found by itself or near a line drawn
//!   along it, how far each point's reflection lands from the scan, and the move that squares
//!   the scan to it.

pub mod base;
pub mod chain;
pub mod curve;
pub mod deviation;
pub mod fit;
pub mod free_form;
pub mod holes;
mod hull;
pub mod merge;
pub mod mirror_fill;
pub mod net;
pub mod pieces;
pub mod place;
pub mod reduce;
pub mod repair;
pub mod section;
pub mod surface;
pub mod surface_edit;
pub mod symmetry;
pub mod wrap;

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

/// A scanned mesh as the commands hand it over. A triangle naming a missing position is skipped,
/// as are positions that are not finite.
#[derive(Clone, Copy, Debug)]
pub struct Scan<'a> {
    pub positions: &'a [Vec3],
    pub triangles: &'a [[u32; 3]],
}

impl<'a> Scan<'a> {
    pub fn new(positions: &'a [Vec3], triangles: &'a [[u32; 3]]) -> Self {
        Scan { positions, triangles }
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// The corners of triangle `t`, or `None` when it names a missing or non-finite position.
    pub fn corners(&self, t: usize) -> Option<[Vec3; 3]> {
        let tri = self.triangles.get(t)?;
        let at = |i: u32| self.positions.get(i as usize).copied().filter(|p| p.is_finite());
        Some([at(tri[0])?, at(tri[1])?, at(tri[2])?])
    }

    /// The finite positions.
    pub fn points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.positions.iter().copied().filter(|p| p.is_finite())
    }

    /// The box round the finite positions, `None` with none.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut points = self.points();
        let first = points.next()?;
        Some(points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// The length of the box's diagonal, 0 with no positions.
    pub fn extent(&self) -> f64 {
        self.bounds().map_or(0.0, |(lo, hi)| (hi - lo).len())
    }
}

/// `items` thinned evenly to about `limit` of them (every k-th from the first).
pub(crate) fn thinned<T: Copy>(items: &[T], limit: usize) -> Vec<T> {
    if items.len() <= limit || limit == 0 {
        return items.to_vec();
    }
    items.iter().step_by(items.len() / limit + 1).copied().collect()
}

/// Two unit directions square to the unit `n` and to each other.
pub(crate) fn in_plane(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = n.cross(helper).normalized().unwrap_or(Vec3::X);
    (u, n.cross(u))
}

/// The area-weighted middle of the scan's surface, `None` for a scan with no area.
pub(crate) fn surface_centre(scan: &Scan) -> Option<Vec3> {
    let (mut total, mut weighted) = (0.0, Vec3::ZERO);
    for t in 0..scan.triangle_count() {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let area = (b - a).cross(c - a).len() / 2.0;
        if area.is_finite() {
            total += area;
            weighted += (a + b + c) * (area / 3.0);
        }
    }
    (total > 0.0).then(|| weighted / total)
}

/// Each position's representative: the first position at the same place.
pub(crate) fn welded(positions: &[Vec3]) -> Vec<u32> {
    let mut first: HashMap<[u64; 3], u32> = HashMap::with_capacity(positions.len());
    positions
        .iter()
        .enumerate()
        .map(|(i, p)| *first.entry([(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]).or_insert(i as u32))
        .collect()
}

/// Sets of vertices joined by the triangles between them.
pub(crate) struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    pub(crate) fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect() }
    }

    pub(crate) fn find(&mut self, mut i: usize) -> usize {
        while let Some(&p) = self.parent.get(i) {
            if p == i {
                break;
            }
            let grand = self.parent.get(p).copied().unwrap_or(p);
            self.parent[i] = grand;
            i = p;
        }
        i
    }

    pub(crate) fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b
            && let Some(slot) = self.parent.get_mut(a)
        {
            *slot = b;
        }
    }
}

#[cfg(test)]
pub(crate) mod test_meshes {
    use solidcore_record::Vec3;

    /// A closed box from `lo` to `hi`, 12 triangles (the placing maths reads no winding).
    pub fn cuboid(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let positions = (0..8).map(p).collect();
        let triangles =
            vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        (positions, triangles)
    }

    /// A box made of a grid of `n` × `n` squares on each side, so that faces have many triangles.
    pub fn fine_cuboid(lo: Vec3, hi: Vec3, n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let size = hi - lo;
        // Each side: the axis it is square to, whether it is the high one, and its two in-plane axes.
        let sides: [(usize, bool); 6] = [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)];
        for (axis, high) in sides {
            let (u, v) = match (axis, high) {
                (0, false) => (2, 1),
                (0, true) => (1, 2),
                (1, false) => (0, 2),
                (1, true) => (2, 0),
                (2, false) => (1, 0),
                _ => (0, 1),
            };
            let base = positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let mut c = [lo.x, lo.y, lo.z];
                    let s = [size.x, size.y, size.z];
                    if high {
                        c[axis] += s[axis];
                    }
                    c[u] += s[u] * f64::from(i) / f64::from(n);
                    c[v] += s[v] * f64::from(j) / f64::from(n);
                    positions.push(Vec3::new(c[0], c[1], c[2]));
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let k = base + j * (n + 1) + i;
                    triangles.push([k, k + 1, k + n + 2]);
                    triangles.push([k, k + n + 2, k + n + 1]);
                }
            }
        }
        (positions, triangles)
    }

    /// `positions` turned by `angle` radians about the unit `axis` through the origin, then moved.
    pub fn turned(positions: &[Vec3], axis: Vec3, angle: f64, shift: Vec3) -> Vec<Vec3> {
        use solidcraft_geom::prelude::*;
        let (s, c) = angle.sin_cos();
        positions.iter().map(|p| *p * c + axis.cross(*p) * s + axis * (axis.dot(*p) * (1.0 - c)) + shift).collect()
    }
}
