//! Reconstruct the Rest: what the body Reconstruct built leaves of the scan unexplained, read and
//! built piece by piece. Each round selects the largest piece of the scan the body does not
//! explain yet (`scan.select {"unexplained"}`), reads it as a part of its own
//! (`scan.reconstruct_selection`) and runs the steps proposed for it, taking back each one that
//! leaves the body further from the scan; a piece that adds nothing is passed over for the next.
//! All of it is one undo step.

use serde_json::{Value, json};

use super::{built_name, reply, run};
use crate::params::bad;
use crate::schema::scan_reconstruct::{ScanRestResult, ScanRestRound, ScanToCadResult, ScanToCadStep};
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

/// A step that built nothing to fit: run (kept) or refused (`why`).
fn record(command: &str, kept: bool, why: Option<String>) -> ScanToCadStep {
    ScanToCadStep { command: command.to_owned(), kept, verdict: None, explained: None, why }
}

/// The clean steps Survey may name that Scan to CAD runs, as the scripted benchmark does.
const CLEAN: [&str; 5] = ["scan.repair", "scan.remove_floating", "scan.fill_holes", "scan.tolerance", "scan.denoise"];

/// Scan to CAD: the automatic route in one command (Survey's clean steps, Find All, Reconstruct,
/// its proposals, the rest rounds, the verdict), one undo step. The scripted benchmark that
/// runs this route matched a real agent on 7 of 8 shape-family parts at 5 to 10 times its speed
/// (2026-11-18); an agent then starts from the body and its verdict instead of from the scan.
pub(super) fn to_cad(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.to_cad";
    let named = crate::params::str_(p, "body").or_else(|| crate::params::str_(p, "scan")).filter(|b| !b.trim().is_empty());
    let scan = super::super::mesh_body_name(s, named, cmd)?;
    let solid = built_name(&scan);
    let rest = match p.get("rest").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| *n <= MOST_ROUNDS).ok_or_else(|| bad(cmd, format!("`rest` is 0 to {MOST_ROUNDS} rounds")))?,
        // Default 6: on the fast scorecard (2026-11-23) the rest rounds raised T-LESS-like 18 from
        // 0.21 to 0.84 and 27 from 0.49 to 0.60, lowered no task, and cost 3 to 17 s.
        None => ROUNDS,
    };
    let tolerance = match p.get("tolerance").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|t| t.is_finite() && *t > 0.0).ok_or_else(|| bad(cmd, "`tolerance` must be a length in mm above 0"))?),
        None => None,
    };
    let mut steps: Vec<ScanToCadStep> = Vec::new();
    let survey = run(s, "scan.survey", json!({"body": scan}))?;
    // Two scans the route must not build on: one in the wrong unit (a part a thousand times too
    // small reads as noise), and a wall or panel (Flat Template's, not Reconstruct's).
    let stop = |why: String| ScanToCadResult {
        body: scan.clone(),
        solid: None,
        steps: Vec::new(),
        kept: 0,
        rest: None,
        verdict: None,
        next: why,
        param_warnings: Vec::new(),
    };
    if let Some(hint) = survey.get("units_hint").and_then(Value::as_str) {
        return reply(stop(format!(
            "nothing was built: the scan looks like it is in the wrong unit ({hint}); set it with mesh.units (scan.survey's next names it), then run scan.to_cad again"
        )));
    }
    if survey.get("sheet_like").and_then(Value::as_bool) == Some(true) {
        return reply(stop(
            "nothing was built: the scan is a wall or a panel, not a part; use scan.flat_template {\"outline\": \"scan\"} (or scan.panel_proposals)"
                .to_owned(),
        ));
    }
    if p.get("clean").and_then(Value::as_bool).unwrap_or(true) {
        for step in survey.get("next").and_then(Value::as_array).cloned().unwrap_or_default() {
            let Some(command) = step.get("command").and_then(Value::as_str) else { continue };
            // A tolerance given here wins over the one Survey suggests.
            if !CLEAN.contains(&command) || (command == "scan.tolerance" && tolerance.is_some()) {
                continue;
            }
            let mut params = step.get("params").cloned().filter(Value::is_object).unwrap_or(json!({}));
            if params.get("body").is_none() && params.get("scan").is_none() {
                params["body"] = json!(scan);
            }
            let done = run(s, command, params);
            steps.push(record(command, done.is_ok(), done.err().map(|e| e.to_string())));
        }
    }
    if let Some(t) = tolerance {
        run(s, "scan.tolerance", json!({"body": scan, "tolerance": t}))?;
        steps.push(record("scan.tolerance", true, None));
    }
    let mut find = json!({"body": scan});
    if let Some(t) = tolerance {
        find["tolerance"] = json!(t);
    }
    run(s, "scan.find_all", find)?;
    let read = run(s, "scan.reconstruct", json!({"body": scan}));
    let (proposals, note) = match read {
        Ok(read) => (
            read.get("proposals").and_then(Value::as_array).cloned().unwrap_or_default(),
            read.get("note").and_then(Value::as_str).map(str::to_owned),
        ),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let mut kept = 0;
    for step in proposals {
        let Some(command) = step.get("command").and_then(Value::as_str).map(str::to_owned) else { continue };
        let mark = s.checkpoint();
        match run(s, &command, step.get("params").cloned().unwrap_or(json!({}))) {
            Ok(r) => {
                let fit = r.get("fit");
                let verdict = fit.and_then(|f| f.get("verdict")).and_then(Value::as_str).map(str::to_owned);
                let explained = fit.and_then(|f| f.pointer("/after/explained")).and_then(Value::as_f64);
                // As the scripted route: only a step that leaves the body further from the scan
                // is taken back; one with no fit to read (a step that builds no body) stays.
                let worse = verdict.as_deref() == Some("worse");
                if worse {
                    s.restore_checkpoint(mark)?;
                } else {
                    kept += 1;
                }
                steps.push(ScanToCadStep { command, kept: !worse, verdict, explained, why: worse.then(|| "worse".to_owned()) });
            }
            Err(e) => {
                s.restore_checkpoint(mark)?;
                steps.push(record(&command, false, Some(e.to_string())));
            }
        }
    }
    let built = s.model.state().body(&solid).is_some();
    let rest = (built && rest > 0)
        .then(|| run(s, "scan.reconstruct_rest", json!({"body": scan, "rounds": rest})).unwrap_or_else(|e| json!({"error": e.to_string()})));
    let verdict = built.then(|| run(s, "scan.verdict", json!({"body": scan, "model": solid})).ok()).flatten();
    let next = if !built {
        let why = note.unwrap_or_else(|| "Reconstruct read no prism, turned part or other route it can build".to_owned());
        format!(
            "nothing was built: {why}. For a freeform part try scan.wrap_scan {{}} or scan.fit_patches {{}}; for a part Reconstruct misreads, fit and model by hand (scan.fit_plane, scan.fit_cylinder, scan.section)"
        )
    } else if verdict.as_ref().and_then(|v| v.get("verdict")).and_then(Value::as_str) == Some("met") {
        format!("`{solid}` meets the plan's target")
    } else {
        format!("`{solid}` is built but not done: the verdict's issues, places and next say what is open; work on those places")
    };
    reply(ScanToCadResult { body: scan, solid: built.then_some(solid), steps, kept, rest, verdict, next, param_warnings: Vec::new() })
}
