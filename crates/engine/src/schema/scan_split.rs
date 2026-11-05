//! Contracts and result types for the commands in `cmd/scan_split.rs`: Split Mesh, a scan cut by
//! a plane, a fitted shape, a body, a 3D curve or a sketch.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{LengthExpr, Point3};

/// Cut a scan along a cutter, keeping one side, both as scans of their own, or only the crossing
/// as 3D sketch curves. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSplitParams {
    /// The scan to cut. Default: the selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// A plane to cut with: XY, XZ, YZ, a construction plane's name, or {origin, normal}.
    #[serde(default)]
    pub plane: Option<Value>,
    /// A shape to cut with: {kind: plane|cylinder|sphere|cone|torus, at: [x, y, z]} fitted to the
    /// scan round that point as the Fit tools fit one, or the shape itself: {kind, point, normal,
    /// axis, radius, center, apex, half_angle (degrees), major, minor} as each kind needs.
    #[serde(default)]
    pub shape: Option<Value>,
    /// A body (a solid, a surface or another scan) to cut with, by its surface.
    #[serde(default)]
    pub cutter: Option<String>,
    /// A 3D sketch curve to cut with: {sketch, curve} (a curve Curve on Scan drew), or
    /// {points: [[x, y, z], …], closed?}.
    #[serde(default)]
    pub curve: Option<Value>,
    /// A sketch (id or name) whose closed outlines, or one open curve, cut square to its plane.
    #[serde(default)]
    pub sketch: Option<Value>,
    /// Part the scan along the edge of what is selected on it (with both, the selection becomes
    /// a scan of its own).
    #[serde(default)]
    pub selection: Option<bool>,
    /// What to keep: both (the default: the scan keeps the cutter's positive side, in front,
    /// outside or left, and a new scan the other), front, back, or curves (the crossing kept as 3D
    /// sketch curves, the scan untouched). The sides' own names (in_front, behind, outside, inside,
    /// left, right) are taken too.
    #[serde(default)]
    pub keep: Option<String>,
    /// A thickness: two cuts half of it either side, and what lies between a scan of its own
    /// (kept with both, taken off with one side). Default: 0, one cut.
    #[serde(default)]
    pub envelope: Option<LengthExpr>,
    /// How far a curve's cut reaches from the curve, or a sketch's either side of its plane, so
    /// that the scan farther off stays whole. Default: 0, right through.
    #[serde(default)]
    pub offset: Option<LengthExpr>,
    /// Only the triangles facing this way are cut (Facing Faces Only).
    #[serde(default)]
    pub facing: Option<Point3>,
}

/// What `scan.split` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSplitResult {
    /// The scan cut, and what cut it.
    pub body: String,
    pub cutter: String,
    /// What was kept, and what the cutter's negative and positive sides are called.
    pub keep: String,
    pub sides: [String; 2],
    /// The scan's triangles after, and the new scans made (the other side, the envelope's slab).
    pub triangles: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub made: Vec<ScanSplitMade>,
    /// Triangles the cut went through, and the new edge's length and runs (how many close).
    pub crossed: usize,
    pub length: f64,
    pub runs: usize,
    pub closed_runs: usize,
    /// How far the middle of a piece of the new edge is from the cutter, at most.
    pub chord_error: f64,
    /// The cutter stops short of the scan, so the cut carried on past its side to finish.
    pub carried_on: bool,
    /// With keep curves: the 3D sketch and its curves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sketch: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<String>,
    /// The scan's feature, changed (none with keep curves).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A scan Split Mesh made.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSplitMade {
    pub body: String,
    /// Which side it is: behind, inside, right… or slab.
    pub side: String,
    pub triangles: usize,
    pub feature: u64,
}

#[cfg(test)]
mod tests {
    use super::ScanSplitParams;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(
            input_schema_for::<ScanSplitParams>(),
            &["body", "curve", "cutter", "envelope", "facing", "keep", "offset", "plane", "selection", "shape", "sketch"],
        );
    }
}
