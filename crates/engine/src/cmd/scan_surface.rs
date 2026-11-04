//! Editing a fitted surface by its control points (`scan.edit_surface`): the surface Fit Surface
//! made (one B-spline face, kept as STEP in an Import feature) has its control points moved, and
//! those round them or all the others fitted to the scan again. The Import feature's STEP is
//! replaced, so an edit is one undo step and the design opens anywhere, with no format change.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcore_record::brep as b;
use solidcraft_geom::Vec3Geom as _;
use solidcraft_geom::bspline::{HPoint, NurbsSurface};
use solidcraft_scan::surface_edit;

use super::{length, mesh};
use crate::params::{bad, str_};
use crate::schema::scan::ScanEditSurfaceResult;
use crate::{EngineError, Result, Sel, Session};

/// The most scan points a refit uses: enough to pin a net of 30 × 30, few enough to refit as a
/// point is dragged.
pub const REFIT_POINTS: usize = 8_000;

/// A surface that can be edited, as it now is.
#[derive(Clone, Debug)]
pub struct EditableSurface {
    /// The surface body and the Import feature that keeps it.
    pub body: String,
    pub feature: u64,
    pub surface: NurbsSurface,
}

/// The surface's one face as a NURBS surface, `None` for anything else.
fn nurbs_of(record: &b::Brep) -> Option<NurbsSurface> {
    let [face] = record.faces.as_slice() else { return None };
    let (u_knots, v_knots, rows): (&Vec<f64>, &Vec<f64>, Vec<Vec<HPoint>>) = match &face.surface {
        b::Surface::Spline { u_knots, v_knots, points } => {
            (u_knots, v_knots, points.iter().map(|row| row.iter().map(|p| HPoint::weighted(Vec3::new(p[0], p[1], p[2]), 1.0)).collect()).collect())
        }
        b::Surface::RationalSpline { u_knots, v_knots, homogeneous } => {
            (u_knots, v_knots, homogeneous.iter().map(|row| row.iter().map(|h| HPoint { x: h[0], y: h[1], z: h[2], w: h[3] }).collect()).collect())
        }
        _ => return None,
    };
    // The record's rows run along u (`rows[i][j]`, i along u); the surface's points go u fastest.
    let nu = rows.len();
    let nv = rows.first()?.len();
    if nu == 0 || nv == 0 || rows.iter().any(|r| r.len() != nv) {
        return None;
    }
    let degree = [u_knots.len().checked_sub(nu + 1)?, v_knots.len().checked_sub(nv + 1)?];
    let ctrl = (0..nv).flat_map(|j| rows.iter().map(move |row| row[j])).collect();
    NurbsSurface::from_homogeneous(degree, [u_knots.clone(), v_knots.clone()], ctrl).ok()
}

/// The surface body named, or the selected one, or the design's only one: a body of one B-spline
/// face, kept as it was imported (nothing later has changed it).
pub fn editable(s: &Session, name: Option<&str>, cmd: &str) -> Result<EditableSurface> {
    let state = s.model.state();
    let usable = |name: &str| -> Option<EditableSurface> {
        let body = state.body(name).filter(|b| !b.body.is_mesh())?;
        let feature = s.doc.features.iter().find(|f| f.id == body.feature)?;
        let solidcraft_doc::FeatureKind::Import { step, .. } = &feature.kind else { return None };
        if state.bodies.iter().filter(|b| b.feature == body.feature).count() != 1 {
            return None;
        }
        let surface = nurbs_of(&solidcraft_kernel::brep_record(&body.body).ok()?)?;
        // As imported: the file's own surface is this one, so writing it back changes nothing else.
        let imported = solidcraft_kernel::step_import_shared(step).ok()?;
        let [only] = imported.bodies.as_slice() else { return None };
        let original = nurbs_of(&solidcraft_kernel::brep_record(&only.body).ok()?)?;
        (original == surface).then(|| EditableSurface { body: name.to_owned(), feature: feature.id, surface })
    };
    let named = name.filter(|n| !n.trim().is_empty());
    if let Some(name) = named {
        if state.body(name).is_none() {
            return Err(bad(cmd, format!("no body `{name}`")));
        }
        return usable(name).ok_or_else(|| {
            bad(
                cmd,
                format!(
                    "`{name}` is not a surface whose control points can be edited: one B-spline face, as Fit Surface makes it, not changed since"
                ),
            )
        });
    }
    let selected: Vec<&str> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if let Some(found) = selected.iter().find_map(|n| usable(n)) {
        return Ok(found);
    }
    let all: Vec<EditableSurface> = state.bodies.iter().filter_map(|b| usable(&b.name)).collect();
    match all.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(bad(cmd, "the design has no surface to edit: fit one with Fit Surface")),
        _ => Err(bad(cmd, "select the surface to edit")),
    }
}

/// The scan points under a surface, for refitting and for how far it is from the scan.
#[derive(Clone, Debug)]
pub struct Covered {
    pub scan: String,
    pub points: Arc<Vec<Vec3>>,
}

/// The default distance a scan point may be from the surface to be under it: a tenth of the control
/// net's size, at least a millimetre, so the scan under a point pulled well off it still counts.
pub fn default_reach(surface: &NurbsSurface) -> f64 {
    let net = surface_edit::points(surface);
    let Some(first) = net.first().copied() else { return 1.0 };
    let (lo, hi) = net.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    ((hi - lo).len() / 10.0).max(1.0)
}

/// The scan points the surface covers (on the scan `scan`, or the selected or only one).
pub fn covered(s: &Session, scan: Option<&str>, surface: &NurbsSurface, reach: f64, cmd: &str) -> Result<Covered> {
    let mesh = mesh(s, &json!({ "body": scan }), cmd)?;
    let points = surface_edit::covered(&mesh.scan(), surface, reach, REFIT_POINTS);
    Ok(Covered { scan: mesh.body, points: Arc::new(points) })
}

/// Writes `surface` back into the Import feature that keeps the surface: one undo step.
fn write_back(s: &mut Session, target: &EditableSurface, surface: &NurbsSurface, cmd: &str) -> Result<()> {
    let ts = solidcraft_geom::tspline::TSpline::from_nurbs(surface).map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let sheet = solidcraft_kernel::tspline_sheet(&ts).map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let step = solidcraft_kernel::step_export(&[&sheet], "SolidCraft").map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let feature = s.doc_mut().feature_mut(target.feature).ok_or_else(|| bad(cmd, "the surface's feature is gone"))?;
    if let solidcraft_doc::FeatureKind::Import { step: kept, .. } = &mut feature.kind {
        *kept = step.into();
    }
    s.refresh();
    match s.model.result(target.feature).and_then(|r| r.error.clone()) {
        Some(e) => Err(EngineError::Other(e)),
        None => Ok(()),
    }
}

/// A control point `[u, v]` by its place in the net, checked against the net's size.
fn index(v: &Value, count: [usize; 2], cmd: &str, what: &str) -> Result<usize> {
    let pair = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_u64()?, a[1].as_u64()?]));
    let [i, j] = pair.ok_or_else(|| bad(cmd, format!("`{what}` must be [u, v], whole numbers from 0")))?;
    let (i, j) = (usize::try_from(i).unwrap_or(usize::MAX), usize::try_from(j).unwrap_or(usize::MAX));
    if i >= count[0] || j >= count[1] {
        return Err(bad(cmd, format!("`{what}` [{i}, {j}] is outside the net of {} × {} control points", count[0], count[1])));
    }
    Ok(j * count[0] + i)
}

pub(super) fn edit_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.edit_surface";
    let target = editable(s, str_(p, "surface"), cmd)?;
    let count = target.surface.count();
    let mut net = surface_edit::points(&target.surface);
    let mut moved = vec![false; net.len()];
    // The whole net, as a dialog that previewed it sends it.
    if let Some(all) = p.get("points").filter(|v| !v.is_null()) {
        let all = all.as_array().ok_or_else(|| bad(cmd, "`points` must be a list of points [x, y, z]"))?;
        if all.len() != net.len() {
            return Err(bad(cmd, format!("`points` must hold all {} control points ({} × {}, u fastest)", net.len(), count[0], count[1])));
        }
        for (k, v) in all.iter().enumerate() {
            let q = crate::params::vec3(v).ok_or_else(|| bad(cmd, format!("`points[{k}]` must be a point [x, y, z] of numbers")))?;
            moved[k] = q.dist(net[k]) > 1e-9;
            net[k] = q;
        }
    }
    // Moves of single points.
    if let Some(moves) = p.get("moves").filter(|v| !v.is_null()) {
        let moves = moves.as_array().ok_or_else(|| bad(cmd, "`moves` must be a list of {point: [u, v], to: [x, y, z]}"))?;
        for (k, m) in moves.iter().enumerate() {
            let at = index(m.get("point").unwrap_or(&Value::Null), count, cmd, &format!("moves[{k}].point"))?;
            let to =
                m.get("to").and_then(crate::params::vec3).ok_or_else(|| bad(cmd, format!("`moves[{k}].to` must be a point [x, y, z] of numbers")))?;
            net[at] = to;
            moved[at] = true;
        }
    }
    let mut held = moved.clone();
    if let Some(hold) = p.get("hold").filter(|v| !v.is_null()) {
        let hold = hold.as_array().ok_or_else(|| bad(cmd, "`hold` must be a list of [u, v]"))?;
        for (k, h) in hold.iter().enumerate() {
            held[index(h, count, cmd, &format!("hold[{k}]"))?] = true;
        }
    }
    let refit = str_(p, "refit").unwrap_or("none");
    let free: Vec<bool> = match refit {
        "none" => vec![false; net.len()],
        "all" => held.iter().map(|h| !h).collect(),
        "around" => {
            // The ring of two round each point moved or held, but not those points themselves.
            let mut near = vec![false; net.len()];
            for (k, _) in held.iter().enumerate().filter(|(_, h)| **h) {
                for (n, r) in surface_edit::around(count, k % count[0], k / count[0], 2).into_iter().enumerate() {
                    near[n] |= r;
                }
            }
            near.iter().zip(&held).map(|(n, h)| *n && !h).collect()
        }
        other => return Err(bad(cmd, format!("`refit` must be none, around or all, not `{other}`"))),
    };
    if !moved.iter().any(|m| *m) && !free.iter().any(|f| *f) {
        return Err(bad(cmd, "nothing to change: move a control point, or refit"));
    }
    let edited = surface_edit::with_points(&target.surface, &net).ok_or_else(|| bad(cmd, "a control point is not a finite place"))?;
    let reach = length(s, p, "reach", default_reach(&target.surface), cmd)?;
    let wants_scan = free.iter().any(|f| *f) || p.get("scan").is_some_and(|v| !v.is_null());
    let under = if wants_scan || s.model.state().bodies.iter().any(|b| b.body.is_mesh()) {
        match covered(s, str_(p, "scan"), &target.surface, reach, cmd) {
            Ok(c) => Some(c),
            Err(e) if wants_scan => return Err(e),
            Err(_) => None,
        }
    } else {
        None
    };
    let result = if free.iter().any(|f| *f) {
        let points = under.as_ref().map(|c| c.points.clone()).unwrap_or_default();
        if points.is_empty() {
            return Err(bad(cmd, "no scan point lies under the surface to refit it to (a larger reach takes in more)"));
        }
        let held: Vec<bool> = free.iter().map(|f| !f).collect();
        surface_edit::refit(&edited, &points, &held).ok_or_else(|| bad(cmd, "the points to refit have no scan under them"))?
    } else {
        edited
    };
    write_back(s, &target, &result, cmd)?;
    let (rms, max) = match &under {
        Some(c) if !c.points.is_empty() => {
            let (r, m) = surface_edit::distance(&result, &c.points);
            (Some(r), Some(m))
        }
        _ => (None, None),
    };
    let reply = ScanEditSurfaceResult {
        surface: target.body,
        controls: count,
        moved: moved.iter().filter(|m| **m).count(),
        refitted: free.iter().filter(|f| **f).count(),
        scan: under.as_ref().map(|c| c.scan.clone()),
        points: under.as_ref().map(|c| c.points.len()),
        rms,
        max,
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The surface maths a dialog previews an edit with, as the command does it.
pub use solidcraft_scan::surface_edit::{around, distance, points as control_points, refit, with_points};
