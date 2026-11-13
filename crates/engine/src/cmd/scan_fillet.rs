//! Fillet from Scan (`scan.fillet`, #8615), after SurfaceCraft's Measure from Scan: the edges of a
//! model picked, the scan's points near them fitted with one round between each edge's two faces,
//! and the edges filleted at the radius measured (rounded to a step), an ordinary Fillet feature.
//! Measuring alone changes nothing.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use solidcraft_scan::fillet::{self, Measured, Station};

use super::{CommandSpec, mesh, mesh_name};
use crate::params::{bad, bool_, num, str_};
use crate::schema::scan_fillet::{ScanFilletParams, ScanFilletResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};

const FILLET_CONTRACT: TypedContract<ScanFilletParams> = TypedContract::new();

/// How far from the edges to look for the scan, and the step the radius is rounded to.
pub const REACH: f64 = 10.0;
pub const STEP: f64 = 0.5;

/// How finely the model is meshed to lay stations along its edges and read its faces there.
pub const CHORD: f64 = 0.01;

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.fillet", "Fillet from Scan", crate::checked!("scan.fillet", fillet_from_scan))
    .at("SCAN", "RECONSTRUCT")
    .icon("fillet")
    .contract(&FILLET_CONTRACT)
    .output(output_schema_for::<ScanFilletResult>)
    .params("edges: [[x,y,z] point on an edge | {body, index}] of the model; body?: the model (default: the one the edges are on, else the selected or only solid or surface body); scan?: the scan (default: the selected or only one); reach?: length (default 10 mm: how far from the edges the scan is read); step?: the radius rounded to a multiple of it (default 0.5; 0 exact); apply?: bool (default true: the edges filleted at the radius measured, a Fillet feature; false only measures): one round fitted to the scan between each edge's two faces (each face free to sit a little off the model's), refused where the scan shows a sharp edge or a round fits no better → body, scan, edges, fitted, radius, step, rms, maximum, sharp_rms, points, on_round, set_aside, station_low, station_high, face_offsets, feature?")];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// What measuring `p` finds: the model, the scan, the edges' indices and the measurement.
pub fn measured(s: &Session, p: &Value, cmd: &str) -> Result<(String, String, Vec<usize>, Measured)> {
    let picks = p
        .get("edges")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad(cmd, "`edges` must list the model's edges to measure"))?;
    let state = s.model.state();
    // The model: named, else the body an edge names, else the selected or only solid or surface body.
    let named = str_(p, "body").filter(|n| !n.trim().is_empty()).map(str::to_owned);
    let on_edge = picks.iter().find_map(|e| e.get("body").and_then(Value::as_str).map(str::to_owned));
    let selected = s.selection.iter().find_map(|x| match x {
        Sel::Edge { body, .. } | Sel::Face { body, .. } | Sel::Body { name: body } if state.body(body).is_some_and(|b| !b.body.is_mesh()) => {
            Some(body.clone())
        }
        _ => None,
    });
    let models: Vec<&str> = state.bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.as_str()).collect();
    let model = match (named.or(on_edge).or(selected), models.as_slice()) {
        (Some(m), _) => m,
        (None, [one]) => (*one).to_owned(),
        (None, []) => return Err(bad(cmd, "the design has no model to fillet: import its CAD, or model it")),
        (None, _) => return Err(bad(cmd, "name the model with `body`")),
    };
    let body = state.body(&model).ok_or_else(|| bad(cmd, format!("no body `{model}`")))?;
    if body.body.is_mesh() {
        return Err(bad(cmd, format!("`{model}` is a scan: pick the edges of the model built from it")));
    }
    let scan_body = mesh_name(s, &json!({ "body": str_(p, "scan") }), cmd)?;
    let reach = num(p, "reach").unwrap_or(REACH);
    let step = num(p, "step").unwrap_or(STEP);
    if !(reach.is_finite() && reach > 0.0) {
        return Err(bad(cmd, "`reach` must be a length above 0"));
    }
    if !(step.is_finite() && step >= 0.0) {
        return Err(bad(cmd, "`step` must be 0 or more"));
    }
    let surface =
        body.body.mesh(&solidcraft_kernel::MeshTolerance::chord(CHORD)).map_err(|e| bad(cmd, format!("`{model}` could not be meshed: {e}")))?;
    let mut indices = Vec::with_capacity(picks.len());
    for pick in picks {
        let index = match (pick.get("index").and_then(Value::as_u64), crate::params::vec3(pick)) {
            (Some(i), _) => i as usize,
            (None, Some(at)) => {
                let tol = (body.body.size() * 1e-3).max(1e-3);
                body.body
                    .nearest_edge(at, tol * 10.0)
                    .map_err(|e| bad(cmd, e.to_string()))?
                    .map(|(i, _)| i)
                    .ok_or_else(|| bad(cmd, format!("no edge of `{model}` at [{:.3}, {:.3}, {:.3}]", at.x, at.y, at.z)))?
            }
            _ => return Err(bad(cmd, "an edge must be a point [x, y, z] on it or {body, index}")),
        };
        if index >= surface.edges.len() {
            return Err(bad(cmd, format!("`{model}` has no edge {index}")));
        }
        if !indices.contains(&index) {
            indices.push(index);
        }
    }
    let stations: Vec<Vec<Station>> =
        indices.iter().map(|&e| stations(&surface, e, reach)).collect::<std::result::Result<_, String>>().map_err(|e| bad(cmd, e))?;
    let scan = mesh(s, &json!({ "body": scan_body }), cmd)?;
    // Only the scan facing out of the corners: the inside of a thin wall near an edge is no part
    // of its round.
    let points = {
        let on = solidcraft_scan::Scan::new(&scan.positions, &scan.triangles);
        fillet::facing_out(&on, &stations, reach, solidcraft_scan::reconstruct::outward(&on))
    };
    let found = fillet::measure(&points, &stations, reach, step).map_err(|e| bad(cmd, e.to_string()))?;
    Ok((model, scan.body, indices, found))
}

/// Stations along edge `e` of the model's mesh, about a fifth of the reach apart (8 to 64 of
/// them, none at its ends): each point, the edge's direction, and its two faces' outward normals and the ways into
/// them, read off each face's triangle nearest the point.
fn stations(m: &solidcraft_geom::Mesh, e: usize, reach: f64) -> std::result::Result<Vec<Station>, String> {
    let line = m.edges.get(e).filter(|l| l.len() >= 2).ok_or("an edge has no length")?;
    let faces: Vec<u32> = m.edge_faces.get(e).cloned().unwrap_or_default();
    let [f1, f2] = faces.as_slice() else { return Err("an edge to fillet must lie between two faces".into()) };
    let of_face = |f: u32| -> Vec<[Vec3; 3]> {
        m.triangles
            .iter()
            .zip(&m.tri_face)
            .filter(|(_, tf)| **tf == f)
            .filter_map(|(t, _)| Some([*m.positions.get(t[0] as usize)?, *m.positions.get(t[1] as usize)?, *m.positions.get(t[2] as usize)?]))
            .collect()
    };
    let (t1, t2) = (of_face(*f1), of_face(*f2));
    let length: f64 = line.windows(2).map(|w| w[0].dist(w[1])).sum();
    let count = ((length / (reach / 5.0)).ceil() as usize).clamp(8, 64);
    let mut out = Vec::with_capacity(count);
    // A spacing in from each end, so the faces that end the edge stay out of what is read.
    for k in 0..count {
        let at = length * (k as f64 + 1.0) / (count as f64 + 1.0);
        let (point, tangent) = along(line, at).ok_or("an edge has no length")?;
        let face = |tris: &[[Vec3; 3]]| -> Option<(Vec3, Vec3)> {
            let near = tris.iter().min_by(|a, b| {
                let d = |t: &[Vec3; 3]| solidcraft_geom::compare::closest_on_triangle(point, t[0], t[1], t[2]).dist(point);
                d(a).total_cmp(&d(b))
            })?;
            let normal = (near[1] - near[0]).cross(near[2] - near[0]).normalized()?;
            let into = normal.cross(tangent).normalized()?;
            let middle = (near[0] + near[1] + near[2]) / 3.0;
            Some((normal, if into.dot(middle - point) < 0.0 { -into } else { into }))
        };
        let (Some(a), Some(b)) = (face(&t1), face(&t2)) else { return Err("an edge's faces could not be read".into()) };
        out.push(Station { point, tangent, faces: [a, b] });
    }
    Ok(out)
}

/// The point `at` along a polyline and its direction there.
fn along(line: &[Vec3], at: f64) -> Option<(Vec3, Vec3)> {
    let mut gone = 0.0;
    for w in line.windows(2) {
        let l = w[0].dist(w[1]);
        if l > 0.0 && gone + l >= at {
            return Some((w[0].lerp(w[1], (at - gone) / l), (w[1] - w[0]).normalized()?));
        }
        gone += l;
    }
    let w = line.windows(2).rev().find(|w| w[0].dist(w[1]) > 0.0)?;
    Some((w[1], (w[1] - w[0]).normalized()?))
}

fn fillet_from_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fillet";
    let (model, scan, edges, found) = measured(s, p, cmd)?;
    let feature = if bool_(p, "apply").unwrap_or(true) {
        let picks = p.get("edges").cloned().unwrap_or(Value::Null);
        let run = super::super::find_command("solid.fillet").ok_or_else(|| EngineError::UnknownCommand("solid.fillet".into()))?;
        Some(run.run_nested(s, &json!({"edges": picks, "radius": format!("{} mm", found.radius), "body": model, "tangent_chain": false}))?)
    } else {
        None
    };
    reply(ScanFilletResult {
        body: model,
        scan,
        edges,
        fitted: found.fitted,
        radius: found.radius,
        step: found.step,
        rms: found.rms,
        maximum: found.maximum,
        sharp_rms: found.sharp_rms,
        points: found.points,
        on_round: found.on_round,
        set_aside: found.set_aside,
        station_low: found.station_low,
        station_high: found.station_high,
        face_offsets: found.face_offsets,
        feature,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_fillet_tests.rs"]
mod tests;
