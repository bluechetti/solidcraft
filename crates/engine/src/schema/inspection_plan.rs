//! The inspection plan's commands: checks picked on the model (`inspect.check_add`), taken out
//! (`inspect.check_remove`), listed (`inspect.check_list`) and measured on a scan
//! (`inspect.check_measure`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{LengthExpr, Point3};

/// A face of the model, as a pick names it: its body and a point on it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckFace {
    pub body: String,
    pub point: Point3,
}

/// Add a check to the inspection plan: what it measures, on which face (two for a distance), and
/// its tolerance (given, or from a general-tolerance class). One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckAddParams {
    /// diameter (a round face), distance (two parallel flat faces), flatness (a flat face),
    /// position (a hole's axis), point (how far the scan is off a face at a point), or auto (a
    /// round face's diameter, a flat face's flatness).
    pub kind: String,
    pub face: CheckFace,
    /// The second face, for a distance.
    #[serde(default)]
    pub face2: Option<CheckFace>,
    /// ± about the nominal (or the zone, for flatness and position).
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Above and below the nominal, when they differ (not for flatness and position).
    #[serde(default)]
    pub plus: Option<LengthExpr>,
    #[serde(default)]
    pub minus: Option<LengthExpr>,
    /// The general-tolerance class when no tolerance is given: fine, medium (default) or coarse.
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// A check as the plan lists it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckItem {
    pub check: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub body: String,
    pub at: [f64; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at2: Option<[f64; 3]>,
    /// The model's value, mm (absent when its faces cannot be found any more).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<f64>,
    pub plus: f64,
    pub minus: f64,
    /// Why the model's value could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// What `inspect.check_add` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckAddResult {
    #[serde(flatten)]
    pub item: CheckItem,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Take a check out of the plan, or every check. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckRemoveParams {
    #[serde(default)]
    pub check: Option<u64>,
    #[serde(default)]
    pub all: Option<bool>,
}

/// What `inspect.check_remove` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckRemoveResult {
    pub removed: Vec<u64>,
    pub left: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// List the plan's checks with their nominal values.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckListParams {}

/// What `inspect.check_list` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckListResult {
    pub checks: Vec<CheckItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Measure every check of the plan on a scan aligned to the model, show the results in the view
/// and, if asked, write a report. A view, not a change to the design: no undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckMeasureParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// How far from the model's face a scan point may be and still be read for it. Default: four
    /// times the check's tolerance, at least 0.5 mm.
    #[serde(default)]
    pub band: Option<LengthExpr>,
    /// Write the results there as a printable HTML report, and as comma-separated values.
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub csv: Option<String>,
    /// False takes the results out of the view. Default: true.
    #[serde(default)]
    pub show: Option<bool>,
}

/// One check measured.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckMeasured {
    #[serde(flatten)]
    pub item: CheckItem,
    /// The value the scan holds, mm, and how far it is from the nominal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation: Option<f64>,
    /// Whether it is within its tolerance (absent when it could not be measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<bool>,
    /// Scan points read for it.
    pub points: usize,
}

/// What `inspect.check_measure` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckMeasureResult {
    pub shown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    pub checks: Vec<CheckMeasured>,
    pub passed: usize,
    pub failed: usize,
    pub unmeasured: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
