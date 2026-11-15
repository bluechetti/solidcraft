//! Contracts and result types for the commands in `cmd/scan_patches.rs`: smooth patches fitted
//! over a region of a scan.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::Point3;

/// Fit smooth patches over a region of a scan, in a grid, in blocks, or as a quad whose edges
/// follow the region's edge. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPatchesParams {
    /// The scan. Default: what is selected on a scan, else the picked or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The region as the scan's triangle numbers.
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// A kept selection (Keep Selection) whose triangles are the region, instead of `triangles`.
    #[serde(default)]
    pub saved: Option<String>,
    /// A point on the scan: the region is the piece there. Default: what is selected, else the
    /// point picked.
    #[serde(default)]
    pub point: Option<Point3>,
    /// The region only as far as this round the point, in mm.
    #[serde(default)]
    pub radius: Option<f64>,
    /// grid (default), blocks or quads.
    #[serde(default)]
    pub layout: Option<String>,
    /// Control points each way over the whole, 4 to 30, or [u, v]. Default: 10.
    #[serde(default)]
    pub controls: Option<Value>,
    /// With blocks: rows and columns, 1 to 8 each. Default: 2 × 2.
    #[serde(default)]
    pub rows: Option<u64>,
    #[serde(default)]
    pub columns: Option<u64>,
    /// 0 to 1: how much the surface is held smooth where the scan says little. Default: 0.1.
    #[serde(default)]
    pub smoothing: Option<f64>,
    /// With no `controls`: as many control points (6 to 30) as bring the fit within half of this
    /// length, or as close as the scan's noise allows.
    #[serde(default)]
    pub tolerance: Option<super::types::LengthExpr>,
}

/// What `scan.fit_patches` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPatchesResult {
    /// The scan and the surface body made.
    pub body: String,
    pub surface: String,
    /// The layout, the patches (faces), their rows and columns, and the control points each way.
    pub layout: String,
    pub patches: usize,
    pub rows: usize,
    pub columns: usize,
    pub controls: [usize; 2],
    /// The points' distance from the surface: root mean square and most, in mm; and how many.
    pub rms: f64,
    pub max: f64,
    pub points: usize,
    /// Where the surface is wrong (Surface Check on it): folds (it doubles back), pinches (it
    /// narrows to nothing) and crossings (it passes through itself). A region too intricate for one
    /// surface, with deep crevices or curling over itself, folds.
    pub folds: usize,
    pub pinches: usize,
    pub crossings: usize,
    /// Said when the surface folds, pinches or crosses itself: what to do instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The Import feature that keeps the surface.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::ScanPatchesParams;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(
            input_schema_for::<ScanPatchesParams>(),
            &["body", "columns", "controls", "layout", "point", "radius", "rows", "saved", "smoothing", "tolerance", "triangles"],
        );
    }
}
