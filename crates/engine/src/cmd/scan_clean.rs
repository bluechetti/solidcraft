//! Cleaning a scan: the tools that change the scan's own triangles (Remove Floating Pieces, Fill
//! Holes, Reduce, Repair). A scan is kept in the Mesh Import feature that brought it in, so a
//! cleaned scan is written back into that feature, in one undo step and with no format change.
//! A scan a later feature has moved (a Move, Stand on Base) is written back moved the other way,
//! so the later features still put it where it is; one a later feature has reshaped is refused.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_scan::Scan;
use solidcraft_scan::holes::{self, Survey};
use solidcraft_scan::pieces::{self, Pieces};
use solidcraft_scan::place::Rigid;

use super::{length, mesh_name};
use crate::params::bad;
use crate::schema::scan::{ScanFloatingResult, ScanHolesResult, ScanReduceResult};
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

/// The share parameter `below`, 0 to 0.5.
fn share(p: &Value, cmd: &str) -> Result<f64> {
    match p.get("below").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=0.5).contains(x)).ok_or_else(|| bad(cmd, "`below` must be a share of the largest piece, 0 to 0.5")),
        None => Ok(FLOATING_SHARE),
    }
}

pub(super) fn remove_floating(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.remove_floating";
    let below = share(p, cmd)?;
    let (name, found) = pieces_of(s, crate::params::str_(p, "body"), cmd)?;
    let floating = found.floating(below);
    let removed: usize = floating.iter().filter(|f| **f).count();
    if removed == 0 {
        return Err(bad(cmd, format!("no piece floats: all {} are at least {:.1} % of the largest", found.sizes.len(), 100.0 * below)));
    }
    let (positions, triangles) = {
        let state = s.model.state();
        let (p, t) = state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, "the scan is gone"))?;
        pieces::keeping(&Scan::new(p, t), |t| found.of.get(t).is_some_and(|&k| k != u32::MAX && !floating[k as usize]))
    };
    let gone: usize = found.sizes.iter().zip(&floating).filter(|(_, f)| **f).map(|(n, _)| n).sum();
    let feature = replace_mesh(s, &name, &positions, &triangles, cmd)?;
    let reply = ScanFloatingResult {
        body: name,
        pieces: found.sizes.len(),
        removed,
        removed_triangles: gone,
        triangles: triangles.len(),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
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

pub(super) fn fill_holes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fill_holes";
    let limit = match p.get("max_edges").filter(|v| !v.is_null()) {
        Some(v) => {
            v.as_u64().filter(|n| (3..=5000).contains(n)).ok_or_else(|| bad(cmd, "`max_edges` must be a whole number from 3 to 5000"))? as usize
        }
        None => MAX_EDGES,
    };
    let fair = crate::params::bool_(p, "fair").unwrap_or(true);
    let (name, found) = holes_of(s, crate::params::str_(p, "body"), cmd)?;
    let holes = found.rings.iter().filter(|r| !r.rim).count();
    let within = found.holes(limit).count();
    if within == 0 {
        return Err(bad(
            cmd,
            if holes == 0 {
                let pinched = if found.unfillable_edges > 0 {
                    format!(" ({} open edges pinch or dead-end, so they make no ring to close)", found.unfillable_edges)
                } else {
                    String::new()
                };
                match found.rings.len() {
                    0 if found.unfillable_edges == 0 => "the scan has no holes: it is closed".to_owned(),
                    0 => format!("the scan has no holes to close{pinched}"),
                    _ => format!("the scan has no holes to close: its only open ring is its outer rim{pinched}"),
                }
            } else {
                format!(
                    "no hole has {limit} edges or fewer: the smallest has {}",
                    found.rings.iter().filter(|r| !r.rim).map(|r| r.points.len()).min().unwrap_or(0)
                )
            },
        ));
    }
    let done = {
        let state = s.model.state();
        let (positions, triangles) = state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, "the scan is gone"))?;
        holes::fill(&Scan::new(positions, triangles), &found, limit, fair)
    }
    .ok_or_else(|| bad(cmd, "no hole could be closed: each ring folds or crosses the scan"))?;
    let feature = replace_mesh(s, &name, &done.positions, &done.triangles, cmd)?;
    let reply = ScanHolesResult {
        body: name,
        holes,
        filled: done.filled,
        failed: done.failed,
        larger: holes - within,
        added_triangles: done.added_triangles,
        unfillable_edges: found.unfillable_edges,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub(super) fn reduce(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.reduce";
    let ratio = match p.get("ratio").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|x| *x > 0.0 && *x <= 1.0).ok_or_else(|| bad(cmd, "`ratio` must be a share above 0 and at most 1"))?),
        None => None,
    };
    let count = match p.get("triangles").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().filter(|n| *n >= 1).ok_or_else(|| bad(cmd, "`triangles` must be a whole number from 1"))? as usize),
        None => None,
    };
    if ratio.is_some() && count.is_some() {
        return Err(bad(cmd, "give `ratio` or `triangles`, not both"));
    }
    let max_error = match crate::params::expr(p, "max_error") {
        Some(_) => length(s, p, "max_error", 1.0, cmd)?,
        None => 0.0,
    };
    let keep_rim = crate::params::bool_(p, "keep_rim").unwrap_or(true);
    let name = mesh_name(s, p, cmd)?;
    let done = {
        let state = s.model.state();
        let (positions, triangles) = state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, "the scan is gone"))?;
        let before = triangles.len();
        // The count to keep (at least one), or none when only the error was asked for; half by default.
        let target = match (ratio, count) {
            (Some(r), _) => ((r * before as f64).round() as usize).max(1),
            (_, Some(n)) => n,
            _ if max_error > 0.0 => 0,
            _ => (before / 2).max(1),
        };
        if target >= before && max_error == 0.0 {
            return Err(bad(cmd, format!("the scan has {before} triangles already: ask for fewer")));
        }
        let settings = solidcraft_scan::reduce::Settings { target, max_error, keep_rim };
        (
            before,
            solidcraft_scan::reduce::reduce(&Scan::new(positions, triangles), settings)
                .ok_or_else(|| bad(cmd, "the scan has no triangles to reduce"))?,
        )
    };
    let (before, reduced) = done;
    if reduced.collapses == 0 {
        return Err(bad(cmd, "nothing could be collapsed within the error allowed"));
    }
    let feature = replace_mesh(s, &name, &reduced.positions, &reduced.triangles, cmd)?;
    let reply = ScanReduceResult {
        body: name,
        before,
        triangles: reduced.triangles.len(),
        collapses: reduced.collapses,
        stopped: reduced.stopped.name().into(),
        largest_error: reduced.largest_error,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
