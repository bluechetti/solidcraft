//! Contracts and result types for the commands in `cmd/scan_mesh.rs`: tools on a scan's mesh as a
//! whole.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;

/// Lay one scan onto another (a second pass onto the first): one undo step, kept as a Move of the
/// scan laid.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRegisterParams {
    /// The scan to move. Default: the selected mesh body, else, with two scans, the one inserted
    /// last.
    #[serde(default)]
    pub body: Option<String>,
    /// The scan to lay it on. Default: the other scan, when there are two.
    #[serde(default)]
    pub to: Option<String>,
    /// Search the ways the scans could lie on each other (their principal axes, matched surface
    /// features) as well as from where it lies. Default: true.
    #[serde(default)]
    pub search: Option<bool>,
    /// The share of the scan, 0 to 0.6, the fit may leave out because the other has nothing near
    /// it. Default: 0.2.
    #[serde(default)]
    pub ignored: Option<f64>,
}

/// What `scan.register` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRegisterResult {
    /// The scan moved and the scan it was laid on.
    pub body: String,
    pub to: String,
    /// Whether it moved (false when it already lay there).
    pub moved: bool,
    /// How far it lay from the other before and after: the root mean square of the nearest
    /// distances, its worst share left out, in mm.
    pub rms_before: f64,
    pub rms_after: f64,
    /// The share of it lying on the other after, 0–1.
    pub matched: f64,
    /// Where the winning start came from, and whether the fit settled.
    pub start: String,
    pub converged: bool,
    /// Another placement nearly as good, when there is one: the part is nearly its own mirror
    /// image, or its overlap repeats along it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rival: Option<String>,
    /// The Move feature, as `solid.move` replies, when it moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Make a mesh body from a point cloud: screened Poisson reconstruction, trimmed where the points
/// are sparse. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMeshFromPointsParams {
    /// The point cloud, by id or name. Default: the design's only one.
    #[serde(default)]
    pub cloud: Option<Value>,
    /// The detail: the finest grid is 2^depth cells across the cloud, 5 to 10. Default: the depth
    /// whose cells are about as wide as the points are apart.
    #[serde(default)]
    pub depth: Option<u32>,
    /// Where the points are sparse the surface stops: vertices covered less than this, 0 to 1, go
    /// (0 keeps the closed surface). Default: 0.3.
    #[serde(default)]
    pub trim: Option<f64>,
    /// The new body's name. Default: the cloud's, with " Mesh".
    #[serde(default)]
    pub name: Option<String>,
}

/// What `scan.mesh_from_points` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMeshFromPointsResult {
    /// The mesh body made, its triangles and vertices.
    pub body: String,
    pub triangles: usize,
    pub vertices: usize,
    /// The detail and the trim used.
    pub depth: u32,
    pub trim: f64,
    /// The cloud's points, and those solved from after thinning to half a cell; whether their
    /// normals were estimated (the cloud stated none).
    pub points: usize,
    pub points_used: usize,
    pub estimated_normals: bool,
    /// The finest cell, the points' typical spacing, and the triangles the trim took away.
    pub cell: f64,
    pub spacing: f64,
    pub trimmed: usize,
    /// How far the points are from the surface: root mean square, 95th percentile, largest, mm.
    pub rms: f64,
    pub p95: f64,
    pub max: f64,
    /// The feature, as `feature.add` replies.
    pub feature: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "scan_mesh_tests.rs"]
mod tests;
