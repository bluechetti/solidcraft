//! Contracts and result types for the commands in `cmd/scan_select.rs`: selecting a scan's
//! triangles, keeping a selection in the design, and the curves of a selection's outline.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{LengthExpr, Point3};

/// A brush swept over the scan: the triangles within `radius` of each place, reached across the
/// scan from the triangle under it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanBrush {
    /// The places the brush passed, on the scan, in mm (1 to 10000).
    pub points: Vec<Point3>,
    /// The brush's radius.
    pub radius: LengthExpr,
}

/// A polygon drawn round what is seen: the triangles whose middles the view sees inside it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPolygon {
    /// The polygon's corners in order, as places in the design (3 to 10000): seen from the view,
    /// they draw the polygon on the picture.
    pub points: Vec<Point3>,
    /// The way the view looks.
    pub direction: Point3,
    /// The eye of a perspective view. Default: none, an orthographic view.
    #[serde(default)]
    pub eye: Option<Point3>,
    /// Take the triangles behind the scan's near side too. Default: false, only what is seen.
    #[serde(default)]
    pub through: Option<bool>,
}

/// A region grown from a place while the scan faces about the way it faces there.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRegion {
    /// A place on the scan, in mm.
    pub point: Point3,
    /// How far the scan may turn from the way it faces at the place, in degrees, 1 to 90.
    /// Default: 30, a flat face up to where its rounds turn away.
    #[serde(default)]
    pub angle: Option<f64>,
}

/// Select triangles of a scan: by a brush, a polygon round what is seen, a smooth region, a shape
/// Find All found, a kept selection, or the triangles by number; or change what is selected
/// (all, none, inverted, grown or shrunk by rings). One way at a time. A view of the scan, not
/// part of the design and not an undo step: keep it with `scan.keep_selection`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSelectParams {
    /// The mesh body. Default: the scan already selected on, else the selected or only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// replace, add or remove: what the triangles found do to what is selected. Default: replace.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub brush: Option<ScanBrush>,
    #[serde(default)]
    pub polygon: Option<ScanPolygon>,
    #[serde(default)]
    pub region: Option<ScanRegion>,
    /// A shape Find All found: its number, or `{point}` for the one under a place.
    #[serde(default)]
    pub shape: Option<Value>,
    /// A selection kept in the design, by name.
    #[serde(default)]
    pub saved: Option<String>,
    /// Triangles by number.
    #[serde(default)]
    pub triangles: Option<Vec<u32>>,
    /// Select the whole scan.
    #[serde(default)]
    pub all: Option<bool>,
    /// Select nothing.
    #[serde(default)]
    pub none: Option<bool>,
    /// Select what is not selected, and not what is.
    #[serde(default)]
    pub invert: Option<bool>,
    /// Grow what is selected by this many rings of triangles (1 to 1000).
    #[serde(default)]
    pub grow: Option<u32>,
    /// Shrink what is selected by this many rings of triangles (1 to 1000).
    #[serde(default)]
    pub shrink: Option<u32>,
    /// The part of the scan a solid built from it does not explain yet: the solid's name, or
    /// `{body, piece?, grow?}` (`piece`: which piece, 0 the largest, the default; `grow`: rings
    /// of triangles added round it, 0 to 1000).
    #[serde(default)]
    pub unexplained: Option<Value>,
}

/// A piece of a scan that a solid built from it does not explain: its triangles farther from
/// the solid than the scan's tolerance allows, reached from one another across the scan.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanUnexplainedPiece {
    /// Its number, 0 the largest.
    pub piece: usize,
    pub triangles: usize,
    /// Its area in mm².
    pub area: f64,
    /// `missing`: the scan stands outside the solid there, material the solid lacks; `extra`: the
    /// scan is inside the solid, material the solid should not have (a hole not cut).
    pub side: String,
    /// How far from the solid its farthest triangle is, in mm.
    pub deepest: f64,
    /// A place on the scan in it, near its middle.
    pub point: [f64; 3],
}

/// What a scan selection command returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSelectResult {
    /// The scan, how many of its triangles are selected and their area in mm², and how many it has.
    pub body: String,
    pub triangles: usize,
    pub area: f64,
    pub of: usize,
    /// Whether the selection is other than it was.
    pub changed: bool,
    /// With `unexplained`: every piece of the scan the solid does not explain, largest first
    /// (empty when it explains the whole scan), and how far from it the scan may be and count
    /// as explained, in mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pieces: Option<Vec<ScanUnexplainedPiece>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Select the triangles inside a closed 3D curve drawn on the scan: those under it inside it and
/// those reached from them across the scan without crossing it, so the far side of a part is not
/// taken. A view, not an undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSelectInsideParams {
    /// The mesh body. Default: the selected or only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// The 3D sketch the curve is in, by id or name. Default: the design's last `Scan Curves`.
    #[serde(default)]
    pub sketch: Option<Value>,
    /// The closed curve, or curves end to end in order. Default: the sketch's last curve.
    #[serde(default)]
    pub curves: Option<Vec<String>>,
    /// replace, add or remove. Default: replace.
    #[serde(default)]
    pub mode: Option<String>,
}

/// Keep what is selected on a scan in the design, to open again with `scan.select {saved}`. A
/// selection of the same name is replaced. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanKeepSelectionParams {
    /// Its name. Default: `Selection n`, the first number free.
    #[serde(default)]
    pub name: Option<String>,
}

/// What `scan.keep_selection` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanKeepSelectionResult {
    pub name: String,
    pub body: String,
    pub triangles: usize,
    /// Whether one of that name was replaced.
    pub replaced: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Take a kept selection out of the design. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanDeleteSelectionParams {
    pub name: String,
}

/// What `scan.delete_selection` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanDeleteSelectionResult {
    pub name: String,
    /// The kept selections left.
    pub left: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Curve from Selection Edge: the outline of what is selected on the scan, a staircase of triangle
/// sides, smoothed (the shrinking a smoothing brings taken back), laid on the scan and kept as a
/// closed 3D sketch spline within the tolerance: the outer outline alone, or each hole's too. One
/// undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCurveFromSelectionParams {
    /// Every outline, each hole's too (a hole of fewer than six triangle sides left out).
    /// Default: false, the longest alone.
    #[serde(default)]
    pub every: Option<bool>,
    /// How far along the outline it is smoothed. Default: twice the scan's typical triangle side.
    #[serde(default)]
    pub smooth: Option<LengthExpr>,
    /// How far each curve may stray from the smoothed outline. Default: the scan's typical
    /// triangle side, as Curve on Scan's.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// The 3D sketch to draw in, by id or name. Default: the 3D sketch being edited, else the
    /// design's `Scan Curves`, made the first time.
    #[serde(default)]
    pub sketch: Option<Value>,
}

/// What `scan.curve_from_selection` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCurveFromSelectionResult {
    pub body: String,
    /// The 3D curves made, longest outline first, and the sketch they are in.
    pub curves: Vec<String>,
    pub sketch: Value,
    /// Each curve's fit points.
    pub fit_points: Vec<usize>,
    /// The farthest a curve strays from its smoothed outline, in mm.
    pub max: f64,
    /// The outlines left out.
    pub skipped: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    use super::*;

    #[test]
    fn the_selection_commands_take_these_keys() {
        assert_keys(
            input_schema_for::<ScanSelectParams>(),
            &["all", "body", "brush", "grow", "invert", "mode", "none", "polygon", "region", "saved", "shape", "shrink", "triangles", "unexplained"],
        );
        assert_keys(input_schema_for::<ScanSelectInsideParams>(), &["body", "curves", "mode", "sketch"]);
        assert_keys(input_schema_for::<ScanKeepSelectionParams>(), &["name"]);
        assert_keys(input_schema_for::<ScanDeleteSelectionParams>(), &["name"]);
        assert_keys(input_schema_for::<ScanCurveFromSelectionParams>(), &["every", "sketch", "smooth", "tolerance"]);
    }
}
