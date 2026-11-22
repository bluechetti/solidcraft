//! Slice Free Form (`scan.slice_surface`, SurfaceCraft's, after QuickSurface's Slicing Free Form):
//! a surface cut by a plane, the side kept chosen, and with Cap each loop of the cut closed with a
//! flat face and sewn in, so that a part made round with free forms becomes a solid. It is the
//! commands that were there, in one undo step: Trim by the plane, Patch on each loop of the cut,
//! Stitch. A cap needs the plane to cut right round the surface (a closed loop of edges on the
//! plane); when it does not, it says so before anything is changed, and the surface can still be
//! cut without one.

use serde_json::{Value, json};
use solidcore_record::{Plane, Vec3};
use solidcraft_geom::prelude::*;

use crate::params::bad;
use crate::schema::scan::ScanSliceSurfaceResult;
use crate::{EngineError, Result, Session};

/// Which side of the plane is kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Keep {
    /// The side the plane's normal points to.
    Front,
    Back,
    /// The side this point is on.
    Point(Vec3),
}

/// What slicing a surface would do, worked out before anything is changed: the plane, a point on
/// the side kept, and the loops of the cut a cap closes (a point halfway along each edge of each).
#[derive(Clone, Debug)]
pub struct Slice {
    pub body: String,
    pub plane: Plane,
    pub keep: Vec3,
    pub loops: Vec<Vec<Vec3>>,
    /// The body's size, for drawing the plane.
    pub size: f64,
}

pub(super) fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// The surface Slice cuts: the one named, else the selected surface body, else the body (not a
/// scan) made or changed last.
pub fn slice_target(s: &Session, name: Option<&str>, cmd: &str) -> Result<String> {
    let state = s.model.state();
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        let body = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
        if body.body.is_mesh() {
            return Err(bad(cmd, format!("`{name}` is a scan: slice a surface made on it")));
        }
        return Ok(name.to_owned());
    }
    let selected = s.selection.iter().find_map(|x| match x {
        crate::Sel::Body { name } | crate::Sel::Face { body: name, .. } | crate::Sel::Edge { body: name, .. } => {
            state.body(name).filter(|b| !b.body.is_mesh()).map(|_| name.clone())
        }
        _ => None,
    });
    if let Some(name) = selected {
        return Ok(name);
    }
    // The surface made or changed last: a merge's sewn body as much as a free form.
    state
        .bodies
        .iter()
        .filter(|b| !b.body.is_mesh())
        .max_by_key(|b| b.feature)
        .map(|b| b.name.clone())
        .ok_or_else(|| bad(cmd, "the design has no surface to slice: make one with New Free Form, Wrap Surface or Fit Surface"))
}

/// What slicing `body` by `plane` (a plane parameter: XY, XZ, YZ, a plane's name or
/// {origin, normal}) keeping `keep` would do.
pub fn sliced(s: &Session, body: &str, plane: &Value, keep: Keep, cmd: &str) -> Result<Slice> {
    let pref = super::super::features::plane_param(s, Some(plane), cmd)?;
    let (vals, _) = s.doc.param_values();
    let state = s.model.state();
    let pl = s.doc.resolve_plane_in(&vals, &pref, 0, s.active_component, Some(&state)).map_err(|e| bad(cmd, e.to_string()))?;
    let mb = state.body(body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
    if mb.body.is_mesh() {
        return Err(bad(cmd, format!("`{body}` is a scan: slice a surface made on it")));
    }
    let size = mb.body.size().max(1.0);
    let n = pl.normal();
    let at = match keep {
        Keep::Front => pl.origin + n * size,
        Keep::Back => pl.origin - n * size,
        Keep::Point(p) => p,
    };
    let trimmed = solidcraft_kernel::trim_keep(&mb.body, &solidcraft_kernel::SplitTool::Plane(pl), solidcraft_kernel::TrimKeep::Side(at))
        .map_err(|e| super::refusal::refused(cmd, "the plane does not slice the surface", &e.to_string()))?;
    let free = solidcraft_kernel::free_loops(&trimmed).map_err(|e| super::refusal::refused(cmd, "the cut could not be followed", &e.to_string()))?;
    // The loops wholly on the plane are the cut's; a loop with other edges is the surface's own rim.
    let on = (1e-5 * size).max(1e-4);
    let loops = free.into_iter().filter(|e| !e.is_empty() && e.iter().all(|q| pl.height(*q).abs() <= on)).collect();
    Ok(Slice { body: body.to_owned(), plane: pl, keep: at, loops, size })
}

/// A body name not yet used: `base`, else `base 2`, `base 3`, …
pub(super) fn unused(s: &Session, base: &str) -> String {
    let state = s.model.state();
    (1..).map(|k| if k == 1 { base.to_owned() } else { format!("{base} {k}") }).find(|n| state.body(n).is_none()).unwrap_or_else(|| base.to_owned())
}

pub(super) fn slice_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.slice_surface";
    let plane = p
        .get("plane")
        .filter(|v| !v.is_null())
        .cloned()
        .ok_or_else(|| bad(cmd, "`plane` is required: XY, XZ, YZ, a construction plane's name, or {origin, normal}"))?;
    let keep = match p.get("keep").filter(|v| !v.is_null()) {
        None => Keep::Front,
        Some(Value::String(side)) if side == "front" => Keep::Front,
        Some(Value::String(side)) if side == "back" => Keep::Back,
        Some(v) => {
            Keep::Point(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`keep` must be front, back or a point [x, y, z] on the side to keep"))?)
        }
    };
    let cap = crate::params::bool_(p, "cap").unwrap_or(true);
    let body = slice_target(s, crate::params::str_(p, "surface"), cmd)?;
    let slice = sliced(s, &body, &plane, keep, cmd)?;
    if cap && slice.loops.is_empty() {
        return Err(bad(
            cmd,
            "the plane does not cut right round the surface, so the cut has no loop to close: slice it without a cap, or merge the surfaces round the part first",
        ));
    }
    let k = slice.keep;
    run(s, "surface.trim", json!({"body": body, "plane": plane, "keep": [k.x, k.y, k.z], "name": "Slice"}))?;
    let mut caps = Vec::new();
    if cap {
        for edges in &slice.loops {
            let name = unused(s, &format!("{body} Cap"));
            let points: Vec<[f64; 3]> = edges.iter().map(|q| [q.x, q.y, q.z]).collect();
            run(s, "surface.patch", json!({"edges": points, "body": body, "continuity": "g0", "body_name": name, "name": "Slice Cap"}))?;
            caps.push(name);
        }
        let mut bodies = vec![body.clone()];
        bodies.extend(caps.iter().cloned());
        run(s, "surface.stitch", json!({"bodies": bodies, "name": "Slice Sew"}))?;
    }
    let solid = s.model.state().body(&body).and_then(|b| solidcraft_kernel::brep_record(&b.body).ok()).is_some_and(|r| r.regions.len() > 1);
    let reply = ScanSliceSurfaceResult { body, caps: caps.len(), solid, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
