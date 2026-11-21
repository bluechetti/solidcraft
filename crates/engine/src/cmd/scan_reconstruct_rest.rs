//! Reconstruct the Rest: what the body Reconstruct built leaves of the scan unexplained, read and
//! built piece by piece. Each round selects the largest piece of the scan the body does not
//! explain yet (`scan.select {"unexplained"}`), reads it as a part of its own
//! (`scan.reconstruct_selection`) and runs the steps proposed for it, taking back each one that
//! leaves the body further from the scan; a piece that adds nothing is passed over for the next.
//! All of it is one undo step.

use serde_json::{Value, json};

use super::{built_name, reply, run};
use crate::params::bad;
use crate::schema::scan_reconstruct::{ScanRestResult, ScanRestRound};
use crate::{Result, Session};

/// Rounds at most, and the default; pieces passed over in a row before it stops.
const MOST_ROUNDS: u64 = 20;
const ROUNDS: u64 = 6;
const TRIES: usize = 3;

pub(super) fn reconstruct_rest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.reconstruct_rest";
    let scan = super::super::mesh_body_name(s, crate::params::str_(p, "body").filter(|b| !b.trim().is_empty()), cmd)?;
    let solid = built_name(&scan);
    if s.model.state().body(&solid).is_none() {
        return Err(bad(cmd, format!("there is no `{solid}` yet: build the part first with the steps Reconstruct proposes")));
    }
    let rounds = match p.get("rounds").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| (1..=MOST_ROUNDS).contains(n)).ok_or_else(|| bad(cmd, format!("`rounds` is 1 to {MOST_ROUNDS}")))?,
        None => ROUNDS,
    };
    let mut done = Vec::new();
    let mut kept_all = 0;
    let mut skip = 0usize;
    let mut stop = "rounds".to_string();
    for _ in 0..rounds {
        let selected = run(s, "scan.select", json!({"body": scan, "unexplained": {"body": solid, "piece": skip, "grow": 2}}));
        let Ok(selected) = selected else {
            stop = "nothing left the scan shows that the body does not explain".into();
            break;
        };
        let pieces = selected.get("pieces").and_then(Value::as_array).cloned().unwrap_or_default();
        if pieces.is_empty() || selected.get("triangles").and_then(Value::as_u64).unwrap_or(0) == 0 {
            stop = "the body explains the scan".into();
            break;
        }
        let area = pieces.get(skip).and_then(|k| k.get("area")).and_then(Value::as_f64).unwrap_or(0.0);
        let read = |s: &mut Session| match run(s, "scan.reconstruct_selection", json!({"body": scan})) {
            Ok(read) => (
                read.get("proposals").and_then(Value::as_array).cloned().unwrap_or_default(),
                read.get("note").and_then(Value::as_str).map(str::to_owned),
            ),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let (mut proposals, mut note) = read(s);
        // Grown, the piece takes in a ring of the face it stands on, which reads as a floor its
        // prism cannot stand on: read again as it is.
        if proposals.is_empty() && run(s, "scan.select", json!({"body": scan, "unexplained": {"body": solid, "piece": skip, "grow": 0}})).is_ok() {
            (proposals, note) = read(s);
        }
        let mut round = ScanRestRound { piece: skip, area, proposed: Vec::new(), kept: Vec::new(), note };
        for step in proposals {
            let Some(command) = step.get("command").and_then(Value::as_str).map(str::to_owned) else { continue };
            round.proposed.push(command.clone());
            let mark = s.checkpoint();
            let verdict = run(s, &command, step.get("params").cloned().unwrap_or(json!({})))
                .ok()
                .and_then(|r| r.get("fit").and_then(|f| f.get("verdict")).and_then(Value::as_str).map(str::to_owned));
            match verdict.as_deref() {
                Some("first" | "better") => round.kept.push(command),
                // Refused, or no closer: taken back.
                _ => s.restore_checkpoint(mark)?,
            }
        }
        kept_all += round.kept.len();
        let added = !round.kept.is_empty();
        done.push(round);
        if added {
            skip = 0;
        } else {
            skip += 1;
            if skip >= TRIES || skip >= pieces.len() {
                stop = "the pieces left add nothing Reconstruct can build".into();
                break;
            }
        }
    }
    // The selection the rounds made is theirs, not the user's.
    let _ = run(s, "scan.select", json!({"body": scan, "none": true}));
    // Whether the body is done: an automatic route ends saying so, not with `ok` alone.
    let verdict = run(s, "scan.verdict", json!({"body": scan, "model": solid})).ok();
    reply(ScanRestResult { body: scan, solid, rounds: done, kept: kept_all, stop, verdict, param_warnings: Vec::new() })
}
