//! Contracts and result types for the commands in `cmd/scan_flatten.rs`: part of a scan unrolled
//! into a flat pattern, and a drawing on the pattern rolled back onto the scan.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::Point3;

/// Unroll a region of the scan onto the XY plane beside it and sketch the pattern's outline. One
/// undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFlattenParams {
    /// The scan. Default: the picked, selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// A point on the scan: the region is the piece of the scan there. Default: what is selected
    /// on the scan, else the point picked.
    #[serde(default)]
    pub point: Option<Point3>,
    /// The region only as far as this round the point, in mm. Default: the whole piece.
    #[serde(default)]
    pub radius: Option<f64>,
    /// The region as the scan's triangle numbers, instead of a point.
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// A kept selection (Keep Selection) whose triangles are the region, instead of `triangles`.
    #[serde(default)]
    pub saved: Option<String>,
    /// Relax the pattern toward keeping each edge's length. Default: true.
    #[serde(default)]
    pub relax: Option<bool>,
    /// How far the outline's lines and arcs may stray from the region's edge, in mm. Default: 0.05.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// The sketch's name. Default: `Flat Pattern`.
    #[serde(default)]
    pub name: Option<String>,
}

/// What `scan.flatten` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFlattenResult {
    /// The scan and the pattern's sketch.
    pub body: String,
    pub sketch: u64,
    /// The region's triangles, and the outline's loops, lines, arcs and circles.
    pub triangles: usize,
    pub loops: usize,
    pub lines: usize,
    pub arcs: usize,
    pub circles: usize,
    /// The region's area on the scan and in the pattern (mm²), and the worst share a triangle's
    /// area changed by.
    pub area: f64,
    pub flat_area: f64,
    pub worst_area: f64,
    /// Each edge's stretch, |flat − scan| / scan: root mean square and most.
    pub strain_rms: f64,
    pub strain_max: f64,
    /// Triangles turned over in the pattern, rounds of relaxation, and whether the map settled.
    pub flipped: usize,
    pub relaxed: usize,
    pub settled: bool,
    /// Where the pattern was laid: its own coordinates plus this, on XY.
    pub shift: [f64; 2],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Roll the curves of a sketch drawn on a flat pattern back onto the scan as 3D sketch curves.
/// One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRollParams {
    /// The sketch drawn on the pattern (id or name). Default: the one being edited, else the
    /// pattern's own sketch.
    #[serde(default)]
    pub sketch: Option<Value>,
    /// The curves to roll. Default: every curve of the sketch that is not the pattern's outline.
    #[serde(default)]
    pub curves: Option<Vec<String>>,
    /// The Flat Pattern sketch (id or name). Default: the last made in this session.
    #[serde(default)]
    pub pattern: Option<Value>,
    /// The flattening's own parameters instead, as `scan.flatten` takes them.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub point: Option<Point3>,
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// A kept selection (Keep Selection) whose triangles are the region, instead of `triangles`.
    #[serde(default)]
    pub saved: Option<String>,
    #[serde(default)]
    pub relax: Option<bool>,
    /// How far a rolled curve may stray from the line it lifts to, in mm. Default: 0.02.
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// What `scan.roll_onto_scan` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRollResult {
    /// The 3D curves made, and the 3D sketch they are in.
    pub curves: Vec<String>,
    pub sketch: Value,
    /// The pieces rolled (a curve leaving the pattern is cut), the points lifted, and those off
    /// the pattern.
    pub pieces: usize,
    pub points: usize,
    pub off_pattern: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ScanFlattenParams, ScanRollParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<ScanFlattenParams>(), &["body", "name", "point", "radius", "relax", "saved", "tolerance", "triangles"]);
        assert_keys(
            input_schema_for::<ScanRollParams>(),
            &["body", "curves", "pattern", "point", "radius", "relax", "saved", "sketch", "tolerance", "triangles"],
        );
    }
}
