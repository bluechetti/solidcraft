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
    ScanFilletItParams, ScanFilletItResult, ScanFilleted, ScanFindAllParams, ScanFindAllResult, ScanHole, ScanHoleForm, ScanHoleItParams,
    ScanNestParams, ScanPatternItParams, ScanPrism, ScanReconstructParams, ScanReconstructResult, ScanReconstructSelectionParams, ScanRepeat,
    ScanRestParams, ScanRestResult, ScanRevolveItParams, ScanRotational, ScanShapeFound, ScanShapeItParams, ScanShell, ScanShellItParams, ScanSpokes,
    ScanTurned,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const FIND_ALL_CONTRACT: TypedContract<ScanFindAllParams> = TypedContract::new();
const RECONSTRUCT_CONTRACT: TypedContract<ScanReconstructParams> = TypedContract::new();
const RECONSTRUCT_SELECTION_CONTRACT: TypedContract<ScanReconstructSelectionParams> = TypedContract::new();
const EXTRUDE_IT_CONTRACT: TypedContract<ScanExtrudeItParams> = TypedContract::new();
const REVOLVE_IT_CONTRACT: TypedContract<ScanRevolveItParams> = TypedContract::new();
const CUT_IT_CONTRACT: TypedContract<ScanCutItParams> = TypedContract::new();
const HOLE_IT_CONTRACT: TypedContract<ScanHoleItParams> = TypedContract::new();
const SHAPE_IT_CONTRACT: TypedContract<ScanShapeItParams> = TypedContract::new();
const REST_CONTRACT: TypedContract<ScanRestParams> = TypedContract::new();
const PATTERN_IT_CONTRACT: TypedContract<ScanPatternItParams> = TypedContract::new();
const FILLET_IT_CONTRACT: TypedContract<ScanFilletItParams> = TypedContract::new();
const CHAMFER_IT_CONTRACT: TypedContract<ScanChamferItParams> = TypedContract::new();
/// How far outside a prism's outline scan is still that prism's wall astray, as a share of the
/// part's size: further out it is another part of the thing, and Nest does not allow for it.
const NEST_ASTRAY: f64 = 0.01;
const NEST_CONTRACT: TypedContract<ScanNestParams> = TypedContract::new();
const SHELL_IT_CONTRACT: TypedContract<ScanShellItParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.find_all", "Find All", crate::checked!("scan.find_all", find_all))
        .at("SCAN", "RECONSTRUCT")
        .icon("colored_components")
        .noundo()
        .contract(&FIND_ALL_CONTRACT)
        .output(output_schema_for::<ScanFindAllResult>)
        .params("body?: mesh body (default: the selected or only one); tolerance?: mm (the least distance a shape may lie from the scan and still take it; default: the scan's tolerance when one is set, else three times its noise, and on a coarse scan (triangles over a hundredth of its size long: a fused depth-camera scan) at least 0.4 of a triangle's edge; raise it for a scan smooth but off by more than its noise, as a fused depth-camera scan is); show?: bool (false takes the colouring away): the whole scan cut into regions, each a plane, cylinder, sphere, cone or torus or freeform (the scan's noise sets how close a shape must lie), coloured in the viewport; a view and not an undo step → shapes [{index, kind, area, triangles, rms, point, on_scan, direction?, radius?, minor?, half_angle?, hole?}], freeform, freeform_area, in_shapes, noise, tolerance"),
    CommandSpec::new("scan.reconstruct", "Reconstruct", crate::checked!("scan.reconstruct", read_scan))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .noundo()
        .contract(&RECONSTRUCT_CONTRACT)
        .output(output_schema_for::<ScanReconstructResult>)
        .params("body?: mesh body (default: the selected or only one); tolerance?: mm (Find All read again with this least tolerance, as scan.find_all takes it); selected?: bool (default false: the whole scan; true: only the part of it selected, read as a part of its own, and each step proposed takes `selected`): what Find All's shapes make (it runs Find All first when it has not run on this scan): prisms (stretches of one section along a direction a plane faces), a turned part (most of the shapes about one axis), repeats (in a row, round an axis, in a grid, drilled along the radius) and the round holes no profile holds; reads only → shapes, in_shapes, prisms [{index, direction, middle, depth, walls, ends, open, draft (degrees the walls lean in, a moulded part's)}], turned? {point, direction, faces, share}, shell? {thickness, inner, open} (walls of one thickness, open at an end: Shell It hollows the body), holes [{index, shape, radius, diameter, mouth, bottom, depth, repeat?, layout?}], repeats, built?, proposals [{command, params}], note?"),
    CommandSpec::new("scan.reconstruct_selection", "Reconstruct Selection", crate::checked!("scan.reconstruct_selection", read_selection))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .noundo()
        .contract(&RECONSTRUCT_SELECTION_CONTRACT)
        .output(output_schema_for::<ScanReconstructResult>)
        .params("body?: mesh body (default: the one selected on): scan.reconstruct with selected: true, what the shapes make on the part of the scan selected, read as a part of its own (a boss, a turned feature, a hole of a part too busy to read whole), and the steps that build it, each with `selected`; refused with nothing selected; reads only → as scan.reconstruct"),
    CommandSpec::new("scan.reconstruct_rest", "Reconstruct the Rest", crate::checked!("scan.reconstruct_rest", rest::reconstruct_rest))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .contract(&REST_CONTRACT)
        .output(output_schema_for::<ScanRestResult>)
        .params("body?: mesh body (default: the selected or only one); rounds?: 1 to 20 (default 6): after the steps Reconstruct proposes have built `<scan> Reconstructed`, what it leaves of the scan unexplained, piece by piece: each round selects the largest piece the body does not explain yet (scan.select unexplained), reads it as a part of its own (scan.reconstruct_selection) and runs the steps proposed for it, taking back each that is refused or leaves the body no closer to the scan (its fit verdict); a piece that adds nothing is passed over for the next, three in a row end it; the selection is cleared after; one undo step → body, solid, rounds [{piece, area, proposed, kept}], kept, stop"),
    CommandSpec::new("scan.extrude_it", "Extrude It", crate::checked!("scan.extrude_it", fitted_extrude_it))
        .icon("extrude")
        .contract(&EXTRUDE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); prism?: index of scan.reconstruct's prisms (default 0); into?: body to join (default: `<scan> Reconstructed`, made new when there is none); pattern?: bool (default true: a new prism whose outline repeats round its middle with lands on its tip circle, a gear, is the circle extruded, one gap cut through and the cut patterned, `count` in the reply; false: the whole outline in one sketch): the prism's profile sketched halfway along it off its walls exactly as fitted (lines and arcs from crossing to crossing, joins held tangent) and extruded symmetric to its depth; a drafted prism (its `draft` not 0) sketched at its low end and extruded tapered by its draft; one undo step → sketch, feature, solid, operation, lines, arcs, tangents, depth, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.revolve_it", "Revolve It", crate::checked!("scan.revolve_it", fitted_revolve_it))
        .icon("revolve")
        .contract(&REVOLVE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); wall?: mm (one side of a skin, a profile open at both ends, `skin` in the reply: revolved as a surface and thickened this much away from the scanned side, not closed into a solid); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the turned part's profile sketched on a plane through its axis, each face held to the axis (cylinders level, ends upright, rounds tangent), and revolved all the way round, one undo step → sketch, feature, solid, operation, lines, arcs, tangents, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.cut_it", "Cut It", crate::checked!("scan.cut_it", fitted_cut_it))
        .icon("hole")
        .contract(&CUT_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); hole?: index of scan.reconstruct's holes (default 0); into?: body to cut (default: `<scan> Reconstructed`): a circle sketched square to the hole outside its mouth and cut as deep as the scan has the hole, one undo step → sketch, feature, solid, operation, depth, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.hole_it", "Hole It", crate::checked!("scan.hole_it", fitted_hole_it))
        .icon("hole")
        .contract(&HOLE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); hole?: index of scan.reconstruct's holes (default: the first with a `form`); thread?: bool (default true: a bore the size of a tap drill drilled as tapped, its thread listed; false: plain); into?: body to drill (default: `<scan> Reconstructed`): a hole read as a Hole feature makes it (its `form`: counterbore, countersink, drilled blind with its drill point or a flat bottom, tapped) drilled with Hole at its mouth along its axis, through or as deep as the scan has it, the sizes read as designed; one undo step → feature, solid, operation, depth, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.loft_it", "Loft It", crate::checked!("scan.loft_it", fitted_loft_it))
        .icon("loft")
        .contract(&SHAPE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the part scan.reconstruct reads as sections lofted (its `loft`: no prism, turned part, pipe or coil read, every slice square to the direction it runs one closed loop; the direction two flat ends face apart along, else its longest): each section sketched as a closed fit-point spline through the slice's points, laid out from matching starts so the loft does not twist, and lofted end to end; one undo step → feature, solid, operation, depth (the length), fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.pipe_it", "Pipe It", crate::checked!("scan.pipe_it", fitted_pipe_it))
        .icon("pipe")
        .contract(&SHAPE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the bar bent along a flat path scan.reconstruct reads (its `pipe`: cylinders for the straight runs, tori of the same tube for the bends, each running on from the one before): the path through their axes sketched as lines and arcs, joined where a run touches a bend, and the bar's circle swept along it with Pipe; one undo step → sketch, feature, solid, operation, lines, arcs, depth (the path's length), fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.tubes_it", "Tubes It", crate::checked!("scan.tubes_it", fitted_tubes_it))
        .icon("pipe")
        .contract(&SHAPE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the round tubes bent through space scan.reconstruct reads (its `tubes`: tori of one tube radius for the bends, cylinders for the straight stretches, each running on from the one before, in any direction; a manifold's runners): each tube's path a 3D spline through points along its axis (its own sketch, `Tube N Path`), the tube's circle swept along it with Pipe and joined into one body; one undo step → feature (the last), solid, operation, count (tubes swept), depth (their length together), note? (the tubes the kernel could not sweep), fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?}"),
    CommandSpec::new("scan.coil_it", "Coil It", crate::checked!("scan.coil_it", fitted_coil_it))
        .icon("coil")
        .contract(&SHAPE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the coil spring scan.reconstruct reads (its `coil`: tori alike round one axis, each leaning by the helix's angle; the pitch that lines the wire up on one helix, the turns the scan's height holds, the hand): the wire's circle sketched where it starts, on the plane through the axis, and wound with Coil; one undo step → sketch, feature, solid, operation, depth (the height wound), fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.pattern_it", "Pattern It", crate::checked!("scan.pattern_it", fitted_pattern_it))
        .icon("pattern_circ")
        .contract(&PATTERN_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); hole?: index of scan.reconstruct's holes, one that repeats (default: the first that does); feature?: the cut to repeat (default: the one Cut It made for that hole): the cut repeated as the row or ring of holes is, with its count and spacing, one undo step → feature, solid, count, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.fillet_it", "Fillet It", crate::checked!("scan.fillet_it", fitted_fillet_it))
        .icon("fillet")
        .contract(&FILLET_IT_CONTRACT)
        .output(output_schema_for::<ScanFilletItResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to fillet (default: `<scan> Reconstructed`): each round along an edge that scan.reconstruct reads and no profile holds (its `fillets`), measured off the scan as Fillet from Scan does at the edge of the body it rounds; the edges where a round fits the scan filleted at the radius measured, one Fillet for each radius, one undo step; an edge where the scan shows a sharp edge, or no edge of the body, is set aside → body, solid, fillets [{radius, edges}], set_aside, features, fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.shell_it", "Shell It", crate::checked!("scan.shell_it", fitted_shell_it))
        .icon("shell")
        .contract(&SHELL_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool (default false; true: the reading of the part of the scan selected, as scan.reconstruct with selected gave it); into?: body to hollow (default: `<scan> Reconstructed`): the body hollowed to the thickness of the walls scan.reconstruct reads (its `shell`: planes in pairs facing apart, one behind the other, the same distance apart), the faces with no inner face behind them left open; run it after Extrude It and before Fillet It; one undo step → feature, solid, operation, depth (the thickness), fit {solid, before?, after: {within, explained, rms, points}, verdict: first|better|same|worse, note?} (the body measured against a sample of the scan before and after the step)"),
    CommandSpec::new("scan.nest", "Nest", crate::checked!("scan.nest", nest))
        .icon("extrude")
        .contract(&NEST_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; selected?: bool; prism?: index of scan.reconstruct's prisms (default 0; a prism with square walls: a drafted one is refused); clearance?: mm or a length expression such as a parameter's name (default 0.3 mm: the gap round the part and under it; the pocket follows it up to twice what it is built with, tight inside corners being cut square for that); allowance?: mm added to the clearance (default: measured, how far the scan stands proud of the fitted outline and of the end it sits on, so the least gap is the clearance; 0 for the clearance alone); margin?: mm of block round the part and under it (default 10); deep?: share of the part's depth the pocket holds (default 0.5); turned?: bool (default: true when Reconstruct reads a turned part and no prism): the prism's outline sketched (`Nest Pocket`), offset outward by the clearance as a sketch offset (so the pocket follows it), and cut into a block (`<scan> Nest`) drawn round it; a turned part (turned: true) lies on its side in a cradle instead: its profile revolved (`<scan> Nest Part`), every face of it offset outward by the clearance, and cut from a block whose top holds that share of the part's diameter (allowance 0); one undo step → sketch, feature (the cut), solid, depth (of the pocket, less the clearance), allowance, lines, arcs"),
    CommandSpec::new("scan.chamfer_it", "Chamfer It", crate::checked!("scan.chamfer_it", fitted_chamfer_it))
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
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
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
        super::shown_mesh(s, &self.scan).is_some_and(|m| {
            let (p, t) = (m.positions.as_slice(), m.triangles.as_slice());
            held(p, t) == self.held || fingerprint(p, t) == self.fingerprint
        })
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
        if !body.body.is_mesh() {
            return Err(bad(cmd, format!("`{name}` is not a scan (a mesh body)")));
        }
        // Where the scan shows: what is read off it and built from it is where it is drawn.
        let shown = super::shown_mesh(s, &name).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        if triangles.is_empty() {
            return Err(bad(cmd, format!("`{name}` has no triangles")));
        }
        let scan = Scan::new(positions, triangles);
        // Cancel (control.cancel, the app's Cancel) stops it at its next region.
        let stop = || solidcraft_kernel::watch::current().is_some_and(|w| w.cancelled());
        let outcome = segment::find_all(&scan, &segment::Options { least_tolerance: least, ..segment::Options::default() }, &stop);
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
        on_scan: [0.0; 3],
        direction: None,
        radius: None,
        minor: None,
        half_angle: None,
        hole: None,
    };
    // A point on the scan in the region's middle: the middle of the region's triangle nearest the
    // mean of a sample of its triangles' middles.
    if let Some(mesh) = super::shown_mesh(s, &found.scan) {
        let scan = Scan::new(&mesh.positions, &mesh.triangles);
        let middle_of = |t: usize| scan.corners(t).map(|[a, b, c]| (a + b + c) / 3.0);
        let step = (segment.triangles.len() / 500).max(1);
        let sample: Vec<Vec3> = segment.triangles.iter().step_by(step).filter_map(|&t| middle_of(t)).collect();
        if !sample.is_empty() {
            let mean = sample.iter().fold(Vec3::ZERO, |a, &p| a + p) / sample.len() as f64;
            if let Some(nearest) = sample.iter().min_by(|a, b| a.dist(mean).total_cmp(&b.dist(mean))) {
                out.on_scan = at(*nearest);
            }
        }
    }
    match shape {
        Shape::Plane { point, normal } => (out.point, out.direction) = (at(point), Some(at(normal))),
        Shape::Cylinder { point, axis, radius } => {
            (out.point, out.direction, out.radius) = (at(point), Some(at(axis)), Some(radius));
            out.hole = super::shown_mesh(s, &found.scan).map(|m| {
                let scan = Scan::new(&m.positions, &m.triangles);
                reconstruct::is_round_hole(&scan, segment, reconstruct::outward(&scan))
            });
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
    /// A hole as a Hole feature makes it (its index), drilled.
    Hole(usize),
    /// A cut hole's row or ring (the hole's index), patterned.
    Pattern(usize),
    /// A prism's row or ring (the first prism's index), its extrusion patterned.
    PatternPrism(usize),
    /// The rounds along edges no profile holds, filleted.
    Fillet,
    /// A coil spring, wound.
    Coil,
    /// A bar bent along a flat path, swept.
    Pipe,
    /// Round tubes bent through space, each swept.
    Tubes,
    /// The part as sections, lofted.
    Loft,
    /// The flat chamfers along edges no profile holds, chamfered.
    Chamfer,
    /// Walls of one thickness: the body shelled.
    Shell,
}

impl Proposal {
    pub fn command(&self) -> &'static str {
        match self {
            Proposal::Revolve => "scan.revolve_it",
            Proposal::Extrude(_) => "scan.extrude_it",
            Proposal::Cut(_) => "scan.cut_it",
            Proposal::Hole(_) => "scan.hole_it",
            Proposal::Pattern(_) | Proposal::PatternPrism(_) => "scan.pattern_it",
            Proposal::Fillet => "scan.fillet_it",
            Proposal::Coil => "scan.coil_it",
            Proposal::Pipe => "scan.pipe_it",
            Proposal::Tubes => "scan.tubes_it",
            Proposal::Loft => "scan.loft_it",
            Proposal::Chamfer => "scan.chamfer_it",
            Proposal::Shell => "scan.shell_it",
        }
    }

    /// Its command's parameters, on scan `body`; `selected` when it was read off the part of the
    /// scan selected.
    pub fn params(&self, body: &str, selected: bool) -> Value {
        let mut p = match *self {
            Proposal::Revolve
            | Proposal::Fillet
            | Proposal::Chamfer
            | Proposal::Shell
            | Proposal::Coil
            | Proposal::Pipe
            | Proposal::Tubes
            | Proposal::Loft => {
                json!({"body": body})
            }
            Proposal::Extrude(k) | Proposal::PatternPrism(k) => json!({"body": body, "prism": k}),
            Proposal::Cut(k) | Proposal::Hole(k) | Proposal::Pattern(k) => json!({"body": body, "hole": k}),
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
    if reading.coil.is_some() {
        out.push(Proposal::Coil);
    }
    if reading.pipe.is_some() {
        out.push(Proposal::Pipe);
    }
    if !reading.tubes.is_empty() {
        out.push(Proposal::Tubes);
    }
    if reading.loft.is_some() {
        out.push(Proposal::Loft);
    }
    if reading.revolve.is_some() && reading.turned_unread.is_none() {
        out.push(Proposal::Revolve);
    }
    // A prism of a row or ring alike is extruded once and patterned.
    let made_by_pattern: std::collections::BTreeSet<usize> = reading.prism_repeats.iter().flat_map(|r| r.members[1..].iter().copied()).collect();
    for k in (0..reading.prisms.len()).filter(|k| !made_by_pattern.contains(k)) {
        out.push(Proposal::Extrude(k));
        if reading.prism_repeats.iter().any(|r| r.members.first() == Some(&k)) {
            out.push(Proposal::PatternPrism(k));
        }
    }
    if out.is_empty() && !built {
        return out;
    }
    // The shell before the cuts and rounds: a round on the rim made first stops the kernel
    // shelling a drafted body, and a hole cut first would be walled round by the shell.
    if reading.shell.is_some() {
        out.push(Proposal::Shell);
    }
    for (k, hole) in reading.holes.iter().enumerate() {
        out.push(if hole.form.is_some() { Proposal::Hole(k) } else { Proposal::Cut(k) });
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

/// How many bends the bar Reconstruct reads bent along a path has (none without one).
pub fn pipe_bends(reading: &Reading) -> usize {
    reading.pipe.as_ref().map_or(0, |p| p.pieces.iter().filter(|k| matches!(k, solidcraft_scan::pipe::Piece::Arc { .. })).count())
}

/// The body Reconstruct builds on scan `scan`.
pub(super) fn built_name(scan: &str) -> String {
    format!("{scan} Reconstructed")
}

/// Whether the body Reconstruct builds on scan `scan` is in the design, so its holes can be cut.
pub fn is_built(s: &Session, scan: &str) -> bool {
    s.model.state().body(&built_name(scan)).is_some()
}

/// The route by hand to a turned part about the axis through `point` along `direction`, as
/// commands with their numbers: a sketch on a plane through the axis, the scan's section drawn in
/// it, and a revolve about the axis once one side's profile is closed.
fn by_hand(scan: &str, point: &[f64; 3], direction: &[f64; 3]) -> String {
    let d = Vec3::new(direction[0], direction[1], direction[2]);
    let helper = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let Some(across) = d.cross(helper).normalized() else { return String::new() };
    let v = |q: Vec3| format!("[{:.4}, {:.4}, {:.4}]", q.x, q.y, q.z);
    let p = format!("[{:.3}, {:.3}, {:.3}]", point[0], point[1], point[2]);
    format!(
        ". Build it by hand: `sketch.create {{\"plane\": {{\"origin\": {p}, \"x_dir\": {}, \"y_dir\": {}}}}}` (a plane through the axis), `scan.section_sketch {{\"body\": \"{scan}\"}}` to draw the scan's cut in it, close one side's profile, then `solid.revolve {{\"axis\": {{\"origin\": {p}, \"dir\": {}}}}}`",
        v(d),
        v(across),
        v(d)
    )
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
    // How each prism's outline repeats round its middle (a gear's teeth), read off its profile.
    let repeating: Vec<Option<ScanRotational>> = {
        let shown = super::shown_mesh(s, &found.scan);
        reading
            .prisms
            .iter()
            .map(|prism| {
                let shown = shown.as_ref()?;
                let scan = view.scan(shown.positions.as_slice(), shown.triangles.as_slice());
                let (plane, outlines) = profile::prism_profile(&scan, view.outcome(), reading, prism).ok()?;
                let found = profile::rotational(&profile::ring(outlines.first()?), view.outcome().tolerance)?;
                // On the middle slice, where the profile was read (a drafted prism is sketched at its low end).
                let on_plane = plane.origin + plane.x * found.centre.x + plane.y * found.centre.y;
                let centre = on_plane + prism.direction * (prism.middle() - on_plane).dot(prism.direction);
                Some(ScanRotational {
                    order: found.order,
                    centre: at(centre),
                    tip_diameter: 2.0 * found.tip,
                    root_diameter: 2.0 * found.root,
                    module: 2.0 * found.tip / (found.order as f64 + 2.0),
                })
            })
            .collect()
    };
    let prisms: Vec<ScanPrism> = reading
        .prisms
        .iter()
        .zip(repeating)
        .enumerate()
        .map(|(index, (prism, rotational))| ScanPrism {
            index,
            direction: at(prism.direction),
            middle: at(prism.middle()),
            depth: prism.depth(),
            walls: prism.walls.clone(),
            ends: prism.ends,
            open: prism.open,
            draft: prism.draft.to_degrees(),
            rotational,
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
            form: h.form.as_ref().map(hole_form),
        })
        .collect();
    let repeats = reading.repeats.iter().map(scan_repeat).collect();
    let as_turned = |r: &solidcraft_scan::reconstruct::Revolve| ScanTurned {
        point: at(r.axis.point),
        direction: at(r.axis.direction),
        faces: r.members.clone(),
        share: r.share,
    };
    let turned = reading.revolve.as_ref().map(as_turned);
    let partly_turned = reading.partly_turned.as_ref().map(as_turned);
    let spokes = reading.spokes.as_ref().map(|sp| ScanSpokes { count: sp.count, faces: sp.faces.clone(), share: sp.share });
    let body = found.scan.clone();
    let proposals =
        proposals(reading, built.is_some()).iter().map(|p| json!({"command": p.command(), "params": p.params(&body, view.selected())})).collect();
    let note = if reading.faces.is_empty() {
        Some("Find All found no shape on the scan: it is all freeform".to_string())
    } else if let (true, Some(why)) = (reading.prisms.is_empty(), &reading.turned_unread) {
        let route = reading.revolve.as_ref().map(|r| by_hand(&found.scan, &at(r.axis.point), &at(r.axis.direction))).unwrap_or_default();
        Some(format!("the part turns about one axis, but Revolve It cannot read its profile: {why}{route}"))
    } else if let (Some(sp), Some(r)) = (&reading.spokes, &reading.revolve) {
        Some(format!(
            "a wheel: {} spokes (`spokes`) repeat round the turned part's axis, {:.0}% of the shapes; Revolve It turns the rim and hub, then cuts one opening between the spokes (read off the scan seen along the axis) and patterns it {} times round the axis (`solid.pattern.circular`, axis {{\"origin\": [{:.3}, {:.3}, {:.3}], \"dir\": [{:.4}, {:.4}, {:.4}]}}); its reply's `openings` says what was cut",
            sp.count,
            100.0 * sp.share,
            sp.count,
            r.axis.point.x,
            r.axis.point.y,
            r.axis.point.z,
            r.axis.direction.x,
            r.axis.direction.y,
            r.axis.direction.z
        ))
    } else if let (true, Some(r)) = (reading.prisms.is_empty(), &reading.partly_turned) {
        Some(format!(
            "no turned part: round shapes share an axis (`partly_turned`) but cover only {:.0}% of the shapes, the rest flat or free (a wheel's spokes){}",
            100.0 * r.share,
            by_hand(&found.scan, &at(r.axis.point), &at(r.axis.direction))
        ))
    } else if reading.prisms.is_empty() && reading.revolve.is_none() {
        let unread = match reading.unread_prisms {
            0 => String::new(),
            1 => " (one set of walls runs along one direction, but no slice halfway along it goes round them)".to_string(),
            n => format!(" ({n} sets of walls run along one direction, but no slice halfway along them goes round them)"),
        };
        Some(format!(
            "the shapes make no prism and no turned part{unread}: build from Fit Plane, Fit Cylinder and Section to Sketch, or with free forms"
        ))
    } else if let Some(held) = prisms_hold(reading).filter(|&h| h < FREEFORM_HELD && reading.revolve.is_none() && reading.tubes.is_empty()) {
        let of = if view.selected() { "what is selected" } else { "the scan" };
        Some(format!(
            "the prisms read hold only {:.0}% of {of} ({:.0}% of it is in shapes): it is mostly freeform (a casting's or a moulding's surfaces). Extrude It builds its machined faces only; surface the rest with Fit Patches (`scan.fit_patches` on a region selected), a free form (`scan.free_form`) or Wrap Surface (`scan.wrap_surface`)",
            100.0 * held,
            100.0 * reading.in_shapes
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
        partly_turned,
        spokes,
        holes,
        repeats,
        built,
        proposals,
        fillets: reading.fillets.iter().map(|f| ScanEdgeRound { face: f.face, radius: f.radius, edge: at(f.edge) }).collect(),
        chamfers: reading.chamfers.iter().map(|c| ScanEdgeChamfer { face: c.face, distance: c.distance, edge: at(c.edge) }).collect(),
        shell: reading.shell.as_ref().map(|w| ScanShell { thickness: w.thickness, inner: w.inner.clone(), open: w.open.clone() }),
        prism_repeats: reading.prism_repeats.iter().map(scan_repeat).collect(),
        coil: reading.coil.as_ref().map(shapes::coil_of),
        pipe: reading.pipe.as_ref().map(shapes::pipe_of),
        tubes: reading.tubes.iter().map(shapes::tube_of).collect(),
        loft: reading.loft.as_ref().map(shapes::loft_of),
        note,
        param_warnings: Vec::new(),
    })
}

/// Below this share of the scan held by the prisms read, the part is mostly freeform.
const FREEFORM_HELD: f64 = 0.1;

/// The share of the scan the prisms read hold (their walls and ends), from the shapes' share of
/// it: `None` with no prism.
fn prisms_hold(reading: &reconstruct::Reading) -> Option<f64> {
    if reading.prisms.is_empty() || !(reading.in_shapes > 0.0) {
        return None;
    }
    let held: std::collections::BTreeSet<usize> = reading.prisms.iter().flat_map(|p| p.walls.iter().chain(&p.ends)).copied().collect();
    let area = |k: &usize| reading.faces.get(*k).map_or(0.0, |f| f.area);
    let shapes: f64 = reading.faces.iter().map(|f| f.area).sum();
    let total = shapes / reading.in_shapes;
    (total > 0.0).then(|| held.iter().map(area).sum::<f64>() / total)
}

/// A repeat as the reply lists it.
fn scan_repeat(r: &reconstruct::Repeat) -> ScanRepeat {
    let mut out = ScanRepeat {
        kind: format!("{:?}", r.kind).to_lowercase(),
        members: r.members.clone(),
        places: r.places,
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
        Layout::Circular { centre, axis, angle } => (out.centre, out.axis, out.angle) = (Some(at(centre)), Some(at(axis)), Some(angle.to_degrees())),
        Layout::Grid { step, across, columns, rows } => {
            (out.step, out.across, out.columns, out.rows) = (Some(at(step)), Some(at(across)), Some(columns), Some(rows))
        }
    }
    out
}

/// What a profile sketch was drawn with.
#[derive(Default)]
struct Drawn {
    lines: usize,
    arcs: usize,
    tangents: usize,
    /// Corners held to their mirror images.
    mirrored: usize,
}

impl Drawn {
    fn add(&mut self, more: &Drawn) {
        self.lines += more.lines;
        self.arcs += more.arcs;
        self.tangents += more.tangents;
        self.mirrored += more.mirrored;
    }
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
fn draw(sk: &mut Sketch, outline: &Outline, level: &Level<'_>, drawn: &mut Drawn, mirror: Option<f64>) -> Result<()> {
    let n = outline.corners.len();
    if n < 2 {
        return Ok(());
    }
    // An outline that is its own mirror image (within `mirror`) is drawn so: a centre line, and
    // each corner held to its image across it, so one size moves both sides.
    let mirrors = mirror.map(|tolerance| profile::mirrored(outline, tolerance)).unwrap_or_default();
    let mirror_tolerance = mirror.unwrap_or(0.0);
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
                        // A line along the turning axis is held on it, level with the sketch's
                        // origin there: held only level, rounding the sizes above it moved it a
                        // hair across, and Revolve refused the profile as crossing its axis.
                        if a != 0 && from.y.abs() < 1e-9 && to.y.abs() < 1e-9 {
                            sk.add_constraint(ConstraintKind::HorizontalPoints { p: 0, q: a }, None)?;
                        }
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
    if !mirrors.is_empty() {
        let (lo, hi) = outline.corners.iter().fold((outline.corners[0], outline.corners[0]), |(lo, hi), c| {
            (Vec2::new(lo.x.min(c.x), lo.y.min(c.y)), Vec2::new(hi.x.max(c.x), hi.y.max(c.y)))
        });
        for mirror in mirrors {
            let (from, to) = if mirror.across {
                (Vec2::new(mirror.at, lo.y), Vec2::new(mirror.at, hi.y))
            } else {
                (Vec2::new(lo.x, mirror.at), Vec2::new(hi.x, mirror.at))
            };
            // The centre line brings no sizes of its own: its ends go as far as the outline's
            // lowest and highest corners (or leftmost and rightmost), and it lies on the sketch's
            // origin when the outline is centred there; else its place is one size.
            let along = |c: Vec2| if mirror.across { c.y } else { c.x };
            let least = (0..n).min_by(|&i, &j| along(outline.corners[i]).total_cmp(&along(outline.corners[j]))).unwrap_or(0);
            let most = (0..n).max_by(|&i, &j| along(outline.corners[i]).total_cmp(&along(outline.corners[j]))).unwrap_or(0);
            let (a, b) = (sk.add_point(from, None)?, sk.add_point(to, None)?);
            let line = sk.add_line_pts(a, b, None)?;
            if let Some(curve) = sk.curves.get_mut(line) {
                curve.construction = true;
            }
            let level =
                |p: usize, q: usize| if mirror.across { ConstraintKind::HorizontalPoints { p, q } } else { ConstraintKind::VerticalPoints { p, q } };
            let in_line =
                |p: usize, q: usize| if mirror.across { ConstraintKind::VerticalPoints { p, q } } else { ConstraintKind::HorizontalPoints { p, q } };
            sk.add_constraint(if mirror.across { ConstraintKind::Vertical { l: line } } else { ConstraintKind::Horizontal { l: line } }, None)?;
            sk.add_constraint(level(ids[least], a), None)?;
            sk.add_constraint(level(ids[most], b), None)?;
            if mirror.at.abs() <= mirror_tolerance {
                sk.add_constraint(in_line(0, a), None)?;
            }
            for (a, b) in mirror.pairs {
                if ids[a] != ids[b] && sk.add_constraint(ConstraintKind::Symmetric { p: ids[a], q: ids[b], l: line }, None).is_ok() {
                    drawn.mirrored += 1;
                }
            }
        }
    }
    Ok(())
}

/// A draft of `radians` in degrees as designed: within 0.05° of a whole, half or tenth of a degree,
/// that.
fn drafted(radians: f64) -> f64 {
    solidcraft_scan::hole_form::snapped(radians.to_degrees(), 0.05)
}

/// `plane` slid in itself so its origin is where the world's origin falls on it (along its X
/// only when `x_only`: a turned profile's X is its axis), and `outlines` moved with it, so the
/// profile's place is measured from the part's own origin, not from where the scan was read.
fn at_world_origin(plane: &Plane, outlines: &[Outline], x_only: bool) -> (Plane, Vec<Outline>) {
    let d = Vec3::ZERO - plane.origin;
    let by = Vec2::new(d.dot(plane.x), if x_only { 0.0 } else { d.dot(plane.y) });
    let moved = Plane { origin: plane.origin + plane.x * by.x + plane.y * by.y, ..*plane };
    let outlines = outlines
        .iter()
        .map(|o| Outline {
            corners: o.corners.iter().map(|c| *c - by).collect(),
            pieces: o
                .pieces
                .iter()
                .map(|p| match *p {
                    Piece::Arc { center, ccw } => Piece::Arc { center: center - by, ccw },
                    Piece::Line => Piece::Line,
                })
                .collect(),
            faces: o.faces.clone(),
        })
        .collect();
    (moved, outlines)
}

/// A sketch on `plane` named `name`, with `outlines` drawn in, its origin where the world's falls
/// on the plane (`x_only`: along the plane's X only), dimensioned from it, each size within `round`
/// of a round one read as designed; its id.
#[allow(clippy::too_many_arguments)]
fn sketch_outlines(
    s: &mut Session,
    plane: &Plane,
    name: &str,
    outlines: &[Outline],
    level: &Level<'_>,
    drawn: &mut Drawn,
    round: f64,
    x_only: bool,
) -> Result<u64> {
    // A prism's outline that is its own mirror image is first drawn held so (one size for both
    // sides); as before when the sketch does not come out fully held that way. Alike within the
    // scan's tolerance, of which `round` is the share sizes are read as designed within.
    if !x_only {
        let before = s.doc.clone();
        let mut tried = Drawn::default();
        let alike = round / solidcraft_scan::hole_form::SNAP_SHARE;
        match sketch_outlines_as(s, plane, name, outlines, level, &mut tried, round, x_only, Some(alike)) {
            Ok((id, finished)) if tried.mirrored > 0 && finished.dof == Some(0) && finished.warning.is_none() => {
                drawn.add(&tried);
                return Ok(id);
            }
            Ok((id, _)) if tried.mirrored == 0 => {
                drawn.add(&tried);
                return Ok(id);
            }
            _ => {
                s.doc = before;
                s.refresh();
            }
        }
    }
    sketch_outlines_as(s, plane, name, outlines, level, drawn, round, x_only, None).map(|(id, _)| id)
}

/// [`sketch_outlines`], with `mirror` (a tolerance) each outline that is its own mirror image
/// held so; the sketch's id and how it was finished.
#[allow(clippy::too_many_arguments)]
fn sketch_outlines_as(
    s: &mut Session,
    plane: &Plane,
    name: &str,
    outlines: &[Outline],
    level: &Level<'_>,
    drawn: &mut Drawn,
    round: f64,
    x_only: bool,
    mirror: Option<f64>,
) -> Result<(u64, super::section::Finished)> {
    let (plane, outlines) = at_world_origin(plane, outlines, x_only);
    let (plane, outlines) = (&plane, outlines.as_slice());
    let create = json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": name});
    let made = super::new_sketch(s, create)?;
    let id = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the sketch was not made".into()))?;
    if s.model.state().sketch(id).is_none() {
        s.refresh();
    }
    let result = super::super::sketch_store::update(s, id, |_, _, sk| {
        for outline in outlines {
            draw(sk, outline, level, drawn, mirror)?;
        }
        Ok(())
    });
    if let Err(e) = result {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    // Dimensioned from its origin, as a profile drawn by hand is: its sizes are parameters.
    let finished = super::section::finish_dimensioned(s, true, Some(round))?;
    Ok((id, finished))
}

/// The profiles of sketch `id` that are material, when some are not: a profile inside a hole of
/// another is the hole itself (a disc where the part has a bore), one inside a hole of that is an
/// island again (a pin standing in the bore). Extrude takes every profile unless told, which would
/// fill each hole the profile has. `None` when every profile is material or the sketch is not
/// there.
pub(super) fn material_profiles(s: &mut Session, id: u64) -> Option<Vec<usize>> {
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

/// A prism's profile (on `plane`, extruded square to it) put exactly onto the body `solid` it
/// joins where it nearly meets it: each corner within `reach` of one of the body's vertices moves
/// onto it, else within `reach` of one of its straight edges, onto that edge's line; an arc moves
/// only whole (both ends by one offset, its centre with them). A `square` prism's walls run square to the plane, so the body is seen along
/// the extrusion (a wall of a prism stacked on it); a drafted one's lean, so only the body's
/// vertices and edges in the plane itself count. Read each on its own from the scan, two
/// prisms' walls nearly in one plane end a few thousandths of a millimetre apart, which the
/// kernel's join can neither merge nor tell apart (it refuses).
fn snap_to_body(s: &Session, solid: &str, plane: &Plane, outlines: &mut [Outline], reach: f64, square: bool) {
    if !(reach.is_finite() && reach > 0.0) {
        return;
    }
    let state = s.model.state();
    let Some(body) = state.body(solid) else { return };
    let Ok(edges) = body.body.edges(reach * 1e-3) else { return };
    let flat = |p: Vec3| Vec2::new((p - plane.origin).dot(plane.x), (p - plane.origin).dot(plane.y));
    let normal = plane.x.cross(plane.y);
    let counts = |p: Vec3| square || (p - plane.origin).dot(normal).abs() <= reach;
    let mut vertices: Vec<Vec2> = Vec::new();
    let mut lines: Vec<(Vec2, Vec2)> = Vec::new();
    for e in &edges {
        let (Some(&a), Some(&b)) = (e.points.first(), e.points.last()) else { continue };
        let (fa, fb) = (flat(a), flat(b));
        vertices.extend([(a, fa), (b, fb)].into_iter().filter(|(p, _)| counts(*p)).map(|(_, f)| f));
        if !(counts(a) && counts(b)) {
            continue;
        }
        // A straight edge, not one running along the extrusion (seen end on, a point).
        let straight = e.points.iter().all(|p| p.dist_to_segment(a, b) <= reach * 1e-3);
        if straight && fa.dist(fb) > reach {
            lines.push((fa, fb));
        }
    }
    for o in outlines.iter_mut() {
        let n = o.corners.len();
        if n < 2 || o.pieces.len() != n {
            continue;
        }
        // Where each corner would go: onto the nearest vertex, else the nearest straight edge.
        let mut moved: Vec<Option<Vec2>> = o
            .corners
            .iter()
            .map(|&c| {
                let vertex = vertices.iter().copied().filter(|v| v.dist(c) <= reach).min_by(|a, b| a.dist(c).total_cmp(&b.dist(c)));
                vertex.or_else(|| {
                    lines
                        .iter()
                        .filter_map(|&(a, b)| {
                            let d = b - a;
                            let t = (c - a).dot(d) / d.dot(d);
                            let q = a + d * t;
                            ((0.0..=1.0).contains(&t) && q.dist(c) <= reach).then_some(q)
                        })
                        .min_by(|a, b| a.dist(c).total_cmp(&b.dist(c)))
                })
            })
            .collect();
        // An arc moves whole or not at all: both its ends by one offset (its centre with them),
        // else neither, so its centre still fits its ends. Settled until no arc is split.
        let offset = |k: usize, moved: &[Option<Vec2>]| moved[k].map(|t| t - o.corners[k]);
        loop {
            let mut changed = false;
            for k in 0..n {
                if !matches!(o.pieces[k], Piece::Arc { .. }) {
                    continue;
                }
                let next = (k + 1) % n;
                let agree = match (offset(k, &moved), offset(next, &moved)) {
                    (Some(a), Some(b)) => a.dist(b) <= reach * 1e-3,
                    (None, None) => true,
                    _ => false,
                };
                if !agree {
                    changed |= moved[k].take().is_some() | moved[next].take().is_some();
                }
            }
            if !changed {
                break;
            }
        }
        let read = (o.corners.clone(), o.pieces.clone());
        for (k, piece) in o.pieces.iter_mut().enumerate().take(n) {
            if let Piece::Arc { center, ccw } = *piece
                && let Some(by) = offset(k, &moved)
            {
                *piece = Piece::Arc { center: center + by, ccw };
            }
        }
        for (k, to) in moved.iter().enumerate() {
            if let Some(to) = to {
                o.corners[k] = *to;
            }
        }
        // Two neighbouring corners put on one point would leave a piece of no length: that outline
        // stays as read.
        if (0..n).any(|k| o.corners[k].dist(o.corners[(k + 1) % n]) <= reach * 1e-3) {
            (o.corners, o.pieces) = read;
        }
    }
}

/// A prism's profile may run past the scan's box by this share of the box's diagonal (a part's
/// draft, a round read sharp) before it is refused as misread.
const PROFILE_PAST_SCAN: f64 = 0.05;

/// The corner of `outlines` (in `plane`) farthest outside the box `(lo, hi)` and how far, when that
/// is more than [`PROFILE_PAST_SCAN`] of the box's diagonal.
fn past_scan(plane: &Plane, outlines: &[profile::Outline], (lo, hi): (Vec3, Vec3)) -> Option<(Vec3, f64)> {
    let grow = PROFILE_PAST_SCAN * (hi - lo).len();
    let off = |v: f64, a: f64, b: f64| (a - v).max(v - b).max(0.0);
    outlines
        .iter()
        .flat_map(|o| o.corners.iter())
        .map(|c| plane.origin + plane.x * c.x + plane.y * c.y)
        .map(|w| (w, off(w.x, lo.x, hi.x).max(off(w.y, lo.y, hi.y)).max(off(w.z, lo.z, hi.z))))
        .filter(|(_, by)| *by > grow)
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// The scan's box, least and most corner.
fn scan_box(scan: &Scan) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in scan.positions {
        if p.x.is_finite() && p.y.is_finite() && p.z.is_finite() {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
    }
    (lo, hi)
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
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        let scan = view.scan(positions, triangles);
        let (plane, outlines) = profile::prism_profile(&scan, view.outcome(), reading, prism).map_err(|e| bad(cmd, e))?;
        // A profile that runs well past the scan is misread, not a part of it (T-LESS 06: a stray
        // corner 75 mm past the part's end, built and replied `first`): refused with where, before
        // anything is built.
        let past = past_scan(&plane, &outlines, scan_box(&scan));
        if let Some((w, by)) = past {
            return Err(bad(
                cmd,
                format!(
                    "prism {index}'s profile reaches {by:.1} mm past the scan (a corner at [{:.1}, {:.1}, {:.1}]): it was misread; build the others, then this part from a section (scan.section_sketch) and an extrude",
                    w.x, w.y, w.z
                ),
            ));
        }
        (plane, outlines)
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    // A prism joined to the body that touches it only along an edge (a cylinder head's boss
    // meeting its flange at a corner) is kept in the one body, not made a body of its own.
    let operation = if how == "join" { merged(operation, json!({"touching": "one_body"})) } else { operation };
    // A prism joined to the body already built meets it where the scan has them meet, exactly:
    // corners within the scan's tolerance of the body's put on it.
    let mut outlines = outlines;
    if how == "join" {
        snap_to_body(s, &solid, &plane, &mut outlines, view.outcome().tolerance, prism.draft == 0.0);
    }
    let mut drawn = Drawn::default();
    // Sizes within a quarter of the scan's tolerance of a round size are read as designed, as
    // Hole It reads a hole's.
    let round = solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance;
    let depth = solidcraft_scan::hole_form::snapped(prism.depth(), round);
    let (draft, name) = (prism.draft, format!("Extrude It {}", index + 1));
    // A join is measured: one that adds no more than a skin the scan's tolerance thick over its
    // outline, a prism the body already holds, is taken back (its walls a hair off the body's
    // would leave slivers whose stray faces a later cut trips on).
    let held = |s: &Session| s.model.state().body(&solid).and_then(|b| solidcraft_kernel::measure(&b.body).ok()).map(|m| m.volume);
    let (before_join, volume_before) = (s.doc.clone(), if how == "join" { held(s) } else { None });
    let skin = outlines.first().map_or(0.0, |o| profile::area(&o.corners).abs()) * view.outcome().tolerance;
    // The outlines sketched and extruded.
    let build = |s: &mut Session, outlines: &[Outline], drawn: &mut Drawn| -> Result<(u64, Value)> {
        let sketch = sketch_outlines(s, &plane, &format!("Prism Profile {}", index + 1), outlines, &|_, _, _| None, drawn, round, false)?;
        let mut extrude = if draft == 0.0 {
            json!({"sketch": sketch, "distance": depth, "direction": "symmetric", "name": name})
        } else {
            // Sketched at the low end, the walls leaning in (a positive draft) as it goes up.
            json!({"sketch": sketch, "distance": depth, "direction": "positive", "taper": format!("{} deg", -drafted(draft)),
                "corner": "preserve_curvature", "name": name})
        };
        if let Some(material) = material_profiles(s, sketch) {
            extrude["profiles"] = json!(material);
        }
        let feature = run(s, "solid.extrude", merged(extrude, operation.clone()))?;
        Ok((sketch, feature))
    };
    // A prism whose outline repeats round its middle (a gear, a knob, a spline) is its tip circle
    // extruded, the gap between two teeth cut through it and the cut patterned round, so that the
    // count is a number in the design; as its whole outline when the kernel refuses any of that.
    let repeat = (draft == 0.0 && how == "new" && bool_(p, "pattern").unwrap_or(true))
        .then(|| outlines.first().and_then(|o| profile::repeated(o, view.outcome().tolerance, |r| solidcraft_scan::hole_form::snapped(r, round))))
        .flatten();
    let mut count = None;
    let patterned = match repeat {
        Some(repeat) => {
            let before = s.doc.clone();
            let mut tried = Drawn::default();
            let made = (|| -> Result<(u64, Value)> {
                let disc = Outline {
                    corners: vec![repeat.centre + Vec2::X * repeat.tip, repeat.centre - Vec2::X * repeat.tip],
                    pieces: vec![profile::Piece::Arc { center: repeat.centre, ccw: true }; 2],
                    faces: vec![None; 2],
                };
                let whole: Vec<Outline> = std::iter::once(disc).chain(outlines.iter().skip(1).cloned()).collect();
                let made = build(s, &whole, &mut tried)?;
                let gap = sketch_outlines(
                    s,
                    &plane,
                    &format!("Prism Profile {} Gap", index + 1),
                    std::slice::from_ref(&repeat.gap),
                    &|_, _, _| None,
                    &mut tried,
                    round,
                    false,
                )?;
                let through = depth + 8.0 * view.outcome().tolerance;
                let cut = run(
                    s,
                    "solid.extrude",
                    json!({"sketch": gap, "distance": format!("{through} mm"), "direction": "symmetric", "operation": "cut", "targets": [solid], "name": format!("{name} Gap")}),
                )?;
                let cut =
                    cut.get("name").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| EngineError::Other("the gap was not cut".into()))?;
                let middle = plane.origin + plane.x * repeat.centre.x + plane.y * repeat.centre.y;
                run(
                    s,
                    "solid.pattern.circular",
                    json!({"features": [cut], "axis": {"origin": at(middle), "dir": at(prism.direction)}, "count": repeat.order, "name": format!("{name} Pattern")}),
                )?;
                Ok(made)
            })();
            match made {
                Ok(made) => {
                    drawn = tried;
                    count = Some(repeat.order);
                    Some(made)
                }
                Err(_) => {
                    s.doc = before;
                    s.refresh();
                    None
                }
            }
        }
        None => None,
    };
    // A drafted prism's rounds that the scan shows as cylinders (one radius all the way up: the
    // part was drafted, then filleted) are not sketched: tapered with the walls each would narrow
    // to a cone as it rises. The corners are extruded sharp and rounded by a Fillet of that
    // radius. A round the scan shows as a cone keeps its taper; so do all of them when the kernel
    // refuses the fillets.
    let sharp = (draft != 0.0)
        .then(|| {
            profile::sharpened(&outlines, |face| match reading.faces.get(face).map(|f| &f.shape) {
                Some(Shape::Cylinder { radius, .. }) => Some(*radius),
                _ => None,
            })
        })
        .filter(|(_, corners)| !corners.is_empty());
    let sharply = match sharp {
        Some((sharpened, corners)) => {
            let before = s.doc.clone();
            let mut tried = Drawn::default();
            let made = build(s, &sharpened, &mut tried).and_then(|made| {
                round_corners(s, &solid, &plane, &corners, draft, depth, round, &format!("{name} Rounds"))?;
                Ok(made)
            });
            match made {
                Ok(made) => {
                    drawn = tried;
                    Some(made)
                }
                Err(_) => {
                    s.doc = before;
                    s.refresh();
                    None
                }
            }
        }
        None => None,
    };
    let (sketch, feature) = match patterned.or(sharply) {
        Some(made) => made,
        None => build(s, &outlines, &mut drawn)?,
    };
    // A prism whose profile is in pieces (a cylinder head's gasket flange in two lobes) is
    // extruded into a body for each, and a prism joined to a body it does not touch (a boss the
    // scan shows apart from the flange, the casting between them freeform) into a body of its
    // own: each is joined into the one the steps after it build on (a join keeps pieces that
    // touch nothing in one body), so they are measured against the scan together, not left as
    // bodies of their own.
    if let Some(made) = feature.get("created_bodies").and_then(Value::as_array) {
        let others: Vec<String> = made.iter().filter_map(Value::as_str).filter(|b| *b != solid).map(str::to_owned).collect();
        if !others.is_empty() && s.model.state().body(&solid).is_some() {
            run(s, "solid.combine", json!({"operation": "join", "target": solid, "tools": others, "name": format!("{name} Pieces")}))?;
        }
    }
    if let (Some(before), Some(after)) = (volume_before, held(s))
        && after - before <= skin
    {
        s.doc = before_join;
        s.refresh();
        return reply(ScanBuildResult {
            body: found.scan.clone(),
            sketch: None,
            feature: Value::Null,
            solid,
            operation: how.into(),
            lines: 0,
            arcs: 0,
            tangents: 0,
            depth: Some(depth),
            count: None,
            openings: None,
            skin: None,
            allowance: None,
            changed: Some(false),
            note: Some(format!(
                "the body already holds prism {index}: joined, it added {:.3} mm³, no more than a skin the scan's tolerance thick over its outline, so nothing was built",
                (after - before).max(0.0)
            )),
            fit: None,
            param_warnings: Vec::new(),
        });
    }
    // Pieces joined that touch nothing (kept in one body on purpose, so they are measured
    // together) are said: what holds them was not read, and a user or an agent should know the
    // body is not one solid yet (T-LESS 06's studs, standing on inner walls no prism holds).
    let pieces = s.model.state().body(&solid).and_then(|b| b.body.lumps().ok()).map_or(1, |l| l.len());
    let note = (pieces > 1).then(|| {
        format!(
            "`{solid}` is now {pieces} separate pieces: a prism here touches nothing built yet (what joins it to the rest was not read off the scan). Build what joins them (a section or a fit there), or undo this step"
        )
    });
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
        count,
        openings: None,
        skin: None,
        allowance: None,
        changed: None,
        note,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// Nest (`scan.nest`): see [`ScanNestParams`].
pub(super) fn nest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.nest";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    let turned = match p.get("turned").filter(|v| !v.is_null()) {
        None => reading.prisms.is_empty() && reading.revolve.is_some() && reading.turned_unread.is_none(),
        Some(v) => v.as_bool().ok_or_else(|| bad(cmd, "`turned` is true or false"))?,
    };
    if turned {
        return nest_turned(s, p, &view);
    }
    if reading.prisms.is_empty() {
        return Err(bad(
            cmd,
            "Reconstruct reads no prism on the scan, and a nest is cut from a prism's outline (or, `turned`, a turned part's profile): for another shape, section it (scan.section_sketch), offset the sketch and cut that from a block",
        ));
    }
    let index = p.get("prism").and_then(Value::as_u64).map_or(0, |k| k as usize);
    let prism = reading
        .prisms
        .get(index)
        .ok_or_else(|| bad(cmd, format!("there are {} prisms: `prism` is 0 to {}", reading.prisms.len(), reading.prisms.len() - 1)))?;
    if prism.draft != 0.0 {
        return Err(bad(
            cmd,
            format!(
                "prism {index} has drafted walls ({:.1} deg), and a nest's pocket is cut square: extrude its sketch tapered by hand",
                drafted(prism.draft)
            ),
        ));
    }
    let (clearance, margin, deep) = nest_sizes(p, cmd)?;
    let (plane, outlines) = {
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        profile::prism_profile(&view.scan(positions, triangles), view.outcome(), reading, prism).map_err(|e| bad(cmd, e))?
    };
    // The pocket's outline is offset in its sketch, and an offset cannot be made once a piece of
    // it has closed to nothing: the pieces that would by twice the clearance asked (short pieces at
    // tight inside corners) are left out, their neighbours run on to meet, so the pocket follows
    // the clearance up to twice what it is built with.
    let asked = s.doc.eval(&clearance, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("clearance: {e}")))?;
    if !(asked > 0.0) {
        return Err(bad(cmd, "`clearance` is a length above 0, in mm or as an expression"));
    }
    // The scan does not lie on the prism everywhere: where it stands proud of the outline (a flank
    // the fitted curve cuts inside of, a corner cut square just above) or of the end it sits on,
    // the pocket would be that much tighter than the clearance, so the offset and the depth are
    // the clearance and that allowance, measured here unless it is given. A larger offset closes more pieces, which may leave the scan prouder:
    // settled in a few turns.
    let given = match p.get("allowance").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(
            v.as_f64().filter(|a| a.is_finite() && *a >= 0.0 && *a <= 1.0e4).ok_or_else(|| bad(cmd, "`allowance` is a number of mm, 0 or more"))?,
        ),
    };
    let base = outlines.first().ok_or_else(|| bad(cmd, "the prism has no outline"))?;
    let half = 0.5 * prism.depth();
    // The part sits on the prism's low end: below the plane's origin (halfway along) when the
    // plane faces the way the prism runs.
    let seat = if plane.normal().dot(prism.direction) > 0.0 { (-half, 1.0) } else { (half, -1.0) };
    let reach = (NEST_ASTRAY * reading.size).max(4.0 * view.outcome().tolerance);
    let mut allowance = given.unwrap_or(0.0);
    let mut outline = profile::roomy(base, 2.0 * asked + allowance);
    if given.is_none() {
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let scan = view.scan(shown.positions.as_slice(), shown.triangles.as_slice());
        for _ in 0..4 {
            // Up to the next thousandth of a millimetre: the number is in the design's expressions.
            let proud = (profile::proud(&scan, &plane, &outline, -half, half, Some(seat), reach) * 1000.0).ceil() / 1000.0;
            if proud <= allowance {
                break;
            }
            allowance = proud;
            outline = profile::roomy(base, 2.0 * asked + allowance);
        }
    }
    let outline = &outline;
    let offset = if allowance > 0.0 { format!("({clearance}) + {allowance} mm") } else { format!("({clearance})") };
    let round = solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance;
    let depth = solidcraft_scan::hole_form::snapped(prism.depth(), round);
    let held = deep * depth;
    // Both sketches on the plane of the block's top: the profile's own plane (halfway along the
    // prism) slid along the prism to where the pocket opens. The pocket goes down from there
    // toward the prism's low end, and the clearance further.
    let normal = plane.x.cross(plane.y).normalized().ok_or_else(|| EngineError::Other("the profile's plane has no normal".into()))?;
    let top = Plane { origin: plane.origin + prism.direction * ((deep - 0.5) * depth), ..plane };
    let down = if normal.dot(prism.direction) > 0.0 { "negative" } else { "positive" };
    let ring = profile::ring(outline);
    let (lo, hi) =
        ring.iter().fold((ring[0], ring[0]), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    let solid = format!("{} Nest", found.scan);
    if s.model.state().body(&solid).is_some() {
        return Err(bad(cmd, format!("`{solid}` is in the design already: change its clearance, or remove it first")));
    }
    let mut drawn = Drawn::default();
    // The block.
    let wall = Vec2::new(margin, margin);
    let (a, b) = (lo - wall, hi + wall);
    let block = Outline { corners: vec![a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)], pieces: vec![Piece::Line; 4], faces: vec![None; 4] };
    let (block_sketch, _) = sketch_outlines_as(s, &top, "Nest Block", &[block], &|_, _, _| None, &mut Drawn::default(), round, false, None)?;
    run(
        s,
        "solid.extrude",
        json!({"sketch": block_sketch, "distance": format!("{} mm", held + margin), "direction": down, "operation": "new", "body_name": solid, "name": "Nest Block"}),
    )?;
    // The pocket: the outline, and round it its offset by the clearance, kept as an offset.
    let (sketch, _) = sketch_outlines_as(s, &top, "Nest Pocket", std::slice::from_ref(outline), &|_, _, _| None, &mut drawn, round, false, None)?;
    let first = super::super::sketch_store::read(&s.doc, sketch)?
        .curves
        .iter()
        .find(|c| !c.construction)
        .map(|c| c.id.clone())
        .ok_or_else(|| EngineError::Other("the pocket's outline was not drawn".into()))?;
    // A point well outside the outline, in the sketch (which is measured from the world's origin).
    let outside = {
        let moved = at_world_origin(&top, std::slice::from_ref(outline), false).1;
        let far = moved[0].corners.iter().fold(Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |m, c| Vec2::new(m.x.max(c.x), m.y.max(c.y)));
        far + (hi - lo) + Vec2::new(10.0 * margin, 10.0 * margin)
    };
    run(s, "sketch.edit", json!({"sketch": sketch}))?;
    if let Err(e) = run(s, "sketch.offset", json!({"curves": [first], "distance": offset, "side": [outside.x, outside.y], "chain": true})) {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    run(s, "sketch.finish", json!({}))?;
    let feature = run(
        s,
        "solid.extrude",
        json!({"sketch": sketch, "distance": format!("{held} mm + {offset}"), "direction": down, "operation": "cut", "targets": [solid], "name": "Nest Pocket"}),
    )?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: Some(sketch),
        feature,
        solid,
        operation: "new".into(),
        lines: drawn.lines,
        arcs: drawn.arcs,
        tangents: drawn.tangents,
        depth: Some(held),
        count: None,
        openings: None,
        skin: None,
        allowance: Some(allowance),
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// Nest's clearance (an expression), margin and depth share, as given or by default.
fn nest_sizes(p: &Value, cmd: &str) -> Result<(String, f64, f64)> {
    let clearance = match p.get("clearance").filter(|v| !v.is_null()) {
        None => "0.3 mm".to_owned(),
        Some(Value::String(text)) if !text.trim().is_empty() => text.trim().to_owned(),
        Some(v) => match v.as_f64().filter(|c| c.is_finite() && *c > 0.0) {
            Some(c) => format!("{c} mm"),
            None => return Err(bad(cmd, "`clearance` is a length above 0, in mm or as an expression")),
        },
    };
    let number = |key: &str, default: f64, most: f64| match p.get(key).filter(|v| !v.is_null()) {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && *x > 0.0 && *x <= most)
            .ok_or_else(|| bad(cmd, format!("`{key}` is a number above 0, at most {most}"))),
    };
    Ok((clearance, number("margin", 10.0, 1.0e4)?, number("deep", 0.5, 1.0)?))
}

/// Nest for a turned part: it lies on its side in a cradle. Its profile is revolved into a body
/// of its own (`<scan> Nest Part`), every face of that offset outward by the clearance (so the
/// pocket follows it), and the body cut from a block drawn round the axis, its top where the
/// pocket holds `deep` of the part's diameter, `margin` of block round and under it.
fn nest_turned(s: &mut Session, p: &Value, view: &View) -> Result<Value> {
    let cmd = "scan.nest";
    let found = &view.found;
    let reading = view.reading();
    if reading.revolve.is_none() {
        return Err(bad(cmd, "Reconstruct reads no turned part on the scan, and `turned` nests one"));
    }
    let (clearance, margin, deep) = nest_sizes(p, cmd)?;
    let asked = s.doc.eval(&clearance, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("clearance: {e}")))?;
    if !(asked > 0.0) {
        return Err(bad(cmd, "`clearance` is a length above 0, in mm or as an expression"));
    }
    let (plane, outline, held) = {
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let scan = view.scan(shown.positions.as_slice(), shown.triangles.as_slice());
        profile::turned_profile(&scan, view.outcome(), reading).map_err(|e| bad(cmd, e))?
    };
    let solid = format!("{} Nest", found.scan);
    let part = format!("{} Nest Part", found.scan);
    for name in [&solid, &part] {
        if s.model.state().body(name).is_some() {
            return Err(bad(cmd, format!("`{name}` is in the design already: change its clearance, or remove it first")));
        }
    }
    // The part's reach along the axis and out from it, in the profile's plane (X along the axis).
    let ring = profile::ring(&outline);
    let (lo, hi, radius) =
        ring.iter().fold((f64::INFINITY, f64::NEG_INFINITY, 0.0_f64), |(lo, hi, r), q| (lo.min(q.x), hi.max(q.x), r.max(q.y.abs())));
    if !(hi > lo && radius > 0.0) {
        return Err(bad(cmd, "the turned profile has no length or no radius"));
    }
    let level = turned_level(&held);
    let mut drawn = Drawn::default();
    let round = solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance;
    let sketch = sketch_outlines(s, &plane, "Nest Part Profile", std::slice::from_ref(&outline), &level, &mut drawn, round, true)?;
    run(s, "solid.revolve", json!({"sketch": sketch, "axis": "x", "operation": "new", "body_name": part, "name": "Nest Part"}))?;
    let faces: Vec<Value> = run(s, "model.faces", json!({"body": part}))?
        .get("faces")
        .and_then(Value::as_array)
        .map(|f| f.iter().filter_map(|face| face.get("name").cloned()).filter(|n| !n.is_null()).collect())
        .unwrap_or_default();
    if faces.is_empty() {
        return Err(EngineError::Other("the turned part's faces have no names to offset".into()));
    }
    run(s, "solid.offset_face", json!({"faces": faces, "distance": format!("({clearance})"), "body": part, "name": "Nest Clearance"}))?;
    // The block: square to the way down (the world's -Z, or across the profile when the axis is
    // upright), its top where the pocket holds `deep` of the diameter.
    let axis = plane.x;
    let down = (-Vec3::Z - axis * (-Vec3::Z).dot(axis)).normalized().unwrap_or(-plane.y);
    let up = -down;
    let across = up.cross(axis).normalized().ok_or_else(|| EngineError::Other("the axis has no way across".into()))?;
    let top = Plane { origin: plane.origin + up * ((2.0 * deep - 1.0) * radius), x: axis, y: across };
    let room = margin + asked;
    let (a, b) = (Vec2::new(lo - room, -(radius + room)), Vec2::new(hi + room, radius + room));
    let block = Outline { corners: vec![a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)], pieces: vec![Piece::Line; 4], faces: vec![None; 4] };
    let (block_sketch, _) = sketch_outlines_as(s, &top, "Nest Block", &[block], &|_, _, _| None, &mut Drawn::default(), round, false, None)?;
    let normal = axis.cross(across);
    let way = if normal.dot(up) > 0.0 { "negative" } else { "positive" };
    let depth = 2.0 * deep * radius;
    run(
        s,
        "solid.extrude",
        json!({"sketch": block_sketch, "distance": format!("{} mm + ({clearance})", depth + margin), "direction": way, "operation": "new", "body_name": solid, "name": "Nest Block"}),
    )?;
    let feature = run(s, "solid.combine", json!({"target": solid, "tools": [part], "operation": "cut", "name": "Nest Pocket"}))?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: Some(sketch),
        feature,
        solid,
        operation: "new".into(),
        lines: drawn.lines,
        arcs: drawn.arcs,
        tangents: drawn.tangents,
        depth: Some(depth),
        count: None,
        openings: None,
        skin: None,
        allowance: Some(0.0),
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// How a turned profile's pieces are held, by the faces they are on (`held`): a cylinder's piece
/// level along the axis, an end's upright, the axis's own along it; a piece with no face off the
/// axis bridges a gap the scan's faces leave, and is held neither way.
fn turned_level(held: &std::collections::BTreeMap<usize, Turned>) -> impl Fn(Option<usize>, Vec2, Vec2) -> Option<bool> + '_ {
    move |face: Option<usize>, from: Vec2, to: Vec2| match face.and_then(|k| held.get(&k)) {
        None if face.is_none() => (from.y.abs() < 1e-9 && to.y.abs() < 1e-9).then_some(true),
        None => Some(true),
        Some(Turned::Cylinder { .. }) => Some(true),
        Some(Turned::Plane { .. }) => Some(false),
        Some(_) => None,
    }
}

/// The sharp `corners` of a drafted prism just extruded (`depth` up from `plane`, its walls
/// leaning in by `draft` radians) rounded on `solid`: one Fillet for each radius, read as designed
/// within `round`, each corner's edge found halfway up, where the lean has moved it in.
fn round_corners(
    s: &mut Session,
    solid: &str,
    plane: &Plane,
    corners: &[profile::SharpCorner],
    draft: f64,
    depth: f64,
    round: f64,
    name: &str,
) -> Result<()> {
    let up = plane.x.cross(plane.y).normalized().ok_or_else(|| EngineError::Other("the profile's plane has no normal".into()))?;
    // Rounds whose radii the scan reads within a little of each other are one radius, as the
    // rounds a profile holds are (`profile::SAME_ROUND`): their mean, read as designed.
    let mut sorted: Vec<&profile::SharpCorner> = corners.iter().collect();
    sorted.sort_by(|a, b| a.radius.total_cmp(&b.radius));
    let mut groups: Vec<Vec<&profile::SharpCorner>> = Vec::new();
    for corner in sorted {
        match groups.last_mut() {
            Some(group) if corner.radius <= group[0].radius * (1.0 + profile::SAME_ROUND) => group.push(corner),
            _ => groups.push(vec![corner]),
        }
    }
    let several = groups.len() > 1;
    for (n, group) in groups.into_iter().enumerate() {
        let mean = group.iter().map(|c| c.radius).sum::<f64>() / group.len() as f64;
        let radius = solidcraft_scan::hole_form::snapped(mean, round);
        let edges: Vec<[f64; 3]> = group
            .iter()
            .map(|corner| {
                let halfway = corner.moved(0.5 * depth * draft.tan());
                at(plane.origin + plane.x * halfway.x + plane.y * halfway.y + up * (0.5 * depth))
            })
            .collect();
        let name = if several { format!("{name} {}", n + 1) } else { name.to_owned() };
        run(s, "solid.fillet", json!({"edges": edges, "radius": format!("{radius} mm"), "body": solid, "tangent_chain": false, "name": name}))?;
    }
    Ok(())
}

pub(super) fn revolve_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.revolve_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let spokes = view.reading().spokes.as_ref().map(|sp| sp.count);
    let axis = view.reading().revolve.as_ref().map(|r| r.axis);
    let (plane, outline, held, opening, closing) = {
        let shown = super::shown_mesh(s, &found.scan).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        let scan = view.scan(positions, triangles);
        let (plane, outline, held, closing) = profile::turned_profile_open(&scan, view.outcome(), view.reading()).map_err(|e| bad(cmd, e))?;
        // A wheel: one opening between its spokes, read off the scan seen along the axis.
        let opening = match (spokes, &axis) {
            (Some(count), Some(axis)) => Some(solidcraft_scan::wheel::opening(&scan, axis, count).ok_or(count)),
            _ => None,
        };
        (plane, outline, held, opening, closing)
    };
    let wall = match p.get("wall").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(v.as_f64().filter(|w| w.is_finite() && *w > 0.0).ok_or_else(|| bad(cmd, "`wall` is a thickness in mm, above 0"))?),
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    let level = turned_level(&held);
    let mut drawn = Drawn::default();
    let round = solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance;
    let sketch = sketch_outlines(s, &plane, "Turned Profile", std::slice::from_ref(&outline), &level, &mut drawn, round, true)?;
    // One side of a skin, its thickness given: the profile without the line that closes it,
    // revolved as a surface and thickened away from the scanned side. Of the two ways to thicken,
    // that is the one whose body is the smaller across (the scanned side is the outside of the
    // closed profile: thickened that way the body grows past it). The solid when any of it fails.
    let ends = closing.map(|k| {
        let moved = at_world_origin(&plane, std::slice::from_ref(&outline), true).1;
        let corners = &moved[0].corners;
        [corners[k], corners[(k + 1) % corners.len()]]
    });
    let skinned = match (wall, ends) {
        (Some(wall), Some(ends)) => {
            let before = s.doc.clone();
            let built = skin(s, sketch, ends, wall, &solid, &operation);
            if built.is_err() {
                s.doc = before;
                s.refresh();
            }
            Some(built)
        }
        _ => None,
    };
    let (feature, skin) = match skinned {
        Some(Ok(feature)) => {
            (feature, Some(format!("one side of a skin: revolved as a surface and thickened {} mm away from the scanned side", wall.unwrap_or(0.0))))
        }
        other => {
            let revolve = merged(json!({"sketch": sketch, "axis": "x", "name": "Revolve It"}), operation);
            let feature = run(s, "solid.revolve", revolve)?;
            let skin = match (other, closing) {
                (Some(Err(e)), _) => Some(format!("one side of a skin, but it could not be thickened ({e}): closed straight across into a solid")),
                (_, Some(_)) => Some("one side of a skin: closed straight across what the scan never saw into a solid; `wall` (its thickness in mm) builds it as a thickened surface".to_owned()),
                _ => None,
            };
            (feature, skin)
        }
    };
    let (count, openings) = match (opening, axis) {
        (Some(Ok(opening)), Some(axis)) => match cut_openings(s, &opening, &axis, spokes.unwrap_or(opening.alike), &solid) {
            Ok(n) => (Some(n), Some(format!("{n} openings between the spokes cut through it and patterned round the axis"))),
            Err(e) => (None, Some(format!("the openings between the spokes were not cut: {e}"))),
        },
        (Some(Err(count)), _) => {
            (None, Some(format!("the scan seen along the axis does not show {count} openings alike between the spokes, so none were cut")))
        }
        _ => (None, None),
    };
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
        count,
        openings,
        skin,
        allowance: None,
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// The turned profile in `sketch` without its closing line (the line whose ends are `ends`, in the
/// sketch), revolved as a surface about the sketch's X and thickened by `wall` into `solid`, on
/// the side that leaves the body smaller across. The thickened feature's reply.
fn skin(s: &mut Session, sketch: u64, ends: [Vec2; 2], wall: f64, solid: &str, operation: &Value) -> Result<Value> {
    let cmd = "scan.revolve_it";
    let curves: Vec<String> = {
        let drawn = super::super::sketch_store::read(&s.doc, sketch)?;
        let off = |c: &solidcraft_sketch::Curve| match c.kind {
            solidcraft_sketch::CurveKind::Line { a, b } => match (drawn.point(a), drawn.point(b)) {
                (Some(p), Some(q)) => (p.dist(ends[0]) + q.dist(ends[1])).min(p.dist(ends[1]) + q.dist(ends[0])),
                _ => f64::INFINITY,
            },
            _ => f64::INFINITY,
        };
        let live: Vec<&solidcraft_sketch::Curve> = drawn.curves.iter().filter(|c| !c.construction).collect();
        let closing = live.iter().copied().min_by(|a, b| off(a).total_cmp(&off(b))).filter(|c| off(c) <= 0.02 * ends[0].dist(ends[1]).max(1.0));
        let closing = closing.ok_or_else(|| bad(cmd, "the line closing the profile is not in its sketch"))?;
        live.iter().filter(|c| c.id != closing.id).map(|c| c.id.clone()).collect()
    };
    let sheet = format!("{solid} Skin");
    let across = |s: &mut Session, thickness: f64| -> Result<(f64, Value)> {
        run(s, "surface.revolve", json!({"sketch": sketch, "curves": curves, "axis": "x", "name": "Revolve It", "body_name": sheet}))?;
        let made = run(
            s,
            "surface.thicken",
            merged(json!({"bodies": [sheet], "thickness": format!("{thickness} mm"), "name": "Revolve It Wall"}), operation.clone()),
        )?;
        let state = s.model.state();
        let body = state.body(solid).ok_or_else(|| bad(cmd, "the skin was not thickened into a body"))?;
        let bbox = solidcraft_kernel::measure(&body.body).map_err(|e| EngineError::Other(e.to_string()))?.bbox;
        Ok(((bbox.max - bbox.min).len(), made))
    };
    let before = s.doc.clone();
    let (one, made) = across(s, wall)?;
    let kept = s.doc.clone();
    s.doc = before;
    s.refresh();
    match across(s, -wall) {
        Ok((other, made)) if other < one => Ok(made),
        _ => {
            s.doc = kept;
            s.refresh();
            Ok(made)
        }
    }
}

/// One opening between a wheel's spokes cut through `solid`: its outline fitted with lines and
/// arcs in a sketch square to the axis at the scan's front, cut through the wheel, and patterned
/// `count` times round the axis. How many there are after.
fn cut_openings(
    s: &mut Session,
    opening: &solidcraft_scan::wheel::Opening,
    axis: &solidcraft_scan::reconstruct::Axis,
    count: usize,
    solid: &str,
) -> Result<usize> {
    let cmd = "scan.revolve_it";
    let size = opening.outline.iter().map(|p| p.len()).fold(0.0_f64, f64::max);
    let tolerance = (0.004 * size).max(0.05);
    let fitted =
        solidcraft_scan::chain::chain(&opening.outline, true, tolerance).ok_or_else(|| bad(cmd, "the opening's outline could not be fitted"))?;
    let origin = axis.point + axis.direction * opening.front;
    let made =
        super::new_sketch(s, json!({"plane": {"origin": at(origin), "x_dir": at(opening.x), "y_dir": at(opening.y)}, "name": "Spoke Opening"}))?;
    let sketch = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the sketch was not made".into()))?;
    if s.model.state().sketch(sketch).is_none() {
        s.refresh();
    }
    let mut added = super::section::Added::default();
    if let Err(e) = super::super::sketch_store::update(s, sketch, |_, _, sk| super::section::draw(sk, &[(fitted, true)], &mut added)) {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    super::section::finish_dimensioned(s, true, None)?;
    let through = 2.0 * (opening.front - opening.back).abs() + 2.0 * tolerance;
    let cut = run(
        s,
        "solid.extrude",
        json!({"sketch": sketch, "distance": format!("{through} mm"), "direction": "symmetric", "operation": "cut", "targets": [solid], "name": "Spoke Opening"}),
    )?;
    let name = cut.get("name").and_then(Value::as_str).unwrap_or("Spoke Opening").to_owned();
    run(
        s,
        "solid.pattern.circular",
        json!({"features": [name], "axis": {"origin": at(axis.point), "dir": at(axis.direction)}, "count": count, "name": "Spoke Openings"}),
    )?;
    Ok(count)
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

/// Shell It: the body Reconstruct builds hollowed to the thickness of the scan's walls, each face
/// with no inner face behind it (an end of the main prism) left open.
pub(super) fn shell_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.shell_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let shell = view
        .reading()
        .shell
        .clone()
        .ok_or_else(|| bad(cmd, "Reconstruct reads no walls of one thickness open at an end: the part is solid, or its inside is not scanned"))?;
    let solid = built_body(s, &found.scan, p, cmd)?;
    let faces: Vec<[f64; 3]> = shell.points.iter().map(|q| at(*q)).collect();
    // Read as designed, as Extrude It reads its depth: 2 mm, not the 2.000021 the scan measures.
    let thickness = solidcraft_scan::hole_form::snapped(shell.thickness, solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance);
    let made = run(s, "solid.shell", json!({"faces": faces, "thickness": format!("{thickness} mm"), "body": solid, "name": "Shell It"}))
        .map_err(|e| super::refusal::refused(cmd, "the kernel could not hollow the body to the scan's walls", &e.to_string()))?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: None,
        feature: made,
        solid,
        operation: "shell".into(),
        lines: 0,
        arcs: 0,
        tangents: 0,
        depth: Some(thickness),
        count: None,
        openings: None,
        skin: None,
        allowance: None,
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
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

/// The latest cut of hole `index` Cut It or Hole It made ([`free_cut_name`]): the one Pattern It
/// repeats.
fn last_cut_name(s: &Session, index: usize) -> String {
    let names = [cut_name(index), hole_name(index)];
    let of = |name: &str| {
        names
            .iter()
            .any(|base| name == base || name.strip_prefix(base.as_str()).and_then(|k| k.strip_prefix('.')).is_some_and(|k| k.parse::<u32>().is_ok()))
    };
    s.doc.features.iter().rev().find(|f| of(&f.name)).map_or_else(|| names[0].clone(), |f| f.name.clone())
}

/// The name Hole It gives the hole of hole `index`.
fn hole_name(index: usize) -> String {
    format!("Hole It {}", index + 1)
}

/// A hole's form as the reply lists it.
fn hole_form(f: &solidcraft_scan::hole_form::Formed) -> ScanHoleForm {
    let kind = if f.counterbore.is_some() {
        "counterbore"
    } else if f.countersink.is_some() {
        "countersink"
    } else if f.thread.is_some() {
        "tapped"
    } else {
        "drilled"
    };
    ScanHoleForm {
        kind: kind.into(),
        diameter: 2.0 * f.radius,
        through: f.depth.is_none(),
        depth: f.depth,
        tip_angle: f.tip.map(f64::to_degrees),
        cb_diameter: f.counterbore.map(|c| 2.0 * c.0),
        cb_depth: f.counterbore.map(|c| c.1),
        cs_diameter: f.countersink.map(|c| 2.0 * c.0),
        cs_angle: f.countersink.map(|c| c.1.to_degrees()),
        thread: f.thread.map(str::to_owned),
        faces: f.faces.iter().copied().collect(),
    }
}

/// Where the line through `mouth` along `inward` (unit) first enters the body `solid`, from
/// outside it up to `reach` past the mouth, by the body's exact point classification: sampled
/// along the line, then halved down to the face. `None` when the line meets no material there.
fn entry_on_body(s: &Session, solid: &str, mouth: Vec3, inward: Vec3, reach: f64) -> Option<Vec3> {
    use solidcraft_kernel::PointClass;
    let state = s.model.state();
    let body = &state.body(solid)?.body;
    // A start outside the body, back along the axis from the mouth.
    let size = body.size().max(1.0);
    let mut back = reach.max(1.0);
    let start = loop {
        let a = mouth - inward * back;
        if body.classify_point(a).ok()? == PointClass::Outside {
            break a;
        }
        back *= 2.0;
        if back > 4.0 * size {
            return None;
        }
    };
    let end = mouth + inward * reach;
    const STEPS: usize = 64;
    let samples: Vec<Vec3> = (0..=STEPS).map(|k| start + (end - start) * (k as f64 / STEPS as f64)).collect();
    let classes = body.classify_points(&samples).ok()?;
    let first = classes.iter().position(|c| *c != PointClass::Outside).filter(|&i| i > 0)?;
    if classes[first] == PointClass::OnBoundary {
        return Some(samples[first]);
    }
    let (mut out, mut inside) = (samples[first - 1], samples[first]);
    for _ in 0..48 {
        let mid = (out + inside) * 0.5;
        match body.classify_point(mid).ok()? {
            PointClass::OnBoundary => return Some(mid),
            PointClass::Outside => out = mid,
            PointClass::Inside => inside = mid,
        }
    }
    Some((out + inside) * 0.5)
}

/// Hole It: a hole read as a Hole feature makes it, drilled with Hole at its mouth along its axis.
pub(super) fn hole_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.hole_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    let index = match p.get("hole").and_then(Value::as_u64) {
        Some(k) => k as usize,
        None => reading
            .holes
            .iter()
            .position(|h| h.form.is_some())
            .ok_or_else(|| bad(cmd, "Reconstruct reads no counterbore, countersink, blind or tapped hole: cut round holes with Cut It"))?,
    };
    let hole = reading
        .holes
        .get(index)
        .ok_or_else(|| bad(cmd, format!("there are {} holes: `hole` is 0 to {}", reading.holes.len(), reading.holes.len().saturating_sub(1))))?;
    let solid = built_body(s, &found.scan, p, cmd)?;
    // A plain round hole is drilled as it is, flat at the bottom, as deep as the scan has it.
    let plain;
    let form = match &hole.form {
        Some(f) => f,
        None => {
            plain = solidcraft_scan::hole_form::Formed {
                bore: hole.face,
                radius: hole.radius,
                mouth: hole.mouth,
                inward: (hole.bottom - hole.mouth).normalized().ok_or_else(|| bad(cmd, "the hole has no depth on the scan"))?,
                bottom: hole.bottom,
                counterbore: None,
                countersink: None,
                depth: Some(hole.depth()),
                tip: Some(std::f64::consts::PI),
                thread: None,
                faces: std::collections::BTreeSet::from([hole.face]),
            };
            &plain
        }
    };
    // The hole starts where its axis enters the body built so far: the mouth the scan shows can lie
    // a little above that body's face or inside it, and a hole must start on a face.
    let reach = form.depth.unwrap_or_else(|| hole.depth()).max(2.0 * form.radius);
    let start = entry_on_body(s, &solid, form.mouth, form.inward, reach)
        .ok_or_else(|| bad(cmd, format!("the hole's axis does not run into `{solid}` near its mouth on the scan: build the part it is in first")))?;
    let mut params = json!({"position": at(start), "direction": at(form.inward), "targets": [solid], "name": free_hole_name(s, index)});
    match bool_(p, "thread").unwrap_or(true).then_some(form.thread).flatten() {
        Some(thread) => params["thread"] = json!(thread),
        None => params["diameter"] = json!(format!("{} mm", 2.0 * form.radius)),
    }
    if let Some((r, depth)) = form.counterbore {
        params["type"] = json!("counterbore");
        params["cb_diameter"] = json!(format!("{} mm", 2.0 * r));
        params["cb_depth"] = json!(format!("{depth} mm"));
    } else if let Some((r, angle)) = form.countersink {
        params["type"] = json!("countersink");
        params["cs_diameter"] = json!(format!("{} mm", 2.0 * r));
        params["cs_angle"] = json!(format!("{} deg", angle.to_degrees()));
    }
    if let Some(depth) = form.depth {
        params["depth"] = json!(format!("{depth} mm"));
        if let Some(tip) = form.tip {
            params["tip_angle"] = json!(format!("{} deg", tip.to_degrees()));
        }
    }
    let feature = run(s, "solid.hole", params).map_err(|e| {
        let text = e.to_string();
        if text.contains("SMLib") || text.contains("solidcore") {
            super::refusal::refused(cmd, "the kernel could not drill the hole the scan shows", &text)
        } else {
            bad(cmd, text)
        }
    })?;
    reply(ScanBuildResult {
        body: found.scan.clone(),
        sketch: None,
        feature,
        solid,
        operation: "cut".into(),
        lines: 0,
        arcs: 0,
        tangents: 0,
        depth: Some(form.depth.unwrap_or_else(|| form.mouth.dist(form.bottom))),
        count: None,
        openings: None,
        skin: None,
        allowance: None,
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// The name for a new hole of hole `index`, as [`free_cut_name`] for a cut.
fn free_hole_name(s: &Session, index: usize) -> String {
    let base = hole_name(index);
    let taken = |n: &str| s.doc.features.iter().any(|f| f.name == n);
    if !taken(&base) {
        return base;
    }
    (2..).map(|k| format!("{base}.{k}")).find(|n| !taken(n)).unwrap_or(base)
}

/// `inward` (a hole's axis read off the scan, into the material) as the body's planar face at the
/// hole's `mouth` faces into it, when that face is within the scan's `tolerance` of the mouth and
/// the read axis so near its normal that, over the hole's `depth`, the two part by no more than
/// the tolerance: a short bore read through noise comes out a few tenths of a degree off its
/// face's normal (the scan benchmark's flange, its four bolt holes 0.1° to 0.6° off at noise
/// 0.1), which the scan cannot tell from square. Else `inward` as it is.
fn square_to_entry(s: &Session, solid: &str, mouth: Vec3, inward: Vec3, depth: f64, tolerance: f64) -> Vec3 {
    let state = s.model.state();
    let Some(body) = state.body(solid) else { return inward };
    let Some((distance, face)) = body.body.nearest_face(mouth) else { return inward };
    let Some(plane) = body.body.planar_face(face).filter(|_| distance <= tolerance) else { return inward };
    let Some(into) = plane.y.cross(plane.x).normalized() else { return inward };
    if inward.dot(into) > 0.0 && inward.cross(into).len() * depth <= tolerance { into } else { inward }
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
    // Along the grid's axis when it is that near it, as Hole It drills: a hole a few thousandths
    // of a degree off leaves the kernel ellipses it cannot convert within its tolerance.
    let inward = (hole.bottom - hole.mouth).normalized().ok_or_else(|| bad(cmd, "the hole has no depth on the scan"))?;
    let inward = solidcraft_scan::reconstruct::on_grid(square_to_entry(s, &solid, hole.mouth, inward, hole.depth(), view.outcome().tolerance));
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
    let round = solidcraft_scan::hole_form::SNAP_SHARE * view.outcome().tolerance;
    let sketch = sketch_outlines(s, &plane, &format!("Hole {}", index + 1), &[circle], &|_, _, _| None, &mut drawn, round, false)?;
    // The sketch's normal (x × y) points into the hole.
    // A hole read off what the body leaves unexplained is short at its ends: the wall's last
    // stretch beside each face of the body lies within the tolerance of that face and counts as
    // explained by it. So a hole whose bottom is within two tolerances of coming out of the body
    // goes through: cut to its bottom alone, it kept a skin a fraction of a millimetre thick.
    let skin = 2.0 * view.outcome().tolerance;
    let through = s
        .model
        .state()
        .body(&solid)
        .is_some_and(|b| matches!(b.body.classify_point(hole.bottom + inward * skin), Ok(solidcraft_kernel::PointClass::Outside)));
    let depth = hole.depth() + margin + if through { skin + margin } else { 0.0 };
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
        openings: None,
        skin: None,
        allowance: None,
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

pub(super) fn pattern_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.pattern_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let reading = view.reading();
    if let Some(k) = p.get("prism").and_then(Value::as_u64).map(|k| k as usize) {
        let repeat = reading
            .prism_repeats
            .iter()
            .find(|r| r.members.first() == Some(&k))
            .ok_or_else(|| bad(cmd, format!("prism {k} does not repeat: Reconstruct reads no row or ring of prisms alike starting with it")))?;
        let name = format!("Extrude It {}", k + 1);
        let feature_name = str_(p, "feature")
            .filter(|f| !f.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| s.doc.features.iter().rev().find(|f| f.name == name).map(|f| f.name.clone()))
            .ok_or_else(|| bad(cmd, format!("there is no `{name}`: extrude prism {k} first with Extrude It")))?;
        let solid = built_body(s, &found.scan, &json!({}), cmd)?;
        let count = repeat.places;
        let feature = repeat_feature(s, cmd, &feature_name, count, &repeat.layout)?;
        return reply(ScanBuildResult {
            body: found.scan.clone(),
            sketch: None,
            feature,
            solid,
            operation: "pattern".into(),
            lines: 0,
            arcs: 0,
            tangents: 0,
            depth: None,
            count: Some(count),
            openings: None,
            skin: None,
            allowance: None,
            changed: None,
            note: None,
            fit: None,
            param_warnings: Vec::new(),
        });
    }
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
    let layout = parallel_to_the_bore(s, &solid, hole.mouth, hole.radius, layout, view.outcome().tolerance);
    let pattern = repeat_feature(s, cmd, &feature_name, count, &layout)?;
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
        openings: None,
        skin: None,
        allowance: None,
        changed: None,
        note: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// A ring of holes turned about the axis of the bore cut for its first (the body's cylinder of
/// the hole's `radius` whose axis passes the hole's `mouth`, both within the scan's `tolerance`),
/// when that axis and the ring's read axis part by no more than the tolerance over the ring's
/// radius: a bolt circle's holes run parallel to its axis, and the ring's axis read from the
/// holes' places through noise is a few tenths of a degree off, which turned every copy that far
/// off square (the scan benchmark's flange at noise 0.1, its copies 0.35° to 0.49° off while
/// the hole they copy was square). Any other layout, or none such bore, as it is.
fn parallel_to_the_bore(s: &Session, solid: &str, mouth: Vec3, radius: f64, layout: Layout, tolerance: f64) -> Layout {
    let Layout::Circular { centre, axis, angle } = layout else { return layout };
    let state = s.model.state();
    let Some(body) = state.body(solid) else { return layout };
    let bore = body.body.record().faces.iter().find_map(|f| match &f.surface {
        solidcore_record::brep::Surface::Cylinder { frame, radius: r } if (r - radius).abs() <= tolerance => {
            let [x, y, o] = [frame.x_axis, frame.y_axis, frame.origin].map(|a| Vec3::new(a[0], a[1], a[2]));
            let along = x.cross(y).normalized()?;
            let off = mouth - o;
            ((off - along * off.dot(along)).len() <= tolerance).then_some(along)
        }
        _ => None,
    });
    let Some(bore) = bore else { return layout };
    let bore = if bore.dot(axis) < 0.0 { -bore } else { bore };
    let out = mouth - centre;
    let reach = (out - axis * out.dot(axis)).len();
    if bore.cross(axis).len() * reach <= tolerance { Layout::Circular { centre, axis: bore, angle } } else { layout }
}

/// Feature `feature_name` repeated `count` times as `layout` lays them out.
fn repeat_feature(s: &mut Session, cmd: &str, feature_name: &str, count: usize, layout: &Layout) -> Result<Value> {
    Ok(match *layout {
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
    })
}

/// Fillet It: the rounds along edges Reconstruct reads that no profile holds, each measured off the
/// scan at the edge of the built body it rounds (Fillet from Scan's measure, one edge at a time,
/// so one the scan shows sharp is set aside rather than refusing the rest). Edges running on
/// tangentially into each other (a prism's end, its outline of lines and arcs) are one chain, and
/// a round cannot stop part way along one, so each chain is filleted whole, at the radius measured
/// most often along it: one Fillet for each radius, all one undo step.
/// The point half way along a polyline by its length (a straight edge's line is its two ends,
/// and either end is where it meets the next edge).
fn halfway_along(line: &[Vec3]) -> Option<Vec3> {
    let total: f64 = line.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut left = 0.5 * total;
    for w in line.windows(2) {
        let step = w[0].dist(w[1]);
        if step >= left && step > 0.0 {
            return Some(w[0] + (w[1] - w[0]) * (left / step));
        }
        left -= step;
    }
    line.first().copied()
}

pub(super) fn fillet_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fillet_it";
    let view = view(s, p, cmd, false)?;
    let found = &view.found;
    let rounds = view.reading().fillets.clone();
    let solid = built_body(s, &found.scan, p, cmd)?;
    // Nothing to fillet is no failure: the body stays as it is, and the steps after this one in a
    // batch still run.
    let nothing = |solid: String, set_aside: usize, note: String| {
        reply(ScanFilletItResult {
            body: found.scan.clone(),
            solid,
            fillets: Vec::new(),
            set_aside,
            features: Vec::new(),
            changed: Some(false),
            note: Some(note),
            fit: None,
            param_warnings: Vec::new(),
        })
    };
    if rounds.is_empty() {
        return nothing(solid, 0, "Reconstruct reads no round along an edge that its other steps leave sharp".into());
    }
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
        // A point on the body's own edge: the one read off the scan's faces is where those faces
        // meet, which the body's edge may sit a little off, nearer another edge or farther than
        // Fillet's pick reaches (T-LESS 19: no edge there).
        let on_edge = surface.edges.get(e).and_then(|line| halfway_along(line)).map_or(edge, at);
        match chains.iter_mut().find(|c| c.0.contains(&e)) {
            Some(chain) => chain.2.push(measured.radius),
            None => chains.push((surface.tangent_chain(e, 2f64.to_radians()), on_edge, vec![measured.radius])),
        }
    }
    if chains.is_empty() {
        let note = format!("none of the {} rounds Reconstruct reads fits the scan better than a sharp edge of `{solid}` there", rounds.len());
        return nothing(solid, rounds.len(), note);
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
        changed: None,
        note: None,
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
    // Chamfers that read alike are one Chamfer of one distance, as drawn: each distance is taken
    // as designed (the roundest of a whole, a half and a tenth of a millimetre within a quarter
    // of the scan's tolerance, as a hole's sizes are), and those still within that reach of each
    // other share the least of them. Read to the micron instead, a scan's noise made every edge
    // of one chamfer its own feature, and the kernel refuses an edge once the edges it ends on
    // are cut.
    let reach = solidcraft_scan::hole_form::SNAP_SHARE * found.outcome.tolerance;
    let mut read: Vec<(f64, [f64; 3])> = chamfers.iter().map(|c| (solidcraft_scan::hole_form::snapped(c.distance, reach), at(c.edge))).collect();
    read.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut groups: Vec<(f64, Vec<[f64; 3]>)> = Vec::new();
    for (distance, edge) in read {
        match groups.last_mut() {
            Some((kept, edges)) if distance - *kept <= reach => edges.push(edge),
            _ => groups.push((distance, vec![edge])),
        }
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
    for (distance, edges) in groups {
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
    fitted_hole_it => hole_it,
    fitted_loft_it => loft_it,
    fitted_pipe_it => pipe_it,
    fitted_tubes_it => tubes_it,
    fitted_coil_it => coil_it,
    fitted_pattern_it => pattern_it,
    fitted_fillet_it => fillet_it,
    fitted_chamfer_it => chamfer_it,
    fitted_shell_it => shell_it,
);

#[path = "scan_reconstruct_shapes.rs"]
mod shapes;
use shapes::{coil_it, loft_it, pipe_it, tubes_it};

#[path = "scan_reconstruct_rest.rs"]
mod rest;
