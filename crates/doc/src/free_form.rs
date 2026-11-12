//! What a Free Form keeps besides its faces (Edit Surface on the Scan tab). A Free Form is a
//! network of faces read from the faces alone, and how its shared edges join is read from the
//! angle the faces meet at; a design keeps only what that reading would get wrong: the shared
//! edges creased by hand where the faces still meet smoothly, those made smooth by hand where they
//! still meet at an angle, and the plane Symmetry keeps it a mirror image in. Each is kept by the
//! Import feature that holds the Free Form; an edge is named by its two ends, as they are in that
//! feature's file (before later features move it).

use serde::{Deserialize, Serialize};

use crate::{DocError, Result};

/// The most Free Forms a design keeps this for, and the most edges one keeps.
pub const MAX_FREE_FORMS: usize = 10_000;
pub const MAX_EDGES: usize = 10_000;

/// A plane a Free Form is kept a mirror image in: a point on it and the way it faces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptPlane {
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// What one Free Form keeps besides its faces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FreeFormKeep {
    /// The Import feature that holds the Free Form.
    pub feature: u64,
    /// Shared edges, each by its two ends, that are creases though the faces meet smoothly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub creases: Vec<[[f64; 3]; 2]>,
    /// Shared edges, each by its two ends, held smooth though the faces meet at an angle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub smooths: Vec<[[f64; 3]; 2]>,
    /// The plane Symmetry keeps it a mirror image in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symmetry: Option<KeptPlane>,
}

impl FreeFormKeep {
    /// Whether it keeps nothing (the faces alone say it all).
    pub fn is_empty(&self) -> bool {
        self.creases.is_empty() && self.smooths.is_empty() && self.symmetry.is_none()
    }
}

/// Every kept Free Form well formed: not too many, one each feature, places that are numbers, a
/// plane that faces some way.
pub fn check_all(list: &[FreeFormKeep]) -> Result<()> {
    if list.len() > MAX_FREE_FORMS {
        return Err(DocError::Invalid(format!("more than {MAX_FREE_FORMS} kept Free Forms")));
    }
    let mut seen = std::collections::BTreeSet::new();
    for k in list {
        if !seen.insert(k.feature) {
            return Err(DocError::Invalid(format!("a Free Form of feature {} is kept twice", k.feature)));
        }
        if k.creases.len() > MAX_EDGES || k.smooths.len() > MAX_EDGES {
            return Err(DocError::Invalid(format!("the Free Form of feature {} keeps too many edges", k.feature)));
        }
        let finite = |p: &[f64; 3]| p.iter().all(|x| x.is_finite());
        if !k.creases.iter().chain(&k.smooths).flatten().all(finite) {
            return Err(DocError::Invalid(format!("an edge the Free Form of feature {} keeps is not a place", k.feature)));
        }
        if let Some(plane) = &k.symmetry {
            let length = plane.normal.iter().map(|x| x * x).sum::<f64>().sqrt();
            if !finite(&plane.point) || !finite(&plane.normal) || !(length > 1e-12) {
                return Err(DocError::Invalid(format!("the symmetry plane of the Free Form of feature {} faces no way", k.feature)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kept Free Form reads back as it was written; a second for one feature, a place that is
    /// not a number and a plane facing no way are refused.
    #[test]
    fn kept_free_forms_round_trip_and_are_checked() {
        let kept = FreeFormKeep {
            feature: 7,
            creases: vec![[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]],
            smooths: Vec::new(),
            symmetry: Some(KeptPlane { point: [0.0; 3], normal: [1.0, 0.0, 0.0] }),
        };
        let text = serde_json::to_string(&kept).unwrap_or_default();
        assert!(!text.contains("smooths"), "an empty list is not written: {text}");
        let back: FreeFormKeep = serde_json::from_str(&text).unwrap_or_else(|_| FreeFormKeep { feature: 0, ..kept.clone() });
        assert_eq!(back, kept);
        assert!(check_all(std::slice::from_ref(&kept)).is_ok());
        assert!(check_all(&[kept.clone(), kept.clone()]).is_err());
        let nan = FreeFormKeep { creases: vec![[[f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0]]], ..kept.clone() };
        assert!(check_all(&[nan]).is_err());
        let flat = FreeFormKeep { symmetry: Some(KeptPlane { point: [0.0; 3], normal: [0.0; 3] }), ..kept };
        assert!(check_all(&[flat]).is_err());
    }
}
