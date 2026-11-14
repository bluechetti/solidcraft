//! Flatten and Roll onto Scan (#8615), after SurfaceCraft's Flatten and Roll onto Scan:
//! `scan.flatten` unrolls a region of the scan onto the XY plane beside it (angles kept as well as
//! they can be, the area kept, then relaxed toward keeping every edge's length) and sketches the
//! pattern's outline as lines and arcs in a sketch `Flat Pattern`, saying how far the pattern is
//! from the scan; `scan.roll_onto_scan` carries the curves of a sketch drawn on the pattern back
//! onto the scan through the triangles they lie over, as 3D sketch splines in `Rolled Curves`.
//! The same parameters flatten the same pattern again, so a design opens anywhere.

use serde_json::{Value, json};
use solidcore_record::{Seg2, Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::chain::{self, Outline};
use solidcraft_scan::curve::{self, Laid};
pub use solidcraft_scan::flatten::Stats;
use solidcraft_scan::flatten::{self, Flat};

use super::{CommandSpec, Mesh, mesh};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_flatten::{ScanFlattenParams, ScanFlattenResult, ScanRollParams, ScanRollResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};

const FLATTEN_CONTRACT: TypedContract<ScanFlattenParams> = TypedContract::new();
const ROLL_CONTRACT: TypedContract<ScanRollParams> = TypedContract::new();

/// The sketches the pattern and the rolled curves go in.
pub const PATTERN_NAME: &str = "Flat Pattern";
pub const ROLLED_NAME: &str = "Rolled Curves";

/// How far the pattern's outline may stray from the region's edge, and a rolled curve from the
/// line it lifts to.
const OUTLINE_TOLERANCE: f64 = 0.05;
const ROLL_TOLERANCE: f64 = 0.02;

/// The flattening parameters, the ones a pattern is remembered by.
const REGION_KEYS: [&str; 5] = ["body", "point", "radius", "triangles", "relax"];

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.flatten", "Flatten", crate::checked!("scan.flatten", flatten_scan))
        .at("SCAN", "SKETCH")
        .icon("flat")
        .contract(&FLATTEN_CONTRACT)
        .output(output_schema_for::<ScanFlattenResult>)
        .params("body?: the scan (default: the picked, selected or only one); point?: [x,y,z] on it (default: what is selected on the scan, else the point picked; the region is the piece of the scan there); radius?: length (the region only as far as this round the point); triangles?: [indices of the scan's triangles] (the region, instead of point and radius); saved?: a kept selection (Keep Selection) whose triangles are the region; relax?: bool (default true: relaxed toward keeping each edge's length after the angle-keeping map); tolerance?: length (default 0.05 mm: how far the outline's lines and arcs may stray from the region's edge); name?: the sketch's (default `Flat Pattern`): the region unrolled onto the XY plane beside the scan, its outline sketched, one undo step; a region that closes round on itself is refused (cut it with Split Mesh) → body, sketch, triangles, loops, lines, arcs, circles, area, flat_area, worst_area, strain_rms, strain_max, flipped, relaxed, settled, shift"),
    CommandSpec::new("scan.roll_onto_scan", "Roll onto Scan", crate::checked!("scan.roll_onto_scan", roll_onto_scan))
        .at("SCAN", "SKETCH")
        .icon("spline")
        .contract(&ROLL_CONTRACT)
        .output(output_schema_for::<ScanRollResult>)
        .params("sketch?: the sketch drawn on the pattern (default: the one being edited, else the pattern sketch itself); curves?: [ids] (default: every curve of the sketch drawn that is not the pattern's outline); pattern?: the Flat Pattern sketch (id or name; default: the last made in this session) or the flattening's own parameters (body, point, radius, triangles, relax, as scan.flatten takes them); tolerance?: length (default 0.02 mm): each curve's points lifted onto the scan through the pattern's triangles they lie over, kept as 3D sketch splines in `Rolled Curves` (one undo step); a curve leaving the pattern is cut where it leaves → curves, sketch, pieces, points, off_pattern"),
];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// The point picked on the scan `body`, if one is.
pub(super) fn picked_on(s: &Session, body: &str) -> Option<Vec3> {
    s.selection.iter().rev().find_map(|x| match x {
        Sel::Face { body: b, point, .. } if b == body => Some(*point),
        _ => None,
    })
}

/// The scan a region tool (Fit Patches, Flatten) works on: `body`, else the scan of the kept
/// selection `saved`, else the scan triangles are selected on now; `None` leaves it to the
/// selected or only scan.
pub(super) fn region_scan(s: &Session, p: &Value) -> Option<String> {
    str_(p, "body")
        .filter(|b| !b.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let name = str_(p, "saved")?;
            s.doc.scan_selections.iter().find(|k| k.name == name).map(|k| k.body.clone())
        })
        .or_else(|| super::select::selected(s).filter(|k| k.count > 0).map(|k| k.scan.clone()))
}

/// The region of the scan `p` names: its triangles, a kept selection's (`saved`), what is selected
/// on the scan, else the smooth part round the point given or picked.
pub(super) fn region(s: &Session, p: &Value, on: &Mesh, cmd: &str) -> Result<Vec<usize>> {
    if let Some(list) = p.get("triangles").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or_else(|| bad(cmd, "`triangles` must list the scan's triangle numbers"))?;
        let mut out: Vec<usize> = list
            .iter()
            .map(|v| {
                v.as_u64()
                    .map(|t| t as usize)
                    .filter(|t| *t < on.triangles.len())
                    .ok_or_else(|| bad(cmd, "a triangle number is not one of the scan's"))
            })
            .collect::<Result<_>>()?;
        out.sort_unstable();
        out.dedup();
        return Ok(out);
    }
    // A kept selection (Keep Selection), as `scan.select {saved}` opens it again.
    if let Some(name) = p.get("saved").filter(|v| !v.is_null()) {
        let name = name.as_str().ok_or_else(|| bad(cmd, "`saved` is the name of a kept selection"))?;
        let kept = s.doc.scan_selections.iter().find(|k| k.name == name).ok_or_else(|| bad(cmd, format!("no kept selection `{name}`")))?;
        if kept.body != on.body || kept.triangles as usize != on.triangles.len() {
            return Err(bad(cmd, format!("the kept selection `{name}` is of `{}` as it was, not of `{}` as it is now", kept.body, on.body)));
        }
        let out: Vec<usize> = kept.flags().iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect();
        if out.is_empty() {
            return Err(bad(cmd, format!("the kept selection `{name}` holds no triangles")));
        }
        return Ok(out);
    }
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be [x, y, z]"))?,
        None => {
            // What is selected on the scan, else the point picked on it.
            if let Some(sel) = super::select::selected(s).filter(|k| k.scan == on.body && k.count > 0) {
                return Ok(sel.selected.iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect());
            }
            picked_on(s, &on.body).ok_or_else(|| bad(cmd, "select or pick the part of the scan to flatten, or give `point` or `triangles`"))?
        }
    };
    let scan = on.scan();
    let seed = solidcraft_scan::base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let reach = match crate::params::num(p, "radius") {
        Some(r) if r.is_finite() && r > 0.0 => r,
        Some(_) => return Err(bad(cmd, "`radius` must be a length above 0")),
        None => f64::INFINITY,
    };
    Ok(solidcraft_scan::fit::around(&scan, seed, point, reach))
}

/// The pattern `p` asks for: the scan, its region flattened, and the shift that lays it on XY
/// beside the scan (its box's corner a tenth of the scan's size past the scan's along X).
pub fn flattened(s: &Session, p: &Value, cmd: &str) -> Result<(String, Flat, Vec2)> {
    let body = region_scan(s, p);
    let on = mesh(s, &json!({ "body": body }), cmd)?;
    let triangles = region(s, p, &on, cmd)?;
    let flat = flatten::flatten(&on.scan(), &triangles, bool_(p, "relax").unwrap_or(true)).map_err(|e| bad(cmd, e.to_string()))?;
    let (lo, hi) = on.scan().bounds().ok_or_else(|| bad(cmd, "the scan has no points"))?;
    let low = flat.points.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
    let gap = 0.1 * (hi - lo).len();
    let shift = Vec2::new(hi.x + gap - low.x, lo.y - low.y);
    Ok((on.body, flat, shift))
}

fn flatten_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.flatten";
    let tolerance = match crate::params::num(p, "tolerance") {
        Some(t) if t.is_finite() && t > 0.0 => t,
        Some(_) => return Err(bad(cmd, "`tolerance` must be a length above 0")),
        None => OUTLINE_TOLERANCE,
    };
    let (body, flat, shift) = flattened(s, p, cmd)?;
    if s.active_sketch.is_some() {
        return Err(bad(cmd, "finish the sketch being edited first"));
    }
    let outlines: Vec<(Outline, bool)> = flat
        .outline
        .iter()
        .filter_map(|lp| {
            let mut pts: Vec<Vec2> = lp.iter().map(|q| *q + shift).collect();
            pts.push(pts[0]);
            chain::chain(&pts, true, tolerance).map(|o| (o, true))
        })
        .collect();
    let name = str_(p, "name").filter(|n| !n.trim().is_empty()).unwrap_or(PATTERN_NAME).to_owned();
    let made = run(s, "sketch.create", json!({"plane": "XY", "name": name}))?;
    let sketch = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the pattern's sketch was not made"))?;
    let mut added = super::section::Added::default();
    let drawn = super::super::sketch_store::update(s, sketch, |_, _, sk| super::section::draw(sk, &outlines, &mut added));
    if let Err(e) = drawn {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    run(s, "sketch.finish", json!({}))?;
    // Remembered by its sketch, with the region it was flattened from and its outline's curves.
    let outline: Vec<String> =
        super::super::sketch_store::read(&s.doc, sketch).map(|sk| sk.curves.iter().map(|c| c.id.clone()).collect()).unwrap_or_default();
    let mut memo = serde_json::Map::new();
    memo.insert("body".into(), json!(body));
    memo.insert("outline".into(), json!(outline));
    for key in REGION_KEYS.iter().skip(1) {
        if let Some(v) = p.get(*key).filter(|v| !v.is_null()) {
            memo.insert((*key).into(), v.clone());
        }
    }
    // A kept selection may be deleted or kept again: the region is kept as its triangles.
    if !memo.contains_key("triangles")
        && let Some(name) = str_(p, "saved")
        && let Some(kept) = s.doc.scan_selections.iter().find(|k| k.name == name && k.body == body)
    {
        let list: Vec<usize> = kept.flags().iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect();
        memo.insert("triangles".into(), json!(list));
    }
    if !memo.contains_key("point") && !memo.contains_key("triangles") {
        // The selection may change: the region is kept as its triangles.
        match super::select::selected(s).filter(|k| k.scan == body && k.count > 0) {
            Some(sel) => {
                let list: Vec<usize> = sel.selected.iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect();
                memo.insert("triangles".into(), json!(list));
            }
            None => {
                if let Some(q) = picked_on(s, &body) {
                    memo.insert("point".into(), json!([q.x, q.y, q.z]));
                }
            }
        }
    }
    s.scan_flats.retain(|(id, _)| *id != sketch);
    s.scan_flats.push((sketch, Value::Object(memo)));
    let st = flat.stats;
    reply(ScanFlattenResult {
        body,
        sketch,
        triangles: flat.triangles.len(),
        loops: flat.outline.len(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        area: st.area,
        flat_area: st.flat_area,
        worst_area: st.worst_area,
        strain_rms: st.strain_rms,
        strain_max: st.strain_max,
        flipped: st.flipped,
        relaxed: st.relaxed,
        settled: st.settled,
        shift: [shift.x, shift.y],
        param_warnings: Vec::new(),
    })
}

/// The flattening a roll goes back through: the pattern named (or the last made) as remembered,
/// or the parameters given.
fn pattern_params(s: &Session, p: &Value, cmd: &str) -> Result<(Option<u64>, Value)> {
    let given = REGION_KEYS.iter().any(|k| k != &"relax" && k != &"body" && p.get(*k).is_some_and(|v| !v.is_null()));
    if given {
        let mut out = serde_json::Map::new();
        for key in REGION_KEYS {
            if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
                out.insert(key.into(), v.clone());
            }
        }
        return Ok((None, Value::Object(out)));
    }
    let named = match p.get("pattern").filter(|v| !v.is_null()) {
        Some(v) => Some(super::super::sketch_store::target(s, &json!({ "sketch": v }), cmd)?),
        None => None,
    };
    let found = match named {
        Some(id) => s.scan_flats.iter().find(|(k, _)| *k == id),
        None => s.scan_flats.last(),
    };
    let (id, params) = found.ok_or_else(|| {
        bad(cmd, "this session has made no flat pattern to roll back through: flatten the region again, or give its parameters (point, radius or triangles)")
    })?;
    Ok((Some(*id), params.clone()))
}

/// Points along a sketch curve's segments, about every half millimetre (at least 8 to a piece).
fn sampled(segs: &[Seg2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::new();
    for seg in segs {
        let steps = ((seg.length() / 0.5).ceil() as usize).clamp(8, 2000);
        for k in 0..=steps {
            let q = seg.point_at(k as f64 / steps as f64);
            if out.last().is_none_or(|last| last.dist(q) > 1e-9) {
                out.push(q);
            }
        }
    }
    out
}

fn roll_onto_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.roll_onto_scan";
    let tolerance = match crate::params::num(p, "tolerance") {
        Some(t) if t.is_finite() && t > 0.0 => t,
        Some(_) => return Err(bad(cmd, "`tolerance` must be a length above 0")),
        None => ROLL_TOLERANCE,
    };
    let (pattern, params) = pattern_params(s, p, cmd)?;
    let (body, flat, shift) = flattened(s, &params, cmd)?;
    // The sketch drawn on the pattern.
    let drawing = match p.get("sketch").filter(|v| !v.is_null()) {
        Some(_) => super::super::sketch_store::target(s, p, cmd)?,
        None => s.active_sketch.or(pattern).ok_or_else(|| bad(cmd, "name the sketch drawn on the pattern with `sketch`"))?,
    };
    if s.model.state().sketch(drawing).is_none() {
        s.refresh();
    }
    let wanted = crate::params::string_list_or_default(p, "curves");
    let lines: Vec<Vec<Vec3>> = {
        let state = s.model.state();
        let solved = state.sketch(drawing).ok_or_else(|| bad(cmd, "the sketch drawn is not evaluated"))?;
        // The pattern's own outline is not rolled, when the drawing is on the pattern's sketch.
        let outline: Vec<String> = if Some(drawing) == pattern { crate::params::string_list_or_default(&params, "outline") } else { Vec::new() };
        let plane = solved.plane;
        solved
            .sketch
            .curves
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.construction && if wanted.is_empty() { !outline.contains(&c.id) } else { wanted.contains(&c.id) })
            .map(|(i, _)| sampled(&solved.sketch.segs(i)).into_iter().map(|q| plane.origin + plane.x * q.x + plane.y * q.y).collect())
            .collect()
    };
    if lines.is_empty() {
        return Err(bad(cmd, "the sketch has no curve drawn on the pattern to roll"));
    }
    let scan = mesh(s, &json!({ "body": body }), cmd)?;
    let lifter = flat.lifter();
    // Each curve's points, off the plane's XY and back to the pattern's own, lifted; a curve that
    // leaves the pattern is cut where it leaves.
    let (mut pieces, mut points, mut off) = (Vec::new(), 0usize, 0usize);
    for line in &lines {
        let mut run: Vec<Vec3> = Vec::new();
        for q in line {
            points += 1;
            let at = Vec2::new(q.x, q.y) - shift;
            match (q.z.abs() < 1e-6).then(|| lifter.lift(&flat, &scan.scan(), at)).flatten() {
                Some(up) => run.push(up),
                None => {
                    off += 1;
                    if run.len() >= 2 {
                        pieces.push(std::mem::take(&mut run));
                    } else {
                        run.clear();
                    }
                }
            }
        }
        if run.len() >= 2 {
            pieces.push(run);
        }
    }
    if pieces.is_empty() {
        return Err(bad(cmd, "no curve of the sketch lies over the pattern: draw on the pattern, in its plane"));
    }
    if s.active_sketch.is_some() {
        run(s, "sketch.finish", json!({}))?;
    }
    let made = run(s, "sketch.create", json!({"plane": "XY", "name": ROLLED_NAME}))?;
    let sketch = made.get("sketch").cloned().ok_or_else(|| bad(cmd, "the sketch for the rolled curves was not made"))?;
    run(s, "sketch.options", json!({"sketch": sketch, "sketch_3d": true}))?;
    let mut curves = Vec::new();
    let drawn = (|| -> Result<()> {
        for piece in &pieces {
            let closed = piece.len() > 3 && piece[0].dist(piece[piece.len() - 1]) < 1e-6;
            let mut path = piece.clone();
            if closed {
                path.pop();
            }
            let n = path.len();
            let at: Vec<usize> = if closed && n >= 3 {
                path.push(path[0]);
                vec![0, n / 3, 2 * n / 3, n]
            } else {
                vec![0, n - 1]
            };
            let picks = at.iter().map(|&i| path[i]).collect();
            let fitted = curve::fit(&Laid { picks, path, at, closed }, tolerance);
            let fit: Vec<[f64; 3]> = fitted.fit.iter().map(|q| [q.x, q.y, q.z]).collect();
            let made = run(s, "sketch.spline3d", json!({"sketch": sketch, "points": fit}))?;
            curves.extend(made.get("curves").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned));
        }
        Ok(())
    })();
    run(s, "sketch.finish", json!({}))?;
    drawn?;
    reply(ScanRollResult { curves, sketch, pieces: pieces.len(), points, off_pattern: off, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "scan_flatten_tests.rs"]
mod tests;
