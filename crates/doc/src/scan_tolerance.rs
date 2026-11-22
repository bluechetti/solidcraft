//! How far off a scan's surface still counts as on it, kept per scan in the design. Every Scan-tab
//! tool that colours the scan by its distance from a result (Scan Deviation, Free Form, Edit
//! Surface, Fit Patches, Flat Template) reads this one number, so "is this still faithful?" has one
//! answer however it is asked; it is set once for a scan and changed in one place.

use std::collections::BTreeMap;

use crate::{DocError, Result};

/// The tolerance a scan has until one is set, in mm.
pub const DEFAULT_MM: f64 = 0.1;
/// The most scans a design keeps a tolerance for, and the longest scan name.
pub const MAX_SCANS: usize = 10_000;
pub const MAX_NAME: usize = 200;
/// The smallest and largest tolerance kept, in mm.
pub const MIN_MM: f64 = 1e-6;
pub const MAX_MM: f64 = 1e6;

/// Whether `mm` is a tolerance that can be kept: a length above 0.
pub fn valid(mm: f64) -> bool {
    mm.is_finite() && (MIN_MM..=MAX_MM).contains(&mm)
}

/// Every kept tolerance well formed.
pub fn check_all(kept: &BTreeMap<String, f64>) -> Result<()> {
    if kept.len() > MAX_SCANS {
        return Err(DocError::Invalid(format!("tolerances kept for more than {MAX_SCANS} scans")));
    }
    for (scan, &mm) in kept {
        if scan.chars().count() > MAX_NAME {
            return Err(DocError::Invalid("a scan's name is too long to keep a tolerance for".into()));
        }
        if !valid(mm) {
            return Err(DocError::Invalid(format!("the tolerance kept for scan `{scan}` is not a length above 0")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerances that are lengths above 0 are kept; zero, negative, infinite and NaN ones are
    /// refused, and so is a name too long.
    #[test]
    fn only_lengths_above_zero_are_kept() {
        let mut kept = BTreeMap::from([("Scan".to_owned(), 0.25)]);
        assert!(check_all(&kept).is_ok());
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
            kept.insert("Scan".into(), bad);
            assert!(check_all(&kept).is_err(), "{bad}");
        }
        let long = BTreeMap::from([("x".repeat(MAX_NAME + 1), 0.1)]);
        assert!(check_all(&long).is_err());
    }
}
