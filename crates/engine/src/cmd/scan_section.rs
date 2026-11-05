//! Section to Sketch (`scan.section_sketch`): where a plane cuts the scan, as a sketch of lines,
//! arcs and circles fitted to the cut (SurfaceCraft's chain fitter): each piece averages the scan's
//! points, so its noise is not traced, the pieces meet end to end, and smooth joins are held
//! tangent. The plane is the sketch being edited, or a new sketch's; the result is ordinary sketch
//! geometry to dimension and extrude.

use serde_json::{Value, json};
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
use solidcraft_scan::chain;
pub use solidcraft_scan::chain::{Outline, Piece};
use solidcraft_sketch::{ConstraintKind, Sketch};

use super::{length, mesh_name};
use crate::params::bad;
use crate::schema::scan::ScanSectionResult;
use crate::{EngineError, Result, Session};

/// The default tolerance: how far a piece may stray from the cut's points.
pub const TOLERANCE: f64 = 0.1;

/// The name of a sketch Section to Sketch makes.
const SKETCH_NAME: &str = "Section";

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
    // Runs shorter than a few tolerances are specks of the cut, not shapes.
    let length_of = |r: &solidcraft_scan::section::Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    let outlines: Vec<(Outline, bool)> = runs
        .iter()
        .filter(|r| length_of(r) > 10.0 * tolerance)
        .filter_map(|r| chain::chain(&r.points, r.closed, tolerance).map(|o| (o, r.closed)))
        .collect();
    if outlines.is_empty() {
        let note = if runs.is_empty() { "the plane does not cut the scan" } else { "the cut is too small to sketch" };
        if made {
            run(s, "sketch.cancel", json!({}))?;
        }
        return Err(bad(cmd, note));
    }
    let mut added = Added::default();
    let drawn = super::super::sketch_store::update(s, sketch, |_, _, sk| {
        for (outline, closed) in &outlines {
            match outline {
                Outline::Circle { center, radius } => {
                    sk.add_circle(*center, *radius, None, None)?;
                    added.circles += 1;
                }
                Outline::Chain(pieces) => add_chain(sk, pieces, *closed, &mut added)?,
            }
        }
        Ok(())
    });
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
