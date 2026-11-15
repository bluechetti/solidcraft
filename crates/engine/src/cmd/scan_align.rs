//! Aligning scans beyond one pass onto another (#8615), after SurfaceCraft's: `scan.align_all`
//! lays several passes of one part together at once, `scan.align_pairs` lays a scan by points
//! picked in pairs (then fits it onto the surface they were picked on), and `scan.align_to_model`
//! lays a scan onto a solid or surface body. Every move is an ordinary Move of the scan.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_scan::Scan;
use solidcraft_scan::align::{self, AllSettings};
use solidcraft_scan::register::{self, Settings};

use super::{CommandSpec, Mesh, mesh, move_by};
use crate::params::{bad, bool_, num, str_};
use crate::schema::scan_align::{
    ScanAlignAllParams, ScanAlignAllResult, ScanAlignLink, ScanAlignModelParams, ScanAlignModelResult, ScanAlignPairsParams, ScanAlignPairsResult,
    ScanAlignPass, ScanCombineParams, ScanCombineResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};

const ALL_CONTRACT: TypedContract<ScanAlignAllParams> = TypedContract::new();
const ALIGN_CONTRACT: TypedContract<crate::schema::scan_align::ScanAlignParams> = TypedContract::new();
const PAIRS_CONTRACT: TypedContract<ScanAlignPairsParams> = TypedContract::new();
const MODEL_CONTRACT: TypedContract<ScanAlignModelParams> = TypedContract::new();
const COMBINE_CONTRACT: TypedContract<ScanCombineParams> = TypedContract::new();

/// How finely a model is meshed to lay a scan on it: a hundredth of a millimetre off at most.
const MODEL_CHORD: f64 = 0.01;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.align", "Align", crate::checked!("scan.align", align))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&ALIGN_CONTRACT)
        .params("method: own_axes (as scan.quick_align) | to_scan (as scan.register) | all (as scan.align_all) | by_points (as scan.align_pairs) | to_model (as scan.align_to_model); the other parameters are that command's: one way of aligning a scan chosen by name → that command's reply"),
    CommandSpec::new("scan.combine", "Combine Scans", crate::checked!("scan.combine", combine))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&COMBINE_CONTRACT)
        .output(output_schema_for::<ScanCombineResult>)
        .params("body?: the scan the others are fused into and held where it is (default: the first selected scan, else the first); with?: [the other passes] (default: the other selected scans, else every other scan); align?: bool (default true: the passes laid on `body` first, as scan.align_all lays them; false when they already lie together); search?, overlap?: as scan.align_all; cell?: length, sharp?: bool as scan.remesh: the passes laid together and fused into `body` as one even surface (the other passes stay as they are, laid on it), one undo step → body, with, aligned? (scan.align_all's reply), fused (scan.remesh's reply)"),
    CommandSpec::new("scan.align_all", "Align All Scans", crate::checked!("scan.align_all", align_all))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&ALL_CONTRACT)
        .output(output_schema_for::<ScanAlignAllResult>)
        .params("bodies?: [scans, in the order they were made] (default: the selected scans, else every scan); hold?: [scans that stay] (default: the first); search?: bool (default true: a pass not yet lying on those before it is placed from wherever it lies, as Align to Scan places one); overlap?: 0.02–0.9 (default 0.1: the least share two passes must have in common to be fitted to each other): every overlapping pair fitted where they lie, then one move for each pass that agrees with all the fits (the errors shared out, not piled on the last); each pass moved kept as a Move of it, one undo step → passes [{body, held, placed, moved, searched?, feature?}], links [{a, b, overlap_before, overlap_after, rms_after, accepted, note?, disagreement?: [mm, degrees]}], rounds"),
    CommandSpec::new("scan.align_pairs", "Align by Points", crate::checked!("scan.align_pairs", align_pairs))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&PAIRS_CONTRACT)
        .output(output_schema_for::<ScanAlignPairsResult>)
        .params("body?: the scan to move (default: the selected or only one); from: [[x,y,z], …] points on it; onto: [[x,y,z], …] their partners, in order (three pairs or more, not all in a line); to?: the scan or model the partners are on; refine?: bool (default true with `to`: fitted onto it after the points, where its surface allows; refined_rms tells): the turn and shift that lay the points on their partners best (no scale, no mirror), kept as a Move of the scan, one undo step → body, to?, residuals, rms, worst, refined_rms?, matched?, moved, feature?"),
    CommandSpec::new("scan.align_to_model", "Align to Model", crate::checked!("scan.align_to_model", align_to_model))
        .at("SCAN", "CHECK")
        .icon("align")
        .contract(&MODEL_CONTRACT)
        .output(output_schema_for::<ScanAlignModelResult>)
        .params("body?: the scan to move (default: the selected or only one); model?: a solid or surface body (default: the selected one, else the only one); search?: bool (default true: from the ways the two could lie on each other too); ignored?: 0–0.6 (default 0.2: the share of the scan with no model under it, a fixture or the table): the scan fitted onto the model's surface (point-to-plane both ways, trimmed and robust), the model not moved, one undo step kept as a Move of the scan → body, model, moved, rms_before, rms_after, matched, start, converged, rival?, feature?"),
];

/// The ways of aligning a scan, as `scan.align` names them, and the command each runs.
pub const ALIGN_METHODS: [(&str, &str); 5] = [
    ("own_axes", "scan.quick_align"),
    ("to_scan", "scan.register"),
    ("all", "scan.align_all"),
    ("by_points", "scan.align_pairs"),
    ("to_model", "scan.align_to_model"),
];

/// Align: the way named by `method`, run with the other parameters.
fn align(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.align";
    let names = ALIGN_METHODS.map(|(n, _)| n).join(", ");
    let method = str_(p, "method").ok_or_else(|| bad(cmd, format!("`method` must be one of {names}")))?;
    let (_, id) = ALIGN_METHODS.iter().find(|(n, _)| *n == method).ok_or_else(|| bad(cmd, format!("`method` must be one of {names}")))?;
    let mut rest = p.clone();
    if let Some(o) = rest.as_object_mut() {
        o.remove("method");
    }
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand((*id).into()))?.run_nested(s, &rest)
}

pub use solidcraft_scan::align::ByPairs;

/// What laying `from` onto `onto` pair by pair would do, for a dialog's preview.
pub fn pairs_preview(from: &[Vec3], onto: &[Vec3]) -> std::result::Result<ByPairs, String> {
    align::by_pairs(from, onto).map_err(|e| e.to_string())
}

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// The scans of the design, in the order they were made.
fn scans(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// The bodies picked, in the order picked, each once.
fn selected(s: &Session) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in &s.selection {
        if let Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } = x
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    out
}

/// A model's surface (a solid or surface body, meshed finely), or a scan's triangles.
fn surface_of(s: &Session, name: &str, cmd: &str) -> Result<Mesh> {
    let state = s.model.state();
    let body = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
    if body.body.is_mesh() {
        return mesh(s, &json!({ "body": name }), cmd);
    }
    let surface =
        body.body.mesh(&solidcraft_kernel::MeshTolerance::chord(MODEL_CHORD)).map_err(|e| bad(cmd, format!("`{name}` could not be meshed: {e}")))?;
    Ok(Mesh::made(name.to_owned(), surface))
}

fn align_all(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.align_all";
    let all = scans(s);
    let bodies: Vec<String> = match p.get("bodies").filter(|v| !v.is_null()) {
        Some(_) => crate::params::string_list_or_default(p, "bodies"),
        None => {
            let picked: Vec<String> = selected(s).into_iter().filter(|n| all.contains(n)).collect();
            if picked.len() >= 2 { picked } else { all.clone() }
        }
    };
    if bodies.len() < 2 {
        return Err(bad(cmd, "align needs two passes or more: insert the other passes with Insert Mesh"));
    }
    for name in &bodies {
        if !all.contains(name) {
            return Err(bad(cmd, format!("`{name}` is not a scan")));
        }
    }
    let hold = crate::params::string_list_or_default(p, "hold");
    for name in &hold {
        if !bodies.contains(name) {
            return Err(bad(cmd, format!("`hold` names `{name}`, which is not among the passes")));
        }
    }
    let overlap = num(p, "overlap").unwrap_or(0.1);
    if !(0.02..=0.9).contains(&overlap) {
        return Err(bad(cmd, "`overlap` is a share from 0.02 to 0.9"));
    }
    let settings = AllSettings { search: bool_(p, "search").unwrap_or(true), least_overlap: overlap };
    let meshes: Vec<Mesh> = bodies.iter().map(|b| mesh(s, &json!({ "body": b }), cmd)).collect::<Result<_>>()?;
    let held: Vec<bool> = bodies.iter().map(|b| hold.contains(b)).collect();
    let passes: Vec<Scan> = meshes.iter().map(Mesh::scan).collect();
    let done = align::align_all(&passes, &held, &settings).map_err(|e| bad(cmd, e.to_string()))?;
    let held_now: Vec<bool> = if held.iter().any(|&h| h) { held } else { (0..bodies.len()).map(|i| i == 0).collect() };
    let mut out = Vec::with_capacity(bodies.len());
    for (i, m) in meshes.iter().enumerate() {
        let feature = if done.placed[i] && !held_now[i] { move_by(s, m, &done.moves[i])? } else { None };
        out.push(ScanAlignPass {
            body: m.body.clone(),
            held: held_now[i],
            placed: done.placed[i],
            moved: feature.is_some(),
            searched: done.searched[i].map(str::to_owned),
            feature,
        });
    }
    let links = done
        .links
        .iter()
        .map(|l| ScanAlignLink {
            a: bodies[l.a].clone(),
            b: bodies[l.b].clone(),
            overlap_before: l.overlap_before,
            overlap_after: l.overlap_after,
            rms_after: l.rms_after,
            accepted: l.accepted,
            note: l.note.clone(),
            disagreement: l.disagreement.map(|(mm, deg)| [mm, deg]),
        })
        .collect();
    if !done.placed.iter().all(|&x| x) {
        let left: Vec<&str> = bodies.iter().zip(&done.placed).filter(|(_, p)| !**p).map(|(b, _)| b.as_str()).collect();
        if left.len() == bodies.len() - 1 && out.iter().all(|o| !o.moved) {
            return Err(bad(
                cmd,
                format!("no pass could be laid on the others ({}): align one by points first, or lower `overlap`", left.join(", ")),
            ));
        }
    }
    reply(ScanAlignAllResult { passes: out, links, rounds: done.rounds, param_warnings: Vec::new() })
}

/// A list of points `key`.
fn points(p: &Value, key: &str, cmd: &str) -> Result<Vec<Vec3>> {
    p.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, format!("`{key}` must list points [[x, y, z], …]")))?
        .iter()
        .map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, format!("a point of `{key}` must be [x, y, z]"))))
        .collect()
}

fn align_pairs(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.align_pairs";
    let (from, onto) = (points(p, "from", cmd)?, points(p, "onto", cmd)?);
    if from.len() != onto.len() {
        return Err(bad(cmd, format!("`from` has {} points and `onto` {}: give them in pairs", from.len(), onto.len())));
    }
    let scan = mesh(s, p, cmd)?;
    let to = str_(p, "to").filter(|t| !t.trim().is_empty()).map(str::to_owned);
    if to.as_deref() == Some(scan.body.as_str()) {
        return Err(bad(cmd, "a scan cannot be laid onto itself"));
    }
    let refine = bool_(p, "refine").unwrap_or(to.is_some());
    if refine && to.is_none() {
        return Err(bad(cmd, "`refine` needs `to`: the scan or model the points were picked on"));
    }
    let found = align::by_pairs(&from, &onto).map_err(|e| bad(cmd, e.to_string()))?;
    let mut moved = found.moved;
    let (mut refined_rms, mut matched) = (None, None);
    if let (true, Some(target)) = (refine, to.as_deref()) {
        let reference = surface_of(s, target, cmd)?;
        let placed: Vec<Vec3> = scan.positions.iter().map(|q| moved.apply(*q)).collect();
        // A finishing step: where the surfaces are too sparse to fit, the points' move stands.
        if let Ok(fit) = register::register(&Scan::new(&placed, &scan.triangles), &reference.scan(), &Settings { search: false, ignored: 0.3 }) {
            // Kept only when it brings the scan closer than the points alone laid it.
            if fit.rms_after <= fit.rms_before {
                moved = moved.then(&fit.moved);
                refined_rms = Some(fit.rms_after);
            } else {
                refined_rms = Some(fit.rms_before);
            }
            matched = Some(fit.matched);
        }
    }
    let feature = move_by(s, &scan, &moved)?;
    reply(ScanAlignPairsResult {
        body: scan.body.clone(),
        to,
        worst: found.worst(),
        residuals: found.residuals,
        rms: found.rms,
        refined_rms,
        matched,
        moved: feature.is_some(),
        feature,
        param_warnings: Vec::new(),
    })
}

/// The model a scan is laid on: `model`, else the selected solid or surface body, else the only one.
fn model_name(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    let state = s.model.state();
    if let Some(name) = str_(p, "model").filter(|n| !n.trim().is_empty()) {
        let body = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
        if body.body.is_mesh() {
            return Err(bad(cmd, format!("`{name}` is a scan: lay one scan on another with Align to Scan")));
        }
        return Ok(name.to_owned());
    }
    let solid = |n: &str| state.body(n).is_some_and(|b| !b.body.is_mesh());
    let picked: Vec<String> = selected(s).into_iter().filter(|n| solid(n)).collect();
    if let [one] = picked.as_slice() {
        return Ok(one.clone());
    }
    let models: Vec<&str> = state.bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.as_str()).collect();
    match models.as_slice() {
        [one] => Ok((*one).to_owned()),
        [] => Err(bad(cmd, "the design has no model to lay the scan on: import its CAD, or model it")),
        _ => Err(bad(cmd, "select the model (a solid or surface body) to lay the scan on")),
    }
}

/// The scan a model alignment moves: `body`, else the selected scan, else the only one.
fn scan_name(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    if let Some(name) = str_(p, "body").filter(|n| !n.trim().is_empty()) {
        return Ok(name.to_owned());
    }
    let all = scans(s);
    let picked: Vec<String> = selected(s).into_iter().filter(|n| all.contains(n)).collect();
    match (picked.as_slice(), all.as_slice()) {
        ([one], _) | ([], [one]) => Ok(one.clone()),
        (_, []) => Err(bad(cmd, "the design has no scan: insert one with Insert Mesh")),
        _ => Err(bad(cmd, "select the scan to lay on the model")),
    }
}

fn align_to_model(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.align_to_model";
    let ignored = num(p, "ignored").unwrap_or(0.2);
    if !(0.0..=0.6).contains(&ignored) {
        return Err(bad(cmd, "`ignored` is a share from 0 to 0.6"));
    }
    let model = model_name(s, p, cmd)?;
    let body = scan_name(s, p, cmd)?;
    let scan = mesh(s, &json!({ "body": body }), cmd)?;
    let reference = surface_of(s, &model, cmd)?;
    let settings = Settings { search: bool_(p, "search").unwrap_or(true), ignored };
    let found = register::register(&scan.scan(), &reference.scan(), &settings).map_err(|e| bad(cmd, e.to_string()))?;
    let feature = move_by(s, &scan, &found.moved)?;
    reply(ScanAlignModelResult {
        body: scan.body.clone(),
        model,
        moved: feature.is_some(),
        rms_before: found.rms_before,
        rms_after: found.rms_after,
        matched: found.matched,
        start: found.start.to_string(),
        converged: found.converged,
        rival: found.rival,
        feature,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_align_tests.rs"]
mod tests;

/// Combine Scans: Align All Scans (`body` held), then Remesh with the other passes fused in, as
/// one command and one undo step. A failure in either leaves the design as it was.
fn combine(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.combine";
    let all = scans(s);
    let picked: Vec<String> = selected(s).into_iter().filter(|n| all.contains(n)).collect();
    let base = match str_(p, "body").filter(|b| !b.trim().is_empty()) {
        Some(b) => b.to_owned(),
        None => picked.first().or(all.first()).cloned().ok_or_else(|| bad(cmd, "the design has no scan: insert the passes with Insert Mesh"))?,
    };
    if !all.contains(&base) {
        return Err(bad(cmd, format!("`{base}` is not a scan")));
    }
    let with: Vec<String> = match p.get("with").filter(|v| !v.is_null()) {
        Some(_) => crate::params::string_list_or_default(p, "with"),
        None if picked.len() >= 2 => picked.iter().filter(|n| **n != base).cloned().collect(),
        None => all.iter().filter(|n| **n != base).cloned().collect(),
    };
    if with.is_empty() {
        return Err(bad(cmd, "combining needs two scans or more: insert the other passes with Insert Mesh"));
    }
    for name in &with {
        if !all.contains(name) {
            return Err(bad(cmd, format!("`{name}` is not a scan")));
        }
        if *name == base {
            return Err(bad(cmd, "`with` names the scan itself"));
        }
    }
    let command = |id: &str| super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_owned()));
    // The keys one step takes and the other does not are passed on only to the step that has them.
    let pass_on = |keys: &[&str], mut to: serde_json::Map<String, Value>| {
        for key in keys {
            if let Some(v) = p.get(*key).filter(|v| !v.is_null()) {
                to.insert((*key).to_owned(), v.clone());
            }
        }
        Value::Object(to)
    };
    let aligned = if bool_(p, "align").unwrap_or(true) {
        let bodies: Vec<String> = std::iter::once(base.clone()).chain(with.iter().cloned()).collect();
        let mut given = serde_json::Map::new();
        given.insert("bodies".into(), json!(bodies));
        given.insert("hold".into(), json!([base]));
        Some(command("scan.align_all")?.run_nested(s, &pass_on(&["search", "overlap"], given))?)
    } else {
        None
    };
    let mut given = serde_json::Map::new();
    given.insert("body".into(), json!(base));
    given.insert("with".into(), json!(with));
    let fused = command("scan.remesh")?.run_nested(s, &pass_on(&["cell", "sharp"], given))?;
    reply(ScanCombineResult { body: base, with, aligned, fused, param_warnings: Vec::new() })
}
