//! Cleaning a scan: the tools that change the scan's own triangles (Remove Floating Pieces, Fill
//! Holes, Reduce, Repair). A scan is kept in the Mesh Import feature that brought it in, so a
//! cleaned scan is written back into that feature, in one undo step and with no format change.
//! A scan a later feature has moved (a Move, Stand on Base) is written back moved the other way,
//! so the later features still put it where it is; one a later feature has reshaped is refused.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_scan::Scan;
use solidcraft_scan::pieces::{self, Pieces};
use solidcraft_scan::place::Rigid;

use super::mesh_name;
use crate::params::bad;
use crate::schema::scan::ScanFloatingResult;
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
    // The stored mesh as the kernel builds the body from it (equal points welded, in order).
    let kept: Vec<Vec3> = match s.doc.feature(feature).map(|f| &f.kind) {
        Some(solidcraft_doc::FeatureKind::MeshImport { meshes, .. }) => meshes
            .get(k)
            .and_then(|m| {
                let points: Vec<Vec3> = m.positions.as_chunks::<3>().0.iter().map(|c| Vec3::new(c[0], c[1], c[2])).collect();
                let built = solidcraft_kernel::mesh_body(&points, m.triangles.as_chunks::<3>().0).ok()?;
                built.triangle_mesh().map(|(p, _)| p.to_vec())
            })
            .unwrap_or_default(),
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
