//! The Scan workspace's marks drawn as point-cloud dots: a deviation map, found shapes, a kept
//! selection, a dialog's preview. The viewport draws them on its point-cloud path, whose records
//! are cells (a point, a segment or a triangle, `solidcraft_viewport_gpu::view::Cell`); a dot is a cell
//! with its three corners on the point. It takes its own colour unless it is given a value and a
//! cloud style that colours by it.

use solidcraft_viewport_gpu::view::{CELL_SIZE, Cell, cloud_cell};

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

/// How far a painted triangle stands off the scan, as a share of its longest side (as the scan
/// tools' own colouring lifts it): drawn over the scan's triangle, not fighting it for depth.
const PAINT_LIFT: f64 = 0.05;

/// One of the scan's triangles, its corners `a`, `b`, `c`, painted in `rgb` for `value`: lifted a
/// sliver off the scan along its normal. Nothing for a triangle with no area.
pub fn cloud_painted(out: &mut Vec<u8>, [a, b, c]: [solidcore_record::Vec3; 3], value: f32, rgb: [u8; 3]) {
    use solidcraft_engine::geom::prelude::*;
    let Some(n) = (b - a).cross(c - a).normalized() else { return };
    let lift = n * (PAINT_LIFT * a.dist(b).max(b.dist(c)).max(c.dist(a)));
    let f = |p: solidcore_record::Vec3| {
        let p = p + lift;
        [p.x as f32, p.y as f32, p.z as f32]
    };
    cloud_cell(out, &Cell { corners: [f(a), f(b), f(c)], value: [value; 3], rgb, ..Cell::default() });
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
}
