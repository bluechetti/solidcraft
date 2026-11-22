//! Contracts for the commands in `cmd/mesh.rs`. Convert Mesh replies with the plain feature
//! reply, [`super::features::FeatureResult`]; Mesh Units with [`MeshUnitsResult`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{LengthExpr, StringList};

/// Rebuild a closed mesh body as an editable solid, with a parametric feature in the timeline.
/// The solid keeps the mesh body's name and colour.
///
/// Whether the body exists and is a mesh, and whether the active component may edit it, stay the
/// handler's to check. The kernel checks the tolerance's range when the feature evaluates, so an
/// expression is taken here whatever its value.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MeshConvertParams {
    /// The mesh body, by name. Default: the one selected body. A blank name also means the
    /// selected body, as the handler reads it.
    #[serde(default)]
    pub body: Option<String>,
    /// How far the rebuilt faces may stray from the mesh's triangles: a run of facets within it of
    /// an arc becomes a cylinder. From 0.000001 mm to 1 mm. Default: 0.05 mm.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    // Not `FeatureCommon`: the solid keeps the mesh body's name, so `body_name` and `body_names`
    // would do nothing. `participants` stays because `timeline.redefine` passes a feature's own
    // back.
    /// The feature's name in the timeline.
    #[serde(default)]
    pub name: Option<String>,
    /// The bodies taking part, which every feature records; each an existing body or `*`. A
    /// conversion changes only `body`, whatever this lists.
    #[serde(default)]
    pub participants: Option<StringList>,
}

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
