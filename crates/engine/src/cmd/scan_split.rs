//! Split Mesh (`scan.split`, #8615), after SurfaceCraft's Split: a scan cut along a cutter (a
//! plane, a fitted shape, another body or scan, a 3D curve drawn on it, a sketch's outlines
//! carried through space). The scan keeps one side and, with Both, a new scan takes the other; an
//! envelope's slab between two cuts is a scan of its own, ready to delete. Or only the crossing is
//! kept, as 3D sketch curves, the scan untouched. A scan is changed in its Insert Mesh feature and
//! a new one is an Insert Mesh of its own, so a design opens anywhere.

use serde_json::{Value, json};
use solidcore_record::{Region2, Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::curve::{self, Laid};
use solidcraft_scan::fit::{Kind, Shape};
use solidcraft_scan::split::{self, Cut, Cutter, Sides};

use super::{CommandSpec, Mesh, mesh};
use crate::params::{bad, str_};
use crate::schema::scan_split::{ScanSplitMade, ScanSplitParams, ScanSplitResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const SPLIT_CONTRACT: TypedContract<ScanSplitParams> = TypedContract::new();

/// The 3D sketch the crossing goes in when only the curves are kept.
pub const SKETCH_NAME: &str = "Split Curves";

/// How far a kept crossing curve may stray from the cut.
const CURVE_TOLERANCE: f64 = 0.01;

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.split", "Split Mesh", crate::checked!("scan.split", split_scan))
    .at("SCAN", "CLEAN")
    .icon("split_body")
    .contract(&SPLIT_CONTRACT)
    .output(output_schema_for::<ScanSplitResult>)
    .params("body?: the scan (default: the selected or only one); one cutter: plane: XY|XZ|YZ|plane name|{origin, normal} | shape: {kind: plane|cylinder|sphere|cone|torus, then at: [x,y,z] on the scan (fitted there, as the Fit tools fit it) or point|center|apex, normal|axis, radius, half_angle (deg), major, minor} | cutter: a body (solid, surface or another scan) | curve: {sketch, curve} or {points, closed?} (a 3D curve on the scan, carried square to its own plane) | sketch: id|name (its closed outlines, or one open curve, square to its plane) | selection: true (parted along the edge of what is selected on the scan; with both, the selection becomes a scan of its own); keep?: both (default: the scan keeps the positive side, in front, outside or left, and a new scan `<scan> <side>` the other) | front | back | curves (the crossing kept as 3D sketch curves in `Split Curves`, the scan untouched); envelope?: thickness (two cuts half of it either side, the slab between a scan of its own with both, taken off with one side); offset?: length (a curve's cut reaches only this far from it, a sketch's only this far either side of its plane; default 0: right through); facing?: [x,y,z] (only triangles facing that way are cut): one undo step; a cut that misses the scan, or only slits it, is refused → body, cutter, keep, sides, triangles, made [{body, side, triangles, feature}], crossed, length, runs, closed_runs, chord_error, carried_on, sketch?, curves?, feature?")];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// A length parameter that may be 0 (`default` when not given).
fn length_or_zero(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<f64> {
    let Some(e) = crate::params::expr(p, key) else { return Ok(0.0) };
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !(v.is_finite() && v >= 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a length of 0 or more")));
    }
    Ok(v)
}

/// What to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// The positive side in the scan, the negative in a new scan.
    Both,
    /// One side: +1 or -1.
    Side(i8),
    /// Only the crossing, as 3D sketch curves.
    Curves,
}

impl Keep {
    pub fn parse(keep: Option<&str>, cmd: &str) -> Result<Keep> {
        Ok(match keep.unwrap_or("both") {
            "both" => Keep::Both,
            "front" | "in_front" | "outside" | "left" | "rest" => Keep::Side(1),
            "back" | "behind" | "inside" | "right" | "selected" => Keep::Side(-1),
            "curves" => Keep::Curves,
            other => return Err(bad(cmd, format!("`keep` is both, front, back or curves, not `{other}`"))),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Keep::Both => "both",
            Keep::Side(1) => "front",
            Keep::Side(_) => "back",
            Keep::Curves => "curves",
        }
    }
}

/// What a split would do: the scan, what cut it, the sides' names and the cut itself, for a
/// dialog's preview and for the command.
#[derive(Clone, Debug)]
pub struct Planned {
    pub body: String,
    pub cutter: String,
    pub sides: Sides,
    pub cut: Cut,
}

/// The cutter `p` names, and what to call it. `on` is the scan, for a curve's facing.
fn cutter(s: &Session, p: &Value, on: &Mesh, cmd: &str) -> Result<(Cutter, String)> {
    let given: Vec<&str> = ["plane", "shape", "cutter", "curve", "sketch"].into_iter().filter(|k| p.get(*k).is_some_and(|v| !v.is_null())).collect();
    let offset = length_or_zero(s, p, "offset", cmd)?;
    match given.as_slice() {
        ["plane"] => {
            let plane = p.get("plane").cloned().unwrap_or(Value::Null);
            let pref = super::super::features::plane_param(s, Some(&plane), cmd)?;
            let (vals, _) = s.doc.param_values();
            let state = s.model.state();
            let pl = s.doc.resolve_plane_in(&vals, &pref, 0, s.active_component, Some(&state)).map_err(|e| bad(cmd, e.to_string()))?;
            let name = match &plane {
                Value::String(n) => format!("plane {n}"),
                _ => "plane".to_owned(),
            };
            Ok((Cutter::shape(Shape::Plane { point: pl.origin, normal: pl.normal() }), name))
        }
        ["shape"] => {
            let v = p.get("shape").unwrap_or(&Value::Null);
            let shape = match v.get("at").filter(|a| !a.is_null()) {
                Some(at) => fitted_at(on, v, at, cmd)?,
                None => shape_param(v, cmd)?,
            };
            let name = match shape {
                Shape::Plane { .. } => "plane",
                Shape::Cylinder { .. } => "cylinder",
                Shape::Sphere { .. } => "sphere",
                Shape::Cone { .. } => "cone",
                Shape::Torus { .. } => "torus",
            };
            Ok((Cutter::shape(shape), name.to_owned()))
        }
        ["cutter"] => {
            let name = str_(p, "cutter").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "`cutter` must name a body"))?;
            if name == on.body {
                return Err(bad(cmd, "`cutter` names the scan itself"));
            }
            let state = s.model.state();
            let body = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
            let shown = if body.body.is_mesh() { super::shown_mesh(s, name) } else { None };
            let cut = match shown.as_deref() {
                Some(m) => Cutter::mesh(&m.positions, &m.triangles),
                None => {
                    // Meshed finely against the scan: a hundredth of a millimetre off at most.
                    let surface = body
                        .body
                        .mesh(&solidcraft_kernel::MeshTolerance::chord(0.01))
                        .map_err(|e| bad(cmd, format!("`{name}` could not be meshed: {e}")))?;
                    Cutter::mesh(&surface.positions, &surface.triangles)
                }
            }
            .map_err(|e| bad(cmd, e.to_string()))?;
            Ok((cut, format!("body {name}")))
        }
        ["curve"] => {
            let (points, closed, name) = curve_param(s, p.get("curve").unwrap_or(&Value::Null), cmd)?;
            let facing = facing_under(s, on, &points, cmd)?;
            let reach = offset.max(on.scan().extent() / 10.0);
            let made = Cutter::curve(&points, closed, facing, reach).map_err(|e| bad(cmd, e.to_string()))?;
            let made = if offset > 0.0 {
                let mut path = points.clone();
                if closed && let Some(&first) = points.first() {
                    path.push(first);
                }
                made.ribbon(&path, offset)
            } else {
                made
            };
            Ok((made, name))
        }
        ["sketch"] => {
            let id = super::super::sketch_store::target(s, p, cmd)?;
            let state = s.model.state();
            let solved = state.sketch(id).ok_or_else(|| bad(cmd, "the sketch has not been solved"))?;
            let plane = solved.plane;
            let regions: Vec<Region2> = solved.profiles.iter().map(|pr| pr.region.clone()).collect();
            let open = if regions.is_empty() { Some(open_chain(&solved.sketch, cmd)?) } else { None };
            let made = Cutter::sketch(plane.origin, plane.x, plane.y, &regions, open.as_deref(), on.scan().extent() / 10.0)
                .map_err(|e| bad(cmd, e.to_string()))?;
            let made = if offset > 0.0 { made.slab(plane.origin, plane.normal(), offset) } else { made };
            Ok((made, format!("sketch {}", solved.name)))
        }
        [] => Err(bad(cmd, "give a cutter: plane, shape, cutter (a body), curve or sketch")),
        _ => Err(bad(cmd, format!("give one cutter, not {}", given.join(" and ")))),
    }
}

/// A `shape` parameter.
fn shape_param(v: &Value, cmd: &str) -> Result<Shape> {
    let point = |k: &str| v.get(k).and_then(crate::params::vec3).ok_or_else(|| bad(cmd, format!("`shape.{k}` must be [x, y, z]")));
    let unit = |k: &str| point(k)?.normalized().ok_or_else(|| bad(cmd, format!("`shape.{k}` has no direction")));
    let positive = |k: &str| {
        v.get(k)
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x > 0.0)
            .ok_or_else(|| bad(cmd, format!("`shape.{k}` must be a length above 0")))
    };
    Ok(match v.get("kind").and_then(Value::as_str) {
        Some("plane") => Shape::Plane { point: point("point")?, normal: unit("normal")? },
        Some("cylinder") => Shape::Cylinder { point: point("point")?, axis: unit("axis")?, radius: positive("radius")? },
        Some("sphere") => Shape::Sphere { center: point("center")?, radius: positive("radius")? },
        Some("cone") => {
            let half = v
                .get("half_angle")
                .and_then(Value::as_f64)
                .filter(|a| *a > 0.0 && *a < 90.0)
                .ok_or_else(|| bad(cmd, "`shape.half_angle` must be an angle in degrees between 0 and 90"))?;
            Shape::Cone { apex: point("apex")?, axis: unit("axis")?, half_angle: half.to_radians() }
        }
        Some("torus") => Shape::Torus { center: point("center")?, axis: unit("axis")?, major: positive("major")?, minor: positive("minor")? },
        _ => return Err(bad(cmd, "`shape.kind` must be plane, cylinder, sphere, cone or torus")),
    })
}

/// The shape of `v.kind` fitted to the scan round the point `at` on it, as the Fit tools fit one.
fn fitted_at(on: &Mesh, v: &Value, at: &Value, cmd: &str) -> Result<Shape> {
    let point = crate::params::vec3(at).ok_or_else(|| bad(cmd, "`shape.at` must be a point [x, y, z] on the scan"))?;
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some("plane") => Kind::Plane,
        Some("cylinder") => Kind::Cylinder,
        Some("sphere") => Kind::Sphere,
        Some("cone") => Kind::Cone,
        Some("torus") => Kind::Torus,
        _ => return Err(bad(cmd, "`shape.kind` must be plane, cylinder, sphere, cone or torus")),
    };
    let scan = on.scan();
    let seed = solidcraft_scan::base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = solidcraft_scan::fit::fit(&scan, seed, kind, None).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(fitted.shape)
}

/// A `curve` parameter's points (in the design), whether it closes, and its name.
fn curve_param(s: &Session, v: &Value, cmd: &str) -> Result<(Vec<Vec3>, bool, String)> {
    if let Some(list) = v.get("points").and_then(Value::as_array) {
        let points: Vec<Vec3> =
            list.iter().map(|q| crate::params::vec3(q).ok_or_else(|| bad(cmd, "a curve point must be [x, y, z]"))).collect::<Result<_>>()?;
        let closed = v.get("closed").and_then(Value::as_bool).unwrap_or(false);
        return Ok((points, closed, "curve".into()));
    }
    let id = super::super::sketch_store::target(s, v, cmd)?;
    let wire = v.get("curve").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`curve.curve` must be a 3D sketch curve's id"))?;
    let state = s.model.state();
    let solved = state.sketch(id).ok_or_else(|| bad(cmd, "the sketch has not been solved"))?;
    let w = solved.sketch.wires.iter().find(|w| w.id == wire).ok_or_else(|| bad(cmd, format!("`{}` has no 3D curve `{wire}`", solved.name)))?;
    let points = w.pts.clone();
    let closed = points.len() > 3 && points.first().zip(points.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-6);
    Ok((points, closed, format!("curve {wire}")))
}

/// The way the scan faces under a curve: its triangles' normals nearest the curve's points,
/// added up (turned to agree with the first).
fn facing_under(s: &Session, on: &Mesh, points: &[Vec3], cmd: &str) -> Result<Vec3> {
    let (_, projector) = super::curve::projector(s, Some(&on.body), cmd)?;
    let mut sum = Vec3::ZERO;
    for &q in points.iter().step_by(points.len() / 200 + 1) {
        if let Some(n) = projector.normal(q) {
            sum += if sum.dot(n) < 0.0 { -n } else { n };
        }
    }
    sum.normalized().ok_or_else(|| bad(cmd, "the scan under the curve has no way it faces"))
}

/// A sketch's one open curve, its curves joined end to end, in the sketch's plane.
fn open_chain(sk: &solidcraft_sketch::Sketch, cmd: &str) -> Result<Vec<Vec2>> {
    let mut pieces: Vec<Vec<Vec2>> = Vec::new();
    for (i, c) in sk.curves.iter().enumerate() {
        if c.construction {
            continue;
        }
        let mut piece: Vec<Vec2> = Vec::new();
        for seg in sk.segs(i) {
            let steps = match seg {
                solidcore_record::Seg2::Line { .. } => 1,
                solidcore_record::Seg2::Arc { sweep, .. } => ((sweep.abs() / std::f64::consts::TAU * 96.0).ceil() as usize).clamp(4, 96),
                _ => 32,
            };
            for k in 0..=steps {
                let q = seg.point_at(k as f64 / steps as f64);
                if piece.last().is_none_or(|last| last.dist(q) > 1e-12) {
                    piece.push(q);
                }
            }
        }
        if piece.len() >= 2 {
            pieces.push(piece);
        }
    }
    let Some(mut line) = pieces.pop() else {
        return Err(bad(cmd, "the sketch has no curve to cut with: draw lines or arcs, or a closed outline"));
    };
    // Join the rest on at either end.
    let near = 1e-6;
    while !pieces.is_empty() {
        let (Some(&head), Some(&tail)) = (line.first(), line.last()) else { break };
        let found = pieces.iter().enumerate().find_map(|(i, piece)| {
            let (a, b) = (*piece.first()?, *piece.last()?);
            [(tail.dist(a), i, false, false), (tail.dist(b), i, false, true), (head.dist(b), i, true, false), (head.dist(a), i, true, true)]
                .into_iter()
                .find(|(d, ..)| *d <= near)
        });
        let Some((_, i, at_head, reverse)) = found else {
            return Err(bad(cmd, format!("the sketch has {} separate curves: cut with one curve, or with closed outlines", pieces.len() + 1)));
        };
        let mut piece = pieces.swap_remove(i);
        if reverse {
            piece.reverse();
        }
        if at_head {
            piece.pop();
            piece.extend(line);
            line = piece;
        } else {
            line.extend(piece.into_iter().skip(1));
        }
    }
    Ok(line)
}

/// What `p` would do to the scan, before anything changes.
pub fn plan(s: &Session, p: &Value, cmd: &str) -> Result<Planned> {
    if crate::params::bool_(p, "selection") == Some(true) {
        // Parted along the edge of what is selected on the scan.
        let sel =
            super::select::selected(s).filter(|k| k.count > 0).ok_or_else(|| bad(cmd, "nothing is selected on a scan: select part of it first"))?;
        let on = mesh(s, &json!({ "body": sel.scan }), cmd)?;
        let others = ["plane", "shape", "cutter", "curve", "sketch"].into_iter().filter(|k| p.get(*k).is_some_and(|v| !v.is_null())).count();
        if others > 0 {
            return Err(bad(cmd, "give one cutter: the selection, or another"));
        }
        let cut = split::by_triangles(&on.scan(), &sel.selected).map_err(|e| match e {
            // The selection's edge is only the rims of whole pieces (a floating bit picked, or all of it).
            split::SplitError::Misses => bad(
                cmd,
                "what is selected has no edge inside the scan to part it along: it is whole pieces of the scan, or all of it (Remove Floating Pieces takes small pieces off)",
            ),
            e => bad(cmd, e.to_string()),
        })?;
        return Ok(Planned { body: on.body, cutter: "selection".into(), sides: Sides::SelectedRest, cut });
    }
    let on = mesh(s, p, cmd)?;
    let (cutter, name) = cutter(s, p, &on, cmd)?;
    let envelope = length_or_zero(s, p, "envelope", cmd)?;
    let facing = match p.get("facing").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).and_then(|w| w.normalized()).ok_or_else(|| bad(cmd, "`facing` must be a direction [x, y, z]"))?),
        None => None,
    };
    let sides = cutter.sides;
    let cut = split::split(&on.scan(), cutter, envelope, facing).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(Planned { body: on.body, cutter: name, sides, cut })
}

/// A body name not yet used: `base`, else `base 2`, `base 3`, …
fn unused(s: &Session, base: &str) -> String {
    let state = s.model.state();
    (1..).map(|k| if k == 1 { base.to_owned() } else { format!("{base} {k}") }).find(|n| state.body(n).is_none()).unwrap_or_else(|| base.to_owned())
}

/// A side's name as a body's: `in_front` → `In Front`.
fn titled(side: &str) -> String {
    side.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The colour a piece split off is drawn in: left the scan's own grey, the two sides of a cut lay
/// together as if one, and where one opens onto the other the scan looked broken.
const SPLIT_COLOR: [f32; 3] = [0.50, 0.66, 0.86];

/// A new scan of `positions` and `triangles`, where they lie, as an Insert Mesh of its own, in
/// [`SPLIT_COLOR`].
fn new_scan(s: &mut Session, name: &str, from: &str, positions: &[Vec3], triangles: &[[u32; 3]]) -> Result<u64> {
    let data = solidcraft_doc::MeshData {
        name: name.to_owned(),
        positions: positions.iter().flat_map(|q| [q.x, q.y, q.z]).collect::<Vec<f64>>().into(),
        triangles: triangles.iter().flatten().copied().collect::<Vec<u32>>().into(),
        corner_normals: Default::default(),
        color: Some(SPLIT_COLOR),
    };
    let kind = solidcraft_doc::FeatureKind::MeshImport { file: format!("{from} (split)"), meshes: vec![data] };
    let made = super::super::features::add_feature(s, &json!({ "body_name": name, "name": "Split Mesh" }), kind)?;
    made.get("feature")
        .and_then(Value::as_u64)
        .or_else(|| made.get("id").and_then(Value::as_u64))
        .ok_or_else(|| EngineError::Other("the new scan's feature was not made".into()))
}

/// The crossing's runs as 3D sketch splines, each within the tolerance of the cut, in a 3D sketch
/// of their own.
fn keep_curves(s: &mut Session, cut: &Cut, cmd: &str) -> Result<(Value, Vec<String>)> {
    if s.active_sketch.is_some() {
        return Err(bad(cmd, "finish the sketch being edited first"));
    }
    let runs: Vec<(Vec<Vec3>, bool)> = cut.runs().into_iter().filter(|(points, _)| points.len() >= 2).collect();
    if runs.is_empty() {
        return Err(bad(cmd, "the cut has no crossing to keep"));
    }
    let made = run(s, "sketch.create", json!({"plane": "XY", "name": SKETCH_NAME}))?;
    let sketch = made.get("sketch").cloned().ok_or_else(|| bad(cmd, "the sketch for the curves was not made"))?;
    run(s, "sketch.options", json!({"sketch": sketch, "sketch_3d": true}))?;
    let mut curves = Vec::new();
    let drawn = (|| -> Result<()> {
        for (points, closed) in &runs {
            let fitted = curve::fit(&laid(points, *closed), CURVE_TOLERANCE);
            let fit: Vec<[f64; 3]> = fitted.fit.iter().map(|q| [q.x, q.y, q.z]).collect();
            let made = run(s, "sketch.spline3d", json!({"sketch": sketch, "points": fit}))?;
            curves.extend(made.get("curves").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
        }
        Ok(())
    })();
    run(s, "sketch.finish", json!({}))?;
    drawn?;
    Ok((sketch, curves))
}

/// A run of the cut as a line laid on the scan, its ends (and two points between, when it
/// closes) the picks the fit keeps.
fn laid(points: &[Vec3], closed: bool) -> Laid {
    let mut path = points.to_vec();
    let n = path.len();
    if closed && let Some(&first) = points.first() {
        path.push(first);
    }
    let at: Vec<usize> = if closed && n >= 3 { vec![0, n / 3, 2 * n / 3, n] } else { vec![0, path.len() - 1] };
    let picks = at.iter().filter_map(|&i| path.get(i).copied()).collect();
    Laid { picks, path, at, closed }
}

pub(super) fn split_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.split";
    let keep = Keep::parse(str_(p, "keep"), cmd)?;
    let planned = plan(s, p, cmd)?;
    let (negative, positive) = planned.sides.names();
    let cut = &planned.cut;
    let runs = cut.runs();
    let mut made: Vec<ScanSplitMade> = Vec::new();
    let (mut feature, mut sketch, mut curves) = (None, None, Vec::new());
    let triangles: usize;
    match keep {
        Keep::Curves => {
            let (id, ids) = keep_curves(s, cut, cmd)?;
            sketch = Some(id);
            curves = ids;
            triangles = s.model.state().body(&planned.body).and_then(|b| b.body.triangle_mesh()).map_or(0, |(_, t)| t.len());
        }
        Keep::Side(side) => {
            let (positions, kept) = cut.keep(&[side]);
            if kept.is_empty() {
                return Err(bad(cmd, format!("nothing of the scan is {}", if side > 0 { positive } else { negative })));
            }
            triangles = kept.len();
            feature = Some(super::clean::replace_mesh(s, &planned.body, &positions, &kept, cmd)?);
        }
        Keep::Both => {
            let (positions, kept) = cut.keep(&[1]);
            let (other, other_kept) = cut.keep(&[-1]);
            let (slab, slab_kept) = cut.keep(&[0]);
            if kept.is_empty() || other_kept.is_empty() {
                return Err(bad(cmd, "the cut leaves nothing on one side: keep the side there is"));
            }
            triangles = kept.len();
            feature = Some(super::clean::replace_mesh(s, &planned.body, &positions, &kept, cmd)?);
            let name = unused(s, &format!("{} {}", planned.body, titled(negative)));
            let f = new_scan(s, &name, &planned.body, &other, &other_kept)?;
            made.push(ScanSplitMade { body: name, side: negative.to_owned(), triangles: other_kept.len(), feature: f });
            if !slab_kept.is_empty() {
                let name = unused(s, &format!("{} Slab", planned.body));
                let f = new_scan(s, &name, &planned.body, &slab, &slab_kept)?;
                made.push(ScanSplitMade { body: name.clone(), side: "slab".into(), triangles: slab_kept.len(), feature: f });
                // The slab chosen, ready to delete.
                s.selection = vec![crate::Sel::Body { name }];
            }
        }
    }
    reply(ScanSplitResult {
        body: planned.body.clone(),
        cutter: planned.cutter.clone(),
        keep: keep.name().into(),
        sides: [negative.to_owned(), positive.to_owned()],
        triangles,
        made,
        crossed: cut.crossed,
        length: cut.length,
        runs: runs.len(),
        closed_runs: runs.iter().filter(|(_, c)| *c).count(),
        chord_error: cut.chord_error,
        carried_on: cut.carried_on,
        sketch,
        curves,
        feature,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_split_tests.rs"]
mod tests;
