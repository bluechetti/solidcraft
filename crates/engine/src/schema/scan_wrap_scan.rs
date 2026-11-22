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
    /// How far the patches may lie from the scan. Default: the scan's tolerance (`scan.tolerance`).
    /// The verdict reads the scan's tolerance whatever this is.
    #[serde(default)]
    pub tolerance: Option<super::types::LengthExpr>,
    /// The most patches, 300 to 60,000. Default: 12,000. A scan that needs more is wrapped with this
    /// many, short of the tolerance (said in `capped`).
    #[serde(default)]
    pub max_patches: Option<u64>,
    /// Where Find All reads a plane (over half a percent of the scan, fitting as well as the scan's
    /// noise lets a plane), one plane face in place of the patches there, bounded by the sides of
    /// the patches round it. Default: true.
    #[serde(default)]
    pub planes: Option<bool>,
    /// Close the scan's small tunnels and bridges first (a scanner's through thin walls): they keep
    /// the coarse mesh from reducing round them. Default: true.
    #[serde(default)]
    pub close_tunnels: Option<bool>,
    /// The widest patch a tunnel is closed in. Default: 12 of the scan's median edges.
    #[serde(default)]
    pub tunnel_size: Option<super::types::LengthExpr>,
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
    /// Tunnels and bridges of the scan closed before it was wrapped, and the parts cut out and
    /// filled for them (the scan itself is unchanged; the model is measured against it as it is).
    pub tunnels_closed: usize,
    pub tunnel_parts: usize,
    /// Rounds of reduction.
    pub rounds: usize,
    /// The wrap's own aim, which its rounds work toward: measured one way, the scan to the patches.
    /// Whether the model is done (both ways, at the scan's tolerance) is `verdict`'s.
    pub goal: ScanWrapScanGoal,
    /// The most patches bounded the wrap (the tolerance given up where it had to be).
    pub capped: bool,
    /// The scan's points to the patches: the share within the tolerance, the 99th percentile and the
    /// largest distance, in mm.
    pub within: f64,
    pub p99: f64,
    pub max: f64,
    /// The tolerance the patches were fitted to, and the scan's (`scan.tolerance`), which the
    /// verdict reads, mm.
    pub tolerance: f64,
    pub scan_tolerance: f64,
    /// When the most patches bounded the wrap: about how many the tolerance asks for (raise
    /// `max_patches` toward it, up to 60,000; past that, `p99` is what the patches hold).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patches_wanted: Option<usize>,
    /// Sentences when the wrap is short of its goal, built looser than the scan's tolerance, or
    /// without the plane faces it was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// `scan.verdict` on the scan and the wrap: met or not_done, with its issues and next steps
    /// (the first of them a wrap with more patches when the cap held it back, after the scan's
    /// tolerance at what the patches hold when the tolerance asks for more than a wrap may have).
    pub verdict: serde_json::Value,
    /// The Scan Surface feature that keeps the patches.
    pub feature: serde_json::Value,
}

/// The wrap's own aim ([`ScanWrapScanResult::goal`]).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWrapScanGoal {
    /// The share of the scan's area that must lie within the tolerance of the patches (0.99), with
    /// none of it farther than three tolerances.
    pub share: f64,
    /// Whether the patches reached it.
    pub reached: bool,
}
