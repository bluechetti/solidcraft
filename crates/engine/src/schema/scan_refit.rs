//! Contracts and result types for `scan.refit` in `cmd/scan_refit.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Fit a Scan Surface again on the scan as it is now: the command that made it (New Free Form or
/// Fit Patches) runs again with the parameters it was given, and what was done by hand since, how
/// far each control point was moved from where the fit put it, is laid on the new fit. For after a
/// re-scan, or a change earlier in the timeline that moved or cleaned the scan. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRefitParams {
    /// The surface body. Default: the selected one, else the one made last.
    #[serde(default)]
    pub surface: Option<String>,
    /// True fits it afresh and drops the hand edits. Needed when the surface was reshaped by hand
    /// in a way the fit cannot carry over (a face or a row of the net added or taken away).
    #[serde(default)]
    pub discard_edits: Option<bool>,
}

/// What `scan.refit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRefitResult {
    pub surface: String,
    /// The scan it was fitted to again.
    pub scan: String,
    pub faces: usize,
    /// The control points carried over: moved by hand and moved the same on the new fit.
    pub carried: usize,
    /// Whether the hand edits were dropped (`discard_edits`).
    pub discarded: bool,
    /// The feature that keeps the surface.
    pub feature: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn scan_refit_takes_these_keys() {
        assert_keys(input_schema_for::<ScanRefitParams>(), &["discard_edits", "surface"]);
    }
}
