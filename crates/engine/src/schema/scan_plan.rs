//! Contracts and result types for `scan.plan`, `scan.plan_region` and `scan.verdict` in
//! `cmd/scan_plan.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::LengthExpr;

/// Set what a scan is being turned into: how the part was made, the share of the scan (and of the
/// model) that must be within the tolerance of the other, and the tolerance itself (as
/// `scan.tolerance` sets it). Kept in the design for that scan; one undo step. With nothing given it
/// changes nothing and says what the scan has.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPlanParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// The same as `scan`, the name every other scan command gives the scan.
    #[serde(default)]
    pub body: Option<String>,
    /// How the part was made: machined | moulded_or_die_cast | sand_cast_forged_or_printed | sheet |
    /// worn | freeform | unknown. It sets the target when none is given (0.90 machined, sheet and
    /// unknown; 0.85 castings, prints and worn parts; 0.75 freeform).
    #[serde(default)]
    pub process: Option<String>,
    /// The share from 0.5 to 1 that must be within the tolerance both ways.
    #[serde(default)]
    pub target: Option<f64>,
    /// The scan's tolerance, a length above 0 (the same as `scan.tolerance`).
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// What the model is for: replacement (a part that must fit: mating sizes to design values) |
    /// copy (the part as scanned) | none (not said).
    #[serde(default)]
    pub purpose: Option<String>,
    /// True takes the plan away (the tolerance stays). Alone.
    #[serde(default)]
    pub reset: Option<bool>,
}

/// A region of a plan as replies show it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPlanRegionView {
    pub name: String,
    pub at: [f64; 3],
    /// open | built | skipped | inferred.
    pub state: String,
    /// The feature that built it (built).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    /// Why (skipped, inferred).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the user marked it as: focus | hold | ignore | keep (don't touch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The size held there, mm (hold).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<f64>,
    /// Where the held size is (hold): one point on a round face, or one on each of two opposite faces.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hold_at: Vec<[f64; 3]>,
    /// The kept scan selection that is its extent (`scan.select {"saved": name}` selects it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
}

/// What `scan.plan` and `scan.plan_region` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPlanResult {
    pub scan: String,
    /// Whether the scan has a plan.
    pub planned: bool,
    pub process: String,
    /// The target share (the plan's, else the process's).
    pub target: f64,
    /// The scan's tolerance in mm.
    pub tolerance: f64,
    pub regions: Vec<ScanPlanRegionView>,
    /// What the model is for (replacement or copy), when said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Whether this call changed the design.
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Add a region to a scan's plan, or change one: built by a feature, skipped or inferred with a
/// reason, open again, or removed. A scan with no plan gets one (process unknown). One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPlanRegionParams {
    #[serde(default)]
    pub scan: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// The region's name.
    pub name: String,
    /// A point [x, y, z] on the scan inside the region: needed to add one.
    #[serde(default)]
    pub at: Option<[f64; 3]>,
    /// open (the default for a new region) | built (with `feature`) | skipped (with `reason`) |
    /// inferred (with `reason`) | removed.
    #[serde(default)]
    pub state: Option<String>,
    /// The feature that built it, by name or id (built).
    #[serde(default)]
    pub feature: Option<String>,
    /// True: the part of the scan selected now is the region's extent, kept in the design as a
    /// selection (`at` then defaults to a point in it).
    #[serde(default)]
    pub selected: Option<bool>,
    /// What the user marked it as: focus (work here first) | hold (a size there, `hold` mm) |
    /// ignore (left out of the verdict: worn, damaged, a fixture) | keep (don't touch: an agent
    /// may not change the scan there or the feature that built it) | none.
    #[serde(default)]
    pub role: Option<String>,
    /// The size held there, mm (role hold).
    #[serde(default)]
    pub hold: Option<f64>,
    /// Where the held size is: one point on a round face (its diameter) or two, one on each of two
    /// opposite faces. Default with `selected`: read off the selection's shapes.
    #[serde(default)]
    pub hold_at: Option<Vec<[f64; 3]>>,
    /// Why it was skipped, or what it was inferred from.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Whether the model of a scan is done: Scan Deviation read both ways at the scan's tolerance
/// against the plan's target (or the default), the plan's regions and the timeline. Changes nothing
/// (the map shown stays as it was).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanVerdictParams {
    #[serde(default)]
    pub scan: Option<String>,
    /// The same as `scan`.
    #[serde(default)]
    pub body: Option<String>,
    /// The model (a solid or surface body). Default: as Scan Deviation picks it.
    #[serde(default)]
    pub model: Option<String>,
}

/// One reason the model is not done (or a note: no_plan, unseen).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanVerdictIssue {
    /// out_of_tolerance | not_on_the_scan | region_open | feature_failed | rolled_back | no_plan | unseen.
    pub code: String,
    pub detail: String,
}

/// A place beyond the tolerance a feature wide.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanVerdictPlace {
    pub at: [f64; 3],
    /// How far beyond, at its deepest (mm).
    pub depth: f64,
    /// Its area (mm²).
    pub area: f64,
    /// extra (the model has material the scan does not) | missing.
    pub kind: String,
}

/// A step to take next.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanVerdictStep {
    pub command: String,
    pub why: String,
    pub params: serde_json::Value,
}

/// What `scan.verdict` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanVerdictResult {
    /// met | not_done.
    pub verdict: String,
    pub scan: String,
    pub model: String,
    pub planned: bool,
    pub target: f64,
    pub tolerance: f64,
    /// The share of the scan's surface within the tolerance of the model.
    pub scan_within: f64,
    /// The share of the model's surface with the scan within the tolerance of it.
    pub model_within: f64,
    pub issues: Vec<ScanVerdictIssue>,
    pub places: Vec<ScanVerdictPlace>,
    pub next: Vec<ScanVerdictStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn scan_plan_commands_take_these_keys() {
        assert_keys(input_schema_for::<ScanPlanParams>(), &["body", "process", "purpose", "reset", "scan", "target", "tolerance"]);
        assert_keys(
            input_schema_for::<ScanPlanRegionParams>(),
            &["at", "body", "feature", "hold", "hold_at", "name", "reason", "role", "scan", "selected", "state"],
        );
        assert_keys(input_schema_for::<ScanVerdictParams>(), &["body", "model", "scan"]);
    }
}

/// Known Size: a size the user measured on the part (callipers across two faces, or a bore's
/// diameter) against the scan's. It says how far the scan is off; with `apply`, it scales the scan
/// to agree, when it is a scanner's scale error (within 5 %), not a wrong pick or unit.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanKnownSizeParams {
    #[serde(default)]
    pub scan: Option<String>,
    /// The same as `scan`.
    #[serde(default)]
    pub body: Option<String>,
    /// Two points on the scan, one on each of two parallel faces: the size is between them.
    #[serde(default)]
    pub between: Option<Vec<[f64; 3]>>,
    /// A point on a round face (a bore or a boss): the size is its diameter.
    #[serde(default)]
    pub diameter: Option<[f64; 3]>,
    /// The size measured on the part, in mm.
    pub value: f64,
    /// Scale the scan so it measures `value` there. Default: false (only say how far off it is).
    #[serde(default)]
    pub apply: Option<bool>,
}

/// What Known Size read.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanKnownSizeResult {
    pub scan: String,
    /// between or diameter.
    pub kind: String,
    /// The size on the scan and the size given, in mm.
    pub measured: f64,
    pub value: f64,
    /// What the scan would be scaled by to agree (value / measured), and how far off it is as a
    /// share (measured / value - 1: above 0 the scan is large).
    pub factor: f64,
    pub off: f64,
    /// Whether the scan was scaled.
    pub applied: bool,
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
