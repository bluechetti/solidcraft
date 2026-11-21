//! Contracts and result types for `scan.wrap_scan` (`cmd/scan_wrap_scan.rs`): a whole scan wrapped in
//! patches fitted to it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Wrap a whole scan in one network of patches, each fitted to the scan by least squares: a solid
/// when the scan is closed, else a surface. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanWrapScanParams {
    /// The scan. Default: what is selected on a scan, else the only one.
    #[serde(default)]
    pub body: Option<String>,
    /// How far the patches may lie from the scan. Default: 0.2 mm.
    #[serde(default)]
    pub tolerance: Option<super::types::LengthExpr>,
    /// The most patches, 300 to 60,000. Default: 12,000. A scan that needs more is wrapped with this
    /// many, short of the tolerance (said in `capped`).
    #[serde(default)]
    pub max_patches: Option<u64>,
    /// Where Find All reads a plane (over half a percent of the scan), one plane face in place of
    /// the patches there, bounded by the sides of the patches round it. Default: true.
    #[serde(default)]
    pub planes: Option<bool>,
}

/// What `scan.wrap_scan` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWrapScanResult {
    /// The scan, and the body made of the patches.
    pub body: String,
    pub surface: String,
    /// Whether the patches close into a solid.
    pub solid: bool,
    /// Patches (faces), and the coarse triangles they were laid on (three patches to a triangle,
    /// four to a pair of triangles laid as one quad).
    pub patches: usize,
    pub coarse_triangles: usize,
    /// Plane faces in the body (each in place of the patches on one of Find All's planes).
    pub plane_faces: usize,
    /// Rounds of reduction, and whether the coarse mesh met the tolerance both ways.
    pub rounds: usize,
    pub met: bool,
    /// The most patches bounded the wrap (the tolerance given up where it had to be).
    pub capped: bool,
    /// The scan's points to the patches: the share within the tolerance, the 99th percentile and the
    /// largest distance, in mm.
    pub within: f64,
    pub p99: f64,
    pub max: f64,
    /// The tolerance used, mm.
    pub tolerance: f64,
    /// When the most patches bounded the wrap: about how many the tolerance asks for (raise
    /// `max_patches` toward it, up to 60,000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patches_wanted: Option<usize>,
    /// A sentence when the wrap is short of the tolerance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// `scan.verdict` on the scan and the wrap: met or not_done, with its issues and next steps
    /// (the first of them a wrap with more patches when the cap held it back).
    pub verdict: serde_json::Value,
    /// The Scan Surface feature that keeps the patches.
    pub feature: serde_json::Value,
}
