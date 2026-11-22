//! Close Surface (`scan.close_surface`): a surface body made round with free forms, merged and
//! sliced, usually still has openings: a missing face, an end left open, a hole where a net was
//! cut away. This lists every loop of free edges (where it is and how big), and unless asked only
//! to look, closes each with a patch sewn in, so the body becomes a solid when nothing else is
//! open. It is the commands that were there, in one undo step: Patch on each loop, Stitch. A loop
//! a patch cannot close is said, with where it is, before anything is changed.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;

use super::slice::{run, slice_target, unused};
use crate::params::bad;
use crate::schema::scan::{ScanCloseSurfaceResult, ScanOpenLoop};
use crate::{EngineError, Result, Session};

/// Most loops of free edges one call closes.
const MOST_LOOPS: usize = 64;

/// Where a loop of free edges is and how big: its edge count, the middle of its edges and the
/// diagonal of the box round them (mm).
fn describe(edges: &[Vec3]) -> ScanOpenLoop {
    let n = edges.len().max(1) as f64;
    let sum = edges.iter().fold(Vec3::ZERO, |a, q| a + *q);
    let c = sum * (1.0 / n);
    let (mut lo, mut hi) =
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for q in edges {
        lo = Vec3::new(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
        hi = Vec3::new(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
    }
    let size = if edges.is_empty() { 0.0 } else { (hi - lo).len() };
    ScanOpenLoop { edges: edges.len(), at: [c.x, c.y, c.z], size }
}

/// Whether the body is a solid: it has an inside as well as an outside.
fn is_solid(s: &Session, body: &str) -> bool {
    s.model.state().body(body).and_then(|b| solidcraft_kernel::brep_record(&b.body).ok()).is_some_and(|r| r.regions.len() > 1)
}

pub(super) fn close_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.close_surface";
    let close = crate::params::bool_(p, "close").unwrap_or(true);
    let continuity = match crate::params::str_(p, "continuity").map(str::trim) {
        None | Some("g0") => "g0",
        Some("g1") => "g1",
        Some(c) => return Err(bad(cmd, format!("`continuity` must be g0 or g1, not `{c}`"))),
    };
    let body = slice_target(s, crate::params::str_(p, "surface"), cmd)?;
    let free = {
        let state = s.model.state();
        let mb = state.body(&body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
        solidcraft_kernel::free_loops(&mb.body).map_err(|e| super::refusal::refused(cmd, "the open edges could not be followed", &e.to_string()))?
    };
    if free.len() > MOST_LOOPS {
        return Err(bad(cmd, format!("the surface has {} open loops; close at most {MOST_LOOPS} at a time (merge its pieces first)", free.len())));
    }
    let loops: Vec<ScanOpenLoop> = free.iter().map(|e| describe(e)).collect();
    let mut caps = Vec::new();
    if close && !free.is_empty() {
        for edges in &free {
            let name = unused(s, &format!("{body} Cap"));
            let points: Vec<[f64; 3]> = edges.iter().map(|q| [q.x, q.y, q.z]).collect();
            let at = describe(edges);
            run(s, "surface.patch", json!({"edges": points, "body": body, "continuity": continuity, "body_name": name, "name": "Close Cap"})).map_err(|e| {
                bad(
                    cmd,
                    format!(
                        "the opening of {} edges about [{:.1}, {:.1}, {:.1}] ({:.1} mm across) could not be closed: {}; try continuity g1, or close it with Patch on its edges",
                        at.edges, at.at[0], at.at[1], at.at[2], at.size, super::refusal::kernel_words(&e.to_string())
                    ),
                )
            })?;
            caps.push(name);
        }
        let mut bodies = vec![body.clone()];
        bodies.extend(caps.iter().cloned());
        run(s, "surface.stitch", json!({"bodies": bodies, "name": "Close Sew"}))?;
    }
    let solid = is_solid(s, &body);
    let open = if caps.is_empty() {
        loops.len()
    } else {
        let state = s.model.state();
        state.body(&body).and_then(|b| solidcraft_kernel::free_loops(&b.body).ok()).map_or(0, |l| l.len())
    };
    // A volume only means something for a closed body: an open shell's is not one.
    let volume = if solid { s.model.state().body(&body).and_then(|b| solidcraft_kernel::measure(&b.body).ok()).map(|m| m.volume) } else { None };
    let reply = ScanCloseSurfaceResult { body, loops, caps: caps.len(), open, solid, volume, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
