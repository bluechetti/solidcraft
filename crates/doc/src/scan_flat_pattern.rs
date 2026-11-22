//! A Flat Pattern (Flatten) kept in the design: the sketch its outline was drawn in and the
//! flattening that made it (the scan and the region, as `scan.flatten` takes them). Roll onto Scan
//! goes back through it to lift what is drawn on the pattern onto the scan, after the design is
//! closed and opened again too.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocError, Result};

/// The most flat patterns a design keeps.
pub const MAX_PATTERNS: usize = 10_000;
/// The most triangles a kept region lists.
pub const MAX_TRIANGLES: usize = 200_000_000;

/// One Flat Pattern: its sketch and the flattening's parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlatPatternKeep {
    /// The sketch the pattern's outline is drawn in.
    pub sketch: u64,
    /// The flattening, as `scan.flatten` takes it (`body`, `point`, `radius`, `triangles`,
    /// `relax`), and `outline`: the ids of the outline's curves in the sketch.
    pub params: Value,
}

/// Every kept pattern well formed: not too many, each one's parameters an object, its region's
/// triangle list not too long.
pub fn check_all(kept: &[FlatPatternKeep]) -> Result<()> {
    if kept.len() > MAX_PATTERNS {
        return Err(DocError::Invalid(format!("more than {MAX_PATTERNS} flat patterns kept")));
    }
    for k in kept {
        let Some(params) = k.params.as_object() else {
            return Err(DocError::Invalid(format!("the flat pattern of sketch {} keeps no parameters", k.sketch)));
        };
        if params.get("triangles").and_then(Value::as_array).is_some_and(|t| t.len() > MAX_TRIANGLES) {
            return Err(DocError::Invalid(format!("the flat pattern of sketch {} lists too many triangles", k.sketch)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_pattern_without_an_object_of_parameters_is_refused() {
        assert!(check_all(&[FlatPatternKeep { sketch: 3, params: json!({"body": "Scan", "point": [0, 0, 0]}) }]).is_ok());
        assert!(check_all(&[FlatPatternKeep { sketch: 3, params: json!([1, 2]) }]).is_err());
    }
}
