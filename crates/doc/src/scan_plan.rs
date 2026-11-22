//! What a scan is being turned into, kept per scan in the design: how the part was made, the share of
//! the scan the model must hold, and the regions of the part, each built, skipped or inferred with a
//! reason. Scan Verdict reads it to say whether the model is done ("met") or what is still open
//! ("not done"), so neither a person nor an agent hands over part of a part as finished. The
//! tolerance stays in [`crate::scan_tolerance`]: one number every Scan-tab tool reads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{DocError, Result};

/// How the part was made: it sets the target share when none is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Process {
    /// Machined: square edges, holes and pockets; read closely.
    Machined,
    /// Moulded or die cast: drafts and rounds.
    MouldedOrDieCast,
    /// Sand cast, forged or printed: rough faces.
    SandCastForgedOrPrinted,
    /// Sheet metal: bends and flanges.
    Sheet,
    /// Worn or damaged: the model is the part as designed, the scan less.
    Worn,
    /// Freeform: organic, sculpted, a body of patches.
    Freeform,
    #[default]
    Unknown,
}

impl Process {
    /// The share of the scan the model must hold within the tolerance when the plan gives none:
    /// less where the surface is rougher than the design (castings, prints, worn parts) or is a
    /// skin of patches.
    pub fn default_target(self) -> f64 {
        match self {
            Process::Machined | Process::Sheet | Process::Unknown => 0.90,
            Process::MouldedOrDieCast | Process::SandCastForgedOrPrinted | Process::Worn => 0.85,
            Process::Freeform => 0.75,
        }
    }
}

/// Where a region of the part stands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RegionState {
    /// Still to model.
    #[default]
    Open,
    /// Modelled, by this feature.
    Built { feature: String },
    /// Left out on purpose, and why (a part of a fixture the scan holds, a region the scanner saw
    /// too poorly to model).
    Skipped { reason: String },
    /// Modelled from what the scan does not show, and why (the inside of a hollow part seen only
    /// through its openings, a mirror image of the side seen).
    Inferred { reason: String },
}

/// A region of the part: a name, a point on the scan inside it, where it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub at: [f64; 3],
    #[serde(flatten)]
    pub state: RegionState,
}

/// A scan's plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScanPlan {
    #[serde(default)]
    pub process: Process,
    /// The share (0 to 1) of the scan, and of the model, that must be within the tolerance of the
    /// other; none: the process's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<Region>,
}

impl ScanPlan {
    /// The target share: the plan's, else the process's default.
    pub fn target(&self) -> f64 {
        self.target.unwrap_or_else(|| self.process.default_target())
    }
}

/// The most scans a design keeps a plan for, the most regions a plan holds, and the longest name
/// or reason kept.
pub const MAX_SCANS: usize = 10_000;
pub const MAX_REGIONS: usize = 1_000;
pub const MAX_TEXT: usize = 500;
/// The least and most target share kept.
pub const MIN_TARGET: f64 = 0.5;
pub const MAX_TARGET: f64 = 1.0;

/// Whether `share` is a target that can be kept.
pub fn valid_target(share: f64) -> bool {
    share.is_finite() && (MIN_TARGET..=MAX_TARGET).contains(&share)
}

/// Every kept plan well formed.
pub fn check_all(kept: &BTreeMap<String, ScanPlan>) -> Result<()> {
    if kept.len() > MAX_SCANS {
        return Err(DocError::Invalid(format!("plans kept for more than {MAX_SCANS} scans")));
    }
    let long = |s: &str| s.chars().count() > MAX_TEXT;
    for (scan, plan) in kept {
        if long(scan) {
            return Err(DocError::Invalid("a scan's name is too long to keep a plan for".into()));
        }
        if plan.target.is_some_and(|t| !valid_target(t)) {
            return Err(DocError::Invalid(format!("the target kept for scan `{scan}` is not a share from {MIN_TARGET} to {MAX_TARGET}")));
        }
        if plan.regions.len() > MAX_REGIONS {
            return Err(DocError::Invalid(format!("the plan for scan `{scan}` holds more than {MAX_REGIONS} regions")));
        }
        for r in &plan.regions {
            let text = match &r.state {
                RegionState::Open => "",
                RegionState::Built { feature } => feature,
                RegionState::Skipped { reason } | RegionState::Inferred { reason } => reason,
            };
            if r.name.trim().is_empty() || long(&r.name) || long(text) || !r.at.iter().all(|x| x.is_finite()) {
                return Err(DocError::Invalid(format!("a region of the plan for scan `{scan}` is not well formed")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(target: Option<f64>, regions: Vec<Region>) -> BTreeMap<String, ScanPlan> {
        BTreeMap::from([("Scan".to_owned(), ScanPlan { process: Process::Machined, target, regions })])
    }

    fn region(name: &str, at: [f64; 3], state: RegionState) -> Region {
        Region { name: name.into(), at, state }
    }

    /// A plan with a target share from 0.5 to 1 and named regions at finite points is kept; a
    /// target out of range, a region with no name, a point not finite or a name too long are
    /// refused.
    #[test]
    fn only_well_formed_plans_are_kept() {
        assert!(check_all(&plan(Some(0.9), vec![region("Boss", [0.0, 1.0, 2.0], RegionState::Open)])).is_ok());
        for bad in [0.0, 0.4, 1.01, f64::NAN] {
            assert!(check_all(&plan(Some(bad), vec![])).is_err(), "{bad}");
        }
        assert!(check_all(&plan(None, vec![region(" ", [0.0; 3], RegionState::Open)])).is_err());
        assert!(check_all(&plan(None, vec![region("A", [f64::NAN, 0.0, 0.0], RegionState::Open)])).is_err());
        let reason = RegionState::Skipped { reason: "x".repeat(MAX_TEXT + 1) };
        assert!(check_all(&plan(None, vec![region("A", [0.0; 3], reason)])).is_err());
    }

    /// The target is the plan's, else the process's: castings and worn parts hold less than a
    /// machined part, freeform less again.
    #[test]
    fn the_target_is_the_plans_else_the_processs() {
        let mut p = ScanPlan { process: Process::SandCastForgedOrPrinted, ..ScanPlan::default() };
        assert_eq!(p.target(), 0.85);
        p.target = Some(0.95);
        assert_eq!(p.target(), 0.95);
        assert_eq!(ScanPlan::default().target(), 0.90);
        assert_eq!(Process::Freeform.default_target(), 0.75);
    }

    /// A region's state is saved flat beside its name and point, and read back the same.
    #[test]
    fn a_region_reads_back_as_saved() {
        let r = region("Inside", [1.0, 2.0, 3.0], RegionState::Inferred { reason: "seen only through the openings".into() });
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(text, r#"{"name":"Inside","at":[1.0,2.0,3.0],"state":"inferred","reason":"seen only through the openings"}"#);
        assert_eq!(serde_json::from_str::<Region>(&text).unwrap(), r);
    }
}
