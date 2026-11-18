//! Contracts and result types for the commands in `cmd/scan_mesh.rs`: tools on a scan's mesh as a
//! whole.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::Point3;

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
    /// What to know besides the numbers, when there is something: the scan was left where it was
    /// because fitting it moved it further off, only a small share of it lies on the other, or the
    /// fit was still settling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// How thick the layer of points is across the surface, mm: a clean pass reads its noise,
    /// passes merged without lying on each other their gap.
    pub thickness: f64,
    /// The points lie in a layer thicker than they are apart (passes that do not lie on each
    /// other): the surface goes through the layer's middle, with cells no finer than it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The feature, as `feature.add` replies.
    pub feature: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Make a scan again as one even surface, with other passes of the part fused into it when they
/// are given; kept as a Scan Clean step in the timeline, one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRemeshParams {
    /// The scan. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// Other passes of the part, laid on it (Align to Scan), fused in.
    #[serde(default)]
    pub with: Option<Vec<String>>,
    /// The grid's cell, about the new edge length. Default: the scan's own typical edge.
    #[serde(default)]
    pub cell: Option<super::types::LengthExpr>,
    /// Keep sharp edges and corners sharp (each vertex where the planes of its cell meet). Default:
    /// false.
    #[serde(default)]
    pub sharp: Option<bool>,
    /// Only part of the scan (its triangles made again as even triangles of about `cell`, the
    /// rest kept; not with `with`), a point on it with `grow` (true or degrees) or `radius` (mm).
    #[serde(default)]
    pub point: Option<Point3>,
    #[serde(default)]
    pub grow: Option<Value>,
    #[serde(default)]
    pub radius: Option<f64>,
    /// Only a kept selection's triangles (Keep Selection).
    #[serde(default)]
    pub saved: Option<String>,
    /// Only what is selected on the scan. A selection is never taken unless asked.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The area as the step keeps it: `{triangles: the scan's count, runs: [[first, count], …]}`.
    #[serde(default)]
    pub area: Option<Value>,
}

/// What `scan.remesh` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRemeshResult {
    /// The scan written back, and the passes fused into it.
    pub body: String,
    pub with: Vec<String>,
    /// Its triangles before and after, and its vertices after.
    pub before: usize,
    pub triangles: usize,
    pub vertices: usize,
    /// The cell and whether edges were kept sharp.
    pub cell: f64,
    pub sharp: bool,
    /// The new vertices' distance from the scans (rms, largest) and the scans' from the new surface
    /// (rms, 95th percentile), in mm; the share of the scans more than two cells from it.
    pub new_rms: f64,
    pub new_max: f64,
    pub scan_rms: f64,
    pub scan_p95: f64,
    pub uncovered: f64,
    /// The scan's feature, changed.
    pub feature: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Smooth a scan's noise away, its sharp edges kept, at the strength that leaves the least error
/// (or the one given); kept as a Scan Clean step in the timeline, one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanDenoiseParams {
    /// The scan. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// How hard, in passes of the filter, 0 to 8 (fractions blend between passes). Default: the
    /// strength the scan's own noise and shrinkage suggest.
    #[serde(default)]
    pub strength: Option<f64>,
    /// Only part of the scan, a point on it with `grow` or `radius`: the face grown from the point
    /// as Select on Scan's Region grows it (degrees, 1 to 90; `true`: 30). Default: the whole scan.
    #[serde(default)]
    pub point: Option<Point3>,
    #[serde(default)]
    pub grow: Option<Value>,
    /// Only the scan within this round `point`, in mm.
    #[serde(default)]
    pub radius: Option<f64>,
    /// Only a kept selection's triangles (Keep Selection).
    #[serde(default)]
    pub saved: Option<String>,
    /// Only what is selected on the scan. A selection is never taken unless asked.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The area as the step keeps it: `{triangles: the scan's count, runs: [[first, count], …]}`.
    #[serde(default)]
    pub area: Option<Value>,
}

/// What `scan.denoise` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanDenoiseResult {
    pub body: String,
    /// The strength used, and the one the scan suggests when no strength was given, in passes.
    pub strength: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested: Option<f64>,
    /// The scan's noise before and after, in mm, read off its sharp edges (none where it is
    /// drawn exactly, as a CAD mesh is).
    pub noise: f64,
    pub roughness: f64,
    /// How far its vertices moved: root mean square and largest, in mm.
    pub rms_moved: f64,
    pub max_moved: f64,
    /// The scan's feature, changed.
    pub feature: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "scan_mesh_tests.rs"]
mod tests;
