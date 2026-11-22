//! Contracts and result types for `scan.section` in `cmd/scan_section_read.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cut the scan, and the model beside it, with a plane and read both cuts as data: the outlines
/// fitted to each (lines and arcs, as Section to Sketch draws them) and how far the scan's cut lies
/// from the model's. Makes nothing: no sketch, no feature.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCutParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// The same as `scan`.
    #[serde(default)]
    pub body: Option<String>,
    /// XY | XZ | YZ (with `offset`, along its normal: Z, -Y, X), or {origin: [x,y,z], normal: [x,y,z]}.
    pub plane: serde_json::Value,
    /// How far the named plane is moved along its normal (mm). Default 0.
    #[serde(default)]
    pub offset: Option<f64>,
    /// The model (a solid or surface body). Default: the design's shown solid, when there is one;
    /// `false` cuts the scan alone.
    #[serde(default)]
    pub model: Option<serde_json::Value>,
    /// How closely the outlines are fitted (mm). Default: the scan's tolerance, no finer than its noise.
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// A piece of an outline, in the plane's coordinates.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutPiece {
    /// line | arc | circle (a whole circle: `from` and `to` its point along the plane's x from the centre).
    pub kind: String,
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// An arc's centre and radius.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// An arc's way round: counter-clockwise seen along the plane's normal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ccw: Option<bool>,
}

/// One outline of a cut.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutOutline {
    pub closed: bool,
    pub pieces: Vec<ScanCutPiece>,
}

/// A place where the scan's cut lies off the model's.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutPlace {
    /// Where on the scan's cut, in world coordinates.
    pub at: [f64; 3],
    /// How far from the model's cut (mm).
    pub distance: f64,
}

/// How far the scan's cut lies from the model's, over the scan's cut points.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutGaps {
    pub max: f64,
    pub mean: f64,
    /// The share of the scan's cut within the tolerance of the model's.
    pub within: f64,
    /// The farthest places, a tenth of the cut apart at least, worst first (at most 5).
    pub worst: Vec<ScanCutPlace>,
}

/// The plane cut along: its origin and in-plane axes (outlines are in these), and its normal.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutPlane {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub normal: [f64; 3],
}

/// What `scan.section` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCutResult {
    pub scan: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub plane: ScanCutPlane,
    pub tolerance: f64,
    pub scan_outlines: Vec<ScanCutOutline>,
    pub model_outlines: Vec<ScanCutOutline>,
    /// With a model: how far the scan's cut lies from the model's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gaps: Option<ScanCutGaps>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn scan_section_takes_these_keys() {
        assert_keys(input_schema_for::<ScanCutParams>(), &["body", "model", "offset", "plane", "scan", "tolerance"]);
    }
}
