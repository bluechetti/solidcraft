//! Contracts and result types for the commands in `cmd/scan_reconstruct.rs`: Find All and
//! Reconstruct.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Cut the whole scan into regions, each a plane, cylinder, sphere, cone or torus, or freeform,
/// and colour the scan by them in the viewport. A view and not an undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFindAllParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// false takes the colouring away. Default: true.
    #[serde(default)]
    pub show: Option<bool>,
    /// The least distance (mm) a shape may lie from the scan and still take it. Default: the
    /// scan's tolerance when one was set for it, else three times the scan's noise. Raise it for a
    /// scan that is smooth but off by more than its noise (a fused depth-camera scan).
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// A shape Find All found.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanShapeFound {
    /// Its number, from 0, as Reconstruct and its tools name it.
    pub index: usize,
    /// plane, cylinder, sphere, cone or torus.
    pub kind: String,
    /// The scan it covers: its area in mm² and its triangles.
    pub area: f64,
    pub triangles: usize,
    /// How far its triangles' corners lie from it: the root mean square, in mm.
    pub rms: f64,
    /// A plane's point, a cylinder's point on its axis, a sphere's or torus's centre, a cone's apex.
    pub point: [f64; 3],
    /// A plane's normal (out of the material), or a round's axis (a cone's toward its widening).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<[f64; 3]>,
    /// A cylinder's or sphere's radius, a torus's major radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// A torus's tube radius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor: Option<f64>,
    /// A cone's half-angle, in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub half_angle: Option<f64>,
    /// For a cylinder: whether it is a hole (the material round it) rather than a boss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hole: Option<bool>,
}

/// What `scan.find_all` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFindAllResult {
    /// Whether the scan is coloured by its regions now.
    pub shown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The shapes, largest first.
    pub shapes: Vec<ScanShapeFound>,
    /// The freeform regions (no shape), and their area in mm².
    pub freeform: usize,
    pub freeform_area: f64,
    /// The share of the scan's area in shapes, 0–1.
    pub in_shapes: f64,
    /// The scan's noise and how close a shape had to lie, in mm.
    pub noise: f64,
    pub tolerance: f64,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Read what the scan's shapes make: its prisms, a turned part, repeats and round holes, and
/// what to build first. Runs Find All when it has not run on this scan. Reads only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanReconstructParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Read only the part of the scan selected (Select on Scan), as a part of its own: a boss,
    /// a turned feature or a hole of a part too busy to read whole. Default: false, the whole
    /// scan.
    #[serde(default)]
    pub selected: Option<bool>,
    /// Find All's least tolerance, as `scan.find_all` takes it (it reads again when it differs).
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// Reconstruct Selection: what the shapes make on the part of the scan selected, read as a part
/// of its own (Reconstruct with `selected`), and what to build. Reads only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanReconstructSelectionParams {
    /// The mesh body. Default: the one selected on.
    #[serde(default)]
    pub body: Option<String>,
}

/// A prism Reconstruct reads.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanPrism {
    /// Its number, from 0, as `scan.extrude_it` takes it.
    pub index: usize,
    /// Along it (unit), and its middle, where its profile is sliced.
    pub direction: [f64; 3],
    pub middle: [f64; 3],
    /// How long it is, end to end, in mm.
    pub depth: f64,
    /// The shapes its sides are (by Find All's numbers), and its end planes.
    pub walls: Vec<usize>,
    pub ends: [usize; 2],
    /// Whether one end is no plane (a domed top): it is then as far as all its walls run.
    pub open: bool,
    /// How far its walls lean in from its low end to its high end, in degrees (a moulded part's
    /// draft): Extrude It sketches it at its low end and extrudes it tapered. 0 for square walls.
    pub draft: f64,
    /// When its outline repeats round its middle (a gear's teeth, a knob's lobes): how.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotational: Option<ScanRotational>,
}

/// How a prism's outline repeats round its middle.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanRotational {
    /// How many times it repeats in one turn: a gear's tooth count.
    pub order: usize,
    /// The middle it repeats about, in the design (on the prism's middle slice).
    pub centre: [f64; 3],
    /// Twice its largest and its least distance from the middle, in mm: a gear's tip and root
    /// diameters.
    pub tip_diameter: f64,
    pub root_diameter: f64,
    /// Were it a spur gear of standard proportions, its module: the tip diameter over the tooth
    /// count plus two.
    pub module: f64,
}

/// Walls of one thickness, open at an end (a moulded housing, a tray): the prisms are read off
/// the outside, and Shell It hollows the body they make.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanShell {
    /// How thick the walls are, in mm.
    pub thickness: f64,
    /// The inner faces, set aside when the prisms are read, and the faces the shell leaves open
    /// (by Find All's numbers).
    pub inner: Vec<usize>,
    pub open: Vec<usize>,
}

/// The part read as turned.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanTurned {
    /// A point on the axis and its direction (unit).
    pub point: [f64; 3],
    pub direction: [f64; 3],
    /// The shapes about the axis (by Find All's numbers), and the share of the shapes' area they cover.
    pub faces: Vec<usize>,
    pub share: f64,
}

/// Faces repeated evenly round a turned part's axis.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanSpokes {
    pub count: usize,
    pub faces: Vec<usize>,
    /// The share of the shapes' area they cover.
    pub share: f64,
}

/// A round hole no profile holds, to cut.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanHole {
    /// Its number, from 0, as `scan.cut_it` and `scan.pattern_it` take it.
    pub index: usize,
    /// The shape it is (by Find All's number).
    pub shape: usize,
    pub radius: f64,
    pub diameter: f64,
    /// On its axis at its mouth and as far in as the scan has it, and how deep that is, in mm.
    pub mouth: [f64; 3],
    pub bottom: [f64; 3],
    pub depth: f64,
    /// For the first of a row or ring of holes alike: how many there are, and `linear`,
    /// `circular` or `grid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// How Hole It drills it, when it is more than a round hole Cut It cuts: counterbore,
    /// countersink, drilled (blind, with its drill point or a flat bottom) or tapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<ScanHoleForm>,
}

/// A hole as one Hole feature makes it, its sizes read as designed (a diameter within a share of
/// the scan's tolerance of a round size is that size).
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanHoleForm {
    /// counterbore, countersink, drilled or tapped.
    pub kind: String,
    /// The bore's diameter, in mm.
    pub diameter: f64,
    /// Through the part, or blind to `depth` (mm, from the mouth to where the bore's wall ends).
    pub through: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<f64>,
    /// A blind hole's drill point, its full angle in degrees (180: a flat bottom).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip_angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cb_diameter: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cb_depth: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cs_diameter: Option<f64>,
    /// The countersink's full angle, in degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cs_angle: Option<f64>,
    /// The thread whose tap drill the bore is (ISO metric coarse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The shapes it is made of, by Find All's numbers.
    pub faces: Vec<usize>,
}

/// Shapes alike and evenly spaced.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanRepeat {
    /// The kind of shape, and the shapes (by Find All's numbers) in order along the layout.
    pub kind: String,
    pub members: Vec<usize>,
    /// linear, circular or grid.
    pub layout: String,
    /// A linear repeat's step, a circular one's angle (degrees) and axis, a grid's two steps and
    /// its columns and rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centre: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
}

/// What `scan.reconstruct` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanReconstructResult {
    pub body: String,
    /// How many shapes Find All found, and the share of the scan's area in them, 0–1.
    pub shapes: usize,
    pub in_shapes: f64,
    pub prisms: Vec<ScanPrism>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turned: Option<ScanTurned>,
    /// With no turned part: the axis round shapes still share when they cover a quarter of the
    /// shapes or more (a wheel's rim and hub, its spokes flat), to rebuild it about by hand (`note`
    /// says how).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partly_turned: Option<ScanTurned>,
    /// A turned part's faces repeated evenly round its axis (a wheel's spokes), left out of its
    /// profile: how many places, and the faces (by Find All's numbers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spokes: Option<ScanSpokes>,
    pub holes: Vec<ScanHole>,
    pub repeats: Vec<ScanRepeat>,
    /// The body Reconstruct builds, once Extrude It or Revolve It has made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
    /// The next steps, as commands with their parameters, the body first: Revolve It for a turned
    /// part, else Extrude It for each prism, then Cut It and Pattern It for the holes, and Fillet
    /// It last for the rounds no profile holds.
    pub proposals: Vec<Value>,
    /// The rounds along edges no profile holds, which Fillet It fillets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fillets: Vec<ScanEdgeRound>,
    /// The flat chamfers along edges no profile holds, which Chamfer It chamfers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chamfers: Vec<ScanEdgeChamfer>,
    /// Walls of one thickness open at an end, which Shell It hollows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<ScanShell>,
    /// Prisms alike laid out evenly (a row of ribs, a ring of bosses): each the prisms' numbers,
    /// the first extruded and patterned by Pattern It with `prism`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prism_repeats: Vec<ScanRepeat>,
    /// A coil spring, which Coil It winds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coil: Option<ScanCoil>,
    /// A bar bent along a flat path, which Pipe It sweeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipe: Option<ScanPipe>,
    /// With nothing else read: the part as sections lofted, which Loft It lofts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loft: Option<ScanLoft>,
    /// Said where the scan is mostly no shape, or nothing can be built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Extrude It: a prism Reconstruct reads, built as an ordinary sketch of its profile (read off
/// its walls exactly as fitted, halfway along it) and an Extrude, symmetric to its depth. The
/// first body Reconstruct builds is new; each after joins it. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanExtrudeItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// Which of `scan.reconstruct`'s prisms, from 0. Default: the first.
    #[serde(default)]
    pub prism: Option<usize>,
    /// The body to join. Default: the body Reconstruct builds (`<scan> Reconstructed`), made new
    /// when there is none.
    #[serde(default)]
    pub into: Option<String>,
}

/// Revolve It: the turned part Reconstruct reads, built as an ordinary sketch of its profile on a
/// plane through the axis (each face held to the axis) and a Revolve all the way round. Joins the
/// body Reconstruct builds, or makes it. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRevolveItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The body to join. Default: the body Reconstruct builds, made new when there is none.
    #[serde(default)]
    pub into: Option<String>,
}

/// Cut It: a round hole Reconstruct reads, cut into the body it builds: a circle sketched square
/// to the hole outside its mouth and an Extrude cut as deep as the scan has the hole. One undo
/// step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanCutItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// Which of `scan.reconstruct`'s holes, from 0. Default: the first.
    #[serde(default)]
    pub hole: Option<usize>,
    /// The body to cut. Default: the body Reconstruct builds.
    #[serde(default)]
    pub into: Option<String>,
}

/// A coil spring as Reconstruct reads it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanCoil {
    /// On the axis where the wire's centre starts, and the direction it climbs.
    pub base: [f64; 3],
    pub axis: [f64; 3],
    /// The coil's diameter (to the wire's centre) and the wire's, in mm.
    pub diameter: f64,
    pub wire: f64,
    /// The climb per turn (mm) and the turns.
    pub pitch: f64,
    pub turns: f64,
    /// A left-hand coil.
    pub clockwise: bool,
    /// The shapes it is read from, by Find All's numbers.
    pub faces: Vec<usize>,
}

/// A bar bent along a flat path, as Reconstruct reads it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanPipe {
    /// The bar's diameter, in mm.
    pub diameter: f64,
    /// How many lines and arcs the path is.
    pub lines: usize,
    pub arcs: usize,
    /// How long the path is, in mm.
    pub length: f64,
    /// The shapes it is read from, by Find All's numbers.
    pub faces: Vec<usize>,
}

/// A part read as sections lofted.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ScanLoft {
    /// The direction it runs, and whether its ends are faces the scan has (flat ends).
    pub direction: [f64; 3],
    pub capped: bool,
    /// How many sections, and how long it is end to end, in mm.
    pub sections: usize,
    pub length: f64,
}

/// Loft It, Pipe It or Coil It: the part as sections lofted, as a bar swept along its path, or as
/// a coil, built as Reconstruct reads it. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanShapeItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it. Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The body to join. Default: the body Reconstruct builds, made new when there is none.
    #[serde(default)]
    pub into: Option<String>,
}

/// Reconstruct the Rest: what the body Reconstruct built leaves of the scan unexplained, read and
/// built piece by piece, each step that leaves the body further from the scan taken back. One undo
/// step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanRestParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// How many pieces to read and build at most, 1 to 20. Default: 6.
    #[serde(default)]
    pub rounds: Option<u64>,
}

/// One piece Reconstruct the Rest read: which piece (0 the largest left), its area in mm², the
/// steps proposed for it and those kept.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRestRound {
    pub piece: usize,
    pub area: f64,
    pub proposed: Vec<String>,
    pub kept: Vec<String>,
    /// Why nothing was proposed for it, when Reconstruct could not read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What Reconstruct the Rest did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanRestResult {
    /// The scan and the body built on it.
    pub body: String,
    pub solid: String,
    pub rounds: Vec<ScanRestRound>,
    /// How many steps were kept in all, and why it stopped.
    pub kept: usize,
    pub stop: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Hole It: a hole Reconstruct reads as a Hole feature makes it (a counterbore, a countersink, a
/// blind hole's drill point or flat bottom, a tap drill), drilled into the body it builds with
/// Hole at its mouth, along its axis, with the sizes read as designed. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanHoleItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// Which of `scan.reconstruct`'s holes, from 0. Default: the first with a `form`.
    #[serde(default)]
    pub hole: Option<usize>,
    /// Drill a bore the size of a tap drill as a tapped hole (its thread listed on the feature).
    /// Default: true; false drills it plain at its diameter.
    #[serde(default)]
    pub thread: Option<bool>,
    /// The body to drill. Default: the body Reconstruct builds.
    #[serde(default)]
    pub into: Option<String>,
}

/// Pattern It: the first of a row or ring of holes, once cut, repeated by a pattern of its cut
/// with the repeat's count and spacing. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPatternItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// Which of `scan.reconstruct`'s holes, from 0: one that repeats. Default: the first that does.
    #[serde(default)]
    pub hole: Option<usize>,
    /// Or which of `scan.reconstruct`'s prisms, from 0: the first of a row or ring of prisms
    /// alike (its `prism_repeats`), whose extrusion is repeated.
    #[serde(default)]
    pub prism: Option<usize>,
    /// The cut to repeat. Default: the one Cut It made for that hole.
    #[serde(default)]
    pub feature: Option<String>,
}

/// A round along an edge of the part that no profile Reconstruct sketches holds.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanEdgeRound {
    /// The shape it is, as Find All numbers them.
    pub face: usize,
    /// Its radius as fitted, in mm.
    pub radius: f64,
    /// A point on the sharp edge of the model it rounds, where Fillet It measures it.
    pub edge: [f64; 3],
}

/// Fillet It: the rounds along edges Reconstruct reads that no profile holds, each measured off
/// the scan at the edge of the built body it rounds, as Fillet from Scan measures, and the edges
/// where a round fits filleted at the radius measured: one Fillet for each radius. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFilletItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The body to fillet. Default: the body Reconstruct builds.
    #[serde(default)]
    pub into: Option<String>,
}

/// One radius Fillet It filleted at, and how many edges.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFilleted {
    pub radius: f64,
    pub edges: usize,
}

/// What Fillet It returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFilletItResult {
    /// The scan, and the body filleted.
    pub body: String,
    pub solid: String,
    /// Each radius measured, with how many edges it filleted.
    pub fillets: Vec<ScanFilleted>,
    /// How many rounds were set aside: the scan shows the edge sharp there, or the body has no
    /// edge where the round is.
    pub set_aside: usize,
    /// The Fillet features made.
    pub features: Vec<Value>,
    /// `false` when no round the scan shows fits better than the body's sharp edge: the body is as
    /// it was, and `note` says why. Left out when a fillet was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// How close the body is to the scan before and after the step (absent when it could not be
    /// measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<ScanStepFit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// How close a body is to the scan, read on a sample of the scan's points.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFitSample {
    /// The share of the points within the scan's tolerance of the body's surface, and within the
    /// limit (five tolerances, at least 0.5 mm): the part of the scan the body explains.
    pub within: f64,
    pub explained: f64,
    /// Root mean square distance over the points within reach of the body (mm).
    pub rms: f64,
    /// The points read.
    pub points: usize,
}

/// How a Reconstruct step changed the fit of the body it builds to the scan.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanStepFit {
    /// The body measured, before the step (absent when the step made it) and after.
    pub solid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ScanFitSample>,
    pub after: ScanFitSample,
    /// first (the step made the body), better, same or worse.
    pub verdict: String,
    /// What to do about a step that made the fit worse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What Extrude It, Revolve It, Cut It and Pattern It return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanBuildResult {
    /// The scan.
    pub body: String,
    /// The sketch made (none for Pattern It), and the feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sketch: Option<u64>,
    pub feature: Value,
    /// The body built, and how: new, join or cut.
    pub solid: String,
    pub operation: String,
    /// The profile's lines and arcs, and joins held tangent.
    pub lines: usize,
    pub arcs: usize,
    pub tangents: usize,
    /// The depth extruded or cut, in mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<f64>,
    /// How many places a pattern makes (for Revolve It on a wheel: the openings cut between its
    /// spokes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Revolve It on a wheel: the openings between its spokes, cut and patterned round the axis,
    /// or why they were not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openings: Option<String>,
    /// How close the body is to the scan before and after the step (absent when it could not be
    /// measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<ScanStepFit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "scan_reconstruct_tests.rs"]
mod tests;

/// A flat chamfer along an edge of the part that no profile Reconstruct sketches holds.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanEdgeChamfer {
    /// The shape it is, as Find All numbers them.
    pub face: usize,
    /// How far it cuts back along each of the two faces it joins, in mm.
    pub distance: f64,
    /// A point on the sharp edge of the model it cuts, where Chamfer It finds that edge.
    pub edge: [f64; 3],
}

/// Chamfer It: the flat chamfers along edges Reconstruct reads that no profile holds, cut at the
/// edge of the built body they cut, at the distance read off the scan's two faces: one Chamfer for
/// each distance. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanChamferItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it (its numbers are that reading's). Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The body to chamfer. Default: the body Reconstruct builds.
    #[serde(default)]
    pub into: Option<String>,
}

/// Shell It: the body Reconstruct builds hollowed to the thickness of the scan's walls, the faces
/// with no inner face behind them left open. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanShellItParams {
    /// The mesh body. Default: the selected mesh body, else the design's only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Build from the reading of the part of the scan selected, as `scan.reconstruct` with
    /// `selected` gave it. Default: false, the whole scan's.
    #[serde(default)]
    pub selected: Option<bool>,
    /// The body to hollow. Default: the body Reconstruct builds.
    #[serde(default)]
    pub into: Option<String>,
}

/// One distance Chamfer It chamfered at, and how many edges.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanChamfered {
    pub distance: f64,
    pub edges: usize,
}

/// What Chamfer It returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanChamferItResult {
    /// The scan, and the body chamfered.
    pub body: String,
    pub solid: String,
    /// Each distance read, with how many edges it chamfered.
    pub chamfers: Vec<ScanChamfered>,
    /// How many chamfers were set aside: the kernel refused the edge.
    pub set_aside: usize,
    /// The Chamfer features made.
    pub features: Vec<Value>,
    /// How close the body is to the scan before and after the step (absent when it could not be
    /// measured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<ScanStepFit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
