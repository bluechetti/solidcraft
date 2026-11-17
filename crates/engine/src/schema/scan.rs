//! Contracts and result types for the commands in `cmd/scan.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::features::FeatureResult;
use super::types::{AngleExpr, LengthExpr, Point3};

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

/// Stand a scan on a base: the smallest turn that puts the base face down, then the base flat on
/// the grid (Z = 0) with its middle over the origin. One undo step, kept as a Move feature.
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
    /// Three or more points on the scan, in mm (feet, rails, a rim): the scan stands on the plane
    /// fitted through them, the part above it. Used instead of `point` and `base`.
    #[serde(default)]
    pub points: Option<Vec<Point3>>,
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
/// `Symmetry Plane` (`Symmetry Plane 2`, … when that is taken), to sketch on. A plane given, or the
/// one near a guide, is kept however much of the scan matches it (the reply says how much); the
/// best plane found unasked is refused when the scan is no mirror image about it.
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
    /// The mirror plane itself, to use as it is (the plane a dialog shows). Instead of `guide`.
    #[serde(default)]
    pub plane: Option<ScanPlane>,
    /// Keep the plane as a construction plane. Default: true; false only reports it.
    #[serde(default)]
    pub keep: Option<bool>,
}

/// Square a scan to its mirror plane: the smallest turn that lays the plane along the YZ plane,
/// then sideways until it is X = 0, so the origin's YZ plane is the mirror plane to sketch on. One
/// undo step, kept as a Move feature. A plane given, or the one near a guide, is used however much
/// of the scan matches it; the best plane found unasked is refused when the scan is no mirror image
/// about it.
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
    /// The mirror plane itself, to use as it is (the plane a dialog shows). Instead of `guide`.
    #[serde(default)]
    pub plane: Option<ScanPlane>,
}

/// A mirror plane given by a point and the way it faces.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPlane {
    /// A point of the plane, in mm.
    pub point: Point3,
    /// The way the plane faces (either way; need not be unit length).
    pub normal: Point3,
}

/// Complete a scan by its mirror image: the scan reflected in its mirror plane, and the reflected
/// triangles kept where the scan has nothing, as a mesh body of their own beside the scan (amber,
/// named `<scan> Mirror Fill`). It is a guess that the two sides are alike. One undo step, kept as
/// a mesh import. A scan missing nothing on either side is refused.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCompleteParams {
    /// The mesh body. Default: the selected mesh body, else the design's only mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// The mirror plane to reflect in. Default: the one found near `guide`, else the best one found.
    /// A scan seen from one side matches little of itself, so this plane is used however little.
    #[serde(default)]
    pub plane: Option<ScanPlane>,
    /// A line along the rough mirror plane, to find that plane rather than the best one. Ignored
    /// with `plane`. Default: the line between the first and last points picked on the scan.
    #[serde(default)]
    pub guide: Option<ScanGuide>,
    /// How far from the scan a reflected triangle must lie to count as missing from it. Default:
    /// one and a half typical edges of the scan, or three times the plane's mismatch if more.
    #[serde(default)]
    pub reach: Option<LengthExpr>,
}

/// What `scan.complete_by_mirroring` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCompleteResult {
    /// The scan.
    pub body: String,
    /// The mesh body added: the reflected triangles where the scan had nothing.
    pub fill: String,
    /// The plane reflected in, and how well the scan matches itself in it.
    pub mirror: ScanMirror,
    /// Separate pieces added, their triangles and area (mm²).
    pub pieces: usize,
    pub triangles: usize,
    pub area: f64,
    /// Corners of the pieces put on a vertex of the scan, so they meet it.
    pub joined: usize,
    /// The reach used, in mm.
    pub reach: f64,
    /// When the scan is no mirror image about the plane: what was added is right for a scan of
    /// one side of a mirror-image part, but a part that is no mirror image does not have it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The feature that keeps it, as a feature command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
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
    /// A point on the scan, in mm, where the region grows from. Default: what is selected on the
    /// scan (fitted as it is), else the point picked on the scan.
    #[serde(default)]
    pub point: Option<Point3>,
    /// With the point, the region is the scan within this length of it, fitted as it is instead of
    /// grown.
    #[serde(default)]
    pub radius: Option<LengthExpr>,
    /// The region chosen by hand instead of grown from the point: the scan's triangle numbers.
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// The region chosen by hand: a kept selection (Keep Selection).
    #[serde(default)]
    pub saved: Option<String>,
    /// How far a triangle may be from the shape and still belong to it, a length above 0. Default:
    /// 0.15 % of the scan's size.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Keep the shape as a construction plane or axis. Default: true; false only reports it.
    #[serde(default)]
    pub keep: Option<bool>,
    /// What keeps it: `construction` (a construction plane, axis or point; the default), `face` (a
    /// surface of the shape trimmed to the region the scan has of it) or `both`.
    #[serde(default)]
    pub make: Option<ScanFitMake>,
}

/// What a fitted shape is kept as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScanFitMake {
    /// A construction plane, axis or point.
    #[default]
    Construction,
    /// A surface of the shape trimmed to the region the scan has of it.
    Face,
    /// Both.
    Both,
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

/// A cone fitted to the scan.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanFittedCone {
    /// Its apex, and its axis (unit) from the apex the way it widens.
    pub apex: [f64; 3],
    pub axis: [f64; 3],
    /// Its half-angle, in degrees.
    pub half_angle: f64,
}

/// A torus fitted to the scan.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanFittedTorus {
    /// Its centre, and its axis (unit).
    pub center: [f64; 3],
    pub axis: [f64; 3],
    /// The radius of its middle circle and of its tube, in mm.
    pub major: f64,
    pub minor: f64,
}

/// What `scan.fit_plane`, `scan.fit_cylinder`, `scan.fit_sphere`, `scan.fit_cone` and
/// `scan.fit_torus` return.
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
    /// The cone fitted (`scan.fit_cone`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<ScanFittedCone>,
    /// The torus fitted (`scan.fit_torus`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torus: Option<ScanFittedTorus>,
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
    /// The trimmed face, with `make` face or both: the surface body and the features that made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<ScanFitFace>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// The trimmed face a fitted shape was kept as.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFitFace {
    /// The surface body.
    pub surface: String,
    /// The sketch its outline or profile is drawn in.
    pub sketch: u64,
    /// The feature that made it (Patch or Surface Revolve), as that command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// How far round its axis it turns, in degrees (a plane: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<f64>,
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
    /// The model it is measured against: a solid or surface body, or a mesh body (a CAD model that
    /// came as an STL). Default: the selected solid or surface body, else the design's only one;
    /// with none, the only other mesh body.
    #[serde(default)]
    pub body: Option<String>,
    /// How far off still counts as on the model, a length above 0. Default: the scan's tolerance (`scan.tolerance`), else 0.1 mm.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Where the colours stop deepening, a length above the tolerance. Default: 0.5 mm. Points
    /// more than four times as far count as parts of the scan the model leaves out.
    #[serde(default)]
    pub limit: Option<LengthExpr>,
    /// True: the limit is read from the scan instead, the distance 99 % of the measured points lie
    /// within (at least just above the tolerance), so a few stray points do not wash out the
    /// colours of the rest. Not with `limit`. Default: false.
    #[serde(default)]
    pub auto_limit: Option<bool>,
    /// The colours in steps instead of a smooth scale: 0 (smooth, the default) or 2 to 12 steps on
    /// each side of within.
    #[serde(default)]
    pub bands: Option<u32>,
    /// Places on the scan whose distances are labelled in the view, each read at the scan point
    /// nearest it (`scan.deviation_pin` adds them by clicking). Default: none.
    #[serde(default)]
    pub pins: Option<Vec<Point3>>,
    /// False takes the map away again. Default: true.
    #[serde(default)]
    pub show: Option<bool>,
}

/// A scan point labelled with its distance from the model.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanDeviationPin {
    /// Where on the scan, mm.
    pub at: [f64; 3],
    /// Its distance from the model, mm: outside it positive, inside negative.
    pub value: f64,
}

/// Take the reference markers (small spheres) off a scan: the one at `at` fitted and every other
/// of its radius found, or the ones given; the holes they leave closed. Kept as a Scan Clean step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMarkersParams {
    /// The scan (mesh body). Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// A place on one marker: it is fitted there, and every other marker of its radius found.
    #[serde(default)]
    pub at: Option<Point3>,
    /// The markers' centres, instead of `at` (with `radius`).
    #[serde(default)]
    pub centres: Option<Vec<Point3>>,
    /// The markers' radius, with `centres`.
    #[serde(default)]
    pub radius: Option<LengthExpr>,
    /// How far a point may be off a marker's sphere and still be on it. Default: from the fit.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Shape each patch to carry the surface across. Default: true.
    #[serde(default)]
    pub fair: Option<bool>,
    /// True finds the markers and changes nothing. Default: false.
    #[serde(default)]
    pub preview: Option<bool>,
    /// Write the scan back into its Insert Mesh feature instead of adding a step.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// The step's name.
    #[serde(default)]
    pub name: Option<String>,
}

/// What `scan.remove_markers` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMarkersResult {
    pub body: String,
    /// The markers' radius, mm, and their centres.
    pub radius: f64,
    pub centres: Vec<[f64; 3]>,
    /// Triangles taken off, holes closed and holes that could not be (none on a preview).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_triangles: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filled: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<usize>,
    /// The scan's triangles after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triangles: Option<usize>,
    /// The Scan Clean step (or Insert Mesh feature, `in_place`) that keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<u64>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
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
    /// Whether the fit passes: at least 95 % of the measured points are within the tolerance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes: Option<bool>,
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
    /// Where the largest distance above and the largest below are (labelled in the view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_above_at: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_below_at: Option<[f64; 3]>,
    /// The colour steps on each side of within, 0 for a smooth scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bands: Option<u32>,
    /// The labelled places, each with its distance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<ScanDeviationPin>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Label a place on the scan with its distance from the model while Scan Deviation's map is shown,
/// or take labels off. A view, not a change to the design: no undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanDeviationPinParams {
    /// The place, read at the scan point nearest it.
    #[serde(default)]
    pub at: Option<Point3>,
    /// Takes off the label at this index (0 is the first added).
    #[serde(default)]
    pub remove: Option<usize>,
    /// True takes off every label.
    #[serde(default)]
    pub clear: Option<bool>,
}

/// What `scan.deviation_pin` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanDeviationPinResult {
    /// The label added (with `at`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<ScanDeviationPin>,
    /// above, below or within: where the added place lies against the model and the tolerance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Every label now shown.
    pub pins: Vec<ScanDeviationPin>,
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
    /// A point on the scan, in mm, the middle of the area to fit. Default: what is selected on
    /// the scan (fitted as it is), else the point picked on it.
    #[serde(default)]
    pub point: Option<Point3>,
    /// The region chosen by hand instead of grown from the point: the scan's triangle numbers.
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// The region chosen by hand: a kept selection (Keep Selection).
    #[serde(default)]
    pub saved: Option<String>,
    /// How far from the point the area reaches, a length above 0. Default: a tenth of the scan's
    /// size.
    #[serde(default)]
    pub radius: Option<LengthExpr>,
    /// Control points each way, 4 to 30. Default: 8, or with `tolerance` as many as bring the
    /// surface within half of it (from 6, stopping where more no longer help). More follow finer
    /// shape.
    #[serde(default)]
    pub controls: Option<u32>,
    /// How smooth against how close, 0 to 1. Default: 0.1.
    #[serde(default)]
    pub smoothing: Option<f64>,
    /// How close the surface must come to the scan, a length above 0: with no `controls`, the
    /// control points are chosen to come within half of it. Default: none (8 control points).
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// Trim the surface to the area's outline (seen square to the way the area faces), keeping
    /// the piece under the point. Default: false (the surface spans the area's bounding rectangle).
    #[serde(default)]
    pub trim: Option<bool>,
    /// The whole face round the point, with no radius: the area grows until the surface turns away
    /// (about 70° from the way it faces at the point). Default: false (the area within `radius`).
    #[serde(default)]
    pub whole_face: Option<bool>,
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
    /// Where the surface is wrong (Surface Check on it): folds (it doubles back), pinches (it
    /// narrows to nothing) and crossings (it passes through itself). A region too intricate for one
    /// surface, with deep crevices or curling over itself, folds.
    pub folds: usize,
    pub pinches: usize,
    pub crossings: usize,
    /// Said when the surface folds, pinches or crosses itself: what to do instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The feature that keeps it, as a feature command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// With `trim`: the Trim that cut it to the area's outline, as `surface.trim` replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed: Option<Value>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// One control point of a surface moved: its place in the net and where it goes (one of `to` or
/// `along`).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanControlMove {
    /// The control point, `[u, v]`: its column along u and row along v, from 0.
    pub point: [u32; 2],
    /// Where it goes, in mm.
    #[serde(default)]
    pub to: Option<Point3>,
    /// How far it goes along the way the surface faces there (negative the other way).
    #[serde(default)]
    pub along: Option<LengthExpr>,
}

/// Edit a surface Fit Surface made by its control points: move some, and fit those round them, or
/// all the others, to the scan again with the moved ones held. One undo step; the surface keeps
/// its name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanEditSurfaceParams {
    /// The surface body. Default: the selected one, else the design's only one.
    #[serde(default)]
    pub surface: Option<String>,
    /// The whole control net, u fastest (as many points as the surface has).
    #[serde(default)]
    pub points: Option<Vec<Point3>>,
    /// Control points moved.
    #[serde(default)]
    pub moves: Option<Vec<ScanControlMove>>,
    /// Control points kept where they are when refitting, besides those moved.
    #[serde(default)]
    pub hold: Option<Vec<[u32; 2]>>,
    /// Which control points are fitted to the scan again: `none`, `around` (the ring of two round
    /// each point moved or held) or `all` (every point not moved or held). Default: none.
    #[serde(default)]
    pub refit: Option<String>,
    /// The scan to refit to. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub scan: Option<String>,
    /// A whole new net, its shape changed (more or fewer points, other knots): given alone.
    #[serde(default)]
    pub net: Option<ScanNet>,
    /// How far a scan point may be from the surface to count as under it. Default: a tenth of the
    /// control net's size, at least 1 mm.
    #[serde(default)]
    pub reach: Option<LengthExpr>,
    /// A soft selection: the points within this distance of a point moved or slid go with it, less
    /// the further they are (none from the distance out), and are held when refitting. Default: 0,
    /// only the points named move.
    #[serde(default)]
    pub falloff: Option<LengthExpr>,
    /// Points slid along the surface instead of moved through space: the net slides over the
    /// surface, which keeps its shape as far as its points allow. Not with `moves` or `points`.
    #[serde(default)]
    pub slide: Option<ScanSlide>,
    /// Points on the surface moved (the surface runs through where each goes), as QuickSurface's
    /// points are dragged; on a Free Form of several faces, the way to edit it besides `faces`.
    #[serde(default)]
    pub dots: Option<Vec<ScanDotMove>>,
    /// With `dots`: `hold` (the default: the surface still runs through every other point on it)
    /// or `follow` (the points round a dot moved go with it, smoothly).
    #[serde(default)]
    pub neighbours: Option<String>,
    /// Every face of the surface as it now is (a Free Form's network, faces added), given alone.
    #[serde(default)]
    pub faces: Option<Vec<ScanNet>>,
    /// Shared edges of a Free Form of several faces made sharp: the faces are no longer held
    /// tangent across each, and the points near it are fitted to the scan again, so they take the
    /// scan's edge there. The design keeps a crease where the faces still meet smoothly. Given alone, or with
    /// `uncrease`.
    #[serde(default)]
    pub crease: Option<Vec<ScanEdgePick>>,
    /// Creased edges made smooth again: the faces held tangent across each, the points near it
    /// fitted to the scan again. Given alone, or with `crease`.
    #[serde(default)]
    pub uncrease: Option<Vec<ScanEdgePick>>,
    /// A smoothing brush dabbed at places on the surface: the points on it within the radius of
    /// each are evened out, more toward its middle, and kept on the scan. Given alone.
    #[serde(default)]
    pub brush: Option<ScanBrush>,
    /// The Free Form smoothed as a whole (Edit Surface's Smooth): `true` or {strength, passes,
    /// snap}. Given alone.
    #[serde(default)]
    pub smooth: Option<Value>,
    /// A face of a Free Form of several faces taken away (with Symmetry its mirror image too); the
    /// last face, and one between others, stay. Given alone.
    #[serde(default)]
    pub delete_face: Option<ScanFacePick>,
    /// A corner on the Free Form's edge dropped onto another: the two are one point, and two open
    /// sides whose ends then meet are sewn into one shared edge (held tangent where the faces go on
    /// from each other). Given alone.
    #[serde(default)]
    pub weld: Option<ScanWeld>,
    /// Symmetry turned off keeping one half: the faces on the far side of the plane taken away.
    /// Given alone.
    #[serde(default)]
    pub keep_half: Option<ScanKeepHalf>,
    /// With `faces`: the shared edges creased or held smooth by hand among them, kept in the design
    /// (Edit Surface's OK sends them). Default: the edges the design keeps.
    #[serde(default)]
    pub kept_edges: Option<ScanKeptEdges>,
    /// The plane the Free Form is kept a mirror image in (`YZ`, `XZ`, `XY`, a construction plane's
    /// name such as Find Symmetry's `Symmetry Plane`, `{point, normal}`, or `none`), with an edit
    /// of a Free Form of several faces: each dot's twin moves to the mirror image of where it goes,
    /// a dot on the plane stays on it, and the Free Form is kept an exact mirror image; the design
    /// keeps the plane. Default: the plane the design keeps for the Free Form (Mirror Free Form).
    /// A plane the Free Form is not a mirror image in is refused (`scan.mirror_free_form` mirrors
    /// a half).
    #[serde(default)]
    pub symmetry: Option<Value>,
}

/// Mirror a Free Form modelled on one side of a plane: its edges within the gap are put onto the
/// plane, and its mirror image is added, joined to it there, so it is one Free Form that Edit
/// Surface's Symmetry keeps a mirror image. One that is a mirror image already is left as it
/// is. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMirrorFreeFormParams {
    /// The Free Form. Default: the selected one, else the one made last.
    #[serde(default)]
    pub surface: Option<String>,
    /// The plane: `YZ`, `XZ`, `XY`, a construction plane's name (Find Symmetry's `Symmetry
    /// Plane`), or `{point, normal}`.
    pub plane: Value,
    /// How far from the plane an edge may be to be put onto it. Default: a tenth of the net's
    /// size, at least 1 mm.
    #[serde(default)]
    pub gap: Option<LengthExpr>,
}

/// What `scan.mirror_free_form` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMirrorFreeFormResult {
    /// The Free Form, and how many faces it has now.
    pub surface: String,
    pub faces: usize,
    /// `mirrored` (a half mirrored: `added` faces, `onto` points put onto the plane) or `already`
    /// (a mirror image as it was; nothing changed).
    pub how: String,
    pub added: usize,
    pub onto: usize,
    /// How many pairs of points the Free Form has, and how many points lie on the plane.
    pub pairs: usize,
    pub on_plane: usize,
    /// How far from tangent the faces meet at worst where they are held tangent, in degrees.
    pub kink: f64,
    /// The Import feature that keeps the Free Form.
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A smoothing brush dabbed on a surface.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanBrush {
    /// Where it is dabbed, in order (1 to 10000 places).
    pub points: Vec<Point3>,
    /// How far round each place it reaches: the points on the surface within it are smoothed,
    /// less the further out they are.
    pub radius: LengthExpr,
    /// How hard, 0 to 1 (default 0.5).
    #[serde(default)]
    pub strength: Option<f64>,
    /// How many times each dab is applied, 1 to 100 (default 4); at most 2000 in all.
    #[serde(default)]
    pub passes: Option<u32>,
    /// Whether the points are kept on the scan (default true); off, the surface is faired.
    #[serde(default)]
    pub snap: Option<bool>,
}

/// A shared edge of a Free Form: by a point near it, or by its two ends.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanEdgePick {
    /// The shared edge nearest this place (within the reach).
    #[serde(default)]
    pub at: Option<Point3>,
    /// Or: the edge whose ends are these (either way round).
    #[serde(default)]
    pub edge: Option<[Point3; 2]>,
    /// The edge and the edges it runs on into, through the corners where four faces meet as a
    /// grid's do, to the Free Form's edge or a corner of other than four faces (a double-click in
    /// Edit Surface). Default: false.
    #[serde(default)]
    pub chain: Option<bool>,
}

/// Smooth a Free Form as a whole: every point but its own corners and the corners where faces meet
/// unevenly straightens its rows and columns.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSmooth {
    /// How hard, above 0 and at most 1 (default 0.5).
    #[serde(default)]
    pub strength: Option<f64>,
    /// How many times, 1 to 20 (default 1).
    #[serde(default)]
    pub passes: Option<u32>,
    /// Whether the surface is kept on the scan (default true); off, it is faired.
    #[serde(default)]
    pub snap: Option<bool>,
}

/// A face of a Free Form, by a place near it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFacePick {
    /// The face that comes nearest this place (within the reach).
    pub at: Point3,
}

/// A corner on a Free Form's edge dropped onto another.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanWeld {
    /// The corner on the Free Form's edge nearest this place (within the reach) moves.
    pub at: Point3,
    /// Onto the other corner on its edge nearest this place (within the reach).
    pub onto: Point3,
}

/// Keep one half of a Free Form that is a mirror image of itself.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanKeepHalf {
    /// A place on the side of the plane whose half stays.
    pub side: Point3,
    /// The plane (`YZ`, `XZ`, `XY`, a construction plane's name, or `{point, normal}`). Default:
    /// the plane the design keeps for the Free Form.
    #[serde(default)]
    pub plane: Option<Value>,
}

/// The shared edges of a Free Form held otherwise than the angle its faces meet at says, each by
/// its two ends.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanKeptEdges {
    /// Creases though the faces meet smoothly.
    #[serde(default)]
    pub creases: Option<Vec<[Point3; 2]>>,
    /// Held tangent though the faces meet at an angle.
    #[serde(default)]
    pub smooths: Option<Vec<[Point3; 2]>>,
}

/// A point on a surface moved: the surface at a control point's place on it (one for each control
/// point, spread evenly over its face) taken to a place.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanDotMove {
    /// The point on the surface nearest this place moves (the way to name one on a Free Form of
    /// several faces, whose faces the body may list in another order after an edit).
    #[serde(default)]
    pub at: Option<Point3>,
    /// Or: the face, from 0 (default 0), and the control point whose place on it moves, `[u, v]`.
    #[serde(default)]
    pub face: Option<u32>,
    #[serde(default)]
    pub point: Option<[u32; 2]>,
    /// Where it goes, in mm.
    #[serde(default)]
    pub to: Option<Point3>,
    /// How far it goes along the way the surface faces there (negative the other way).
    #[serde(default)]
    pub along: Option<LengthExpr>,
}

/// Add a face to a Free Form on one of its open edges, out to two far corners: the new face shares
/// the edge, meets the face it grows from tangentially, and is fitted to the scan. One undo step;
/// the Free Form stays one surface body of several faces.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanAddFaceParams {
    /// The Free Form. Default: the selected one, else the one made last.
    #[serde(default)]
    pub surface: Option<String>,
    /// Two points at (or near) the ends of the open edge to build on, in mm.
    pub edge: Vec<Point3>,
    /// The new face's far corners, in mm, in either order.
    pub far: Vec<Point3>,
    /// The new face's points across, from the edge out: 4 to 12. Default: 4.
    #[serde(default)]
    pub rows: Option<u32>,
    /// The scan met no farther than this from a place. Default: no limit.
    #[serde(default)]
    pub reach: Option<LengthExpr>,
    /// The scan. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The plane the Free Form is kept a mirror image in (`YZ`, `XZ`, `XY`, a construction plane's
    /// name, `{point, normal}`, or `none`): the face's mirror image is added too. Default: the
    /// plane the design keeps for the Free Form (Mirror Free Form, Edit Surface's Symmetry), if any.
    #[serde(default)]
    pub symmetry: Option<Value>,
}

/// What `scan.add_face` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanAddFaceResult {
    /// The Free Form, and how many faces it has now.
    pub surface: String,
    pub faces: usize,
    /// The new face (its place among the faces, from 0) and its points each way.
    pub face: usize,
    /// With Symmetry: the face added as its mirror image (its place among the faces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twin: Option<usize>,
    pub controls: [usize; 2],
    /// The scan, and how far its points under the new face are from it (root mean square and
    /// largest, mm).
    pub body: String,
    pub rms: f64,
    pub max: f64,
    /// How far from tangent the faces meet at worst where they are held tangent, in degrees.
    pub kink: f64,
    /// Corners where other than four faces meet (inside) or more than three (on the edge).
    pub irregular: usize,
    /// The sides the new face shares that are left creases: where the scan turns sharply, it is not
    /// held tangent to the face there.
    pub sharp: usize,
    /// The Import feature that keeps the surface.
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Control points slid along a surface.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSlide {
    /// The control points, each `[u, v]`.
    pub points: Vec<[u32; 2]>,
    /// How far along u and along v, about, on the surface (negative the other way). At least one.
    #[serde(default)]
    pub u: Option<LengthExpr>,
    #[serde(default)]
    pub v: Option<LengthExpr>,
}

/// Make a Free Form: a cubic net over four corners clicked round its first face, fitted to the scan
/// under it, kept as a surface body of its own. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFreeFormParams {
    /// The four corners, in order round the face, in mm. Give these or `curves`.
    #[serde(default)]
    pub corners: Vec<Point3>,
    /// Instead of corners, the face's four sides: four curves, each a list of points in mm (a
    /// Curve on Scan curve's points), in any order and direction, meeting end to end. The face's
    /// edges follow them and its inside is fitted to the scan.
    #[serde(default)]
    pub curves: Option<Vec<Vec<Point3>>>,
    /// off (flat over the corners), follow (fitted to the scan, the default), overall (fitted, the
    /// places far off it left out, so it spans openings, dents and ribs) or flat (exactly flat: in
    /// the one plane fitted to the scan under it, the places far off it left out). With `plane`,
    /// flat is the default.
    #[serde(default)]
    pub fit: Option<String>,
    /// A plane to lay the face flat on instead of fitting one to the scan: `YZ`, `XZ`, `XY`, a
    /// construction plane's name, or `{point, normal}`. The corners are put onto it; no scan is
    /// needed.
    #[serde(default)]
    pub plane: Option<Value>,
    /// The net's points each way, 4 to 12. Default: 4. Follow (with stiffness 0) adds points, two at
    /// a time up to 12, until the face is within half the scan's tolerance.
    #[serde(default)]
    pub count: Option<u32>,
    /// How much the net keeps from bending to the scan, 0 to 1. Default: 0.
    #[serde(default)]
    pub stiffness: Option<f64>,
    /// The snap distance: the scan is met no farther than this from the face.
    #[serde(default)]
    pub reach: Option<LengthExpr>,
    /// The scan. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
}

/// What `scan.free_form` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFreeFormResult {
    /// The scan fitted to (absent when not fitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The surface body made, and its points each way.
    pub surface: String,
    pub controls: [usize; 2],
    /// The places fitted to, those met on the scan, and those kept.
    pub places: usize,
    pub met: usize,
    pub kept: usize,
    /// How far the scan under the face is from it, root mean square and largest, in mm (what Edit
    /// Surface shows for it; with no scan under it, the places' own distance).
    pub rms: f64,
    pub max: f64,
    /// When the face, following the scan, still lies farther off it than the scan's tolerance in
    /// places (it bows over a round too tight for one face): how far, and what to do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The feature that keeps it, as a feature command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Wrap a Free Form on a tube-like part of the scan: a cylinder fitted to the part round a point
/// on it, a net of rectangles laid on the cylinder over the part's reach and fitted to the scan,
/// kept as a surface body of its own. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanWrapSurfaceParams {
    /// A point on the part, in mm. Default: the point picked on the scan.
    #[serde(default)]
    pub point: Option<Point3>,
    /// How far round the point the part reaches. Default: a fifth of the scan's size.
    #[serde(default)]
    pub reach: Option<LengthExpr>,
    /// The part chosen by hand instead of the reach round the point: the scan's triangle numbers.
    #[serde(default)]
    pub triangles: Option<Vec<u64>>,
    /// The part chosen by hand: a kept selection (Keep Selection).
    #[serde(default)]
    pub saved: Option<String>,
    /// The part grown from the point: `true`, as far as the scan lies on the cylinder fitted there
    /// (as Fit Shape grows one); an angle (degrees, 1 to 90), the face there as far as the scan
    /// faces within it of the way it faces at the point, as Select on Scan's Region grows it. Not
    /// with `reach`.
    #[serde(default)]
    pub grow: Option<Value>,
    /// The cylinder's axis: fitted (to the part, the default), x, y or z (the world's, its line and
    /// radius fitted across the part).
    #[serde(default)]
    pub axis: Option<String>,
    /// How many points the net has the shorter way: low (4), medium (8, the default) or fine (14).
    #[serde(default)]
    pub details: Option<String>,
    /// How far round the cylinder the net goes, about the middle of the part. Default: the part's
    /// own reach.
    #[serde(default)]
    pub arc: Option<AngleExpr>,
    /// How much the net keeps from bending to the scan, 0 to 1. Default: 0.
    #[serde(default)]
    pub stiffness: Option<f64>,
    /// The scan. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
}

/// What `scan.wrap_surface` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWrapSurfaceResult {
    /// The scan wrapped on.
    pub body: String,
    /// The surface body made, and its points each way.
    pub surface: String,
    pub controls: [usize; 2],
    /// The cylinder the net was laid on: a point of its axis, the axis, the radius (mm).
    pub point: [f64; 3],
    pub axis: [f64; 3],
    pub radius: f64,
    /// How far the part is from the cylinder on average, in mm.
    pub off: f64,
    /// The places fitted to, and those met on the scan.
    pub places: usize,
    pub met: usize,
    /// How far the scan under the net is from it, root mean square and largest, in mm (what Edit
    /// Surface shows for it; with no scan under it, the places' own distance).
    pub rms: f64,
    pub max: f64,
    /// For a net that goes the whole way round the part: how far apart its two ends are, in mm (it is
    /// open there; Merge Surfaces with only this surface closes a seam within its gap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seam: Option<f64>,
    /// Where the surface is wrong (Surface Check on it): folds (it doubles back), pinches (it
    /// narrows to nothing) and crossings (it passes through itself). A net following rims and ribs
    /// it is too coarse for folds there.
    pub folds: usize,
    pub pinches: usize,
    pub crossings: usize,
    /// Said when the surface folds, pinches or crosses itself: what to do instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The feature that keeps it, as a feature command replies.
    #[schemars(with = "FeatureResult")]
    pub feature: Value,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Merge Free Forms whose sides run along each other: the sides within the gap made one curve (the
/// nets' knots matched, the corners and points put half way between), then the surfaces sewn into
/// one body. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMergeSurfacesParams {
    /// The surface bodies. Default: the selected ones, else every surface Fit Surface, New Free
    /// Form or Wrap Surface made.
    #[serde(default)]
    pub surfaces: Option<Vec<String>>,
    /// How far apart two sides may be, on average, to be made one. Default: a tenth of the
    /// shortest side.
    #[serde(default)]
    pub gap: Option<LengthExpr>,
    /// Sew the surfaces into one body (a solid when they close). Default: true.
    #[serde(default)]
    pub stitch: Option<bool>,
}

/// What `scan.merge_surfaces` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMergeSurfacesResult {
    /// The surfaces merged.
    pub surfaces: Vec<String>,
    /// How many pairs of sides were made one, and the widest gap closed, in mm.
    pub seams: usize,
    pub gap: f64,
    /// The one Free Form they are now (the first surface), and how many faces it has: edited
    /// with Edit Surface. Absent when they were made to meet but left as bodies of their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<usize>,
    /// The Remove feature that takes the other surfaces out of the timeline from here on, as a
    /// feature command replies; absent when one surface was closed on its own seam.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<FeatureResult>")]
    pub feature: Option<Value>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Slice a surface by a plane, keeping one side, and with `cap` close each loop of the cut with a
/// flat face sewn in (a solid when nothing else is open). One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSliceSurfaceParams {
    /// The plane: XY, XZ, YZ, a construction plane's name, or {origin, normal}.
    pub plane: Value,
    /// The side kept: front (where the plane's normal points, the default), back, or a point
    /// [x, y, z] on it.
    #[serde(default)]
    pub keep: Option<Value>,
    /// Close each loop of the cut with a flat face sewn in. Default: true; refused when the plane
    /// does not cut right round the surface.
    #[serde(default)]
    pub cap: Option<bool>,
    /// The surface body. Default: the selected one, else the surface made last.
    #[serde(default)]
    pub surface: Option<String>,
}

/// What `scan.slice_surface` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSliceSurfaceResult {
    /// The surface sliced.
    pub body: String,
    /// How many loops of the cut were capped, and whether the body is now a solid.
    pub caps: usize,
    pub solid: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Close the openings of a surface body with patches sewn in, so it becomes a solid; or only list
/// them. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCloseSurfaceParams {
    /// The surface body. Default: the selected one, else the surface made last.
    #[serde(default)]
    pub surface: Option<String>,
    /// Close the openings (default: true); false only lists them and changes nothing.
    #[serde(default)]
    pub close: Option<bool>,
    /// How a patch meets the faces round its opening: g0 (default) or g1 (tangent).
    #[serde(default)]
    pub continuity: Option<String>,
}

/// One loop of free edges: an opening of a surface.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanOpenLoop {
    /// How many edges it has.
    pub edges: usize,
    /// The middle of its edges, and the diagonal of the box round them (mm).
    pub at: [f64; 3],
    pub size: f64,
}

/// What `scan.close_surface` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCloseSurfaceResult {
    /// The surface body.
    pub body: String,
    /// The openings it had before.
    pub loops: Vec<ScanOpenLoop>,
    /// How many were closed, how many are open now, and whether the body is a solid.
    pub caps: usize,
    pub open: usize,
    pub solid: bool,
    /// The solid's volume (mm3), only when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A whole control net: its points each way, its knots each way, and its points (u fastest).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanNet {
    pub count: [u32; 2],
    pub knots: [Vec<f64>; 2],
    pub points: Vec<Point3>,
    /// The degree each way. Default: 3 and 3.
    #[serde(default)]
    pub degree: Option<[u32; 2]>,
}

/// A line of a net's points: a row (one `v` index, all along u) or a column (one `u` index).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanNetLine {
    #[serde(default)]
    pub row: Option<u32>,
    #[serde(default)]
    pub column: Option<u32>,
}

/// Change a surface's net as a whole (SurfaceCraft's net tools): `refine` doubles its points (in
/// u, v or both), `split` puts a line between `line` and its neighbour, `duplicate` copies `line`
/// beside itself (to drag the copy out), `remove` takes it away, `crease` makes it sharp (the
/// surface runs along it) and `uncrease` smooth again, `extend` copies a side `distance` out along
/// the surface's tangent. Refine and split leave the surface as it was. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanEditNetParams {
    /// The surface body. Default: the selected one, else the one made last.
    #[serde(default)]
    pub surface: Option<String>,
    /// refine, split, duplicate, remove, crease, uncrease or extend.
    pub op: String,
    /// The line the change is at (all but refine).
    #[serde(default)]
    pub line: Option<ScanNetLine>,
    /// duplicate: the copy after the line (default) or before it.
    #[serde(default)]
    pub after: Option<bool>,
    /// extend: how far out. Default: 10 mm.
    #[serde(default)]
    pub distance: Option<LengthExpr>,
    /// refine: in u, v or both (default).
    #[serde(default)]
    pub r#in: Option<String>,
}

/// What `scan.edit_net` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanEditNetResult {
    pub surface: String,
    /// The net's points each way now.
    pub controls: [usize; 2],
    /// The line made (split, duplicate, extend) or changed (crease, uncrease).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Value>,
    /// The lines that are creases.
    pub creases: Vec<Value>,
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `scan.edit_surface` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanEditSurfaceResult {
    /// The surface body.
    pub surface: String,
    /// Its control points along u and v.
    pub controls: [usize; 2],
    /// How many control points were moved (or slid), how many a falloff carried with them, and how
    /// many were fitted to the scan again.
    pub moved: usize,
    pub carried: usize,
    pub refitted: usize,
    /// The scan measured against, the scan points under the surface, and their distance from it
    /// (root mean square and largest, mm); absent with no scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// How many faces the surface has (more than one for a Free Form's network), and how many of
    /// the edges they share are creases (the faces meet at an angle there, not held tangent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creases: Option<usize>,
    /// `weld`: how many pairs of open sides were sewn into shared edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sewn: Option<usize>,
    /// The Import feature that keeps the surface.
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Draw a curve on a scan through points clicked along it: the smooth line through them laid on
/// the scan, kept as a 3D sketch spline through the fewest points within the tolerance. One undo
/// step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCurveParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The points clicked along the scan, in order, in mm (2 or more; 3 or more closed).
    pub points: Vec<Point3>,
    /// Close the curve back to its first point. Default: false.
    #[serde(default)]
    pub closed: Option<bool>,
    /// How far the curve may stray from the line laid on the scan. Default: the scan's typical
    /// triangle side, at most 0.1% of its size.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
    /// The 3D sketch to draw it in, by id or name. Default: the 3D sketch being edited, else the
    /// design's `Scan Curves`, made the first time.
    #[serde(default)]
    pub sketch: Option<Value>,
}

/// What `scan.curve_on_scan` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCurveResult {
    /// The scan.
    pub body: String,
    /// The 3D curve made, and the sketch it is in (id or name).
    pub curve: String,
    pub sketch: Value,
    pub closed: bool,
    /// The points clicked (on the scan) and the fit points kept.
    pub points: usize,
    pub fit_points: usize,
    /// The length of the line on the scan, in mm.
    pub length: f64,
    /// The farthest the curve strays from the line on the scan, and from the scan itself, in mm.
    pub max: f64,
    pub off_scan: f64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Take the floating pieces off a scan: the pieces (triangles joined through their corners) far
/// smaller than the largest. A Scan Clean step in the timeline keeps the settings; one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFloatingParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// A piece with fewer triangles than this share of the largest piece's floats (and never one
    /// of fewer than two). 0 to 0.5; default 0.02.
    #[serde(default)]
    pub below: Option<f64>,
    /// Points [x, y, z] on pieces to keep whatever their size.
    #[serde(default)]
    pub keep: Option<Vec<[f64; 3]>>,
    /// Points [x, y, z] on pieces to take off whatever their size.
    #[serde(default)]
    pub remove: Option<Vec<[f64; 3]>>,
}

/// What `scan.remove_floating` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFloatingResult {
    /// The scan.
    pub body: String,
    /// The pieces found, and those taken off with their triangles.
    pub pieces: usize,
    pub removed: usize,
    pub removed_triangles: usize,
    /// The triangles the scan has left.
    pub triangles: usize,
    /// The Scan Clean step that keeps the settings (with `in_place`, the Insert Mesh feature the
    /// scan was written back into).
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Close the holes in a scan with smooth patches (Liepa 2003): each hole's ring triangulated,
/// refined to the size of the scan's triangles round it, and faired to carry the surface across.
/// Each piece's outer rim is not a hole. A Scan Clean step in the timeline keeps the settings; one
/// undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanHolesParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// The largest hole to close, in edges round it. 3 to 5000; default 300.
    #[serde(default)]
    pub max_edges: Option<u32>,
    /// Shape each patch to carry the surface's curve across. Default: true; false leaves it flat.
    #[serde(default)]
    pub fair: Option<bool>,
    /// Points [x, y, z] by the holes to close: exactly the rings nearest them are closed, whatever
    /// their size, an outer rim among them capped; `max_edges` is then not used.
    #[serde(default)]
    pub at: Option<Vec<[f64; 3]>>,
}

/// What `scan.fill_holes` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanHolesResult {
    /// The scan.
    pub body: String,
    /// The holes found; those closed, those within the limit that could not be, and those larger.
    pub holes: usize,
    pub filled: usize,
    pub failed: usize,
    pub larger: usize,
    /// The triangles the patches added.
    pub added_triangles: usize,
    /// Open edges in no simple ring (a pinch, a dead end), left alone.
    pub unfillable_edges: usize,
    /// The Scan Clean step that keeps the settings (with `in_place`, the Insert Mesh feature the
    /// scan was written back into).
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Reduce a scan's triangles: edges collapsed cheapest first, where a collapse costs how far it
/// moves the surface (quadric errors), never folding it. A Scan Clean step in the timeline keeps
/// the settings; one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanReduceParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// The share of the triangles to keep, above 0 and at most 1. Default: 0.5 (unless `triangles`
    /// or `max_error` is given).
    #[serde(default)]
    pub ratio: Option<f64>,
    /// How many triangles to keep. Instead of `ratio`.
    #[serde(default)]
    pub triangles: Option<u64>,
    /// The most a collapse may move the surface, root mean square: it stops first if this is
    /// reached.
    #[serde(default)]
    pub max_error: Option<LengthExpr>,
    /// Keep the scan's open edges where they are. Default: true.
    #[serde(default)]
    pub keep_rim: Option<bool>,
}

/// What `scan.reduce` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanReduceResult {
    /// The scan.
    pub body: String,
    /// Its triangles before and after.
    pub before: usize,
    pub triangles: usize,
    pub collapses: usize,
    /// What ended it: `target` (the count asked for), `error` (`max_error`) or `no_more_collapses`.
    pub stopped: String,
    /// The largest error of any collapse made, root mean square, in mm.
    pub largest_error: f64,
    /// The Scan Clean step that keeps the settings (with `in_place`, the Insert Mesh feature the
    /// scan was written back into).
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Clean a scan in one go: Repair Mesh, Remove Floating Pieces, Fill Holes and, when asked,
/// Reduce Mesh, each with its usual settings and each kept as its own Scan Clean step, so any one
/// can be edited or taken out later. A step with nothing to do is left out and says why.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCleanUpParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Repair the triangles first. Default: true.
    #[serde(default)]
    pub repair: Option<bool>,
    /// Take off the floating pieces (under 2 % of the largest). Default: true.
    #[serde(default)]
    pub floating: Option<bool>,
    /// Close the holes of up to 300 edges. Default: true.
    #[serde(default)]
    pub holes: Option<bool>,
    /// Then reduce to this share of the triangles (above 0, below 1). Default: not reduced.
    #[serde(default)]
    pub reduce: Option<f64>,
}

/// One step of `scan.clean_up`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCleanUpStep {
    /// The tool's command.
    pub command: String,
    /// Whether it changed the scan (a step was kept).
    pub done: bool,
    /// What it did, or why it was left out.
    pub note: String,
    /// The Scan Clean step kept, when done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<u64>,
}

/// What `scan.clean_up` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanCleanUpResult {
    /// The scan.
    pub body: String,
    /// Each tool, in the order run.
    pub steps: Vec<ScanCleanUpStep>,
    /// The triangles the scan has at the end.
    pub triangles: usize,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Repair a scan's triangles: copies of a triangle taken off, an edge three or more faces meet
/// along left with the two that continue each other best, slivers flipped away. A Scan Clean
/// step in the timeline keeps the settings; one undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRepairParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Write the cleaned scan back into its Insert Mesh feature, as before, instead of adding a
    /// step to the timeline that keeps the settings. Default: false.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// Take off faces that pass through others they share no edge with, and the ring round them,
    /// and close the holes left with smooth patches. Default: true.
    #[serde(default)]
    pub crossings: Option<bool>,
}

/// What `scan.repair` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRepairResult {
    /// The scan.
    pub body: String,
    /// Copies taken off, faces taken off edges of three or more, slivers put right.
    pub duplicates: usize,
    pub non_manifold: usize,
    pub slivers: usize,
    /// Pairs of faces found passing through each other, the faces taken off for them (with the
    /// ring round them) and the holes that left closed again.
    pub crossings: usize,
    pub crossing_faces: usize,
    pub refilled: usize,
    /// The triangles the scan has now.
    pub triangles: usize,
    /// The Scan Clean step that keeps the settings (with `in_place`, the Insert Mesh feature the
    /// scan was written back into).
    pub feature: u64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Sketch where a plane cuts a scan: lines, arcs and circles each fitted to the cut's points (the
/// scan's noise averaged, not traced), end to end, smooth joins held tangent. In the sketch being
/// edited, or a new sketch on `plane`. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSectionParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The plane, as `sketch.create` takes it, when no sketch is being edited. Default: XY.
    #[serde(default)]
    pub plane: Option<Value>,
    /// How far along its normal to move the plane, as `sketch.create` takes it.
    #[serde(default)]
    pub offset: Option<LengthExpr>,
    /// How far a piece may stray from the cut's points. Default: three times the cut's noise, at
    /// least 0.1 mm and at most 1 % of the cut's size.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
}

/// What `scan.section_sketch` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSectionResult {
    /// The scan, and the sketch drawn in.
    pub body: String,
    pub sketch: u64,
    /// The outlines of the cut drawn, and the lines, arcs and circles they are.
    pub outlines: usize,
    pub lines: usize,
    pub arcs: usize,
    pub circles: usize,
    /// Joins held tangent.
    pub tangents: usize,
    /// The tolerance used, in mm.
    pub tolerance: f64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Sections along a curve: the scan cut by planes square to a curve of a 3D sketch at even steps
/// along it (end to end of an open curve, round a closed one), each kept as a sketch on its plane
/// of lines, arcs and circles fitted to the cut, as Section to Sketch draws. The planes turn with
/// the curve and never about it, so the sections loft in order without twisting. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanSectionsAlongParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The 3D sketch the curve is in, by id or name. Default: the design's last `Scan Curves`.
    #[serde(default)]
    pub sketch: Option<Value>,
    /// The curves the sections follow, end to end in order. Default: the sketch's last curve.
    #[serde(default)]
    pub curves: Option<Vec<String>>,
    /// How many sections, 1–60. Default: 8.
    #[serde(default)]
    pub count: Option<u32>,
    /// Keep every loop each plane cuts. Default: false, the loop round the curve with the loops
    /// inside it (a pipe's bore), or the loop nearest it.
    #[serde(default)]
    pub every: Option<bool>,
    /// How far a piece may stray from the cut's points. Default: three times the cut's noise, at
    /// least 0.1 mm and at most 1 % of the cut's size.
    #[serde(default)]
    pub tolerance: Option<LengthExpr>,
}

/// What `scan.sections_along_curve` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanSectionsAlongResult {
    /// The scan, and the sketches made, one a section, in order along the curve.
    pub body: String,
    pub sketches: Vec<u64>,
    /// The planes that cut nothing kept near the curve.
    pub missed: usize,
    /// The outlines drawn, and the lines, arcs and circles they are.
    pub outlines: usize,
    pub lines: usize,
    pub arcs: usize,
    pub circles: usize,
    /// Joins held tangent.
    pub tangents: usize,
    /// How long the curve is, and the tolerance used, in mm.
    pub length: f64,
    pub tolerance: f64,
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
