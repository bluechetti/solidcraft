//! How a Scan Surface was made, kept with it so it can be made again. A Scan Surface keeps the
//! surface itself (its STEP text); its recipe adds what made it: the scan, the command that fitted
//! it with the exact parameters it was given (the corners or curves clicked, the fit, the
//! stiffness), and where the fit put each control point. What was done by hand since is the
//! difference between the surface and that baseline, so it needs no list of its own: `scan.refit`
//! runs the command again on the scan as it is now (a new scan, or one moved or cleaned earlier in
//! the timeline) and lays the difference on the new fit, so an hour of dragging points is not lost
//! to a re-scan.
//!
//! The baseline is in the surface's own coordinates (before later features move it), by face and
//! control point (u fastest). A change that cannot be told against it (a face or a row of the net
//! added or taken away) leaves the surface as it is; `scan.refit` then asks to discard the edits.

use serde::{Deserialize, Serialize};

use crate::{DocError, Result};

/// The most faces a recipe keeps a baseline for, and the most control points in all.
pub const MAX_FACES: usize = 10_000;
pub const MAX_POINTS: usize = 4_000_000;

/// What made a Scan Surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanRecipe {
    /// The scan it was fitted to (a mesh body).
    pub scan: String,
    /// The command that made it, as `scan.refit` runs it again: `scan.free_form` or `scan.fit_patches`.
    pub command: String,
    /// The command's parameters, as it was given them.
    pub params: serde_json::Value,
    /// Where the fit put each control point, face by face (u fastest), in mm.
    pub baseline: Vec<Vec<[f64; 3]>>,
}

impl ScanRecipe {
    /// A recipe for a surface just fitted.
    pub fn new(scan: &str, command: &str, params: serde_json::Value, baseline: Vec<Vec<[f64; 3]>>) -> ScanRecipe {
        ScanRecipe { scan: scan.to_owned(), command: command.to_owned(), params, baseline }
    }

    /// How many control points the baseline holds.
    pub fn points(&self) -> usize {
        self.baseline.iter().map(Vec::len).sum()
    }

    /// Well formed: a command refit can run, places that are numbers, not too many.
    pub fn check(&self) -> Result<()> {
        if !matches!(self.command.as_str(), "scan.free_form" | "scan.fit_patches") {
            return Err(DocError::Invalid(format!("a Scan Surface recipe names the command `{}`, which cannot be run again", self.command)));
        }
        if self.baseline.len() > MAX_FACES || self.points() > MAX_POINTS {
            return Err(DocError::Invalid("a Scan Surface recipe keeps too many control points".into()));
        }
        if !self.baseline.iter().flatten().all(|p| p.iter().all(|x| x.is_finite())) {
            return Err(DocError::Invalid("a control point a Scan Surface recipe keeps is not a place".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A recipe reads back as it was written; an unknown command, a point that is not a number and
    /// too many faces are refused.
    #[test]
    fn a_recipe_round_trips_and_is_checked() {
        let r =
            ScanRecipe::new("Scan", "scan.free_form", json!({"corners": [[0, 0, 0]], "fit": "follow"}), vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 2.5]]]);
        assert!(r.check().is_ok());
        let back: ScanRecipe =
            serde_json::from_str(&serde_json::to_string(&r).unwrap_or_default()).unwrap_or_else(|_| ScanRecipe::new("", "", json!(null), Vec::new()));
        assert_eq!(back, r);
        assert_eq!(back.points(), 2);
        assert!(ScanRecipe { command: "solid.box".into(), ..r.clone() }.check().is_err());
        let nan = ScanRecipe { baseline: vec![vec![[f64::NAN, 0.0, 0.0]]], ..r.clone() };
        assert!(nan.check().is_err());
        let many = ScanRecipe { baseline: vec![Vec::new(); MAX_FACES + 1], ..r };
        assert!(many.check().is_err());
    }
}
