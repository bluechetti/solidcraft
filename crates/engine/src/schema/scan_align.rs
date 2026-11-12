//! Contracts and result types for the commands in `cmd/scan_align.rs`: aligning several passes at
//! once, by points picked in pairs, and a scan onto a model.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::Point3;

/// Lay several passes of one part together at once: each placed onto those before it from
/// wherever it lies, then every overlapping pair fitted and the errors shared out. Each pass moved
/// is kept as a Move of it, all in one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanAlignAllParams {
    /// The passes, in the order they were made. Default: the selected scans, else every scan.
    #[serde(default)]
    pub bodies: Option<Vec<String>>,
    /// Passes that stay where they are. Default: the first.
    #[serde(default)]
    pub hold: Option<Vec<String>>,
    /// Place a pass that does not yet lie on the others from wherever it lies. Default: true.
    #[serde(default)]
    pub search: Option<bool>,
    /// The least share of a pass two passes must have in common to be fitted to each other, 0.02
    /// to 0.9. Default: 0.1.
    #[serde(default)]
    pub overlap: Option<f64>,
}

/// What `scan.align_all` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAlignAllResult {
    /// Each pass and what was done to it.
    pub passes: Vec<ScanAlignPass>,
    /// Each pair of passes fitted (or not) to each other.
    pub links: Vec<ScanAlignLink>,
    /// The rounds the shared-out solution took.
    pub rounds: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A pass aligned with the others.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAlignPass {
    pub body: String,
    /// Held where it was, placed (joined to a held pass by fits), and moved.
    pub held: bool,
    pub placed: bool,
    pub moved: bool,
    /// For a pass placed from wherever it lay, where the winning start came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searched: Option<String>,
    /// Its Move feature, as `solid.move` replies, when it moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
}

/// A pair of passes and what fitting them together found.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAlignLink {
    pub a: String,
    pub b: String,
    /// The share of one lying on the other before and after, and their typical gap after (mm).
    pub overlap_before: f64,
    pub overlap_after: f64,
    pub rms_after: f64,
    /// Whether it took part, and why not (or what to know) otherwise.
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How far the pair's own fit is from where the solution lays them (mm and degrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disagreement: Option<[f64; 2]>,
}

/// Lay a scan by points picked in pairs: each point on it onto its partner on what it is laid on
/// (another scan, a model, or anywhere), then, with `refine`, fitted onto that surface. One undo
/// step, kept as a Move of the scan.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanAlignPairsParams {
    /// The scan to move. Default: the selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Points on the scan, in order.
    pub from: Vec<Point3>,
    /// Their partners, in the same order (three pairs or more).
    pub onto: Vec<Point3>,
    /// What the partners are on: another scan or a model body, to fit onto after the points.
    #[serde(default)]
    pub to: Option<String>,
    /// Fit onto `to` after the points. Default: true when `to` is given.
    #[serde(default)]
    pub refine: Option<bool>,
}

/// What `scan.align_pairs` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAlignPairsResult {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// How far each point lies from its partner after the points' move, in order, their root
    /// mean square, and the pair that fits worst (from 0).
    pub residuals: Vec<f64>,
    pub rms: f64,
    pub worst: usize,
    /// With refine: how far the scan lies from `to` after the fit, and the share matched (none
    /// when the surfaces were too sparse to fit, and the points' move stands).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refined_rms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<f64>,
    pub moved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Lay a scan onto a model (a solid or surface body, the CAD it was made from or is checked
/// against): fitted onto its surface as Align to Scan fits one pass onto another. One undo step,
/// kept as a Move of the scan; the model is not moved.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanAlignModelParams {
    /// The scan to move. Default: the selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The model. Default: the selected solid or surface body, else the only one.
    #[serde(default)]
    pub model: Option<String>,
    /// Search the ways the scan could lie on the model as well as from where it lies. Default:
    /// true.
    #[serde(default)]
    pub search: Option<bool>,
    /// The share of the scan, 0 to 0.6, the fit may leave out because the model has nothing near
    /// it (fixtures, the table). Default: 0.2.
    #[serde(default)]
    pub ignored: Option<f64>,
}

/// What `scan.align_to_model` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAlignModelResult {
    pub body: String,
    pub model: String,
    pub moved: bool,
    /// How far the scan lay from the model before and after (trimmed root mean square, mm), and
    /// the share of it lying on the model after.
    pub rms_before: f64,
    pub rms_after: f64,
    pub matched: f64,
    pub start: String,
    pub converged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rival: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ScanAlignAllParams, ScanAlignModelParams, ScanAlignPairsParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<ScanAlignAllParams>(), &["bodies", "hold", "overlap", "search"]);
        assert_keys(input_schema_for::<ScanAlignPairsParams>(), &["body", "from", "onto", "refine", "to"]);
        assert_keys(input_schema_for::<ScanAlignModelParams>(), &["body", "ignored", "model", "search"]);
    }
}

/// Combine several passes of one part into one scan: the others are laid on `body` as Align All
/// Scans lays them (`body` held where it is), then fused into it as Remesh fuses passes, one even
/// surface from all of them. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCombineParams {
    /// The scan the others are fused into, and held where it is. Default: the first of the
    /// selected scans, else the first scan.
    #[serde(default)]
    pub body: Option<String>,
    /// The other passes. Default: the other selected scans, else every other scan.
    #[serde(default)]
    pub with: Option<Vec<String>>,
    /// Lay the passes on `body` first. Default: true; false when they already lie together.
    #[serde(default)]
    pub align: Option<bool>,
    /// Place a pass that does not yet lie on the others from wherever it lies. Default: true.
    #[serde(default)]
    pub search: Option<bool>,
    /// The least share of a pass two passes must have in common to be fitted to each other, 0.02
    /// to 0.9. Default: 0.1.
    #[serde(default)]
    pub overlap: Option<f64>,
    /// The new mesh's edge length. Default: the scan's typical edge.
    #[serde(default)]
    pub cell: Option<Value>,
    /// Keep sharp edges and corners sharp. Default: false.
    #[serde(default)]
    pub sharp: Option<bool>,
}

/// What `scan.combine` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCombineResult {
    /// The scan the passes were fused into, and the passes.
    pub body: String,
    pub with: Vec<String>,
    /// What Align All Scans did; absent when `align` was false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ScanAlignAllResult>")]
    pub aligned: Option<Value>,
    /// What Remesh did: the new mesh and how well it keeps to the passes.
    #[schemars(with = "super::scan_mesh::ScanRemeshResult")]
    pub fused: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
