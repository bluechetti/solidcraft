//! Contract and result type for `scan.survey` in `cmd/scan_survey.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A first look at a scan, changing nothing: its size, fineness, noise, holes and scraps, how much
/// of it is flat, what Find All read off it, and the commands to run next, in the order the work
/// goes.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSurveyParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
}

/// What Find All read off the scan, as shares of its area (from the last Find All or Reconstruct
/// on the scan as it is now).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSurveyShapes {
    pub in_shapes: f64,
    pub planes: f64,
    pub cylinders: f64,
    pub spheres: f64,
    pub cones: f64,
    pub tori: f64,
    pub freeform: f64,
    /// Regions found: shapes and freeform.
    pub regions: usize,
}

/// A command worth running next, why, and parameters to start from.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSurveyStep {
    pub command: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// What `scan.survey` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSurveyResult {
    pub body: String,
    pub triangles: usize,
    pub points: usize,
    /// Triangles naming a missing or non-finite point.
    pub unusable: usize,
    /// The box round the scan in the design's directions, and the middle of its surface (mm).
    pub low: [f64; 3],
    pub high: [f64; 3],
    pub center: [f64; 3],
    /// Its size along its own directions, longest first (mm).
    pub own_size: [f64; 3],
    /// Surface area (mm²) and the median triangle side (mm).
    pub area: f64,
    pub typical_edge: f64,
    /// Present when the scan's size suggests it was read in the wrong unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_hint: Option<String>,
    /// The noise (mm) and the Denoise strength suggested (0: leave it), read at a few places; absent
    /// when no place could be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smoothing: Option<f64>,
    /// The scan's tolerance (mm, see `scan.tolerance`) and whether it was set for it.
    pub tolerance: f64,
    pub tolerance_set: bool,
    /// Holes inside the scan's rims, the longest one's length (mm), rims (a scan seen from one
    /// side ends at one), open edges in all, and open edges in no ring (to repair).
    pub holes: usize,
    pub longest_hole: f64,
    pub rims: usize,
    pub open_edges: usize,
    pub tangled_edges: usize,
    /// Of those, the cracks between points a hair apart (a file's two copies of one point), which
    /// Repair Mesh joins.
    pub crack_edges: usize,
    /// Pieces not joined to each other, those far smaller than the largest, and their area share.
    pub pieces: usize,
    pub floating: usize,
    pub floating_share: f64,
    /// The share of the surface in broad flat faces.
    pub flat_share: f64,
    /// Thin for its size and broad: a wall, a panel or a sheet.
    pub sheet_like: bool,
    /// What Find All read, when it has read this scan as it is now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shapes: Option<ScanSurveyShapes>,
    /// The design's other scans, and its solid and surface bodies (models to check against).
    pub other_scans: Vec<String>,
    pub models: Vec<String>,
    /// What to run next, in the order the work goes.
    pub next: Vec<ScanSurveyStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn scan_survey_takes_these_keys() {
        assert_keys(input_schema_for::<ScanSurveyParams>(), &["body"]);
    }
}
