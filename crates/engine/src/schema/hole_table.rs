//! `inspect.hole_table`: every round hole of the shown solids, numbered, for a check of a part
//! against its drawing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// List every round hole of the shown solid bodies (or the ones named): its diameter, depth,
/// whether it goes through, where it starts and which way it runs, numbered in the view. A view,
/// not a change to the design: no undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct HoleTableParams {
    /// The solid bodies to read. Default: every shown solid body.
    #[serde(default)]
    pub bodies: Option<Vec<String>>,
    /// Also write the table to this file as comma-separated values (mm).
    #[serde(default)]
    pub csv: Option<String>,
    /// False takes the numbers out of the view again. Default: true.
    #[serde(default)]
    pub show: Option<bool>,
}

/// One diameter of a hole: a counterbored hole has two, the bore and the counterbore.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HoleTableStep {
    /// Diameter, mm.
    pub diameter: f64,
    /// How far this diameter runs along the hole's axis, mm.
    pub depth: f64,
}

/// A hole: one or more round walls on one axis, end to end.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HoleTableRow {
    /// Its number in the view, 1 first.
    pub hole: usize,
    /// The body it is in.
    pub body: String,
    /// through (open at both ends), blind (closed at the far end) or closed (closed at both).
    pub kind: String,
    /// Where it starts (its axis at the open end, or either end of a hole closed at both), mm.
    pub at: [f64; 3],
    /// Which way it runs from `at` into the part.
    pub axis: [f64; 3],
    /// Its diameters from `at` inwards, each with its depth.
    pub steps: Vec<HoleTableStep>,
    /// The whole depth, mm.
    pub depth: f64,
}

/// A body left out, and why.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HoleTableSkipped {
    pub body: String,
    pub reason: String,
}

/// What `inspect.hole_table` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct HoleTableResult {
    /// Whether the numbers are shown in the view now.
    pub shown: bool,
    /// The holes, numbered.
    pub holes: Vec<HoleTableRow>,
    /// Bodies that could not be read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<HoleTableSkipped>,
    /// The file the table was written to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv: Option<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
