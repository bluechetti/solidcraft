//! Contract and result types for `inspect.surface_check` (`cmd/surface_check.rs`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Check the B-spline faces of bodies for folds, pinches and crossings, and how square and even
/// their parameters are. Reads only: nothing changes and there is no undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SurfaceCheckParams {
    /// The bodies to check. Default: the selected bodies, else every visible body that is not a
    /// mesh.
    #[serde(default)]
    pub bodies: Option<Vec<String>>,
    /// Cells each way on each face's grid, 4 to 200. Default: 40.
    #[serde(default)]
    pub grid: Option<u32>,
}

/// A place where a face is wrong.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SurfaceCheckPlace {
    /// fold, pinch or crossing.
    pub kind: String,
    /// Where, in mm: the middle of a cell that folds, a pinched place, the middle of a piece that
    /// another part passes through.
    pub at: [f64; 3],
}

/// One B-spline face checked.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SurfaceCheckFace {
    pub body: String,
    /// The face's index in the body.
    pub face: usize,
    /// No folds, pinches or crossings.
    pub clean: bool,
    /// Cells of the grid where the surface doubles back on itself.
    pub folds: usize,
    /// Cells with a place where the surface narrows to no area (not a corner of its square).
    pub pinches: usize,
    /// Pairs of the grid's pieces that are not neighbours and pass through each other.
    pub crossings: usize,
    /// Cells checked: those on the face (inside its trims).
    pub cells: usize,
    /// The least angle between the surface's directions along u and v, in degrees (90 is square).
    pub least_angle: f64,
    /// How unevenly its parameters cover it: the square root of the largest area a step covers
    /// over the least (1 is even).
    pub stretch: f64,
    /// Where the trouble is (at most 256 places a face).
    pub places: Vec<SurfaceCheckPlace>,
}

/// What `inspect.surface_check` returns.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct SurfaceCheckResult {
    /// Every B-spline face checked, body by body.
    pub faces: Vec<SurfaceCheckFace>,
    /// How many were checked and how many were clean.
    pub checked: usize,
    pub clean: usize,
    /// Faces of the bodies that are not B-spline surfaces (planes, cylinders, …), not checked.
    pub skipped: usize,
    /// The bodies checked.
    pub bodies: Vec<String>,
    /// One line for a reader.
    pub summary: String,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<SurfaceCheckParams>(), &["bodies", "grid"]);
    }
}
