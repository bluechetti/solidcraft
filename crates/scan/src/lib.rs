//! Scan to CAD (#8615): the maths of the Scan workspace, on a scanned mesh as shared positions and
//! triangles in millimetres, Z up. It knows nothing of documents or commands; the engine's `scan.*`
//! commands hand it a mesh body's triangles and turn its answers into ordinary features (a move is
//! a Move). Ported from SurfaceCraft, the same author's scan-to-CAD app.
//!
//! - [`align`]: scans aligned by points picked in pairs, and several passes of one part at once.
//! - [`base`]: the ways a scan could stand on the ground (its broad flat faces, the poses it would
//!   rest in, its own axes) and a flat face grown from a clicked triangle.
//! - [`chain`]: a run of a section as lines and arcs, each fitted to its points (the chain fitter).
//! - [`curve`]: a curve drawn on the scan through points clicked on it, kept as a 3D sketch spline.
//! - [`denoise`]: a scan's noise smoothed away, its sharp edges kept, at the strength that leaves
//!   least error.
//! - [`deviation`]: how far the scan is from the model built from it.
//! - [`fillet`]: a fillet's radius measured from the scan near a model's edges.
//! - [`fit`]: a plane or a cylinder fitted to the part of the scan round a picked point.
//! - [`flatten`]: part of a scan unrolled onto a plane, and points of the pattern lifted back.
//! - [`free_form`]: a Free Form: a net over four corners clicked on the scan, fitted to it.
//! - [`holes`]: the holes in a scan and the smooth patches that close them.
//! - [`mirror_fill`]: a scan completed by its mirror image where it has nothing.
//! - [`net`]: a Free Form's net changed as a whole: split, copied out at an edge, doubled, creased.
//! - [`network`]: a Free Form of several faces sharing whole sides: how they join, a face added on
//!   an open edge, and points dragged on the surface with the faces held tangent where they meet.
//! - [`patches`]: smooth patches over a region of the scan, in a grid, in blocks, or as a quad whose
//!   edges follow the region's edge.
//! - [`pieces`]: the pieces of a scan, and those small enough to float.
//! - [`place`]: rigid moves that stand a scan on a base, rest it on the grid, centre it over the
//!   origin or lay it along its own axes.
//! - [`poisson`]: a surface made from a point cloud by screened Poisson reconstruction, trimmed
//!   where the points are sparse.
//! - [`profile`]: the profiles Reconstruct sketches, read off the faces fitted: a prism's from its
//!   walls, a turned part's from its faces held to the axis.
//! - [`reconstruct`]: what Reconstruct reads off Find All's shapes: prisms, a turned part,
//!   repeats and round holes.
//! - [`reduce`]: fewer triangles for a scan, cheapest collapses first (quadric errors).
//! - [`register`]: one scan laid onto another (a second pass onto the first): point-to-plane
//!   ICP both ways, from starts the principal axes and matched surface features give.
//! - [`remesh`]: a scan, or passes of one part laid on each other, made again as one even
//!   surface (volumetric fusion, sharp edges kept by dual contouring).
//! - [`repair`]: copies, edges of three or more faces and slivers put right.
//! - [`section`]: where a plane cuts the scan, as runs of points in the plane.
//! - [`segment`]: Find All: the scan cut into regions, each a simple shape or freeform.
//! - [`split`]: Split Mesh: a scan cut by a plane, a shape, a body, a curve or a sketch.
//! - [`surface`]: a smooth B-spline surface fitted to the part of the scan round a picked point.
//! - [`surface_check`]: Surface Check: where a B-spline surface folds, pinches or passes through
//!   itself, and how square and even its parameters are.
//! - [`surface_edit`]: a surface's control points moved (with a falloff, or slid along the
//!   surface) and the rest fitted to the scan again.
//! - [`symmetry`]: the plane a scan is a mirror image about, found by itself or near a line drawn
//!   along it, how far each point's reflection lands from the scan, and the move that squares
//!   the scan to it.

pub mod align;
pub mod base;
pub mod boundary_face;
pub mod chain;
pub mod constrain;
pub mod curve;
pub mod denoise;
pub mod deviation;
pub mod face_options;
pub mod fillet;
pub mod fit;
pub mod flatten;
pub mod free_form;
pub mod holes;
mod hull;
pub mod merge;
pub mod mirror_fill;
pub mod net;
mod net_fit;
pub mod network;
mod par;
pub mod patches;
pub mod pattern;
pub mod pieces;
pub mod place;
pub mod poisson;
pub mod profile;
pub mod reconstruct;
pub mod reduce;
pub mod register;
pub mod remesh;
pub mod repair;
pub mod section;
pub mod segment;
pub mod selection;
pub mod split;
mod subdivide;
pub mod surface;
pub mod surface_check;
pub mod surface_edit;
pub mod symmetric;
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

/// `a · x = b` by Gaussian elimination with partial pivoting; `None` when singular.
pub(crate) fn solve_dense(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if !(a[pivot][col].abs() > 1e-300) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, below) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (offset, row) in below.iter_mut().enumerate() {
            let f = row[col] / pivot_row[col];
            if f != 0.0 {
                for (cell, &p) in row.iter_mut().zip(pivot_row.iter()).skip(col) {
                    *cell -= f * p;
                }
                b[col + 1 + offset] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
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

    /// A closed cylinder of `radius` and `height` on the Z axis as a CAD program writes one, `n`
    /// long slivers down its side and a fan of slivers on each end, wound outward.
    pub fn cad_cylinder(radius: f64, height: f64, n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for z in [0.0, height] {
            for i in 0..n {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                positions.push(Vec3::new(radius * a.cos(), radius * a.sin(), z));
            }
        }
        positions.extend([Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, height)]);
        let (bottom, top) = (2 * n, 2 * n + 1);
        let mut triangles = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            let (i2, j2) = (i + n, j + n);
            triangles.extend([[i, j, j2], [i, j2, i2], [bottom, j, i], [top, i2, j2]]);
        }
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

    /// A part turned about Z: each point of `profile` (height, radius), in order, a ring of
    /// `sides` points round the axis, a point on the axis one pole, and the rings joined in order
    /// (the last to the first when `closed`). Wound outward for a profile that runs
    /// counter-clockwise in (height, radius).
    pub fn revolved(profile: &[[f64; 2]], sides: u32, closed: bool) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        // Each profile point's first position, and whether it is a pole.
        let mut rings = Vec::new();
        for &[z, r] in profile {
            rings.push((positions.len() as u32, r <= 1e-12));
            if r <= 1e-12 {
                positions.push(Vec3::new(0.0, 0.0, z));
            } else {
                for k in 0..sides {
                    let a = std::f64::consts::TAU * f64::from(k) / f64::from(sides);
                    positions.push(Vec3::new(r * a.cos(), r * a.sin(), z));
                }
            }
        }
        let at = |(base, pole): (u32, bool), k: u32| if pole { base } else { base + k % sides };
        let mut triangles = Vec::new();
        let pairs = if closed { rings.len() } else { rings.len().saturating_sub(1) };
        for i in 0..pairs {
            let (a, b) = (rings[i], rings[(i + 1) % rings.len()]);
            for k in 0..sides {
                let (a0, a1, b0, b1) = (at(a, k), at(a, k + 1), at(b, k), at(b, k + 1));
                if !a.1 {
                    triangles.push([a0, a1, b1]);
                }
                if !b.1 {
                    triangles.push([a0, b1, b0]);
                }
            }
        }
        (positions, triangles)
    }

    /// A prism: the closed counter-clockwise `outline` (in X and Y) from z `low` to `high`, its
    /// walls cut into `rows` along it and its ends fanned from their middles, wound outward.
    pub fn extruded(outline: &[[f64; 2]], low: f64, high: f64, rows: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let n = outline.len() as u32;
        let mut positions = Vec::new();
        for j in 0..=rows {
            let z = low + (high - low) * f64::from(j) / f64::from(rows);
            positions.extend(outline.iter().map(|&[x, y]| Vec3::new(x, y, z)));
        }
        let mut triangles = Vec::new();
        for j in 0..rows {
            for k in 0..n {
                let (a, b) = (j * n + k, j * n + (k + 1) % n);
                triangles.push([a, b, b + n]);
                triangles.push([a, b + n, a + n]);
            }
        }
        let middle = outline.iter().fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
        let middle = [middle[0] / f64::from(n), middle[1] / f64::from(n)];
        for (z, top) in [(low, false), (high, true)] {
            let centre = positions.len() as u32;
            positions.push(Vec3::new(middle[0], middle[1], z));
            let ring = if top { rows * n } else { 0 };
            for k in 0..n {
                let (a, b) = (ring + k, ring + (k + 1) % n);
                triangles.push(if top { [centre, a, b] } else { [centre, b, a] });
            }
        }
        (positions, triangles)
    }

    /// A rounded rectangle from (0, 0) to (`w`, `h`), corners of radius `r`, counter-clockwise,
    /// its sides cut every `step` or so.
    pub fn rounded_rectangle(w: f64, h: f64, r: f64, step: f64) -> Vec<[f64; 2]> {
        let mut points = Vec::new();
        let corners = [
            ([w - r, r], -std::f64::consts::FRAC_PI_2),
            ([w - r, h - r], 0.0),
            ([r, h - r], std::f64::consts::FRAC_PI_2),
            ([r, r], std::f64::consts::PI),
        ];
        let sides = [([r, 0.0], [w - r, 0.0]), ([w, r], [w, h - r]), ([w - r, h], [r, h]), ([0.0, h - r], [0.0, r])];
        for (([cx, cy], start), (from, to)) in corners.iter().zip(sides) {
            let length = (to[0] - from[0]).hypot(to[1] - from[1]);
            let cuts = ((length / step).ceil() as u32).max(1);
            points.extend(steps(from, to, cuts).into_iter().take(cuts as usize));
            for i in 0..24 {
                let a = start + std::f64::consts::FRAC_PI_2 * f64::from(i) / 24.0;
                points.push([cx + r * a.cos(), cy + r * a.sin()]);
            }
        }
        points
    }

    /// Points `from` to `to` in `n` steps (`n + 1` points, both ends).
    pub fn steps(from: [f64; 2], to: [f64; 2], n: u32) -> Vec<[f64; 2]> {
        (0..=n).map(|i| f64::from(i) / f64::from(n)).map(|t| [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t]).collect()
    }

    /// `positions` turned by `angle` radians about the unit `axis` through the origin, then moved.
    pub fn turned(positions: &[Vec3], axis: Vec3, angle: f64, shift: Vec3) -> Vec<Vec3> {
        use solidcraft_geom::prelude::*;
        let (s, c) = angle.sin_cos();
        positions.iter().map(|p| *p * c + axis.cross(*p) * s + axis * (axis.dot(*p) * (1.0 - c)) + shift).collect()
    }
}
