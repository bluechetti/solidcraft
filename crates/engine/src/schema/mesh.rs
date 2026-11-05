//! Contracts and result types for the commands in `cmd/mesh.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A mesh import read in the wrong length unit (a scan saved in metres read as millimetres): its
/// points are scaled from `from` to `units` where they are, in one undo step. Refused once a
/// feature follows the import in the timeline.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MeshUnitsParams {
    /// The length unit the mesh file was saved in: mm, cm, m, in or ft (any case).
    pub units: String,
    /// A body of the mesh import; every body of that import is scaled. Default: the selected
    /// mesh body, else the last mesh import in the timeline.
    #[serde(default)]
    pub body: Option<String>,
    /// The unit the mesh was read in: mm, cm, m, in or ft (any case). Default: mm, as Insert Mesh
    /// reads a file given no `units`.
    #[serde(default)]
    pub from: Option<String>,
}

/// What `mesh.units` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MeshUnitsResult {
    /// The mesh import feature scaled.
    pub feature: u64,
    /// Its bodies.
    pub bodies: Vec<String>,
    /// What its points were multiplied by.
    pub scale: f64,
    /// The unit it is read in now, and the one it was read in before.
    pub units: String,
    pub from: String,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
