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
use solidcraft_scan::reconstruct::{self, Layout, Reading};
use solidcraft_scan::segment::{self, Outcome};
use solidcraft_sketch::{ConstraintKind, Sketch};

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::schema::scan_reconstruct::{
    ScanBuildResult, ScanCutItParams, ScanExtrudeItParams, ScanFindAllParams, ScanFindAllResult, ScanHole, ScanPatternItParams, ScanPrism,
    ScanReconstructParams, ScanReconstructResult, ScanRepeat, ScanRevolveItParams, ScanShapeFound, ScanTurned,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const FIND_ALL_CONTRACT: TypedContract<ScanFindAllParams> = TypedContract::new();
const RECONSTRUCT_CONTRACT: TypedContract<ScanReconstructParams> = TypedContract::new();
const EXTRUDE_IT_CONTRACT: TypedContract<ScanExtrudeItParams> = TypedContract::new();
const REVOLVE_IT_CONTRACT: TypedContract<ScanRevolveItParams> = TypedContract::new();
const CUT_IT_CONTRACT: TypedContract<ScanCutItParams> = TypedContract::new();
const PATTERN_IT_CONTRACT: TypedContract<ScanPatternItParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.find_all", "Find All", crate::checked!("scan.find_all", find_all))
        .at("SCAN", "RECONSTRUCT")
        .icon("colored_components")
        .noundo()
        .contract(&FIND_ALL_CONTRACT)
        .output(output_schema_for::<ScanFindAllResult>)
        .params("body?: mesh body (default: the selected or only one); show?: bool (false takes the colouring away): the whole scan cut into regions, each a plane, cylinder, sphere, cone or torus or freeform (the scan's noise sets how close a shape must lie), coloured in the viewport; a view and not an undo step → shapes [{index, kind, area, triangles, rms, point, direction?, radius?, minor?, half_angle?, hole?}], freeform, freeform_area, in_shapes, noise, tolerance"),
    CommandSpec::new("scan.reconstruct", "Reconstruct", crate::checked!("scan.reconstruct", read_scan))
        .at("SCAN", "RECONSTRUCT")
        .icon("auto_constrain")
        .noundo()
        .contract(&RECONSTRUCT_CONTRACT)
        .output(output_schema_for::<ScanReconstructResult>)
        .params("body?: mesh body (default: the selected or only one): what Find All's shapes make (it runs Find All first when it has not run on this scan): prisms (stretches of one section along a direction a plane faces), a turned part (most of the shapes about one axis), repeats (in a row, round an axis, in a grid, drilled along the radius) and the round holes no profile holds; reads only → shapes, in_shapes, prisms [{index, direction, middle, depth, walls, ends, open}], turned? {point, direction, faces, share}, holes [{index, shape, radius, diameter, mouth, bottom, depth, repeat?, layout?}], repeats, built?, proposals [{command, params}], note?"),
    CommandSpec::new("scan.extrude_it", "Extrude It", crate::checked!("scan.extrude_it", extrude_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("extrude")
        .contract(&EXTRUDE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; prism?: index of scan.reconstruct's prisms (default 0); into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the prism's profile sketched halfway along it off its walls exactly as fitted (lines and arcs from crossing to crossing, joins held tangent) and extruded symmetric to its depth, one undo step → sketch, feature, solid, operation, lines, arcs, tangents, depth"),
    CommandSpec::new("scan.revolve_it", "Revolve It", crate::checked!("scan.revolve_it", revolve_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("revolve")
        .contract(&REVOLVE_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; into?: body to join (default: `<scan> Reconstructed`, made new when there is none): the turned part's profile sketched on a plane through its axis, each face held to the axis (cylinders level, ends upright, rounds tangent), and revolved all the way round, one undo step → sketch, feature, solid, operation, lines, arcs, tangents"),
    CommandSpec::new("scan.cut_it", "Cut It", crate::checked!("scan.cut_it", cut_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("hole")
        .contract(&CUT_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; hole?: index of scan.reconstruct's holes (default 0); into?: body to cut (default: `<scan> Reconstructed`): a circle sketched square to the hole outside its mouth and cut as deep as the scan has the hole, one undo step → sketch, feature, solid, operation, depth"),
    CommandSpec::new("scan.pattern_it", "Pattern It", crate::checked!("scan.pattern_it", pattern_it))
        .at("SCAN", "RECONSTRUCT")
        .icon("pattern_circ")
        .contract(&PATTERN_IT_CONTRACT)
        .output(output_schema_for::<ScanBuildResult>)
        .params("body?: mesh body; hole?: index of scan.reconstruct's holes, one that repeats (default: the first that does); feature?: the cut to repeat (default: the one Cut It made for that hole): the cut repeated as the row or ring of holes is, with its count and spacing, one undo step → feature, solid, count"),
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

    /// The same, shown or not.
    fn with_shown(&self, shown: bool) -> ScanShapes {
        ScanShapes {
            scan: self.scan.clone(),
            fingerprint: self.fingerprint,
            held: self.held,
            outcome: self.outcome.clone(),
            reading: self.reading.clone(),
            regions: self.regions.clone(),
            shown,
        }
    }
}

fn held(positions: &[Vec3], triangles: &[[u32; 3]]) -> [usize; 4] {
    [positions.as_ptr() as usize, positions.len(), triangles.as_ptr() as usize, triangles.len()]
}

/// A cheap fingerprint of a mesh: its sizes and every position's and triangle's bits, folded.
fn fingerprint(positions: &[Vec3], triangles: &[[u32; 3]]) -> u64 {
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

/// Find All on the scan `p` names (or the selected or only one), kept in the session: what was
/// found before on the same mesh is used again.
fn shapes(s: &mut Session, p: &Value, cmd: &str, show: bool) -> Result<Arc<ScanShapes>> {
    let name = super::mesh_body_name(s, str_(p, "body").filter(|b| !b.trim().is_empty()), cmd)?;
    if let Some(known) = s.scan_shapes.clone().filter(|k| k.scan == name && k.current(s)) {
        if known.shown || !show {
            return Ok(known);
        }
        let shown = Arc::new(known.with_shown(true));
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
        let outcome = segment::find_all(&scan, &segment::Options::default(), &|| false);
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
    let found = Arc::new(ScanShapes { scan: name, fingerprint: print, held: at, outcome, reading, regions, shown: show });
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
            s.scan_shapes = Some(Arc::new(known.with_shown(false)));
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
}

impl Proposal {
    pub fn command(&self) -> &'static str {
        match self {
            Proposal::Revolve => "scan.revolve_it",
            Proposal::Extrude(_) => "scan.extrude_it",
            Proposal::Cut(_) => "scan.cut_it",
            Proposal::Pattern(_) => "scan.pattern_it",
        }
    }

    /// Its command's parameters, on scan `body`.
    pub fn params(&self, body: &str) -> Value {
        match *self {
            Proposal::Revolve => json!({"body": body}),
            Proposal::Extrude(k) => json!({"body": body, "prism": k}),
            Proposal::Cut(k) | Proposal::Pattern(k) => json!({"body": body, "hole": k}),
        }
    }
}

/// What Reconstruct proposes, in the order to build it: the turned part, else each prism, the
/// main body first; then each hole cut and, where it repeats, patterned.
pub fn proposals(reading: &Reading) -> Vec<Proposal> {
    let mut out = Vec::new();
    if reading.revolve.is_some() {
        out.push(Proposal::Revolve);
    }
    out.extend((0..reading.prisms.len()).map(Proposal::Extrude));
    for (k, hole) in reading.holes.iter().enumerate() {
        out.push(Proposal::Cut(k));
        if hole.repeat.is_some() {
            out.push(Proposal::Pattern(k));
        }
    }
    out
}

/// The body Reconstruct builds on scan `scan`.
fn built_name(scan: &str) -> String {
    format!("{scan} Reconstructed")
}

fn layout_name(layout: &Layout) -> &'static str {
    match layout {
        Layout::Linear { .. } => "linear",
        Layout::Circular { .. } => "circular",
        Layout::Grid { .. } => "grid",
    }
}

pub(super) fn read_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.reconstruct";
    let found = shapes(s, p, cmd, true)?;
    let reading = &found.reading;
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
    let proposals = proposals(reading).iter().map(|p| json!({"command": p.command(), "params": p.params(&body)})).collect();
    let note = if reading.faces.is_empty() {
        Some("Find All found no shape on the scan: it is all freeform".to_string())
    } else if reading.prisms.is_empty() && reading.revolve.is_none() {
        Some("the shapes make no prism and no turned part: build from Fit Plane, Fit Cylinder and Section to Sketch, or with free forms".to_string())
    } else if found.outcome.in_shapes < 0.5 {
        Some(format!("only {:.0}% of the scan is in shapes: the rest is freeform, for free forms", 100.0 * found.outcome.in_shapes))
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

/// Outline `outline` drawn into the sketch: its corners shared by the pieces either side, each
/// line a line and each arc an arc, smooth joins held tangent. `level` holds each piece on a face
/// level (along the sketch's X) or upright, by its face: for a turned profile.
fn draw(sk: &mut Sketch, outline: &Outline, level: &dyn Fn(Option<usize>) -> Option<bool>, drawn: &mut Drawn) -> Result<()> {
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
                match level(outline.faces.get(k).copied().flatten()) {
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
fn sketch_outlines(
    s: &mut Session,
    plane: &Plane,
    name: &str,
    outlines: &[Outline],
    level: &dyn Fn(Option<usize>) -> Option<bool>,
    drawn: &mut Drawn,
) -> Result<u64> {
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
    let found = shapes(s, p, cmd, false)?;
    let reading = &found.reading;
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
        profile::prism_profile(&Scan::new(positions, triangles), &found.outcome, reading, prism).map_err(|e| bad(cmd, e))?
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    let mut drawn = Drawn::default();
    let sketch = sketch_outlines(s, &plane, &format!("Prism Profile {}", index + 1), &outlines, &|_| None, &mut drawn)?;
    let depth = prism.depth();
    let extrude =
        merged(json!({"sketch": sketch, "distance": depth, "direction": "symmetric", "name": format!("Extrude It {}", index + 1)}), operation);
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
        param_warnings: Vec::new(),
    })
}

pub(super) fn revolve_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.revolve_it";
    let found = shapes(s, p, cmd, false)?;
    let (plane, outline, held) = {
        let state = s.model.state();
        let (positions, triangles) =
            state.body(&found.scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{}` is not a scan", found.scan)))?;
        profile::turned_profile(&Scan::new(positions, triangles), &found.outcome, &found.reading).map_err(|e| bad(cmd, e))?
    };
    let (operation, solid, how) = into_params(s, &found.scan, p, cmd)?;
    // A cylinder's piece runs level along the axis, an end's upright, the axis's own along it.
    let level = |face: Option<usize>| match face.and_then(|k| held.get(&k)) {
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

pub(super) fn cut_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.cut_it";
    let found = shapes(s, p, cmd, false)?;
    let reading = &found.reading;
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
    let sketch = sketch_outlines(s, &plane, &format!("Hole {}", index + 1), &[circle], &|_| None, &mut drawn)?;
    // The sketch's normal (x × y) points into the hole.
    let depth = hole.depth() + margin;
    let feature =
        run(s, "solid.extrude", json!({"sketch": sketch, "distance": depth, "operation": "cut", "targets": [solid], "name": cut_name(index)}))?;
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
        param_warnings: Vec::new(),
    })
}

pub(super) fn pattern_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.pattern_it";
    let found = shapes(s, p, cmd, false)?;
    let reading = &found.reading;
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
    let feature_name = str_(p, "feature").filter(|f| !f.trim().is_empty()).map_or_else(|| cut_name(index), str::to_owned);
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
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_reconstruct_tests.rs"]
mod tests;
