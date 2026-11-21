//! Contracts and result types for `scan.tolerance` in `cmd/scan_tolerance.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::LengthExpr;

/// Set how far off a scan's surface still counts as on it: the one tolerance Scan Deviation, Free
/// Form, Edit Surface, Fit Patches and Flat Template colour and report against. Kept in the design
/// for that scan; one undo step. With neither `tolerance` nor `reset` it changes nothing and says
/// what the scan has.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanToleranceParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// The same as `scan`, the name every other scan command gives the scan.
    #[serde(default)]
    pub body: Option<String>,
    /// The tolerance, a length above 0.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// True takes the scan back to the default (0.1 mm). Not with `tolerance`.
    #[serde(default)]
    pub reset: Option<bool>,
}

/// What `scan.tolerance` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanToleranceResult {
    pub scan: String,
    /// The scan's tolerance now, in mm.
    pub tolerance: f64,
    /// Whether it is one set for the scan (false: the default).
    pub set: bool,
    /// Whether this call changed it.
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn scan_tolerance_takes_these_keys() {
        assert_keys(input_schema_for::<ScanToleranceParams>(), &["body", "reset", "scan", "tolerance"]);
    }
}
