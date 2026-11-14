//! Cleaning a scan: the tools that change the scan's own triangles (Remove Floating Pieces, Fill
//! Holes, Reduce, Repair, and Denoise and Remesh in `scan_mesh`). Each adds a Scan Clean step to
//! the timeline that keeps only the tool's settings (`solidcraft_doc::scan_clean`): the scan stays
//! in its Mesh Import as it came in and is made again, step by step, when the design is evaluated,
//! so a step can be edited, suppressed or deleted later.
//!
//! With `in_place` (the Scan preference "Clean scans in place", the way before) the cleaned scan
//! is written back into the Mesh Import instead, in one undo step and with no step in the
//! timeline. A scan a later feature has moved (a Move, Stand on Base) is then written back moved
//! the other way, so the later features still put it where it is; one a later feature has
//! reshaped is refused.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_doc::FeatureKind;
use solidcraft_doc::scan_clean::{self, CleanStep, Cleaned, Report};
use solidcraft_scan::Scan;
use solidcraft_scan::holes::{self, Survey};
use solidcraft_scan::pieces::{self, Pieces};
use solidcraft_scan::place::Rigid;

use super::{length, mesh_name};
use crate::params::bad;
use crate::schema::scan::{ScanFloatingResult, ScanHolesResult, ScanReduceResult, ScanRepairResult};
use crate::{EngineError, Result, Session};

/// The default share of the largest piece below which a piece floats.
pub const FLOATING_SHARE: f64 = 0.02;

/// Where a scan is kept: its Mesh Import feature and which of its meshes it is.
fn source(s: &Session, body: &str) -> Option<(u64, usize)> {
    let state = s.model.state();
    let made_by = state.body(body)?.feature;
    let named = |f: &solidcraft_doc::Feature, k: usize, m: &solidcraft_doc::MeshData| {
        f.body_names.get(k).filter(|n| !n.trim().is_empty()).unwrap_or(&m.name) == body
    };
    s.doc.features.iter().rev().find_map(|f| {
        let solidcraft_doc::FeatureKind::MeshImport { meshes, .. } = &f.kind else { return None };
        if f.id == made_by {
            // The k-th body the feature made.
            let k = state.bodies.iter().filter(|b| b.feature == f.id).position(|b| b.name == body)?;
            return (k < meshes.len()).then_some((f.id, k));
        }
        meshes.iter().enumerate().find(|(k, m)| named(f, *k, m)).map(|(k, _)| (f.id, k))
    })
}

/// Replaces the scan `body`'s triangles with `positions` and `triangles` (where the scan lies
/// now), in the Mesh Import feature that keeps it: one undo step.
pub fn replace_mesh(s: &mut Session, body: &str, positions: &[Vec3], triangles: &[[u32; 3]], cmd: &str) -> Result<u64> {
    let (feature, k) =
        source(s, body).ok_or_else(|| bad(cmd, format!("`{body}` is not a scan brought in by Insert Mesh, so it cannot be changed here")))?;
    let now: Vec<Vec3> = s.model.state().body(body).and_then(|b| b.body.triangle_mesh()).map(|(p, _)| p.to_vec()).unwrap_or_default();
    // The stored points as the kernel builds the body from them: equal points (to the bit) welded,
    // the first of each kept, in order.
    let kept: Vec<Vec3> = match s.doc.feature(feature).map(|f| &f.kind) {
        Some(solidcraft_doc::FeatureKind::MeshImport { meshes, .. }) => meshes.get(k).map(|m| welded_points(&m.positions)).unwrap_or_default(),
        _ => Vec::new(),
    };
    // Where the scan lies now against where it was brought in: the same, or moved rigidly.
    let back = if kept == now {
        None
    } else {
        Some(
            Rigid::between(&kept, &now)
                .ok_or_else(|| bad(cmd, format!("`{body}` has been reshaped by a later feature: clean it before that")))?
                .inverse(),
        )
    };
    let flat: Vec<f64> = positions.iter().map(|p| back.as_ref().map_or(*p, |b| b.apply(*p))).flat_map(|p| [p.x, p.y, p.z]).collect();
    let tris: Vec<u32> = triangles.iter().flatten().copied().collect();
    let f = s.doc_mut().feature_mut(feature).ok_or_else(|| bad(cmd, "the scan's feature is gone"))?;
    if let solidcraft_doc::FeatureKind::MeshImport { meshes, .. } = &mut f.kind
        && let Some(m) = meshes.get_mut(k)
    {
        m.positions = Arc::from(flat);
        m.triangles = Arc::from(tris);
    }
    s.refresh();
    match s.model.result(feature).and_then(|r| r.error.clone()) {
        Some(e) => Err(EngineError::Other(e)),
        None => Ok(feature),
    }
}

/// Flat x, y, z coordinates as points, equal ones (to the bit) kept once, first first: the points
/// of the body the kernel builds from them (`kernel::mesh_body`), without building it.
fn welded_points(flat: &[f64]) -> Vec<Vec3> {
    let mut seen: std::collections::HashSet<[u64; 3]> = std::collections::HashSet::with_capacity(flat.len() / 3);
    flat.as_chunks::<3>()
        .0
        .iter()
        .filter(|c| seen.insert([(c[0] + 0.0).to_bits(), (c[1] + 0.0).to_bits(), (c[2] + 0.0).to_bits()]))
        .map(|c| Vec3::new(c[0], c[1], c[2]))
        .collect()
}

/// The pieces last found: for which scan (its name and where its points are held) and the pieces.
type CachedPieces = (String, usize, usize, Arc<Pieces>);
static PIECES: std::sync::Mutex<Option<CachedPieces>> = std::sync::Mutex::new(None);

/// The pieces of the scan `body` (or the selected or only one), largest first: found once per scan
/// and kept while the scan is unchanged.
pub fn pieces_of(s: &Session, body: Option<&str>, cmd: &str) -> Result<(String, Arc<Pieces>)> {
    let name = mesh_name(s, &json!({ "body": body }), cmd)?;
    let state = s.model.state();
    let (positions, triangles) =
        state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let key = (positions.as_ptr() as usize, positions.len());
    if let Ok(cached) = PIECES.lock()
        && let Some((n, ptr, len, p)) = cached.as_ref()
        && *n == name
        && (*ptr, *len) == key
    {
        return Ok((name, p.clone()));
    }
    let found = Arc::new(pieces::pieces(&Scan::new(positions, triangles)));
    if let Ok(mut cached) = PIECES.lock() {
        *cached = Some((name.clone(), key.0, key.1, found.clone()));
    }
    Ok((name, found))
}

/// The piece of `found` (the pieces of the scan `body`) under `point`: the piece of the
/// triangle nearest it.
pub fn piece_at(s: &Session, body: &str, found: &Pieces, point: Vec3) -> Option<usize> {
    let state = s.model.state();
    let (positions, triangles) = state.body(body).and_then(|b| b.body.triangle_mesh())?;
    let t = solidcraft_scan::base::nearest_triangle(&Scan::new(positions, triangles), point)?;
    found.of.get(t).copied().filter(|&k| k != u32::MAX).map(|k| k as usize)
}

/// The share parameter `below`, 0 to 0.5.
fn share(p: &Value, cmd: &str) -> Result<f64> {
    match p.get("below").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=0.5).contains(x)).ok_or_else(|| bad(cmd, "`below` must be a share of the largest piece, 0 to 0.5")),
        None => Ok(FLOATING_SHARE),
    }
}

/// The default largest hole Fill Holes closes, in edges round it.
pub const MAX_EDGES: usize = 300;

/// The rings last surveyed: for which scan (its name and where its points are held) and the survey.
type CachedSurvey = (String, usize, usize, Arc<Survey>);
static SURVEY: std::sync::Mutex<Option<CachedSurvey>> = std::sync::Mutex::new(None);

/// The holes and rims of the scan `body` (or the selected or only one): surveyed once per scan and
/// kept while the scan is unchanged.
pub fn holes_of(s: &Session, body: Option<&str>, cmd: &str) -> Result<(String, Arc<Survey>)> {
    let name = mesh_name(s, &json!({ "body": body }), cmd)?;
    let state = s.model.state();
    let (positions, triangles) =
        state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let key = (positions.as_ptr() as usize, positions.len());
    if let Ok(cached) = SURVEY.lock()
        && let Some((n, ptr, len, found)) = cached.as_ref()
        && *n == name
        && (*ptr, *len) == key
    {
        return Ok((name, found.clone()));
    }
    let found = Arc::new(holes::survey(&Scan::new(positions, triangles)));
    if let Ok(mut cached) = SURVEY.lock() {
        *cached = Some((name.clone(), key.0, key.1, found.clone()));
    }
    Ok((name, found))
}

/// Whether a clean tool writes the scan back into its Mesh Import (`in_place`, the old way, kept
/// behind a preference) rather than adding a Scan Clean step to the timeline.
fn in_place(p: &Value) -> bool {
    crate::params::bool_(p, "in_place").unwrap_or(false)
}

/// `step` done on the scan `body`: as a Scan Clean step added to the timeline (its settings only;
/// the scan is made again from the one before it each time the design is evaluated), or with
/// `in_place` written back into the scan's Mesh Import. What it did, and the feature that keeps it.
pub(super) fn apply(s: &mut Session, p: &Value, body: &str, step: CleanStep, cmd: &str) -> Result<(Cleaned, u64)> {
    step.check().map_err(|e| bad(cmd, e.to_string()))?;
    let (done, component) = {
        let state = s.model.state();
        let scan = state.body(body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
        if scan.body.triangle_mesh().is_none() {
            return Err(bad(cmd, format!("`{body}` is not a scan (a mesh body)")));
        }
        let mut with = Vec::with_capacity(step.with().len());
        for name in step.with() {
            if name.0 == body {
                return Err(bad(cmd, "`with` names the scan itself"));
            }
            let other = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{}`", name.0)))?;
            if other.body.triangle_mesh().is_none() {
                return Err(bad(cmd, format!("`{}` is not a scan (a mesh body)", name.0)));
            }
            with.push(&other.body);
        }
        (scan_clean::clean(&step, &scan.body, &with).map_err(|e| bad(cmd, e.why()))?, s.doc.model_body_component(scan))
    };
    if in_place(p) {
        let (positions, triangles) = done.body.triangle_mesh().ok_or_else(|| bad(cmd, "the cleaned scan has no triangles"))?;
        let (positions, triangles) = (positions.to_vec(), triangles.to_vec());
        let feature = replace_mesh(s, body, &positions, &triangles, cmd)?;
        return Ok((done, feature));
    }
    let mut params = json!({});
    if let Some(name) = crate::params::str_(p, "name") {
        params["name"] = json!(name);
    }
    // The step belongs where the scan does: an inserted scan is in a component of its own.
    let active = std::mem::replace(&mut s.active_component, component);
    let added = super::super::features::add_feature_in(s, &params, FeatureKind::ScanClean { body: body.into(), step }, false);
    s.active_component = active;
    let feature = added?.get("feature").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the step was not added"))?;
    Ok((done, feature))
}

/// Points clicked on the scan, `key` a list of [x, y, z]; none when not given.
fn points(p: &Value, key: &str, cmd: &str) -> Result<Vec<[f64; 3]>> {
    let Some(list) = p.get(key).filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let why = || bad(cmd, format!("`{key}` must be a list of at most 10000 points [x, y, z]"));
    let list = list.as_array().filter(|l| l.len() <= 10_000).ok_or_else(why)?;
    list.iter().map(|v| crate::params::vec3(v).map(|q| [q.x, q.y, q.z]).ok_or_else(why)).collect()
}

pub(super) fn remove_floating(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.remove_floating";
    let below = share(p, cmd)?;
    let name = mesh_name(s, p, cmd)?;
    let (keep, remove) = (points(p, "keep", cmd)?, points(p, "remove", cmd)?);
    let (done, feature) = apply(s, p, &name, CleanStep::RemoveFloating { below, keep, remove }, cmd)?;
    let Report::Floating { pieces, removed, removed_triangles } = done.report else { return Err(bad(cmd, "the step did something else")) };
    let reply =
        ScanFloatingResult { body: name, pieces, removed, removed_triangles, triangles: done.triangles(), feature, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub(super) fn fill_holes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fill_holes";
    let max_edges = match p.get("max_edges").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| (3..=5000).contains(n)).ok_or_else(|| bad(cmd, "`max_edges` must be a whole number from 3 to 5000"))? as u32,
        None => MAX_EDGES as u32,
    };
    let fair = crate::params::bool_(p, "fair").unwrap_or(true);
    let name = mesh_name(s, p, cmd)?;
    let at = points(p, "at", cmd)?;
    let (done, feature) = apply(s, p, &name, CleanStep::FillHoles { max_edges, fair, at }, cmd)?;
    let Report::Holes { holes, filled, failed, larger, added_triangles, unfillable_edges } = done.report else {
        return Err(bad(cmd, "the step did something else"));
    };
    let reply = ScanHolesResult { body: name, holes, filled, failed, larger, added_triangles, unfillable_edges, feature, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub(super) fn reduce(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.reduce";
    let ratio = match p.get("ratio").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|x| *x > 0.0 && *x <= 1.0).ok_or_else(|| bad(cmd, "`ratio` must be a share above 0 and at most 1"))?),
        None => None,
    };
    let triangles = match p.get("triangles").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().filter(|n| *n >= 1).ok_or_else(|| bad(cmd, "`triangles` must be a whole number from 1"))?),
        None => None,
    };
    if ratio.is_some() && triangles.is_some() {
        return Err(bad(cmd, "give `ratio` or `triangles`, not both"));
    }
    let max_error = match crate::params::expr(p, "max_error") {
        Some(_) => length(s, p, "max_error", 1.0, cmd)?,
        None => 0.0,
    };
    let keep_rim = crate::params::bool_(p, "keep_rim").unwrap_or(true);
    let name = mesh_name(s, p, cmd)?;
    let (done, feature) = apply(s, p, &name, CleanStep::Reduce { ratio, triangles, max_error, keep_rim }, cmd)?;
    let count = done.triangles();
    let Report::Reduce { before, collapses, stopped, largest_error } = done.report else { return Err(bad(cmd, "the step did something else")) };
    let reply = ScanReduceResult {
        body: name,
        before,
        triangles: count,
        collapses,
        stopped: stopped.into(),
        largest_error,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub(super) fn repair(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.repair";
    let name = mesh_name(s, p, cmd)?;
    let crossings = crate::params::bool_(p, "crossings").unwrap_or(true);
    let (done, feature) = apply(s, p, &name, CleanStep::Repair { crossings }, cmd)?;
    let count = done.triangles();
    let Report::Repair { duplicates, non_manifold, slivers, crossings, crossing_faces, refilled } = done.report else {
        return Err(bad(cmd, "the step did something else"));
    };
    let reply = ScanRepairResult {
        body: name,
        duplicates,
        non_manifold,
        slivers,
        crossings,
        crossing_faces,
        refilled,
        triangles: count,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
