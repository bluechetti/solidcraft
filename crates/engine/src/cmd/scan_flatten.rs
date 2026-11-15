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
        .params("body?: the scan (default: the picked, selected or only one); point?: [x,y,z] on it (default: what is selected on the scan, else the point picked; the region is the piece of the scan there, or on a piece that wraps round, a closed part, the side of it facing the click); radius?: length (the region only as far as this round the point); grow?: true | degrees (instead of radius: the face at the point, as far as it faces within 30° or that angle of the way it faces there); triangles?: [indices of the scan's triangles] (the region, instead of point and radius); saved?: a kept selection (Keep Selection) whose triangles are the region; relax?: bool (default true: relaxed toward keeping each edge's length after the angle-keeping map); tolerance?: length (default 0.05 mm: how far the outline's lines and arcs may stray from the region's edge); name?: the sketch's (default `Flat Pattern`): the region unrolled onto the XY plane beside the scan, its outline sketched, one undo step; a region that closes round on itself is refused (cut it with Split Mesh) → body, sketch, triangles, loops, lines, arcs, circles, area, flat_area, worst_area, strain_rms, strain_max, flipped, relaxed, settled, shift"),
    CommandSpec::new("scan.roll_onto_scan", "Roll onto Scan", crate::checked!("scan.roll_onto_scan", roll_onto_scan))
        .at("SCAN", "SKETCH")
        .icon("spline")
        .contract(&ROLL_CONTRACT)
        .output(output_schema_for::<ScanRollResult>)
        .params("sketch?: the sketch drawn on the pattern (default: the one being edited, else the pattern sketch itself); curves?: [ids] (default: every curve of the sketch drawn that is not the pattern's outline); pattern?: the Flat Pattern sketch (id or name; default: the one being edited when it is one, else the last the design keeps; the design keeps each with the flattening that made it) or the flattening's own parameters (body, point, radius, triangles, relax, as scan.flatten takes them); tolerance?: length (default 0.02 mm): each curve's points lifted onto the scan through the pattern's triangles they lie over, kept as 3D sketch splines in `Rolled Curves` (one undo step); a curve leaving the pattern is cut where it leaves → curves, sketch, pieces, points, off_pattern"),
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

/// The region chosen by hand, if `p` chose one: its `triangles`, a kept selection (`saved`), the
/// face grown from the point (`grow`), with `radius_too` the scan within `radius` of the point,
/// else (no `point` given) what is selected on the scan. `None`: the tool finds its region its own way (grown from the point).
pub(super) fn chosen_region(s: &Session, p: &Value, on: &Mesh, cmd: &str, radius_too: bool) -> Result<Option<Vec<usize>>> {
    let has = |key: &str| p.get(key).is_some_and(|v| !v.is_null());
    if has("triangles") || has("saved") || has("grow") || (radius_too && has("radius")) {
        return region(s, p, on, cmd).map(Some);
    }
    if !has("point")
        && let Some(sel) = super::select::selected(s).filter(|k| k.scan == on.body && k.count > 0)
    {
        return Ok(Some(sel.selected.iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect()));
    }
    Ok(None)
}

/// The region of the scan `p` names: its triangles, a kept selection's (`saved`), what is selected
/// on the scan, else round the point given or picked: within `radius` of it, else the piece there
/// (on a piece that wraps round, the part of it facing the way the scan faces there).
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
            picked_on(s, &on.body).ok_or_else(|| bad(cmd, "select or pick the part of the scan to work on, or give `point` or `triangles`"))?
        }
    };
    // Grown from the point as Select on Scan's Region grows: the face there, as far as the scan
    // faces within the angle (degrees; `true`: the Region's) of the way it faces at the point.
    if let Some(grow) = p.get("grow").filter(|v| !v.is_null() && **v != Value::Bool(false)) {
        if p.get("radius").is_some_and(|v| !v.is_null()) {
            return Err(bad(cmd, "give `grow` or `radius`, not both"));
        }
        let angle = match grow {
            Value::Bool(true) => super::select::REGION_ANGLE,
            v => v.as_f64().filter(|a| (1.0..=90.0).contains(a)).ok_or_else(|| bad(cmd, "`grow` is true or an angle of 1 to 90 degrees"))?,
        };
        let topology = super::select::topology(s, &on.body, cmd)?;
        let (seed, _) = topology.nearest(point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
        return Ok(topology.region(seed, angle.to_radians()).iter().enumerate().filter(|(_, on)| **on).map(|(t, _)| t).collect());
    }
    let scan = on.scan();
    let seed = solidcraft_scan::base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    // A number is mm; text is a length ("10 mm", an expression of the design's parameters).
    let radius = match p.get("radius").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(match v.as_f64() {
            Some(r) => r,
            None => {
                let e = crate::params::expr(p, "radius").ok_or_else(|| bad(cmd, "`radius` must be a length above 0"))?;
                s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("radius: {e}")))?
            }
        }),
    };
    match radius {
        Some(r) if r.is_finite() && r > 0.0 => Ok(solidcraft_scan::fit::around(&scan, seed, point, r)),
        Some(_) => Err(bad(cmd, "`radius` must be a length above 0")),
        // The piece clicked; on one that wraps round (a closed part), the side facing the click.
        None => Ok(solidcraft_scan::fit::clicked_part(&scan, seed, point)),
    }
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
    // Beside the scan, but past the patterns already laid there: each keeps its own place.
    let (shift, laid) = clear_of_patterns(s, &flat, shift);
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
    // Where the pattern was laid, so a roll goes back through this place and not a new one.
    memo.insert("shift".into(), json!([shift.x, shift.y]));
    memo.insert("box".into(), json!([[laid.0.x, laid.0.y], [laid.1.x, laid.1.y]]));
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
    // Kept in the design, so Roll onto Scan finds it after the design is opened again.
    let kept = &mut s.doc_mut().scan_flat_patterns;
    kept.retain(|k| k.sketch != sketch);
    kept.push(solidcraft_doc::scan_flat_pattern::FlatPatternKeep { sketch, params: Value::Object(memo) });
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

/// The box a kept pattern was laid in on XY, when it says.
fn kept_box(params: &Value) -> Option<(Vec2, Vec2)> {
    let corner = |k: usize| {
        let c = params.get("box")?.get(k)?;
        Some(Vec2::new(c.get(0)?.as_f64()?, c.get(1)?.as_f64()?))
    };
    Some((corner(0)?, corner(1)?))
}

/// `shift` moved along Y until the pattern `flat` laid there overlaps none of the patterns the
/// design keeps (with a gap of a tenth of its own size), and the box it then takes on XY.
fn clear_of_patterns(s: &Session, flat: &Flat, mut shift: Vec2) -> (Vec2, (Vec2, Vec2)) {
    let (low, high) = flat.points.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    let gap = 0.1 * (high - low).len().max(1e-6);
    let others: Vec<(Vec2, Vec2)> = s
        .doc
        .scan_flat_patterns
        .iter()
        .filter(|k| super::super::sketch_store::read(&s.doc, k.sketch).is_ok())
        .filter_map(|k| kept_box(&k.params))
        .collect();
    // Each move clears one box upward; as many moves as boxes clear them all.
    for _ in 0..=others.len() {
        let (a, b) = (low + shift, high + shift);
        let Some(hit) = others
            .iter()
            .filter(|(lo, hi)| a.x < hi.x + gap && lo.x < b.x + gap && a.y < hi.y + gap && lo.y < b.y + gap)
            .map(|(_, hi)| hi.y)
            .reduce(f64::max)
        else {
            break;
        };
        shift.y = hit + gap - low.y;
    }
    (shift, (low + shift, high + shift))
}

/// The flattening a roll goes back through: the pattern named as the design keeps it, else the one
/// being edited, else the last one the design still has; or the parameters given.
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
    let kept = &s.doc.scan_flat_patterns;
    // A pattern whose sketch was deleted is no pattern.
    let alive = |id: u64| super::super::sketch_store::read(&s.doc, id).is_ok();
    let found = match named {
        Some(id) => kept.iter().find(|k| k.sketch == id),
        None => super::super::sketch_store::active(s)
            .and_then(|a| kept.iter().find(|k| k.sketch == a))
            .or_else(|| kept.iter().rev().find(|k| alive(k.sketch))),
    };
    let k = found.ok_or_else(|| {
        bad(cmd, "the design has no flat pattern to roll back through: flatten the region again, or give its parameters (point, radius or triangles)")
    })?;
    Ok((Some(k.sketch), k.params.clone()))
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
    // The place the pattern was laid, when the design keeps it: past other patterns, not the first.
    let kept_shift = params.get("shift").and_then(|v| Some(Vec2::new(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?)));
    let shift = kept_shift.filter(|v| v.x.is_finite() && v.y.is_finite()).unwrap_or(shift);
    // The sketch drawn on the pattern.
    let drawing = match p.get("sketch").filter(|v| !v.is_null()) {
        Some(_) => super::super::sketch_store::target(s, p, cmd)?,
        None => super::super::sketch_store::active(s).or(pattern).ok_or_else(|| bad(cmd, "name the sketch drawn on the pattern with `sketch`"))?,
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
