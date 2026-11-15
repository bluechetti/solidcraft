//! Contracts and result types for the commands in `cmd/scan_fillet.rs`: a fillet's radius
//! measured from the scan, and the edges filleted at it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::Point3;

/// Measure the round the scan shows along a model's edges and fillet them at it. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFilletParams {
    /// The model's edges: points [x, y, z] on them, or {body, index}.
    pub edges: Vec<Value>,
    /// The model. Default: the one the edges are on, else the selected or only solid or surface
    /// body.
    #[serde(default)]
    pub body: Option<String>,
    /// The scan. Default: the selected or only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// How far from the edges the scan is read, in mm. Default: 10.
    #[serde(default)]
    pub reach: Option<f64>,
    /// The radius rounded to a multiple of this, in mm (0: exact). Default: 0.5.
    #[serde(default)]
    pub step: Option<f64>,
    /// Fillet the edges at the radius measured. Default: true; false only measures.
    #[serde(default)]
    pub apply: Option<bool>,
}

/// What `scan.fillet` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFilletResult {
    /// The model, the scan and the edges measured (the model's edge numbers).
    pub body: String,
    pub scan: String,
    pub edges: Vec<usize>,
    /// The radius that fits best, the one used, and the step it was rounded to (mm).
    pub fitted: f64,
    pub radius: f64,
    pub step: f64,
    /// How far the scan is from the round at the radius used (root mean square and most), and
    /// from the sharp edge.
    pub rms: f64,
    pub maximum: f64,
    pub sharp_rms: f64,
    /// Points used, of them on the round, and set aside as too far off.
    pub points: usize,
    pub on_round: usize,
    pub set_aside: usize,
    /// The lowest and highest radius station by station along the edges.
    pub station_low: f64,
    pub station_high: f64,
    /// How far the scan's faces sit from the model's along their outward normals, two to an
    /// edge.
    pub face_offsets: Vec<f64>,
    /// The Fillet feature, as `solid.fillet` replies, when the edges were filleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Measure the round under a point of the scan, from the scan alone; with `apply`, fillet the
/// model's edge that runs along it at that radius. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRadiusParams {
    /// A point on a round of the scan.
    pub at: Point3,
    /// The scan. Default: the selected or only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// The radius rounded to a multiple of this, in mm (0: exact). Default: 0.5.
    #[serde(default)]
    pub step: Option<f64>,
    /// The model whose edge runs along the round. Default: the selected or only solid or surface
    /// body, if there is one.
    #[serde(default)]
    pub body: Option<String>,
    /// Fillet the model's edge along the round at the radius measured. Default: false.
    #[serde(default)]
    pub apply: Option<bool>,
}

/// What `scan.radius` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRadiusResult {
    pub scan: String,
    /// The radius that fits best, the one given, and the step it was rounded to (mm).
    pub fitted: f64,
    pub radius: f64,
    pub step: f64,
    /// True for a round on an outside edge, false for one filling an inside corner.
    pub convex: bool,
    /// The angle the round turns through, degrees.
    pub swept: f64,
    /// The round's axis at its middle, the point on it level with the middle, and the middle of
    /// the round.
    pub axis: [f64; 3],
    pub centre: [f64; 3],
    pub middle: [f64; 3],
    /// Where the faces the round joins would meet were the edge sharp (none past 170°).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner: Option<[f64; 3]>,
    /// How far the round runs, mm.
    pub length: f64,
    /// How far the round's points lie off it, root mean square and most (mm).
    pub rms: f64,
    pub maximum: f64,
    /// Round a circular edge: the radius of the circle the round runs round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub along_circle: Option<f64>,
    /// The scan's triangles on the round.
    pub triangles: usize,
    /// The model's edge that runs along the round, {body, index}.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge: Option<Value>,
    /// The Fillet feature, as `solid.fillet` replies, when the edge was filleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ScanFilletParams, ScanRadiusParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<ScanFilletParams>(), &["apply", "body", "edges", "reach", "scan", "step"]);
        assert_keys(input_schema_for::<ScanRadiusParams>(), &["apply", "at", "body", "scan", "step"]);
    }
}
