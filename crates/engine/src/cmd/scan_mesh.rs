//! Tools on a scan's mesh as a whole (#8615), after SurfaceCraft's: `scan.register` lays one scan
//! onto another, a second pass onto the first, and keeps the move as an ordinary Move of it.

use serde_json::{Value, json};
use solidcraft_scan::register::{self, Settings};

use super::{CommandSpec, Mesh, mesh, move_by};
use crate::params::{bad, bool_, num, str_};
use crate::schema::scan_mesh::{ScanRegisterParams, ScanRegisterResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};

const REGISTER_CONTRACT: TypedContract<ScanRegisterParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.register", "Align to Scan", crate::checked!("scan.register", register_scan))
    .at("SCAN", "ALIGN")
    .icon("align")
    .contract(&REGISTER_CONTRACT)
    .output(output_schema_for::<ScanRegisterResult>)
    .params("body?: the scan to move (default: the selected mesh body, else with two scans the one inserted last); to?: the scan to lay it on (default: the other, when there are two); search?: bool (default true: from the ways their principal axes and matched surface features lie on each other too, not only from where it lies); ignored?: share 0–0.6 (default 0.2: how much of it may have nothing under it): laid on the other by point-to-plane ICP both ways, trimmed and robust, one undo step kept as a Move → body, to, moved, rms_before, rms_after, matched, start, converged, rival?, feature")];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// The scans of the design, in the order they were made.
fn scans(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// The scan to move and the scan to lay it on.
fn pair(s: &Session, p: &Value, cmd: &str) -> Result<(String, String)> {
    let all = scans(s);
    let named = |k: &str| str_(p, k).filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let selected: Vec<String> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.clone()),
            _ => None,
        })
        .filter(|n| all.contains(n))
        .collect();
    let moving = match (named("body"), selected.first(), all.as_slice()) {
        (Some(body), _, _) => body,
        (None, Some(one), _) => one.clone(),
        (None, None, [only]) => only.clone(),
        (None, None, [_, second]) => second.clone(),
        (None, None, []) => return Err(bad(cmd, "the design has no scan: insert two passes with Insert Mesh")),
        (None, None, _) => return Err(bad(cmd, "select the scan to move onto another")),
    };
    let others: Vec<&String> = all.iter().filter(|n| **n != moving).collect();
    let to = match (named("to"), others.as_slice()) {
        (Some(to), _) => to,
        (None, [one]) => (*one).clone(),
        (None, []) => return Err(bad(cmd, "the design has one scan: insert the other pass to lay it on")),
        (None, _) => return Err(bad(cmd, "name the scan to lay it on with `to`")),
    };
    if to == moving {
        return Err(bad(cmd, "a scan cannot be laid onto itself"));
    }
    Ok((moving, to))
}

pub(super) fn register_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.register";
    let (moving, to) = pair(s, p, cmd)?;
    let ignored = num(p, "ignored").unwrap_or(0.2);
    if !(0.0..=0.6).contains(&ignored) {
        return Err(bad(cmd, "`ignored` is a share from 0 to 0.6"));
    }
    let settings = Settings { search: bool_(p, "search").unwrap_or(true), ignored };
    let mesh_of = |name: &str| mesh(s, &json!({ "body": name }), cmd);
    let (scan, other): (Mesh, Mesh) = (mesh_of(&moving)?, mesh_of(&to)?);
    let found = register::register(&scan.scan(), &other.scan(), &settings).map_err(|e| bad(cmd, e.to_string()))?;
    let feature = move_by(s, &scan, &found.moved)?;
    reply(ScanRegisterResult {
        body: moving,
        to,
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
#[path = "scan_mesh_tests.rs"]
mod tests;
