//! A scan's plan (`scan.plan`, `scan.plan_region`) and the verdict on its model (`scan.verdict`):
//! whether the model is done, computed from the design (Scan Deviation both ways at the scan's
//! tolerance against the plan's target, the plan's regions, the timeline), never from what was
//! clicked, so neither a person nor an agent hands over part of a part as finished.

use serde_json::{Value, json};
use solidcraft_doc::scan_plan::{MAX_REGIONS, MAX_SCANS, MAX_TEXT, Process, Region, RegionState, ScanPlan, valid_target};
use solidcraft_scan::verdict::{Place, Reading, judge};

use super::{CommandSpec, mesh_name, tolerance};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_plan::{
    ScanPlanParams, ScanPlanRegionParams, ScanPlanRegionView, ScanPlanResult, ScanVerdictIssue, ScanVerdictParams, ScanVerdictPlace,
    ScanVerdictResult, ScanVerdictStep,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const PLAN: TypedContract<ScanPlanParams> = TypedContract::new();
const REGION: TypedContract<ScanPlanRegionParams> = TypedContract::new();
const VERDICT: TypedContract<ScanVerdictParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.plan", "Scan Plan", crate::checked!("scan.plan", plan))
        .icon("curvature_map")
        .contract(&PLAN)
        .output(output_schema_for::<ScanPlanResult>)
        .params("scan? (or body?): mesh body (default: the selected or only one); process?: machined|moulded_or_die_cast|sand_cast_forged_or_printed|sheet|worn|freeform|unknown (sets the target when none is given: 0.90 machined, sheet, unknown; 0.85 castings, prints, worn; 0.75 freeform); target?: 0.5…1, the share of the scan and of the model that must be within the tolerance of the other; tolerance?: length (as scan.tolerance); reset?: true takes the plan away (the tolerance stays): what the scan is being turned into, kept in the design and read by scan.verdict; one undo step when it changes → scan, planned, process, target, tolerance, regions, changed"),
    CommandSpec::new("scan.plan_region", "Plan Region", crate::checked!("scan.plan_region", plan_region))
        .icon("curvature_map")
        .contract(&REGION)
        .output(output_schema_for::<ScanPlanResult>)
        .params("scan? (or body?): mesh body; name: the region's name; at?: [x,y,z] a point on the scan inside it (needed to add one); state?: open|built|skipped|inferred|removed (default open for a new one); feature?: the feature that built it (built); reason?: why it was skipped, or what it was inferred from (skipped, inferred): a region of the part in the scan's plan, which scan.verdict wants built, skipped or inferred before it says met; a scan with no plan gets one; one undo step → as scan.plan"),
    CommandSpec::new("scan.verdict", "Scan Verdict", crate::checked!("scan.verdict", verdict))
        .icon("curvature_map")
        .contract(&VERDICT)
        .output(output_schema_for::<ScanVerdictResult>)
        .params("scan? (or body?): mesh body; model?: the solid or surface body (default: as scan.deviation picks it): whether the model of the scan is done, met only when the scan and the model are each within the tolerance of the other over the plan's target share (else the default, 0.90), every region of the plan is built, skipped or inferred, no feature failed and the design is not rolled back; changes nothing → verdict: met|not_done, scan, model, planned, target, tolerance, scan_within, model_within, issues [{code, detail}], places [{at, depth, area, kind: extra|missing}] (the worst three), next [{command, why, params}]"),
];

fn to_value<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| EngineError::Other(e.to_string()))
}

/// The scan named by `scan` or `body` (the same thing), else the selected or only one.
fn scan_of(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    let named = match (str_(p, "scan"), str_(p, "body")) {
        (Some(a), Some(b)) if a != b => return Err(bad(cmd, format!("`scan` (`{a}`) and `body` (`{b}`) name different scans: give one"))),
        (a, b) => a.or(b),
    };
    mesh_name(s, &json!({ "body": named }), cmd)
}

fn process_name(p: Process) -> &'static str {
    match p {
        Process::Machined => "machined",
        Process::MouldedOrDieCast => "moulded_or_die_cast",
        Process::SandCastForgedOrPrinted => "sand_cast_forged_or_printed",
        Process::Sheet => "sheet",
        Process::Worn => "worn",
        Process::Freeform => "freeform",
        Process::Unknown => "unknown",
    }
}

fn process_of(name: &str) -> Option<Process> {
    [
        Process::Machined,
        Process::MouldedOrDieCast,
        Process::SandCastForgedOrPrinted,
        Process::Sheet,
        Process::Worn,
        Process::Freeform,
        Process::Unknown,
    ]
    .into_iter()
    .find(|p| process_name(*p) == name.trim())
}

fn view(r: &Region) -> ScanPlanRegionView {
    let (state, feature, reason) = match &r.state {
        RegionState::Open => ("open", None, None),
        RegionState::Built { feature } => ("built", Some(feature.clone()), None),
        RegionState::Skipped { reason } => ("skipped", None, Some(reason.clone())),
        RegionState::Inferred { reason } => ("inferred", None, Some(reason.clone())),
    };
    ScanPlanRegionView { name: r.name.clone(), at: r.at, state: state.into(), feature, reason }
}

fn plan_reply(s: &Session, scan: String, changed: bool) -> Result<Value> {
    let kept = s.doc.scan_plans.get(&scan);
    let plan = kept.cloned().unwrap_or_default();
    to_value(ScanPlanResult {
        planned: kept.is_some(),
        process: process_name(plan.process).into(),
        target: plan.target(),
        tolerance: tolerance::of(s, &scan),
        regions: plan.regions.iter().map(view).collect(),
        scan,
        changed,
        param_warnings: Vec::new(),
    })
}

/// Room for one more scan's plan.
fn room(s: &Session, scan: &str, cmd: &str) -> Result<()> {
    if scan.chars().count() > MAX_TEXT || (!s.doc.scan_plans.contains_key(scan) && s.doc.scan_plans.len() >= MAX_SCANS) {
        return Err(bad(cmd, "the design keeps as many scan plans as it can"));
    }
    Ok(())
}

fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.plan";
    let scan = scan_of(s, p, cmd)?;
    let reset = bool_(p, "reset") == Some(true);
    let process =
        match str_(p, "process") {
            Some(name) => Some(process_of(name).ok_or_else(|| {
                bad(cmd, "`process` is machined, moulded_or_die_cast, sand_cast_forged_or_printed, sheet, worn, freeform or unknown")
            })?),
            None => None,
        };
    let target = match p.get("target").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|t| valid_target(*t)).ok_or_else(|| bad(cmd, "`target` is a share from 0.5 to 1"))?),
        None => None,
    };
    let given_tolerance = crate::params::expr(p, "tolerance");
    if reset && (process.is_some() || target.is_some() || given_tolerance.is_some()) {
        return Err(bad(cmd, "`reset` is given alone"));
    }
    let mut changed = false;
    // The tolerance as `scan.tolerance` sets it: one number every Scan-tab tool reads.
    if let Some(e) = given_tolerance {
        use solidcraft_doc::scan_tolerance;
        let mm = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tolerance: {e}")))?;
        if !scan_tolerance::valid(mm) {
            return Err(bad(cmd, "`tolerance` must be a length above 0"));
        }
        if !s.doc.scan_tolerances.contains_key(&scan) && s.doc.scan_tolerances.len() >= scan_tolerance::MAX_SCANS {
            return Err(bad(cmd, "the design keeps as many scan tolerances as it can"));
        }
        if s.doc.scan_tolerances.get(&scan) != Some(&mm) {
            s.doc_mut().scan_tolerances.insert(scan.clone(), mm);
            changed = true;
        }
    }
    if reset {
        if s.doc.scan_plans.contains_key(&scan) {
            s.doc_mut().scan_plans.remove(&scan);
            changed = true;
        }
    } else if process.is_some() || target.is_some() {
        room(s, &scan, cmd)?;
        let mut next = s.doc.scan_plans.get(&scan).cloned().unwrap_or_default();
        let was = s.doc.scan_plans.get(&scan).cloned();
        if let Some(process) = process {
            next.process = process;
        }
        if target.is_some() {
            next.target = target;
        }
        if was.as_ref() != Some(&next) {
            s.doc_mut().scan_plans.insert(scan.clone(), next);
            changed = true;
        }
    }
    plan_reply(s, scan, changed)
}

fn plan_region(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.plan_region";
    let scan = scan_of(s, p, cmd)?;
    let name = str_(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "needs the region's `name`"))?.to_owned();
    let text = |key: &str| str_(p, key).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
    let (feature, reason) = (text("feature"), text("reason"));
    for t in [Some(&name), feature.as_ref(), reason.as_ref()].into_iter().flatten() {
        if t.chars().count() > MAX_TEXT {
            return Err(bad(cmd, format!("a name or reason is kept to {MAX_TEXT} characters")));
        }
    }
    let at = match p.get("at").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).filter(|q| q.is_finite()).ok_or_else(|| bad(cmd, "`at` is a point [x, y, z] of numbers"))?),
        None => None,
    };
    let mut plan = s.doc.scan_plans.get(&scan).cloned().unwrap_or_default();
    let found = plan.regions.iter().position(|r| r.name == name);
    let state = match str_(p, "state").map(str::trim) {
        None => None,
        Some("open") => Some(RegionState::Open),
        Some("built") => {
            let f = feature.clone().ok_or_else(|| bad(cmd, "a built region names its `feature`"))?;
            let known = s.doc.find_feature(&f).ok_or_else(|| bad(cmd, format!("no feature `{f}`")))?;
            Some(RegionState::Built { feature: known.name.clone() })
        }
        Some("skipped") => Some(RegionState::Skipped { reason: reason.clone().ok_or_else(|| bad(cmd, "a skipped region says why in `reason`"))? }),
        Some("inferred") => {
            Some(RegionState::Inferred { reason: reason.clone().ok_or_else(|| bad(cmd, "an inferred region says what from in `reason`"))? })
        }
        Some("removed") => {
            let k = found.ok_or_else(|| bad(cmd, format!("the plan has no region `{name}`")))?;
            plan.regions.remove(k);
            room(s, &scan, cmd)?;
            s.doc_mut().scan_plans.insert(scan.clone(), plan);
            return plan_reply(s, scan, true);
        }
        Some(_) => return Err(bad(cmd, "`state` is open, built, skipped, inferred or removed")),
    };
    let before = plan.clone();
    match found {
        Some(k) => {
            if let Some(at) = at {
                plan.regions[k].at = [at.x, at.y, at.z];
            }
            if let Some(state) = state {
                plan.regions[k].state = state;
            }
        }
        None => {
            let at = at.ok_or_else(|| bad(cmd, format!("a new region `{name}` needs `at`, a point on the scan inside it")))?;
            if plan.regions.len() >= MAX_REGIONS {
                return Err(bad(cmd, format!("a plan holds at most {MAX_REGIONS} regions")));
            }
            plan.regions.push(Region { name, at: [at.x, at.y, at.z], state: state.unwrap_or_default() });
        }
    }
    let changed = !s.doc.scan_plans.contains_key(&scan) || plan != before;
    if changed {
        room(s, &scan, cmd)?;
        s.doc_mut().scan_plans.insert(scan.clone(), plan);
    }
    plan_reply(s, scan, changed)
}

fn verdict(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.verdict";
    let scan = scan_of(s, p, cmd)?;
    let model = str_(p, "model").filter(|m| !m.trim().is_empty()).map(str::to_owned);
    // Scan Deviation's reading, the map shown left as it was.
    let shown = s.scan_deviation.take();
    let read = super::deviation(s, &json!({"scan": scan, "body": model}));
    s.scan_deviation = shown;
    let read = read.map_err(|e| bad(cmd, format!("{e}")))?;
    let num = |k: &str| read.get(k).and_then(Value::as_f64);
    let model = read.get("body").and_then(Value::as_str).unwrap_or_default().to_owned();
    let plan = s.doc.scan_plans.get(&scan).cloned();
    let target = plan.as_ref().map_or_else(|| ScanPlan::default().target(), ScanPlan::target);
    let places = read
        .get("regions")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .filter_map(|r| {
                    let at = r.get("at").and_then(crate::params::vec3)?;
                    Some(Place {
                        at,
                        depth: r.get("deepest").and_then(Value::as_f64)?.abs(),
                        area: r.get("area").and_then(Value::as_f64)?,
                        extra: r.get("side").and_then(Value::as_str) == Some("below"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let failed: Vec<String> =
        s.doc.features.iter().filter(|f| s.model.result(f.id).is_some_and(|r| r.error.is_some())).map(|f| f.name.clone()).collect();
    let reading = Reading {
        target,
        planned: plan.is_some(),
        scan_within: num("within_all").unwrap_or(0.0),
        model_within: num("model_within").unwrap_or(0.0),
        unseen: num("unseen"),
        open_regions: plan.iter().flat_map(|p| &p.regions).filter(|r| r.state == RegionState::Open).map(|r| r.name.clone()).collect(),
        failed,
        rolled_back: crate::cmd::file::rolled_back_note(s),
        places,
        // A feature's size as Deviation reads it: a square this many of the scan's triangles' edges wide.
        feature_area: super::shown_mesh(s, &scan)
            .map(|m| (super::FEATURE_EDGES * solidcraft_scan::remesh::typical_edge(&solidcraft_scan::Scan::new(&m.positions, &m.triangles))).powi(2))
            .filter(|a| a.is_finite())
            .unwrap_or(0.0),
        // A region the plan skips or infers holds the places within its reach: a tenth of the scan's size.
        accounted: {
            let reach = super::shown_mesh(s, &scan).map(|m| 0.1 * solidcraft_scan::Scan::new(&m.positions, &m.triangles).extent()).unwrap_or(0.0);
            plan.iter()
                .flat_map(|p| &p.regions)
                .filter(|r| matches!(r.state, RegionState::Skipped { .. } | RegionState::Inferred { .. }))
                .map(|r| (solidcore_record::Vec3::new(r.at[0], r.at[1], r.at[2]), reach))
                .collect()
        },
    };
    let v = judge(&reading);
    let mut next = Vec::new();
    for issue in &v.issues {
        let step = match issue.code {
            "out_of_tolerance" => Some(("scan.reconstruct_rest", "read and build what the model does not hold yet", json!({"body": scan}))),
            "feature_off" => Some((
                "scan.deviation",
                "see the places a feature wide beyond the tolerance (`regions`), and model them or mark their region skipped or inferred",
                json!({"scan": scan, "body": model}),
            )),
            "not_on_the_scan" => Some((
                "scan.deviation",
                "see where the model has material the scan does not (`regions` of side below)",
                json!({"scan": scan, "body": model}),
            )),
            "region_open" => {
                Some(("scan.plan_region", "build each open region, or mark it skipped or inferred with a reason", json!({"body": scan})))
            }
            "rolled_back" => Some(("timeline.rollback", "build the features after the history marker", json!({"position": "end"}))),
            "no_plan" => Some(("scan.plan", "say how the part was made, so the target is the part's", json!({"body": scan}))),
            _ => None,
        };
        if let Some((command, why, params)) = step
            && next.len() < 3
        {
            next.push(ScanVerdictStep { command: command.into(), why: why.into(), params });
        }
    }
    to_value(ScanVerdictResult {
        verdict: if v.met { "met" } else { "not_done" }.into(),
        planned: reading.planned,
        target,
        tolerance: tolerance::of(s, &scan),
        scan_within: reading.scan_within,
        model_within: reading.model_within,
        issues: v.issues.into_iter().map(|i| ScanVerdictIssue { code: i.code.into(), detail: i.detail }).collect(),
        places: v
            .places
            .into_iter()
            .map(|q| ScanVerdictPlace {
                at: [q.at.x, q.at.y, q.at.z],
                depth: q.depth,
                area: q.area,
                kind: if q.extra { "extra" } else { "missing" }.into(),
            })
            .collect(),
        next,
        scan,
        model,
        param_warnings: Vec::new(),
    })
}
