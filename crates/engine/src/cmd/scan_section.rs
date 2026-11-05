//! Section to Sketch (`scan.section_sketch`): where a plane cuts the scan, as a sketch of lines,
//! arcs and circles fitted to the cut (SurfaceCraft's chain fitter): each piece averages the scan's
//! points, so its noise is not traced, the pieces meet end to end, and smooth joins are held
//! tangent. The plane is the sketch being edited, or a new sketch's; the result is ordinary sketch
//! geometry to dimension and extrude.
//!
//! Sections Along Curve (`scan.sections_along_curve`): planes square to a curve of a 3D sketch at
//! even steps along it cut the scan, each section drawn the same way in a sketch of its own on its
//! plane, in order along the curve, ready to loft.

use serde_json::{Value, json};
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
use solidcraft_scan::chain;
pub use solidcraft_scan::chain::{Outline, Piece};
pub use solidcraft_scan::section::Along;
use solidcraft_sketch::{ConstraintKind, Sketch};

use super::{length, mesh_name};
use crate::params::{bad, bool_, string_list_or_default};
use crate::schema::scan::{ScanSectionResult, ScanSectionsAlongResult};
use crate::{EngineError, Result, Session};

/// The default tolerance: how far a piece may stray from the cut's points.
pub const TOLERANCE: f64 = 0.1;

/// The name of a sketch Section to Sketch makes.
const SKETCH_NAME: &str = "Section";

/// What Sections Along Curve names its sketches, before their number.
const ALONG_NAME: &str = "Curve Section";

/// How many sections Sections Along Curve cuts unless told.
pub const SECTIONS: usize = 8;

/// How near the curve a cut must come to be kept, as a share of the scan's size.
const ALONG_REACH: f64 = 0.1;

/// Joins whose directions differ by less than this are held tangent.
const SMOOTH: f64 = 3.0 * std::f64::consts::PI / 180.0;

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// What was added to the sketch.
#[derive(Default)]
struct Added {
    lines: usize,
    arcs: usize,
    circles: usize,
    tangents: usize,
}

/// One closed or open chain added to the sketch, its ends shared and its smooth joins tangent.
fn add_chain(sk: &mut Sketch, pieces: &[Piece], closed: bool, added: &mut Added) -> Result<()> {
    let mut ends: Vec<usize> = pieces.iter().map(|p| sk.add_point(p.from(), None)).collect::<std::result::Result<_, _>>()?;
    match (closed, pieces.last()) {
        (true, _) => ends.push(ends[0]),
        (false, Some(last)) => ends.push(sk.add_point(last.to(), None)?),
        (false, None) => return Ok(()),
    }
    let mut curves = Vec::with_capacity(pieces.len());
    for (k, piece) in pieces.iter().enumerate() {
        let (a, b) = (ends[k], ends[k + 1]);
        let curve = match *piece {
            Piece::Arc { center, from, to, ccw, .. } => {
                // Sketch arcs run counter-clockwise: a clockwise one is the same arc from its end.
                let made = if ccw {
                    sk.add_arc(center, from, to, [None, Some(a), Some(b)], None)
                } else {
                    sk.add_arc(center, to, from, [None, Some(b), Some(a)], None)
                };
                match made {
                    Ok(c) => {
                        added.arcs += 1;
                        c
                    }
                    Err(_) => {
                        added.lines += 1;
                        sk.add_line_pts(a, b, None)?
                    }
                }
            }
            Piece::Line { .. } => {
                added.lines += 1;
                sk.add_line_pts(a, b, None)?
            }
        };
        curves.push(curve);
    }
    // Smooth joins held tangent (where an arc meets anything; two lines in line are left alone).
    let count = pieces.len();
    let joins = if closed { count } else { count.saturating_sub(1) };
    for k in 0..joins {
        let next = (k + 1) % count;
        let (here, there) = (&pieces[k], &pieces[next]);
        let arcy = matches!(here, Piece::Arc { .. }) || matches!(there, Piece::Arc { .. });
        if let (true, Some(u), Some(v)) = (arcy, here.direction(true), there.direction(false))
            && u.dot(v).clamp(-1.0, 1.0).acos() < SMOOTH
            && sk.add_constraint(ConstraintKind::Tangent { a: curves[k], b: curves[next] }, None).is_ok()
        {
            added.tangents += 1;
        }
    }
    Ok(())
}

/// The outlines of a cut, each closed or not, drawn into the sketch.
fn draw(sk: &mut Sketch, outlines: &[(Outline, bool)], added: &mut Added) -> Result<()> {
    for (outline, closed) in outlines {
        match outline {
            Outline::Circle { center, radius } => {
                sk.add_circle(*center, *radius, None, None)?;
                added.circles += 1;
            }
            Outline::Chain(pieces) => add_chain(sk, pieces, *closed, added)?,
        }
    }
    Ok(())
}

/// The outlines fitted to a cut's runs, leaving out specks shorter than a few tolerances.
fn fitted(runs: &[solidcraft_scan::section::Run], tolerance: f64) -> Vec<(Outline, bool)> {
    let length_of = |r: &solidcraft_scan::section::Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    runs.iter()
        .filter(|r| length_of(r) > 10.0 * tolerance)
        .filter_map(|r| chain::chain(&r.points, r.closed, tolerance).map(|o| (o, r.closed)))
        .collect()
}

pub(super) fn section_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.section_sketch";
    let tolerance = length(s, p, "tolerance", TOLERANCE, cmd)?;
    let scan = mesh_name(s, p, cmd)?;
    // The sketch being edited, or a new one on the plane asked for.
    let (sketch, made) = match super::super::sketch_store::active(s) {
        Some(id) => (id, false),
        None => {
            let mut create = json!({"plane": p.get("plane").cloned().unwrap_or(json!("XY")), "name": SKETCH_NAME});
            if let Some(offset) = p.get("offset").filter(|v| !v.is_null()) {
                create["offset"] = offset.clone();
            }
            let reply = run(s, "sketch.create", create)?;
            (reply.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch was not made"))?, true)
        }
    };
    if s.model.state().sketch(sketch).is_none() {
        s.refresh();
    }
    let finish = |s: &mut Session| if made { run(s, "sketch.finish", json!({})).map(|_| ()) } else { Ok(()) };
    let runs = {
        let state = s.model.state();
        let plane = state.sketch(sketch).map(|ss| ss.plane).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
        let (positions, triangles) =
            state.body(&scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
        solidcraft_scan::section::section(&Scan::new(positions, triangles), &plane)
    };
    let outlines = fitted(&runs, tolerance);
    if outlines.is_empty() {
        let note = if runs.is_empty() { "the plane does not cut the scan" } else { "the cut is too small to sketch" };
        if made {
            run(s, "sketch.cancel", json!({}))?;
        }
        return Err(bad(cmd, note));
    }
    let mut added = Added::default();
    let drawn = super::super::sketch_store::update(s, sketch, |_, _, sk| draw(sk, &outlines, &mut added));
    if let Err(e) = drawn {
        if made {
            run(s, "sketch.cancel", json!({}))?;
        }
        return Err(e);
    }
    finish(s)?;
    let reply = ScanSectionResult {
        body: scan,
        sketch,
        outlines: outlines.len(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        tangents: added.tangents,
        tolerance,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The curve Sections Along Curve follows: the 3D sketch (`sketch`, by id or name; default the
/// design's last `Scan Curves`), the curves (`curves`; default its last), and their points end to
/// end, each turned round to run on from the one before.
pub fn along_path(s: &Session, sketch: Option<&Value>, curves: &[String], cmd: &str) -> Result<(u64, Vec<String>, Vec<solidcore_record::Vec3>)> {
    let sketch = match sketch.filter(|v| !v.is_null()) {
        Some(v) => super::super::features::sketch_id(s, Some(v), cmd, "sketch")?,
        None => s
            .doc
            .features
            .iter()
            .rev()
            .find(|f| f.name == super::curve::SKETCH_NAME && matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. }))
            .map(|f| f.id)
            .ok_or_else(|| bad(cmd, "no curve to follow: draw one with Curve on Scan, or give `sketch` and `curves`"))?,
    };
    let state = s.model.state();
    let wires = &state.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?.sketch.wires;
    let curves: Vec<String> = if curves.is_empty() {
        let last = wires.iter().rev().find(|w| w.pts.len() >= 2).ok_or_else(|| bad(cmd, "the sketch has no 3D curve to follow"))?;
        vec![last.id.clone()]
    } else {
        curves.to_vec()
    };
    if curves.len() > 1000 {
        return Err(bad(cmd, "`curves` lists at most 1000 curves"));
    }
    let mut path: Vec<solidcore_record::Vec3> = Vec::new();
    for id in &curves {
        let wire = wires.iter().find(|w| w.id == *id).ok_or_else(|| bad(cmd, format!("no 3D curve `{id}` in the sketch")))?;
        let mut pts = wire.pts.clone();
        if let (Some(end), Some(first), Some(last)) = (path.last(), pts.first(), pts.last())
            && end.dist(*last) < end.dist(*first)
        {
            pts.reverse();
        }
        let skip = usize::from(path.last().zip(pts.first()).is_some_and(|(a, b)| a.dist(*b) < 1e-9));
        path.extend(pts.into_iter().skip(skip));
    }
    if path.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() <= 0.0 {
        return Err(bad(cmd, "the curve has no length to cut along"));
    }
    Ok((sketch, curves, path))
}

/// The sections of the scan `body` along `path`, as Sections Along Curve cuts them.
pub fn along_cut(s: &Session, body: &str, path: &[solidcore_record::Vec3], count: usize, every: bool, cmd: &str) -> Result<Vec<Along>> {
    let state = s.model.state();
    let (positions, triangles) = state.body(body).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{body}` is not a scan")))?;
    let scan = Scan::new(positions, triangles);
    Ok(solidcraft_scan::section::along(&scan, path, count, every, ALONG_REACH * scan.extent()))
}

/// What the command would cut, for a dialog's preview: the scan, the curve's points, and the
/// sections with the outlines fitted to each.
pub fn along_preview(
    s: &Session,
    body: Option<&str>,
    sketch: Option<u64>,
    curves: &[String],
    count: usize,
    every: bool,
    tolerance: f64,
) -> Result<(String, Vec<solidcore_record::Vec3>, Vec<(Along, Vec<Outline>)>)> {
    let cmd = "scan.sections_along_curve";
    let scan = mesh_name(s, &json!({ "body": body }), cmd)?;
    let (_, _, path) = along_path(s, sketch.map(|id| json!(id)).as_ref(), curves, cmd)?;
    let cut = along_cut(s, &scan, &path, count.clamp(1, 60), every, cmd)?;
    let sections = cut
        .into_iter()
        .map(|a| {
            let outlines = fitted(&a.runs, tolerance).into_iter().map(|(o, _)| o).collect();
            (a, outlines)
        })
        .collect();
    Ok((scan, path, sections))
}

pub(super) fn sections_along_curve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.sections_along_curve";
    let tolerance = length(s, p, "tolerance", TOLERANCE, cmd)?;
    let count = match p.get("count").filter(|v| !v.is_null()) {
        None => SECTIONS,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=60).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| bad(cmd, "`count` must be a whole number of sections from 1 to 60"))?,
    };
    let every = bool_(p, "every").unwrap_or(false);
    let scan = mesh_name(s, p, cmd)?;
    let (_, _, path) = along_path(s, p.get("sketch"), &string_list_or_default(p, "curves"), cmd)?;
    let length_along: f64 = path.windows(2).map(|w| w[0].dist(w[1])).sum();
    let sections = along_cut(s, &scan, &path, count, every, cmd)?;
    let planned: Vec<(solidcore_record::Plane, Vec<(Outline, bool)>)> =
        sections.iter().map(|a| (a.station.plane, fitted(&a.runs, tolerance))).filter(|(_, o)| !o.is_empty()).collect();
    if planned.is_empty() {
        return Err(bad(cmd, "no plane square to the curve cuts the scan near it: put the curve on the scan, or through the middle of the part"));
    }
    let missed = sections.len() - planned.len();
    // Numbered on from the sections already in the design.
    let taken: std::collections::BTreeSet<&str> = s.doc.features.iter().map(|f| f.name.as_str()).collect();
    let mut names = Vec::with_capacity(planned.len());
    let mut number = 1;
    while names.len() < planned.len() {
        let name = format!("{ALONG_NAME} {number}");
        if !taken.contains(name.as_str()) {
            names.push(name);
        }
        number += 1;
    }
    let mut added = Added::default();
    let mut sketches = Vec::with_capacity(planned.len());
    let at = |v: solidcore_record::Vec3| [v.x, v.y, v.z];
    for ((plane, outlines), name) in planned.iter().zip(&names) {
        let create = json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": name});
        let made = run(s, "sketch.create", create)?;
        let id = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch was not made"))?;
        if s.model.state().sketch(id).is_none() {
            s.refresh();
        }
        if let Err(e) = super::super::sketch_store::update(s, id, |_, _, sk| draw(sk, outlines, &mut added)) {
            run(s, "sketch.cancel", json!({}))?;
            return Err(e);
        }
        run(s, "sketch.finish", json!({}))?;
        sketches.push(id);
    }
    let reply = ScanSectionsAlongResult {
        body: scan,
        sketches,
        missed,
        outlines: planned.iter().map(|(_, o)| o.len()).sum(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        tangents: added.tangents,
        length: length_along,
        tolerance,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The plane `sketch.create` makes for "XY", "XZ" or "YZ" moved `offset` along its normal (XY:
/// x, y along +X, +Y; XZ: +X, −Z; YZ: +Y, +Z), for a dialog to preview the cut on.
pub fn standard_plane(name: &str, offset: f64) -> Option<solidcore_record::Plane> {
    use solidcore_record::{Plane, Vec3};
    let plane = match name {
        "XY" => Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y },
        "XZ" => Plane { origin: Vec3::ZERO, x: Vec3::X, y: -Vec3::Z },
        "YZ" => Plane { origin: Vec3::ZERO, x: Vec3::Y, y: Vec3::Z },
        _ => return None,
    };
    offset.is_finite().then(|| plane.offset(offset))
}

/// The cut the command would sketch, for a dialog's preview: the scan, the plane (the sketch's
/// being edited, else `plane`), and the outlines fitted to the cut.
pub fn preview(
    s: &Session,
    body: Option<&str>,
    plane: solidcore_record::Plane,
    tolerance: f64,
) -> Result<(String, solidcore_record::Plane, Vec<Outline>)> {
    let cmd = "scan.section_sketch";
    let scan = mesh_name(s, &json!({ "body": body }), cmd)?;
    let state = s.model.state();
    let plane = super::super::sketch_store::active(s).and_then(|id| state.sketch(id)).map_or(plane, |ss| ss.plane);
    let (positions, triangles) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
    let runs = solidcraft_scan::section::section(&Scan::new(positions, triangles), &plane);
    let outlines = runs
        .iter()
        .filter(|r| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() > 10.0 * tolerance)
        .filter_map(|r| chain::chain(&r.points, r.closed, tolerance))
        .collect();
    Ok((scan, plane, outlines))
}
