//! Contracts and result types for the commands in `cmd/scan.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::{LengthExpr, Point3};

/// The ways a scan could stand on the ground, steadiest first: its broad flat faces and the poses
/// it would rest in, then lying along its own axes. Reads only; `scan.stand_on_base` stands it on
/// one.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanBasesParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
}

/// Move a scan by one of the placing moves; one undo step, kept as a Move feature. A scan already
/// in place is not moved.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPlaceParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
}

/// Stand a scan on a base: the smallest turn that puts the base face down, then onto the grid
/// (Z = 0). One undo step, kept as a Move feature.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanStandParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// Which of `scan.bases`'s bases, from 0. Default: the steadiest, or, when the scan was last
    /// stood on a base this way and nothing has changed since, the next one. Ignored with `point`.
    #[serde(default)]
    pub base: Option<u32>,
    /// A point on the scan, in mm: the flat face around it is grown and the scan stands on it.
    /// A point on a curved wall is refused. Default: the point picked on the scan, if any.
    #[serde(default)]
    pub point: Option<Point3>,
}

/// A line drawn along a scan's rough mirror plane, to search near it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanGuide {
    /// One end of the line, in mm.
    pub from: Point3,
    /// The other end, in mm.
    pub to: Point3,
    /// The way the person was looking when they drew it, from the scan toward the eye. The plane
    /// is searched near the one through the line and this direction, turned about the line by up
    /// to 40° and tilted by up to 15°. Default: straight down from above, `[0, 0, 1]`.
    #[serde(default)]
    pub toward: Option<Point3>,
}

/// Find the plane a scan is a mirror image about and keep it as a construction plane, named
/// `Symmetry Plane`, to sketch on. A scan that is not a mirror image is refused, with how much of
/// it matched.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSymmetryParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// A line along the rough mirror plane, to find that plane rather than the best one. Default:
    /// the line between the first and last points picked on the scan, if there are two.
    #[serde(default)]
    pub guide: Option<ScanGuide>,
    /// Keep the plane as a construction plane. Default: true; false only reports it.
    #[serde(default)]
    pub keep: Option<bool>,
}

/// Square a scan to its mirror plane: the smallest turn that lays the plane along the YZ plane,
/// then sideways until it is X = 0, so the origin's YZ plane is the mirror plane to sketch on. One
/// undo step, kept as a Move feature. A scan that is not a mirror image is refused.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSquareParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// A line along the rough mirror plane, to find that plane rather than the best one. Default:
    /// the line between the first and last points picked on the scan, if there are two.
    #[serde(default)]
    pub guide: Option<ScanGuide>,
}

/// A scan's mirror plane and how well the scan matches its reflection in it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanMirror {
    /// A point of the plane over the middle of the scan, in mm.
    pub point: [f64; 3],
    /// A unit vector square to the plane.
    pub normal: [f64; 3],
    /// The typical distance from a point's reflection to the scan, over the best four fifths, in mm.
    pub rms: f64,
    /// The share of the scan whose reflection lands on the scan, 0 to 1.
    pub coverage: f64,
    /// Whether the scan is a mirror image about it: 85 % matched, typically within 2 % of its size.
    pub symmetric: bool,
}

/// What `scan.symmetry` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSymmetryResult {
    /// The mesh body.
    pub body: String,
    /// The plane found.
    pub mirror: ScanMirror,
    /// The construction plane that keeps it, as `construct.plane.three_points` replies, unless
    /// `keep` was false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub plane: Option<Value>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Fit a plane or a cylinder to the part of a scan round a point on it, and keep it as a
/// construction plane or axis to model from. The region grows from the point to every connected
/// triangle that lies on the shape; a region that is not that shape is refused.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFitParams {
    /// The mesh body. Default: the body of the point picked on a scan, else the selected mesh body,
    /// else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// A point on the scan, in mm, where the region grows from. Default: the point picked on the scan.
    #[serde(default)]
    pub point: Option<Point3>,
    /// How far a triangle may be from the shape and still belong to it, a length above 0. Default:
    /// 0.15 % of the scan's size.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Keep the shape as a construction plane or axis. Default: true; false only reports it.
    #[serde(default)]
    pub keep: Option<bool>,
}

/// A fitted plane.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFittedPlane {
    /// The middle of the region, on the plane, in mm.
    pub point: [f64; 3],
    /// A unit vector square to the plane.
    pub normal: [f64; 3],
}

/// A fitted cylinder.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFittedCylinder {
    /// The middle of the region's reach along the axis, on the axis, in mm.
    pub point: [f64; 3],
    /// A unit vector along the axis.
    pub axis: [f64; 3],
    /// The radius and the diameter, in mm.
    pub radius: f64,
    pub diameter: f64,
    /// How far the region reaches along the axis, in mm.
    pub length: f64,
}

/// A fitted sphere.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFittedSphere {
    /// Its centre, in mm.
    pub center: [f64; 3],
    /// The radius and the diameter, in mm.
    pub radius: f64,
    pub diameter: f64,
}

/// What `scan.fit_plane`, `scan.fit_cylinder` and `scan.fit_sphere` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFitResult {
    /// The mesh body.
    pub body: String,
    /// The plane fitted (`scan.fit_plane`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plane: Option<ScanFittedPlane>,
    /// The cylinder fitted (`scan.fit_cylinder`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cylinder: Option<ScanFittedCylinder>,
    /// The sphere fitted (`scan.fit_sphere`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sphere: Option<ScanFittedSphere>,
    /// How many triangles the region holds, and its area in mm².
    pub triangles: usize,
    pub area: f64,
    /// The region's points' distance from the shape: root mean square and largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// The construction plane, axis or point that keeps it, as the construction command replies,
    /// unless `keep` was false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Measure how far a scan is from the model built from it and colour the scan by it in the
/// viewport: within the tolerance one colour, above and below it two others, deepening to the
/// limit. A view, not a change to the design: no undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanDeviationParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub scan: Option<String>,
    /// The model it is measured against (a solid or surface body). Default: the selected one, else
    /// the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// How far off still counts as on the model, a length above 0. Default: 0.1 mm.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Where the colours stop deepening, a length above the tolerance. Default: 0.5 mm. Points
    /// more than four times as far count as parts of the scan the model leaves out.
    #[serde(default)]
    pub limit: Option<LengthExpr>,
    /// False takes the map away again. Default: true.
    #[serde(default)]
    pub show: Option<bool>,
}

/// What `scan.deviation` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanDeviationResult {
    /// Whether the map is shown now.
    pub shown: bool,
    /// The scan and the model measured (when shown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Over the points within reach, in mm: root mean square, mean, and the largest above (outside
    /// the model) and below (inside it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_below: Option<f64>,
    /// The share of the points within reach that are within the tolerance, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<f64>,
    /// How many scan points were measured, and how many were too far from the model to count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmatched: Option<usize>,
    /// The tolerance and the limit used, in mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Fit a smooth surface to the part of a scan round a point on it and keep it as a surface body
/// (an imported B-spline surface, one undo step). An area that wraps round is refused.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFitSurfaceParams {
    /// The mesh body. Default: the body of the point picked on a scan, else the selected mesh body,
    /// else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// A point on the scan, in mm, the middle of the area to fit. Default: the point picked on it.
    #[serde(default)]
    pub point: Option<Point3>,
    /// How far from the point the area reaches, a length above 0. Default: a tenth of the scan's
    /// size.
    #[serde(default)]
    pub radius: Option<LengthExpr>,
    /// Control points each way, 4 to 30. Default: 8. More follow finer shape.
    #[serde(default)]
    pub controls: Option<u32>,
    /// How smooth against how close, 0 to 1. Default: 0.1.
    #[serde(default)]
    pub smoothing: Option<f64>,
}

/// What `scan.fit_surface` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFitSurfaceResult {
    /// The scan.
    pub body: String,
    /// The surface body made.
    pub surface: String,
    /// The scan's points' distance from the surface: root mean square and largest, in mm.
    pub rms: f64,
    pub max: f64,
    /// How many scan points it was fitted to, and its control points each way.
    pub points: usize,
    pub controls: usize,
    /// The feature that keeps it, as a feature command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A way a scan could stand.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanBase {
    /// `flat` (a broad flat face), `resting` (a pose it rests in on several places), `axis` (lying
    /// along its own axes) or `grown` (a face grown from a point).
    pub kind: String,
    /// The way the part faces where it would touch the ground (unit, outward).
    pub normal: [f64; 3],
    /// A point of the ground plane, under the middle of where the part touches it, in mm.
    pub anchor: [f64; 3],
    /// A face's area, or the area of the outline it rests on, in mm²; 0 for an axis.
    pub area: f64,
    /// A face's share of the scan's surface, 0 to 1; 0 for a pose.
    pub share: f64,
    /// How far it must be tilted to tip over, in degrees; 0 when it would not stand or this is
    /// not known.
    pub tip_degrees: f64,
    /// A grown face's distance from its plane, root mean square, in mm; 0 otherwise.
    pub rms: f64,
}

/// What `scan.bases` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanBasesResult {
    /// The mesh body.
    pub body: String,
    /// The bases, steadiest first, at most 8, no two within 15° of each other.
    pub bases: Vec<ScanBase>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What the placing commands return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPlaceResult {
    /// The mesh body.
    pub body: String,
    /// Whether the scan moved: false when it was already in place (to a thousandth of a mm).
    pub moved: bool,
    /// The base it stands on (`scan.stand_on_base` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ScanBase>,
    /// Which of `scan.bases`'s bases it is, and how many there are, when it was chosen by number
    /// or by pressing again (not for a face grown from a point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_count: Option<usize>,
    /// The mirror plane it was squared to, before the move (`scan.square_to_symmetry` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<ScanMirror>,
    /// The Move feature that moved it, as `solid.move` replies, when it moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
