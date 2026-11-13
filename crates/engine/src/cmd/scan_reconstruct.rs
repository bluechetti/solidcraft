//! Find All and Reconstruct (#8615), after SurfaceCraft's Scan to CAD: `scan.find_all` cuts the
//! scan into regions, each a simple shape or freeform, and colours the scan by them;
//! `scan.reconstruct` reads what those shapes make (prisms, a turned part, repeats, round holes)
//! and proposes what to build. Each proposal is built as ordinary features, so the result is an
//! editable model: `scan.extrude_it` sketches a prism's profile off its walls and extrudes it,
//! `scan.revolve_it` sketches a turned part's profile off its faces held to its axis and revolves
//! it, `scan.cut_it` cuts a round hole, and `scan.pattern_it` repeats a cut hole as its row or
//! ring does. The first body built is new (`<scan> Reconstructed`) and each after joins it.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
pub use solidcraft_scan::fit::Shape;
use solidcraft_scan::profile::{self, Outline, Piece, Turned};
/// What Reconstruct reads off a scan, as its dialog lists it.
pub use solidcraft_scan::reconstruct::Reading;
use solidcraft_scan::reconstruct::{self, Layout};
use solidcraft_scan::segment::{self, Outcome};
use solidcraft_sketch::{ConstraintKind, Sketch};

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::schema::scan_reconstruct::{
    ScanBuildResult, ScanChamferItParams, ScanChamferItResult, ScanChamfered, ScanCutItParams, ScanEdgeChamfer, ScanEdgeRound, ScanExtrudeItParams,
    ScanFilletItParams, ScanFilletItResult, ScanFilleted, ScanFindAllParams, ScanFindAllResult, ScanHole, ScanPatternItParams, ScanPrism,
    ScanReconstructParams, ScanReconstructResult, ScanReconstructSelectionParams, ScanRepeat, ScanRevolveItParams, ScanShapeFound, ScanTurned,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const FIND_ALL_CONTRACT: TypedContract<ScanFindAllParams> = TypedContract::new();
const RECONSTRUCT_CONTRACT: TypedContract<ScanReconstructParams> = TypedContract::new();
const RECONSTRUCT_SELECTION_CONTRACT: TypedContract<ScanReconstructSelectionParams> = TypedContract::new();
const EXTRUDE_IT_CONTRACT: TypedContract<ScanExtrudeItParams> = TypedContract::new();
const REVOLVE_IT_CONTRACT: TypedContract<ScanRevolveItParams> = TypedContract::new();
const CUT_IT_CONTRACT: TypedContract<ScanCutItParams> = TypedContract::new();
const PATTERN_IT_CONTRACT: TypedContract<ScanPatternItParams> = TypedContract::new();
const FILLET_IT_CONTRACT: TypedContract<ScanFilletItParams> = TypedContract::new();
const CHAMFER_IT_CONTRACT: TypedContract<ScanChamferItParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.find_all", "Find All", crate::checked!("scan.find_all", find_all))
        .at("SCAN", "RECONSTRUCT")
        .icon("colored_components")
        .noundo()
        .contract(&FIND_ALL_CONTRACT)
        .output(output_schema_for::<ScanFindAllResult>)
        .params("body?: mesh body (default: the selected or only one); tolerance?: mm (the least distance a shape may lie from the scan and still take it; default: the scan's tolerance when one is set, else three times its noise; raise it for a scan smooth but off by more than its noise, as a fused depth-camera scan is); show?: bool (false takes the colouring away): the whole scan cut into regions, each a plane, cylinder, sphere, cone or torus or freeform (the scan's noise sets how close a shape must lie), coloured in the viewport; a view and not an undo step → shapes [{index, kind, area, triangles, rms, point, direction?, radius?, minor?, half_angle?, hole?}], freeform, freeform_area, in_shapes, noise, tolerance"),
    CommandSpec::new("scan.reconstruct", "Reconstruct", crate::checked!("scan.reconstruct", read_scan))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .noundo()
        .contract(&RECONSTRUCT_CONTRACT)
        .output(output_schema_for::<ScanReconstructResult>)
        .params("body?: mesh body (default: the selected or only one); tolerance?: mm (Find All read again with this least tolerance, as scan.find_all takes it); selected?: bool (default false: the whole scan; true: only the part of it selected, read as a part of its own, and each step proposed takes `selected`): what Find All's shapes make (it runs Find All first when it has not run on this scan): prisms (stretches of one section along a direction a plane faces), a turned part (most of the shapes about one axis), repeats (in a row, round an axis, in a grid, drilled along the radius) and the round holes no profile holds; reads only → shapes, in_shapes, prisms [{index, direction, middle, depth, walls, ends, open}], turned? {point, direction, faces, share}, holes [{index, shape, radius, diameter, mouth, bottom, depth, repeat?, layout?}], repeats, built?, proposals [{command, params}], note?"),
    CommandSpec::new("scan.reconstruct_selection", "Reconstruct Selection", crate::checked!("scan.reconstruct_selection", read_selection))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .noundo()
        .contract(&RECONSTRUCT_SELECTION_CONTRACT)
        .output(output_schema_for::<ScanReconstructResult>)
        .params("body?: mesh body (default: the one selected on): scan.reconstruct with selected: true, what the shapes make on the part of the scan selected, read as a part of its own (a boss, a turned feature, a hole of a part too busy to read whole), and the steps that build it, each with `selected`; refused with nothing selected; reads only → as scan.reconstruct"),
    CommandSpec::new("scan.extrude_it", "Extrude It", crate::checked!("scan.extrude_it", fitted_extrude_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("extrude")
        .contract(&EXTRUDE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); prism?: index of scan.reconstruct's prisms (default 0); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the prism's profile sketched halfway along it off its walls exactly as fitted (lines and arcs from crossing to crossing, joins held tangent) and extruded symmetric to its depth, one undo step → sketch, feature, solid, operation, lines, arcs, tangents, depth, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.revolve_it", "Revolve It", crate::checked!("scan.revolve_it", fitted_revolve_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("revolve")
        .contract(&REVOLVE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the turned part's profile sketched on a plane through its axis, each face held to the axis (cylinders level, ends upright, rounds tangent), and revolved all the way round, one undo step → sketch, feature, solid, operation, lines, arcs, tangents, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.cut_it", "Cut It", crate::checked!("scan.cut_it", fitted_cut_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("hole")
        .contract(&CUT_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); hole?: index of scan.reconstruct's holes (default 0); into?: body to cut (default: `<scan> Reconstructed`): a circle sketched square to the hole outside its mouth and cut as deep as the scan has the hole, one undo step → sketch, feature, solid, operation, depth, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.pattern_it", "Pattern It", crate::checked!("scan.pattern_it", fitted_pattern_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("pattern_circ")
        .contract(&PATTERN_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); hole?: index of scan.reconstruct's holes, one that repeats (default: the first that does); feature?: the cut to repeat (default: the one Cut It made for that hole): the cut repeated as the row or ring of holes is, with its count and spacing, one undo step → feature, solid, count, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.fillet_it", "Fillet It", crate::checked!("scan.fillet_it", fitted_fillet_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("fillet")
        .contract(&FILLET_IT_CONTRACT)
        .output(output_schema_for::<ScanFilletItResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to fillet (default: `<scan> Reconstructed`): each round along an edge that scan.reconstruct reads and no profile holds (its `fillets`), measured off the scan as Fillet from Scan does at the edge of the body it rounds; the edges where a round fits the scan filleted at the radius measured, one Fillet for each radius, one undo step; an edge where the scan shows a sharp edge, or no edge of the body, is set aside → body, solid, fillets [{radius, edges}], set_aside, features, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.chamfer_it", "Chamfer It", crate::checked!("scan.chamfer_it", fitted_chamfer_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("chamfer")
        .contract(&CHAMFER_IT_CONTRACT)
        .output(output_schema_for::<ScanChamferItResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to chamfer (default: `<scan> Reconstructed`): each flat chamfer along an edge that scan.reconstruct reads and no profile holds (its `chamfers`), cut at the edge of the body it cuts at the distance its two faces give; one Chamfer for each distance, one undo step; an edge the kernel refuses is set aside → body, solid, chamfers [{distance, edges}], set_aside, features, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
];

/// What Find All found on a scan, and what Reconstruct reads off it: view state for the viewport
/// to colour, kept while the scan's mesh is the same (building from it leaves it so).
#[derive(Debug)]
pub struct ScanShapes {
    pub scan: String,
    fingerprint: u64,
    /// Where the scan's positions and triangles were held, and how many: the same storage is the
    /// same mesh, told without reading it.
    held: [usize; 4],
    pub outcome: Outcome,
    pub reading: Reading,
    /// Each triangle's region (an index into the outcome's segments), for colouring.
    pub regions: Vec<u32>,
    /// Whether the viewport colours the scan by its regions.
    pub shown: bool,
    /// What Reconstruct last read off the part of the scan selected (`selected`), if it has.
    pub region: Option<Arc<RegionShapes>>,
    /// The least tolerance Find All was asked to read with (0: its own from the scan's noise).
    pub least: f64,
}

/// What Reconstruct read off the part of a scan selected, and which selection that was.
#[derive(Debug)]
pub struct RegionShapes {
    /// One flag a triangle of the scan.
    selection: Arc<Vec<bool>>,
    pub region: reconstruct::Region,
}

impl RegionShapes {
    /// Whether it was read off `selected`.
    fn of(&self, selected: &Arc<Vec<bool>>) -> bool {
        Arc::ptr_eq(&self.selection, selected) || *self.selection == **selected
    }
}

/// The reading a Reconstruct step works from: the whole scan's, or (`selected`) the part of it
/// selected now.
struct View {
    found: Arc<ScanShapes>,
    region: Option<Arc<RegionShapes>>,
}

impl View {
    fn reading(&self) -> &Reading {
        self.region.as_ref().map_or(&self.found.reading, |r| &r.region.reading)
    }

    fn outcome(&self) -> &Outcome {
        self.region.as_ref().map_or(&self.found.outcome, |r| &r.region.outcome)
    }

    /// The scan the reading is of: the whole, or the part selected, on the scan's points.
    fn scan<'a>(&'a self, positions: &'a [Vec3], triangles: &'a [[u32; 3]]) -> Scan<'a> {
        match &self.region {
            Some(r) => Scan::new(positions, &r.region.triangles),
            None => Scan::new(positions, triangles),
        }
    }

    fn selected(&self) -> bool {
        self.region.is_some()
    }
}

/// The reading for `p`: the whole scan's ([`shapes`]), or with `selected` the part of it selected
/// now, read the first time it is asked for and kept while the selection and the scan are the
/// same.
fn view(s: &mut Session, p: &Value, cmd: &str, show: bool) -> Result<View> {
    let found = shapes(s, p, cmd, show)?;
    if !bool_(p, "selected").unwrap_or(false) {
        return Ok(View { found, region: None });
    }
    let selection = super::select::selected(s)
        .filter(|k| k.scan == found.scan && k.count > 0)
        .ok_or_else(|| bad(cmd, format!("nothing is selected on `{}`: select the part to rebuild first (Select on Scan)", found.scan)))?;
    if let Some(region) = found.region.clone().filter(|r| r.of(&selection.selected)) {
        return Ok(View { found, region: Some(region) });
    }
    let region = {
        let state = s.model.state();
        let (positions, triangles) =
            state.body(&found.scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        reconstruct::read_region(&Scan::new(positions, triangles), &found.outcome, &selection.selected)
            .ok_or_else(|| bad(cmd, format!("too little is selected on `{}` to read: select at least 50 of its triangles", found.scan)))?
    };
    let region = Arc::new(RegionShapes { selection: selection.selected.clone(), region });
    let with = Arc::new(found.with(found.shown, Some(region.clone())));
    s.scan_shapes = Some(with.clone());
    Ok(View { found: with, region: Some(region) })
}

impl ScanShapes {
    /// Whether it was found on `scan` as it is now: the same storage (asked every frame, at no
    /// cost), else the same fingerprint.
    pub fn current(&self, s: &Session) -> bool {
        let state = s.model.state();
        state
            .body(&self.scan)
            .and_then(|b| b.body.triangle_mesh())
            .is_some_and(|(p, t)| held(p, t) == self.held || fingerprint(p, t) == self.fingerprint)
    }

    /// What Reconstruct read off the part of the scan selected now, if it has read it.
    pub fn selected_region(&self, s: &Session) -> Option<Arc<RegionShapes>> {
        let selection = super::select::selected(s).filter(|k| k.scan == self.scan && k.count > 0)?;
        self.region.clone().filter(|r| r.of(&selection.selected))
    }

    /// The same, shown or not, with the reading of a part of it selected.
    fn with(&self, shown: bool, region: Option<Arc<RegionShapes>>) -> ScanShapes {
        ScanShapes {
            scan: self.scan.clone(),
            fingerprint: self.fingerprint,
            held: self.held,
            outcome: self.outcome.clone(),
            reading: self.reading.clone(),
            regions: self.regions.clone(),
            shown,
            region,
            least: self.least,
        }
    }
}

pub(super) fn held(positions: &[Vec3], triangles: &[[u32; 3]]) -> [usize; 4] {
    [positions.as_ptr() as usize, positions.len(), triangles.as_ptr() as usize, triangles.len()]
}

/// A cheap fingerprint of a mesh: its sizes and every position's and triangle's bits, folded.
pub(super) fn fingerprint(positions: &[Vec3], triangles: &[[u32; 3]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (positions.len() as u64) ^ ((triangles.len() as u64) << 32);
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
    };
    for p in positions {
        mix(p.x.to_bits());
        mix(p.y.to_bits());
        mix(p.z.to_bits());
    }
    for t in triangles {
        mix(u64::from(t[0]) | (u64::from(t[1]) << 21) | (u64::from(t[2]) << 42));
    }
    h
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// The least tolerance Find All reads `scan` with: `tolerance` when given, else the scan's own
/// tolerance when one was set for it (`scan.tolerance`), else none (0: its own from the noise).
fn least_tolerance(s: &Session, p: &Value, scan: &str, cmd: &str) -> Result<f64> {
    match p.get("tolerance").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|t| t.is_finite() && *t > 0.0 && *t < 1e6).ok_or_else(|| bad(cmd, "`tolerance` must be a length in mm above 0")),
        None => Ok(if super::tolerance::is_set(s, scan) { super::tolerance::of(s, scan) } else { 0.0 }),
    }
}

/// Find All on the scan `p` names (or the selected or only one), kept in the session: what was
/// found before on the same mesh is used again.
fn shapes(s: &mut Session, p: &Value, cmd: &str, show: bool) -> Result<Arc<ScanShapes>> {
    let name = super::mesh_body_name(s, str_(p, "body").filter(|b| !b.trim().is_empty()), cmd)?;
    let least = least_tolerance(s, p, &name, cmd)?;
    // Find All reads again when its tolerance changed, Reconstruct only when given one; the build
    // steps take the reading they were proposed from, whatever it was read with.
    let reads = cmd == "scan.find_all" || (cmd.starts_with("scan.reconstruct") && p.get("tolerance").is_some_and(|v| !v.is_null()));
    if let Some(known) = s.scan_shapes.clone().filter(|k| k.scan == name && k.current(s) && (!reads || k.least == least)) {
        if known.shown || !show {
            return Ok(known);
        }
        let shown = Arc::new(known.with(true, known.region.clone()));
        s.scan_shapes = Some(shown.clone());
        return Ok(shown);
    }
    let (outcome, reading, print, at, regions) = {
        let state = s.model.state();
        let body = state.body(&name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
        let (positions, triangles) = body.body.triangle_mesh().ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
        if triangles.is_empty() {
            return Err(bad(cmd, format!("`{name}` has no triangles")));
        }
        let scan = Scan::new(positions, triangles);
        let outcome = segment::find_all(&scan, &segment::Options { least_tolerance: least, ..segment::Options::default() }, &|| false);
        let reading = reconstruct::read(&scan, &outcome);
        let mut regions = vec![u32::MAX; triangles.len()];
        for (k, segment) in outcome.segments.iter().enumerate() {
            for &t in &segment.triangles {
                if let Some(slot) = regions.get_mut(t) {
                    *slot = k as u32;
                }
            }
        }
        (outcome, reading, fingerprint(positions, triangles), held(positions, triangles), regions)
    };
    let found = Arc::new(ScanShapes { scan: name, fingerprint: print, held: at, outcome, reading, regions, shown: show, region: None, least });
    s.scan_shapes = Some(found.clone());
    Ok(found)
}

fn at(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn kind_name(shape: &Shape) -> &'static str {
    match shape {
        Shape::Plane { .. } => "plane",
        Shape::Cylinder { .. } => "cylinder",
        Shape::Sphere { .. } => "sphere",
        Shape::Cone { .. } => "cone",
        Shape::Torus { .. } => "torus",
    }
}

fn shape_found(s: &Session, found: &ScanShapes, index: usize) -> Option<ScanShapeFound> {
    let segment = found.outcome.segments.get(index)?;
    let shape = segment.shape?;
    let mut out = ScanShapeFound {
        index,
        kind: kind_name(&shape).into(),
        area: segment.area,
        triangles: segment.triangles.len(),
        rms: segment.rms,
        point: [0.0; 3],
        direction: None,
        radius: None,
        minor: None,
        half_angle: None,
        hole: None,
    };
    match shape {
        Shape::Plane { point, normal } => (out.point, out.direction) = (at(point), Some(at(normal))),
        Shape::Cylinder { point, axis, radius } => {
            (out.point, out.direction, out.radius) = (at(point), Some(at(axis)), Some(radius));
            let state = s.model.state();
            out.hole = state
                .body(&found.scan)
                .and_then(|b| b.body.triangle_mesh())
                .map(|(p, t)| reconstruct::is_round_hole(&Scan::new(p, t), segment, reconstruct::outward(&Scan::new(p, t))));
        }
        Shape::Sphere { center, radius } => (out.point, out.radius) = (at(center), Some(radius)),
        Shape::Cone { apex, axis, half_angle } => {
            (out.point, out.direction, out.half_angle) = (at(apex), Some(at(axis)), Some(half_angle.to_degrees()))
        }
        Shape::Torus { center, axis, major, minor } => {
            (out.point, out.direction, out.radius, out.minor) = (at(center), Some(at(axis)), Some(major), Some(minor))
        }
    }
    Some(out)
}

pub(super) fn find_all(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.find_all";
    if bool_(p, "show") == Some(false) {
        if let Some(known) = s.scan_shapes.take() {
            s.scan_shapes = Some(Arc::new(known.with(false, known.region.clone())));
        }
        return reply(ScanFindAllResult {
            shown: false,
            body: None,
            shapes: Vec::new(),
            freeform: 0,
            freeform_area: 0.0,
            in_shapes: 0.0,
            noise: 0.0,
            tolerance: 0.0,
            param_warnings: Vec::new(),
        });
    }
    let found = shapes(s, p, cmd, true)?;
    let count = found.reading.faces.len();
    let freeform: Vec<&segment::Segment> = found.outcome.segments.iter().filter(|g| g.shape.is_none()).collect();
    reply(ScanFindAllResult {
        shown: true,
        body: Some(found.scan.clone()),
        shapes: (0..count).filter_map(|k| shape_found(s, &found, k)).collect(),
        freeform: freeform.len(),
        freeform_area: freeform.iter().map(|g| g.area).sum(),
        in_shapes: found.outcome.in_shapes,
        noise: found.outcome.noise,
        tolerance: found.outcome.tolerance,
        param_warnings: Vec::new(),
    })
}

/// A step Reconstruct proposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proposal {
    /// The turned part, revolved.
    Revolve,
    /// A prism (its index), extruded.
    Extrude(usize),
    /// A round hole (its index), cut.
    Cut(usize),
    /// A cut hole's row or ring (the hole's index), patterned.
    Pattern(usize),
    /// The rounds along edges no profile holds, filleted.
    Fillet,
    /// The flat chamfers along edges no profile holds, chamfered.
    Chamfer,
}

impl Proposal {
    pub fn command(&self) -> &'static str {
        match self {
            Proposal::Revolve => "scan.revolve_it",
            Proposal::Extrude(_) => "scan.extrude_it",
            Proposal::Cut(_) => "scan.cut_it",
            Proposal::Pattern(_) => "scan.pattern_it",
            Proposal::Fillet => "scan.fillet_it",
            Proposal::Chamfer => "scan.chamfer_it",
        }
    }

    /// Its command's parameters, on scan `body`; `selected` when it was read off the part of the
    /// scan selected.
    pub fn params(&self, body: &str, selected: bool) -> Value {
        let mut p = match *self {
            Proposal::Revolve | Proposal::Fillet | Proposal::Chamfer => json!({"body": body}),
            Proposal::Extrude(k) => json!({"body": body, "prism": k}),
            Proposal::Cut(k) | Proposal::Pattern(k) => json!({"body": body, "hole": k}),
        };
        if let (true, Some(map)) = (selected, p.as_object_mut()) {
            map.insert("selected".into(), Value::Bool(true));
        }
        p
    }
}

/// What Reconstruct proposes, in the order to build it: the turned part (when its profile can be
/// read), else each prism, the main body first; then each hole cut and, where it repeats,
/// patterned, and last the rounds no profile holds filleted, once there is a body to cut and
/// fillet: one a step above builds, or one `built` before.
pub fn proposals(reading: &Reading, built: bool) -> Vec<Proposal> {
    let mut out = Vec::new();
    if reading.revolve.is_some() && reading.turned_unread.is_none() {
        out.push(Proposal::Revolve);
    }
    out.extend((0..reading.prisms.len()).map(Proposal::Extrude));
    if out.is_empty() && !built {
        return out;
    }
    for (k, hole) in reading.holes.iter().enumerate() {
        out.push(Proposal::Cut(k));
        if hole.repeat.is_some() {
            out.push(Proposal::Pattern(k));
        }
    }
    if !reading.fillets.is_empty() {
        out.push(Proposal::Fillet);
    }
    if !reading.chamfers.is_empty() {
        out.push(Proposal::Chamfer);
    }
    out
}

/// The body Reconstruct builds on scan `scan`.
pub(super) fn built_name(scan: &str) -> String {
    format!("{scan} Reconstructed")
}

/// Whether the body Reconstruct builds on scan `scan` is in the design, so its holes can be cut.
pub fn is_built(s: &Session, scan: &str) -> bool {
    s.model.state().body(&built_name(scan)).is_some()
}

fn layout_name(layout: &Layout) -> &'static str {
    match layout {
        Layout::Linear { .. } => "linear",
        Layout::Circular { .. } => "circular",
        Layout::Grid { .. } => "grid",
    }
}

/// Reconstruct Selection: Reconstruct on the part of the scan selected.
pub(super) fn read_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let mut p = p.clone();
    if let Some(map) = p.as_object_mut() {
        map.insert("selected".into(), Value::Bool(true));
    } else {
        p = json!({"selected": true});
    }
    // Named the scan selected on, unless told.
    if p.get("body").and_then(Value::as_str).is_none_or(|b| b.trim().is_empty())
        && let Some(on) = super::select::selected(s).filter(|k| k.count > 0)
        && let Some(map) = p.as_object_mut()
    {
        map.insert("body".into(), Value::String(on.scan.clone()));
    }
    read_scan(s, &p)
}

pub(super) fn read_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.reconstruct";
    let view = view(s, p, cmd, true)?;
    let found = &view.found;
    let reading = view.reading();
    let built = built_name(&found.scan);
    let built = s.model.state().body(&built).is_some().then_some(built);
    let prisms: Vec<ScanPrism> = reading
        .prisms
        .iter()
        .enumerate()
        .map(|(index, prism)| ScanPrism {
            index,
            direction: at(prism.direction),
            middle: at(prism.middle()),
            depth: prism.depth(),
            walls: prism.walls.clone(),
            ends: prism.ends,
            open: prism.open,
        })
        .collect();
    let holes: Vec<ScanHole> = reading
        .holes
        .iter()
        .enumerate()
        .map(|(index, h)| ScanHole {
            index,
            shape: h.face,
            radius: h.radius,
            diameter: 2.0 * h.radius,
            mouth: at(h.mouth),
            bottom: at(h.bottom),
            depth: h.depth(),
            repeat: h.repeat.map(|r| r.0),
            layout: h.repeat.map(|r| layout_name(&r.1).to_string()),
        })
        .collect();
    let repeats = reading
        .repeats
        .iter()
        .map(|r| {
            let mut out = ScanRepeat {
                kind: format!("{:?}", r.kind).to_lowercase(),
                members: r.members.clone(),
                layout: layout_name(&r.layout).into(),
                step: None,
                angle: None,
                axis: None,
                centre: None,
                across: None,
                columns: None,
                rows: None,
            };
            match r.layout {
                Layout::Linear { step } => out.step = Some(at(step)),
                Layout::Circular { centre, axis, angle } => {
                    (out.centre, out.axis, out.angle) = (Some(at(centre)), Some(at(axis)), Some(angle.to_degrees()))
                }
                Layout::Grid { step, across, columns, rows } => {
                    (out.step, out.across, out.columns, out.rows) = (Some(at(step)), Some(at(across)), Some(columns), Some(rows))
                }
            }
            out
        })
        .collect();
    let turned = reading.revolve.as_ref().map(|r| ScanTurned {
        point: at(r.axis.point),
        direction: at(r.axis.direction),
        faces: r.members.clone(),
        share: r.share,
    });
    let body = found.scan.clone();
    let proposals =
        proposals(reading, built.is_some()).iter().map(|p| json!({"command": p.command(), "params": p.params(&body, view.selected())})).collect();
    let note = if reading.faces.is_empty() {
        Some("Find All found no shape on the scan: it is all freeform".to_string())
    } else if let (true, Some(why)) = (reading.prisms.is_empty(), &reading.turned_unread) {
        Some(format!("the part turns about one axis, but Revolve It cannot read its profile: {why}"))
    } else if reading.prisms.is_empty() && reading.revolve.is_none() {
        let unread = match reading.unread_prisms {
            0 => String::new(),
            1 => " (one set of walls runs along one direction, but no slice halfway along it goes round them)".to_string(),
            n => format!(" ({n} sets of walls run along one direction, but no slice halfway along them goes round them)"),
        };
        Some(format!(
            "the shapes make no prism and no turned part{unread}: build from Fit Plane, Fit Cylinder and Section to Sketch, or with free forms"
        ))
    } else if view.outcome().in_shapes < 0.5 {
        let of = if view.selected() { "what is selected" } else { "the scan" };
        Some(format!("only {:.0}% of {of} is in shapes: the rest is freeform, for free forms", 100.0 * view.outcome().in_shapes))
    } else {
        None
    };
    reply(ScanReconstructResult {
        body,
        shapes: reading.faces.len(),
        in_shapes: reading.in_shapes,
        prisms,
        turned,
        holes,
        repeats,
        built,
        proposals,
        fillets: reading.fillets.iter().map(|f| ScanEdgeRound { face: f.face, radius: f.radius, edge: at(f.edge) }).collect(),
        chamfers: reading.chamfers.iter().map(|c| ScanEdgeChamfer { face: c.face, distance: c.distance, edge: at(c.edge) }).collect(),
        note,
        param_warnings: Vec::new(),
    })
}

/// What a profile sketch was drawn with.
#[derive(Default)]
struct Drawn {
    lines: usize,
    arcs: usize,
    tangents: usize,
}

/// A piece's direction at its start (`at_end` false) or its end, along the outline.
fn piece_direction(piece: &Piece, from: Vec2, to: Vec2, at_end: bool) -> Option<Vec2> {
    match *piece {
        Piece::Line => (to - from).normalized(),
        Piece::Arc { center, ccw } => {
            let radial = if at_end { to - center } else { from - center };
            let tangent = radial.perp().normalized()?;
            Some(if ccw { tangent } else { -tangent })
        }
    }
}

/// How a profile's line is held, by its face and its two ends: level (`Some(true)`, along the
/// sketch's X), upright (`Some(false)`) or neither.
type Level<'a> = dyn Fn(Option<usize>, Vec2, Vec2) -> Option<bool> + 'a;

/// Outline `outline` drawn into the sketch: its corners shared by the pieces either side, each
/// line a line and each arc an arc, smooth joins held tangent. `level` holds each line level (along
/// the sketch's X) or upright, by its face and its two ends: for a turned profile.
fn draw(sk: &mut Sketch, outline: &Outline, level: &Level<'_>, drawn: &mut Drawn) -> Result<()> {
    let n = outline.corners.len();
    if n < 2 {
        return Ok(());
    }
    // Corners that come together are one point, and the piece between them none.
    let mut ids: Vec<usize> = Vec::with_capacity(n);
    for k in 0..n {
        if k > 0 && outline.corners[k].dist(outline.corners[k - 1]) < 1e-9 {
            ids.push(ids[k - 1]);
        } else {
            ids.push(sk.add_point(outline.corners[k], None)?);
        }
    }
    if outline.corners[n - 1].dist(outline.corners[0]) < 1e-9 {
        ids[n - 1] = ids[0];
    }
    let mut curves: Vec<(usize, usize)> = Vec::new();
    for k in 0..n {
        let (a, b) = (ids[k], ids[(k + 1) % n]);
        if a == b {
            continue;
        }
        let (from, to) = (outline.corners[k], outline.corners[(k + 1) % n]);
        let curve = match outline.pieces.get(k).copied().unwrap_or(Piece::Line) {
            Piece::Arc { center, ccw } => {
                // Sketch arcs run counter-clockwise: a clockwise one is the same arc from its end.
                let made = if ccw {
                    sk.add_arc(center, from, to, [None, Some(a), Some(b)], None)
                } else {
                    sk.add_arc(center, to, from, [None, Some(b), Some(a)], None)
                };
                match made {
                    Ok(c) => {
                        drawn.arcs += 1;
                        c
                    }
                    Err(_) => {
                        drawn.lines += 1;
                        sk.add_line_pts(a, b, None)?
                    }
                }
            }
            Piece::Line => {
                drawn.lines += 1;
                let line = sk.add_line_pts(a, b, None)?;
                match level(outline.faces.get(k).copied().flatten(), from, to) {
                    Some(true) => {
                        sk.add_constraint(ConstraintKind::Horizontal { l: line }, None)?;
                    }
                    Some(false) => {
                        sk.add_constraint(ConstraintKind::Vertical { l: line }, None)?;
                    }
                    None => {}
                }
                line
            }
        };
        curves.push((curve, k));
    }
    // Smooth joins where an arc meets anything: held tangent.
    let count = curves.len();
    for i in 0..count {
        let ((here, k), (there, j)) = (curves[i], curves[(i + 1) % count]);
        if count < 2 || here == there {
            continue;
        }
        let pieces = |k: usize| outline.pieces.get(k).copied().unwrap_or(Piece::Line);
        let arcy = matches!(pieces(k), Piece::Arc { .. }) || matches!(pieces(j), Piece::Arc { .. });
        let u = piece_direction(&pieces(k), outline.corners[k], outline.corners[(k + 1) % n], true);
        let v = piece_direction(&pieces(j), outline.corners[j], outline.corners[(j + 1) % n], false);
        if let (true, Some(u), Some(v)) = (arcy, u, v)
            && u.dot(v) > 3f64.to_radians().cos()
            && sk.add_constraint(ConstraintKind::Tangent { a: here, b: there }, None).is_ok()
        {
            drawn.tangents += 1;
        }
    }
    Ok(())
}

/// A sketch on `plane` named `name`, with `outlines` drawn in; its id.
fn sketch_outlines(s: &mut Session, plane: &Plane, name: &str, outlines: &[Outline], level: &Level<'_>, drawn: &mut Drawn) -> Result<u64> {
    let create = json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": name});
    let made = run(s, "sketch.create", create)?;
    let id = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the sketch was not made".into()))?;
    if s.model.state().sketch(id).is_none() {
        s.refresh();
    }
    let result = super::super::sketch_store::update(s, id, |_, _, sk| {
        for outline in outlines {
            draw(sk, outline, level, drawn)?;
        }
        Ok(())
    });
    if let Err(e) = result {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    run(s, "sketch.finish", json!({}))?;
    Ok(id)
}

/// The profiles of sketch `id` that are material, when some are not: a profile inside a hole of
/// another is the hole itself (a disc where the part has a bore), one inside a hole of that is an
/// island again (a pin standing in the bore). Extrude takes every profile unless told, which would
/// fill each hole the profile has. `None` when every profile is material or the sketch is not
/// there.
fn material_profiles(s: &mut Session, id: u64) -> Option<Vec<usize>> {
    // A sketch just finished has its profiles once the model is brought up to date.
    if s.model.state().sketch(id).is_none_or(|k| k.profiles.is_empty()) {
        s.refresh();
    }
    let state = s.model.state();
    let profiles = &state.sketch(id)?.profiles;
    let key = |curves: &[String]| {
        let mut k: Vec<String> = solidcraft_sketch::distinct_curves(curves);
        k.sort();
        k
    };
    let outers: Vec<Vec<String>> = profiles.iter().map(|p| key(&p.outer_curves)).collect();
    // Each profile's parent: the profile one of whose holes is its outer loop.
    let parent: Vec<Option<usize>> =
        outers.iter().map(|outer| profiles.iter().position(|q| q.hole_curves.iter().any(|h| key(h) == *outer))).collect();
    let depth = |mut k: usize| {
        let mut d = 0usize;
        while let Some(up) = parent.get(k).copied().flatten() {
            d += 1;
            if d > profiles.len() {
                break;
            }
            k = up;
        }
        d
    };
    let material: Vec<usize> = (0..profiles.len()).filter(|&k| depth(k) % 2 == 0).collect();
    (material.len() < profiles.len() && !material.is_empty()).then_some(material)
}

/// How a feature builds into the body Reconstruct builds: joining `into` (default: that body,
/// when it is there), else making it new.
fn into_params(s: &Session, scan: &str, p: &Value, cmd: &str) -> Result<(Value, String, &'static str)> {
    let named = str_(p, "into").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let state = s.model.state();
    match named {
        Some(into) => {
            let body = state.body(&into).ok_or_else(|| bad(cmd, format!("no body `{into}` to join")))?;
            if body.body.is_mesh() {
                return Err(bad(cmd, format!("`{into}` is a mesh: join a solid body")));
            }
            Ok((json!({"operation": "join", "targets": [into]}), into, "join"))
        }
        None => {
            let built = built_name(scan);
            if state.body(&built).is_some() {
                Ok((json!({"operation": "join", "targets": [built]}), built, "join"))
            } else {
                Ok((json!({"operation": "new", "body_name": built}), built, "new"))
            }
        }
    }
}

fn merged(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

pub(super) fn extrude_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.extrude_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    if reading.prisms.is_empty() {
        return Err(bad(cmd, "Reconstruct reads no prism on the scan: run Find All and see what it found"));
    }
    let index = p.get("prism").and_then(Value::as_u64).map_or(0, |k| k as usize);
    let prism = reading
        .prisms
        .get(index)
        .ok_or_else(|| bad(cmd, format!("there are {} prisms: `prism` is 0 to {}", reading.prisms.len(), reading.prisms.len() - 1)))?;
    let (plane, outlines) = {
        let state = s.model.state();
        let (positions, triangles) =
            state.body(&found.scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        profile::prism_profile(&view.scan(positions, triangles), view.outcome(), reading, prism).map_err(|e| bad(cmd, e))?
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    let mut drawn = Drawn::default();
    let sketch = sketch_outlines(s, &plane, &format!("Prism Profile {}", index + 1), &outlines, &|_, _, _| None, &mut drawn)?;
    let depth = prism.depth();
    let mut extrude = json!({"sketch": sketch, "distance": depth, "direction": "symmetric", "name": format!("Extrude It {}", index + 1)});
    if let Some(material) = material_profiles(s, sketch) {
        extrude["profiles"] = json!(material);
    }
    let extrude = merged(extrude, operation);
    let feature = run(s, "solid.extrude", extrude)?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: Some(sketch),
        feature,
        solid,
        operation: how.into(),
        lines: drawn.lines,
        arcs: drawn.arcs,
        tangents: drawn.tangents,
        depth: Some(depth),
        count: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

pub(super) fn revolve_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.revolve_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let (plane, outline, held) = {
        let state = s.model.state();
        let (positions, triangles) =
            state.body(&found.scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        profile::turned_profile(&view.scan(positions, triangles), view.outcome(), view.reading()).map_err(|e| bad(cmd, e))?
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    // A cylinder's piece runs level along the axis, an end's upright, the axis's own along it. A
    // piece with no face off the axis bridges a gap the scan's faces leave, and is held neither way.
    let level = |face: Option<usize>, from: Vec2, to: Vec2| match face.and_then(|k| held.get(&k)) {
        None if face.is_none() => (from.y.abs() < 1e-9 && to.y.abs() < 1e-9).then_some(true),
        None => Some(true),
        Some(Turned::Cylinder { .. }) => Some(true),
        Some(Turned::Plane { .. }) => Some(false),
        Some(_) => None,
    };
    let mut drawn = Drawn::default();
    let sketch = sketch_outlines(s, &plane, "Turned Profile", std::slice::from_ref(&outline), &level, &mut drawn)?;
    let revolve = merged(json!({"sketch": sketch, "axis": "x", "name": "Revolve It"}), operation);
    let feature = run(s, "solid.revolve", revolve)?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: Some(sketch),
        feature,
        solid,
        operation: how.into(),
        lines: drawn.lines,
        arcs: drawn.arcs,
        tangents: drawn.tangents,
        depth: None,
        count: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// The body to cut a hole into or pattern in: `into`, else the body Reconstruct builds.
fn built_body(s: &Session, scan: &str, p: &Value, cmd: &str) -> Result<String> {
    let name = str_(p, "into").filter(|b| !b.trim().is_empty()).map_or_else(|| built_name(scan), str::to_owned);
    match s.model.state().body(&name) {
        Some(b) if !b.body.is_mesh() => Ok(name),
        Some(_) => Err(bad(cmd, format!("`{name}` is a mesh: cut a solid body"))),
        None => Err(bad(cmd, format!("there is no `{name}` yet: build it first with Extrude It or Revolve It"))),
    }
}

/// The name Cut It gives the cut of hole `index`.
fn cut_name(index: usize) -> String {
    format!("Cut It {}", index + 1)
}

/// The name for a new cut of hole `index`: [`cut_name`], or where a cut of another reading (a part
/// of the scan selected and built before) has it, `Cut It 1.2`, `Cut It 1.3` and so on, so that
/// Pattern It repeats this cut and not that one.
fn free_cut_name(s: &Session, index: usize) -> String {
    let base = cut_name(index);
    let taken = |n: &str| s.doc.features.iter().any(|f| f.name == n);
    if !taken(&base) {
        return base;
    }
    (2..).map(|k| format!("{base}.{k}")).find(|n| !taken(n)).unwrap_or(base)
}

/// The latest cut of hole `index` Cut It made ([`free_cut_name`]): the one Pattern It repeats.
fn last_cut_name(s: &Session, index: usize) -> String {
    let base = cut_name(index);
    let prefix = format!("{base}.");
    s.doc
        .features
        .iter()
        .rev()
        .find(|f| f.name == base || f.name.strip_prefix(&prefix).is_some_and(|k| k.parse::<u32>().is_ok()))
        .map_or(base, |f| f.name.clone())
}

pub(super) fn cut_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.cut_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    if reading.holes.is_empty() {
        return Err(bad(cmd, "Reconstruct reads no round hole to cut"));
    }
    let index = p.get("hole").and_then(Value::as_u64).map_or(0, |k| k as usize);
    let hole = reading
        .holes
        .get(index)
        .ok_or_else(|| bad(cmd, format!("there are {} holes: `hole` is 0 to {}", reading.holes.len(), reading.holes.len() - 1)))?;
    let solid = built_body(s, &found.scan, p, cmd)?;
    let inward = (hole.bottom - hole.mouth).normalized().ok_or_else(|| bad(cmd, "the hole has no depth on the scan"))?;
    // Started a little outside the mouth, so the cut begins clear of the material.
    let margin = (0.01 * reading.size).max(0.5);
    let (x, y) = {
        let x = inward.any_perp().normalized().unwrap_or(Vec3::X);
        (x, inward.cross(x))
    };
    let plane = Plane { origin: hole.mouth - inward * margin, x, y };
    let circle = Outline {
        corners: vec![Vec2::new(hole.radius, 0.0), Vec2::new(-hole.radius, 0.0)],
        pieces: vec![Piece::Arc { center: Vec2::ZERO, ccw: true }; 2],
        faces: vec![None; 2],
    };
    let mut drawn = Drawn::default();
    let sketch = sketch_outlines(s, &plane, &format!("Hole {}", index + 1), &[circle], &|_, _, _| None, &mut drawn)?;
    // The sketch's normal (x × y) points into the hole.
    let depth = hole.depth() + margin;
    let feature = run(
        s,
        "solid.extrude",
        json!({"sketch": sketch, "distance": depth, "operation": "cut", "targets": [solid], "name": free_cut_name(s, index)}),
    )?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: Some(sketch),
        feature,
        solid,
        operation: "cut".into(),
        lines: 0,
        arcs: drawn.arcs,
        tangents: 0,
        depth: Some(depth),
        count: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

pub(super) fn pattern_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.pattern_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    let index = match p.get("hole").and_then(Value::as_u64) {
        Some(k) => k as usize,
        None => {
            reading.holes.iter().position(|h| h.repeat.is_some()).ok_or_else(|| bad(cmd, "Reconstruct reads no row or ring of holes to pattern"))?
        }
    };
    let hole = reading
        .holes
        .get(index)
        .ok_or_else(|| bad(cmd, format!("there are {} holes: `hole` is 0 to {}", reading.holes.len(), reading.holes.len().saturating_sub(1))))?;
    let (count, layout) = hole.repeat.ok_or_else(|| bad(cmd, format!("hole {index} does not repeat: cut it with Cut It alone")))?;
    let feature_name = str_(p, "feature").filter(|f| !f.trim().is_empty()).map_or_else(|| last_cut_name(s, index), str::to_owned);
    if !s.doc.features.iter().any(|f| f.name == feature_name) {
        return Err(bad(cmd, format!("there is no `{feature_name}`: cut the hole first with Cut It")));
    }
    let solid = built_body(s, &found.scan, &json!({}), cmd)?;
    let pattern = match layout {
        Layout::Circular { centre, axis, .. } => {
            run(s, "solid.pattern.circular", json!({"features": [feature_name], "axis": {"origin": at(centre), "dir": at(axis)}, "count": count}))?
        }
        Layout::Linear { step } => {
            let dir = step.normalized().ok_or_else(|| bad(cmd, "the row of holes has no step"))?;
            run(s, "solid.pattern.rectangular", json!({"features": [feature_name], "dir1": at(dir), "count1": count, "spacing1": step.len()}))?
        }
        Layout::Grid { step, across, columns, rows } => {
            let (one, two) = (
                step.normalized().ok_or_else(|| bad(cmd, "the grid has no step"))?,
                across.normalized().ok_or_else(|| bad(cmd, "the grid has no step"))?,
            );
            run(
                s,
                "solid.pattern.rectangular",
                json!({"features": [feature_name], "dir1": at(one), "count1": columns, "spacing1": step.len(), "dir2": at(two), "count2": rows, "spacing2": across.len()}),
            )?
        }
    };
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: None,
        feature: pattern,
        solid,
        operation: "pattern".into(),
        lines: 0,
        arcs: 0,
        tangents: 0,
        depth: None,
        count: Some(count),
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// Fillet It: the rounds along edges Reconstruct reads that no profile holds, each measured off the
/// scan at the edge of the built body it rounds (Fillet from Scan's measure, one edge at a time,
/// so one the scan shows sharp is set aside rather than refusing the rest). Edges running on
/// tangentially into each other (a prism's end, its outline of lines and arcs) are one chain, and
/// a round cannot stop part way along one, so each chain is filleted whole, at the radius measured
/// most often along it: one Fillet for each radius, all one undo step.
pub(super) fn fillet_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fillet_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let rounds = view.reading().fillets.clone();
    if rounds.is_empty() {
        return Err(bad(cmd, "Reconstruct reads no round along an edge that its other steps leave sharp"));
    }
    let solid = built_body(s, &found.scan, p, cmd)?;
    // The body's edges as Fillet from Scan reads them, for their tangent chains.
    let surface = {
        let state = s.model.state();
        let body = state.body(&solid).ok_or_else(|| bad(cmd, format!("no body `{solid}`")))?;
        body.body
            .mesh(&solidcraft_kernel::MeshTolerance::chord(super::fillet::CHORD))
            .map_err(|e| bad(cmd, format!("`{solid}` could not be meshed: {e}")))?
    };
    // Each chain (its edges), a point on it, and the radii measured along it.
    let mut chains: Vec<(Vec<usize>, [f64; 3], Vec<f64>)> = Vec::new();
    let mut set_aside = 0;
    for round in &rounds {
        let edge = at(round.edge);
        let Ok((_, _, edges, measured)) = super::fillet::measured(s, &json!({"body": solid, "scan": found.scan, "edges": [edge]}), cmd) else {
            set_aside += 1;
            continue;
        };
        let Some(&e) = edges.first() else { continue };
        match chains.iter_mut().find(|c| c.0.contains(&e)) {
            Some(chain) => chain.2.push(measured.radius),
            None => chains.push((surface.tangent_chain(e, 2f64.to_radians()), edge, vec![measured.radius])),
        }
    }
    if chains.is_empty() {
        return Err(bad(
            cmd,
            format!("none of the {} rounds Reconstruct reads fits the scan better than a sharp edge of `{solid}` there", rounds.len()),
        ));
    }
    // The radius measured most often along each chain (the smaller of a tie), and the chains by it.
    let key = |r: f64| (r * 1e6).round() as i64;
    let mut by_radius: std::collections::BTreeMap<i64, (f64, Vec<[f64; 3]>)> = std::collections::BTreeMap::new();
    for (_, edge, radii) in &chains {
        let count = |r: f64| radii.iter().filter(|&&q| key(q) == key(r)).count();
        let Some(radius) = radii.iter().copied().max_by(|&a, &b| count(a).cmp(&count(b)).then(b.total_cmp(&a))) else { continue };
        by_radius.entry(key(radius)).or_insert((radius, Vec::new())).1.push(*edge);
    }
    let run_fillet = super::super::find_command("solid.fillet").ok_or_else(|| EngineError::UnknownCommand("solid.fillet".into()))?;
    // A Fillet the kernel refuses is taken back, so the next is tried on the body as it was.
    let fillet = |s: &mut Session, radius: f64, edges: &[[f64; 3]]| -> Result<Value> {
        let before = s.doc.clone();
        let made = run_fillet
            .run_nested(s, &json!({"edges": edges, "radius": format!("{radius} mm"), "body": solid, "tangent_chain": true, "fit": "reduce"}));
        if made.is_err() {
            s.doc = before;
            s.refresh();
        }
        made
    };
    let (mut fillets, mut features, mut refused, mut first_refusal) = (Vec::new(), Vec::new(), 0, None);
    for (radius, edges) in by_radius.into_values() {
        // All the chains of one radius as one Fillet; if the kernel refuses that, each alone.
        let tries: Vec<Vec<[f64; 3]>> = match fillet(s, radius, &edges) {
            Ok(made) => {
                fillets.push(ScanFilleted { radius, edges: edges.len() });
                features.push(made);
                continue;
            }
            Err(_) if edges.len() > 1 => edges.iter().map(|e| vec![*e]).collect(),
            Err(e) => {
                refused += 1;
                first_refusal.get_or_insert(e.to_string());
                continue;
            }
        };
        for one in tries {
            match fillet(s, radius, &one) {
                Ok(made) => {
                    fillets.push(ScanFilleted { radius, edges: 1 });
                    features.push(made);
                }
                Err(e) => {
                    refused += 1;
                    first_refusal.get_or_insert(e.to_string());
                }
            }
        }
    }
    if fillets.is_empty() {
        return Err(bad(cmd, format!("the kernel could fillet none of the edges the scan shows round: {}", first_refusal.unwrap_or_default())));
    }
    reply(ScanFilletItResult {
        body: found.scan.clone(),
        solid,
        fillets,
        set_aside: set_aside + refused,
        features,
        fit: None,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_reconstruct_tests.rs"]
mod tests;

/// Chamfer It: the flat chamfers Reconstruct reads along edges no profile holds, cut on the built
/// body at the distance each one's two faces give, one Chamfer for each distance. A chamfer the
/// kernel refuses is taken back and the rest tried; it is set aside.
pub(super) fn chamfer_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.chamfer_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let chamfers = view.reading().chamfers.clone();
    if chamfers.is_empty() {
        return Err(bad(cmd, "Reconstruct reads no chamfer along an edge that its other steps leave sharp"));
    }
    let solid = built_body(s, &found.scan, p, cmd)?;
    // The distance read most often is not needed: each chamfer's own, grouped to the micron so a
    // row of the same chamfer is one Chamfer.
    let key = |d: f64| (d * 1e3).round() as i64;
    let mut by_distance: std::collections::BTreeMap<i64, (f64, Vec<[f64; 3]>)> = std::collections::BTreeMap::new();
    for c in &chamfers {
        by_distance.entry(key(c.distance)).or_insert((c.distance, Vec::new())).1.push(at(c.edge));
    }
    let run_chamfer = super::super::find_command("solid.chamfer").ok_or_else(|| EngineError::UnknownCommand("solid.chamfer".into()))?;
    let chamfer = |s: &mut Session, distance: f64, edges: &[[f64; 3]]| -> Result<Value> {
        let before = s.doc.clone();
        let made = run_chamfer.run_nested(s, &json!({"edges": edges, "distance": format!("{distance} mm"), "body": solid, "tangent_chain": false}));
        if made.is_err() {
            s.doc = before;
            s.refresh();
        }
        made
    };
    let (mut done, mut features, mut refused, mut first_refusal) = (Vec::new(), Vec::new(), 0, None);
    for (distance, edges) in by_distance.into_values() {
        // All the edges of one distance as one Chamfer; if the kernel refuses that, each alone.
        let tries: Vec<Vec<[f64; 3]>> = match chamfer(s, distance, &edges) {
            Ok(made) => {
                done.push(ScanChamfered { distance, edges: edges.len() });
                features.push(made);
                continue;
            }
            Err(_) if edges.len() > 1 => edges.iter().map(|e| vec![*e]).collect(),
            Err(e) => {
                refused += 1;
                first_refusal.get_or_insert(e.to_string());
                continue;
            }
        };
        for one in tries {
            match chamfer(s, distance, &one) {
                Ok(made) => {
                    done.push(ScanChamfered { distance, edges: 1 });
                    features.push(made);
                }
                Err(e) => {
                    refused += 1;
                    first_refusal.get_or_insert(e.to_string());
                }
            }
        }
    }
    if done.is_empty() {
        return Err(super::refusal::refused(
            cmd,
            "the kernel could chamfer none of the edges the scan shows cut",
            &first_refusal.unwrap_or_default(),
        ));
    }
    reply(ScanChamferItResult {
        body: found.scan.clone(),
        solid,
        chamfers: done,
        set_aside: refused,
        features,
        fit: None,
        param_warnings: Vec::new(),
    })
}

macro_rules! fitted {
    ($($name:ident => $step:ident),* $(,)?) => {
        $(fn $name(s: &mut Session, p: &Value) -> Result<Value> {
            super::step_fit::with_fit(s, p, concat!("scan.", stringify!($step)), $step)
        })*
    };
}

fitted!(
    fitted_extrude_it => extrude_it,
    fitted_revolve_it => revolve_it,
    fitted_cut_it => cut_it,
    fitted_pattern_it => pattern_it,
    fitted_fillet_it => fillet_it,
    fitted_chamfer_it => chamfer_it,
);
