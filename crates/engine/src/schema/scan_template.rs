//! Contract and result type for `scan.flat_template` (`cmd/scan_template.rs`): a flat template of
//! a wall from its scan, sketched with a cut-out for each part standing off it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::Point3;

/// A plane given by a point on it and the way it faces.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TemplatePlane {
    pub point: Point3,
    pub normal: Point3,
}

/// Make a flat template of a wall from its scan, as a sketch on a plane through the wall: the outline picked, laid flat
/// on the wall's plane, a cut-out for each part that stands off the plane, and the scan's own edge
/// as construction lines. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFlatTemplateParams {
    /// The scan. Default: the picked, selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The outline, three or more points [x, y, z] in order round it. They are put onto the plane
    /// exactly; nothing else snaps to the scan.
    pub outline: Vec<Point3>,
    /// The wall's plane. Default: fitted to the scan under the outline, the parts standing far off
    /// it left out.
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
    /// How far off the plane a part must stand to be cut out. Default: four times the wall's
    /// noise, at least 1 mm.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// Which parts are cut out: `both` (default), `out` (on the side the parts stand farthest off,
    /// or the plane's normal when `plane` is given) or `in`.
    #[serde(default)]
    pub side: Option<String>,
    /// The least area of a cut-out, in mm². Default: 25.
    #[serde(default)]
    pub least_area: Option<f64>,
    /// Draw the scan's own edge as construction lines. Default: true.
    #[serde(default)]
    pub show_scan_edge: Option<bool>,
    /// Also make a flat Free Form face over the template, each cut-out a hole in it. Default:
    /// false.
    #[serde(default)]
    pub make_face: Option<bool>,
    /// The sketch's name. Default: `Wall Template`.
    #[serde(default)]
    pub name: Option<String>,
}

/// A part cut out of the template.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanTemplateCutout {
    /// The footprint's area (mm²) and how far the part stands off the plane at most (mm).
    pub area: f64,
    pub height: f64,
    /// `out` or `in`: the side of the plane it stands off.
    pub side: String,
    /// Its footprint's middle in space, and its size along the template's x and y.
    pub center: [f64; 3],
    pub size: [f64; 2],
    /// The part runs off the scan's edge (its footprint was closed with a straight line).
    pub at_scan_edge: bool,
    /// The footprint crosses the outline: clipped to it, or kept whole for an outline that is not
    /// convex.
    pub crosses: bool,
    pub clipped: bool,
}

/// What `scan.flat_template` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFlatTemplateResult {
    pub body: String,
    pub sketch: u64,
    /// The plane used: a point on it and its unit normal.
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// Whether the plane was fitted to the scan, the scan's points it was fitted to and how many
    /// were under the outline.
    pub fitted: bool,
    pub kept: usize,
    pub under: usize,
    /// The tolerance used (mm).
    pub tolerance: f64,
    /// The outline's area and the area the cut-outs take from it (mm²).
    pub area: f64,
    pub cut_area: f64,
    pub cutouts: Vec<ScanTemplateCutout>,
    /// Parts found, left out for being under the least area, and left out for lying outside.
    pub found: usize,
    pub specks: usize,
    pub outside: usize,
    /// Loops of the scan's edge drawn, and lines, arcs and circles drawn in all.
    pub scan_edge_loops: usize,
    pub lines: usize,
    pub arcs: usize,
    pub circles: usize,
    /// Where the template was laid: its own coordinates minus this (the outline's corner is the
    /// sketch's origin, on a plane through the wall).
    pub shift: [f64; 2],
    /// The Free Form body made (with `make_face`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<String>,
    /// The outline's sides in order, in mm: the dimensions between the points picked.
    pub edge_lengths: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Put a Free Form flat: every control point of it moved square onto one plane. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMakeFlatParams {
    /// The Free Form. Default: the selected one, else the only one.
    #[serde(default)]
    pub surface: Option<String>,
    /// The plane. Default: fitted to the Free Form's net, the points far off it left out, so a
    /// bump does not tilt it.
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
}

/// What `scan.make_flat` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMakeFlatResult {
    pub surface: String,
    pub faces: usize,
    /// The plane used: a point on it and its unit normal.
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// How far the net's points were from the plane, root mean square and most (mm).
    pub rms: f64,
    pub max: f64,
    /// The feature that keeps the surface.
    pub feature: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
