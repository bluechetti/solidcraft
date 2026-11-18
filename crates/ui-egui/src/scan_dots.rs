//! The Scan workspace's marks drawn as point-cloud dots: a deviation map, found shapes, a kept
//! selection, a dialog's preview. The viewport draws them on its point-cloud path, whose records
//! are cells (a point, a segment or a triangle, `solidcraft_viewport_gpu::view::Cell`); a dot is a cell
//! with its three corners on the point. It takes its own colour unless it is given a value and a
//! cloud style that colours by it.

use solidcraft_viewport_gpu::view::{CELL_SIZE, Cell, LIT_BY_SURFACE, cloud_cell};

/// Bytes per dot's record.
pub const DOT_SIZE: usize = CELL_SIZE;

/// One mark: where it is, a value, and its own colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dot {
    pub at: [f32; 3],
    pub value: f32,
    pub rgb: [u8; 3],
}

/// Append a dot's record.
pub fn cloud_dot(out: &mut Vec<u8>, d: &Dot) {
    cloud_cell(out, &Cell { corners: [d.at; 3], value: [d.value; 3], rgb: d.rgb, ..Cell::default() });
}

/// The most of a scan's triangles a colouring built once per change paints (Scan Deviation, Flat
/// Template): a painted triangle's record is a dot's size and a scan has about twice as many
/// triangles as points, so past this its points are drawn as dots instead.
pub const PAINT_MOST: usize = 1_000_000;

/// How far a painted point stands off the scan, as a share of the longest side round it
/// ([`PaintLifts`]): drawn over the scan's triangles, not fighting them for depth.
const PAINT_LIFT: f64 = 0.05;

/// A scan's triangles painted lifted corner by corner: each point once, along the scan's way out
/// there (the normals of the triangles round it, weighted by their areas), by [`PAINT_LIFT`] of
/// the longest side round it; and each triangle lit by its corners' way out, not its own normal.
/// Lifted each along its own normal, the triangles of a fine noisy scan came apart and the bare
/// scan showed through the cracks between them, and lit each by its own normal they sparkled: a
/// selection on the holes plate's top was speckled white.
pub struct PaintLifts {
    up: Vec<solidcore_record::Vec3>,
    lift: Vec<solidcore_record::Vec3>,
}

impl PaintLifts {
    pub fn of(positions: &[solidcore_record::Vec3], triangles: &[[u32; 3]]) -> PaintLifts {
        use solidcraft_engine::geom::prelude::*;
        let zero = solidcore_record::Vec3::ZERO;
        let (mut sum, mut longest) = (vec![zero; positions.len()], vec![0.0f64; positions.len()]);
        for tri in triangles {
            let at = |i: u32| positions.get(i as usize).copied();
            let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) else { continue };
            let (n, side) = ((b - a).cross(c - a), a.dist(b).max(b.dist(c)).max(c.dist(a)));
            for &v in tri {
                if let (Some(s), Some(l)) = (sum.get_mut(v as usize), longest.get_mut(v as usize)) {
                    *s += n;
                    *l = l.max(side);
                }
            }
        }
        let up: Vec<_> = sum.iter().map(|n| n.normalized().unwrap_or(zero)).collect();
        let lift = up.iter().zip(&longest).map(|(u, l)| *u * (PAINT_LIFT * l)).collect();
        PaintLifts { up, lift }
    }

    /// The triangle `tri` of the scan whose points are `positions`, painted in `rgb` for `value`.
    /// Nothing for a triangle with no area or a corner out of range.
    pub fn paint(&self, out: &mut Vec<u8>, positions: &[solidcore_record::Vec3], tri: [u32; 3], value: f32, rgb: [u8; 3]) {
        use solidcraft_engine::geom::prelude::*;
        let zero = solidcore_record::Vec3::ZERO;
        let at = |i: u32| positions.get(i as usize).map(|p| *p + self.lift.get(i as usize).copied().unwrap_or(zero));
        let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) else { return };
        if (b - a).cross(c - a).normalized().is_none() {
            return;
        }
        let up = tri.iter().map(|&v| self.up.get(v as usize).copied().unwrap_or(zero)).fold(zero, |s, n| s + n);
        let (neighbours, has) = match up.normalized() {
            Some(u) => ([[u.x as f32, u.y as f32, u.z as f32], [0.0; 3]], LIT_BY_SURFACE),
            None => ([[0.0; 3]; 2], 0),
        };
        let f = |p: solidcore_record::Vec3| [p.x as f32, p.y as f32, p.z as f32];
        cloud_cell(out, &Cell { corners: [f(a), f(b), f(c)], neighbours, value: [value; 3], rgb, has, ..Cell::default() });
    }
}

/// Triangles given by their corners (a scan's, as a tool found them), each with a value and a
/// colour, painted as [`PaintLifts`] paints a scan: their corners joined where they are the same
/// point, so neighbours are lifted together and no crack opens between them.
pub fn paint_corners(out: &mut Vec<u8>, triangles: impl IntoIterator<Item = ([solidcore_record::Vec3; 3], f32, [u8; 3])>) {
    let mut index: std::collections::HashMap<[u64; 3], u32> = std::collections::HashMap::new();
    let (mut positions, mut joined, mut paint) = (Vec::new(), Vec::new(), Vec::new());
    for (corners, value, rgb) in triangles {
        let tri = corners.map(|p| {
            *index.entry([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).or_insert_with(|| {
                positions.push(p);
                (positions.len() - 1) as u32
            })
        });
        joined.push(tri);
        paint.push((tri, value, rgb));
    }
    let lifts = PaintLifts::of(&positions, &joined);
    out.reserve(paint.len() * DOT_SIZE);
    for (tri, value, rgb) in paint {
        lifts.paint(out, &positions, tri, value, rgb);
    }
}

/// What a cloud of only dots has of segments and triangles.
pub fn none() -> std::sync::Arc<Vec<u8>> {
    std::sync::Arc::new(Vec::new())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dot_is_one_record_with_its_corners_on_the_point() {
        let mut out = Vec::new();
        super::cloud_dot(&mut out, &super::Dot { at: [1.0, 2.0, 3.0], value: 0.5, rgb: [10, 20, 30] });
        assert_eq!(out.len(), super::DOT_SIZE);
        let float = |i: usize| f32::from_le_bytes([out[4 * i], out[4 * i + 1], out[4 * i + 2], out[4 * i + 3]]);
        assert_eq!((0..9).map(float).collect::<Vec<_>>(), [1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
        assert_eq!(&out[super::DOT_SIZE - 4..], [10, 20, 30, 0], "its own colour, and no value or filter to colour or hide it by");
    }

    /// Two triangles of a scan folded 60° along their shared edge: painted lifted corner by
    /// corner, the edge's two corners are the same in both (no crack for the scan to show through,
    /// as there was lifting each along its own normal), and both are lit by the same way out.
    #[test]
    fn painted_triangles_stay_joined_and_lit_alike() {
        use solidcore_record::Vec3;
        let positions = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.5, 1.0, 0.0), Vec3::new(0.5, -0.5, -0.866)];
        let triangles = [[0u32, 1, 2], [1u32, 0, 3]];
        let lifts = super::PaintLifts::of(&positions, &triangles);
        let mut out = Vec::new();
        for tri in triangles {
            lifts.paint(&mut out, &positions, tri, 1.0, [1, 2, 3]);
        }
        assert_eq!(out.len(), 2 * super::DOT_SIZE);
        let corner = |cell: usize, k: usize| -> [f32; 3] {
            let at = cell * super::DOT_SIZE + k * 12;
            std::array::from_fn(|i| f32::from_le_bytes(out[at + i * 4..at + i * 4 + 4].try_into().unwrap()))
        };
        // Triangle 0's corners 0 and 1 are triangle 1's corners 1 and 0.
        assert_eq!(corner(0, 0), corner(1, 1), "point 0 lifted once");
        assert_eq!(corner(0, 1), corner(1, 0), "point 1 lifted once");
        assert!(corner(0, 0) != [0.0, 0.0, 0.0], "lifted off the scan");
        let lit = |cell: usize| out[cell * super::DOT_SIZE + super::DOT_SIZE - 1];
        assert_eq!((lit(0), lit(1)), (super::LIT_BY_SURFACE, super::LIT_BY_SURFACE), "lit by the surface");
    }
}
