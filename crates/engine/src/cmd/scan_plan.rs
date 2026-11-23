//! A scan's plan (`scan.plan`, `scan.plan_region`) and the verdict on its model (`scan.verdict`):
//! whether the model is done, computed from the design (Scan Deviation both ways at the scan's
//! tolerance against the plan's target, the plan's regions, the timeline), never from what was
//! clicked, so neither a person nor an agent hands over part of a part as finished.

use serde_json::{Value, json};
use solidcraft_doc::scan_plan::{MAX_REGIONS, MAX_SCANS, MAX_TEXT, Process, Purpose, Region, RegionState, Role, ScanPlan, valid_target};
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::verdict::{Place, Reading, judge};

use super::{CommandSpec, mesh_name, tolerance};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_plan::{
    ScanKnownSizeParams, ScanKnownSizeResult, ScanPlanParams, ScanPlanRegionParams, ScanPlanRegionView, ScanPlanResult, ScanVerdictIssue,
    ScanVerdictParams, ScanVerdictPlace, ScanVerdictResult, ScanVerdictStep,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const PLAN: TypedContract<ScanPlanParams> = TypedContract::new();
const REGION: TypedContract<ScanPlanRegionParams> = TypedContract::new();
const VERDICT: TypedContract<ScanVerdictParams> = TypedContract::new();
const KNOWN_SIZE: TypedContract<ScanKnownSizeParams> = TypedContract::new();
/// The most a scan is scaled to a known size: a scanner's scale error, not a wrong pick or unit.
const MOST_SCALE: f64 = 0.05;

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
    CommandSpec::new("scan.known_size", "Known Size", crate::checked!("scan.known_size", known_size))
        .icon("measure")
        .contract(&KNOWN_SIZE)
        .output(output_schema_for::<ScanKnownSizeResult>)
        .params("scan? (or body?): mesh body; between?: [[x,y,z], [x,y,z]] two points on the scan, one on each of two parallel faces; diameter?: [x,y,z] a point on a round face; value: mm, the size measured on the part (callipers); apply?: bool (default false): the scan's size there (planes or a cylinder fitted at the points, nothing kept) against the size measured, how far off the scan is, and with `apply` the scan scaled about its middle to agree, refused when it is more than 5 % off (a wrong pick, or the wrong unit: mesh.units); one undo step when applied → scan, kind, measured, value, factor, off, applied, note"),
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
    ScanPlanRegionView {
        name: r.name.clone(),
        at: r.at,
        state: state.into(),
        feature,
        reason,
        role: r.role.map(|x| role_name(x).to_owned()),
        hold: r.hold,
        hold_at: r.hold_at.clone(),
        selection: r.selection.clone(),
    }
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Focus => "focus",
        Role::Hold => "hold",
        Role::Ignore => "ignore",
        Role::Keep => "keep",
    }
}

fn purpose_name(p: Purpose) -> &'static str {
    match p {
        Purpose::Replacement => "replacement",
        Purpose::Copy => "copy",
    }
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
        purpose: plan.purpose.map(|x| purpose_name(x).to_owned()),
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
    // Some(None): "none", the purpose taken away.
    let purpose = match str_(p, "purpose").map(str::trim) {
        None => None,
        Some("replacement") => Some(Some(Purpose::Replacement)),
        Some("copy") => Some(Some(Purpose::Copy)),
        Some("none") => Some(None),
        Some(_) => return Err(bad(cmd, "`purpose` is replacement, copy or none")),
    };
    let given_tolerance = crate::params::expr(p, "tolerance");
    if reset && (process.is_some() || target.is_some() || given_tolerance.is_some() || purpose.is_some()) {
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
    } else if process.is_some() || target.is_some() || purpose.is_some() {
        room(s, &scan, cmd)?;
        let mut next = s.doc.scan_plans.get(&scan).cloned().unwrap_or_default();
        let was = s.doc.scan_plans.get(&scan).cloned();
        if let Some(process) = process {
            next.process = process;
        }
        if target.is_some() {
            next.target = target;
        }
        if let Some(purpose) = purpose {
            next.purpose = purpose;
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
    // Some(None): "none", the role taken away.
    let role = match str_(p, "role").map(str::trim) {
        None => None,
        Some("focus") => Some(Some(Role::Focus)),
        Some("hold") => Some(Some(Role::Hold)),
        Some("ignore") => Some(Some(Role::Ignore)),
        Some("keep") => Some(Some(Role::Keep)),
        Some("none") => Some(None),
        Some(_) => return Err(bad(cmd, "`role` is focus, hold, ignore, keep or none")),
    };
    let hold = match p.get("hold").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|h| h.is_finite() && *h > 0.0).ok_or_else(|| bad(cmd, "`hold` is the size held there, in mm, above 0"))?),
        None => None,
    };
    let role = match (role, hold) {
        (None, Some(_)) => Some(Some(Role::Hold)),
        (Some(Some(Role::Hold)), None) => return Err(bad(cmd, "a region that holds a size gives it: `hold`, in mm")),
        (r, _) => r,
    };
    // The part of the scan selected now as the region's extent: kept in the design as a selection.
    let (selection, at) = if bool_(p, "selected") == Some(true) {
        let picked = s
            .scan_selection
            .clone()
            .filter(|k| k.scan == scan && k.count > 0 && k.current(s))
            .ok_or_else(|| bad(cmd, format!("nothing is selected on `{scan}`: select the region first (scan.select)")))?;
        let kept_name: String = format!("Region {name}").chars().take(200).collect();
        super::super::find_command("scan.keep_selection")
            .ok_or_else(|| EngineError::UnknownCommand("scan.keep_selection".into()))?
            .run_nested(s, &json!({"name": kept_name}))?;
        // A point in it: the middle of its first triangle, unless `at` was given.
        let first = super::shown_mesh(s, &scan).and_then(|m| {
            let t = picked.selected.iter().position(|on| *on)?;
            let [a, b, c] = *m.triangles.get(t)?;
            let q = |i: u32| m.positions.get(i as usize).copied();
            let (a, b, c) = (q(a)?, q(b)?, q(c)?);
            Some((a + b + c) / 3.0)
        });
        (Some(kept_name), at.or(first))
    } else {
        (None, at)
    };
    // A held size on the selection: where it is, from the shapes Find All reads there.
    let placed_hold = if role == Some(Some(Role::Hold)) && bool_(p, "selected") == Some(true) && p.get("hold_at").is_none() {
        hold_place(s, &scan)?
    } else {
        Vec::new()
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
    // Ignore says why for itself: the region is skipped, the user's reason or the role's.
    let state = match (state, role) {
        (None, Some(Some(Role::Ignore))) => Some(RegionState::Skipped {
            reason: reason.clone().unwrap_or_else(|| "marked by the user to ignore (worn, damaged or not part of it)".into()),
        }),
        (state, _) => state,
    };
    let before = plan.clone();
    let k = match found {
        Some(k) => {
            if let Some(at) = at {
                plan.regions[k].at = [at.x, at.y, at.z];
            }
            if let Some(state) = state {
                plan.regions[k].state = state;
            }
            k
        }
        None => {
            let at = at.ok_or_else(|| bad(cmd, format!("a new region `{name}` needs `at`, a point on the scan inside it (or `selected: true`)")))?;
            if plan.regions.len() >= MAX_REGIONS {
                return Err(bad(cmd, format!("a plan holds at most {MAX_REGIONS} regions")));
            }
            plan.regions.push(Region::new(name, [at.x, at.y, at.z], state.unwrap_or_default()));
            plan.regions.len() - 1
        }
    };
    let region = &mut plan.regions[k];
    if let Some(role) = role {
        region.role = role;
        if role != Some(Role::Hold) {
            region.hold = None;
        }
    }
    if hold.is_some() {
        region.hold = hold;
    }
    if selection.is_some() {
        region.selection = selection;
    }
    // Where a held size is: given (`hold_at`), else read off the selection's shapes.
    if let Some(points) = p.get("hold_at").filter(|v| !v.is_null()) {
        let points: Vec<[f64; 3]> = points
            .as_array()
            .filter(|a| (1..=2).contains(&a.len()))
            .and_then(|a| a.iter().map(|v| crate::params::vec3(v).filter(|q| q.is_finite()).map(|q| [q.x, q.y, q.z])).collect::<Option<Vec<_>>>())
            .ok_or_else(|| bad(cmd, "`hold_at` is one point on a round face (its diameter) or two, one on each of two opposite faces"))?;
        region.hold_at = points;
    } else if region.role == Some(Role::Hold) && bool_(p, "selected") == Some(true) {
        region.hold_at = placed_hold.clone();
    }
    let changed = !s.doc.scan_plans.contains_key(&scan) || plan != before;
    if changed {
        room(s, &scan, cmd)?;
        s.doc_mut().scan_plans.insert(scan.clone(), plan);
    }
    plan_reply(s, scan, changed)
}

/// The verdict `scan.verdict` last gave, with the design it was read on: the agent chat shows it
/// as a card when a message's work read one (one card, no second reading of the scan).
#[derive(Debug)]
pub struct KeptVerdict {
    pub doc: std::sync::Arc<solidcraft_doc::Document>,
    pub reply: Value,
}

impl KeptVerdict {
    /// Whether it was read on the design as it is now: the same design, or an equal one (a
    /// command that ends leaves the design in a new copy even when nothing in it changed).
    pub fn current(&self, s: &Session) -> bool {
        std::sync::Arc::ptr_eq(&self.doc, &s.doc) || *self.doc == *s.doc
    }
}

pub(crate) fn verdict(s: &mut Session, p: &Value) -> Result<Value> {
    let reply = read_verdict(s, p)?;
    s.scan_verdict = Some(std::sync::Arc::new(KeptVerdict { doc: s.doc.clone(), reply: reply.clone() }));
    Ok(reply)
}

fn read_verdict(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.verdict";
    let scan = scan_of(s, p, cmd)?;
    let model = str_(p, "model").filter(|m| !m.trim().is_empty()).map(str::to_owned);
    // Scan Deviation's reading, the map shown left as it was.
    let shown = s.scan_deviation.take();
    let read = super::deviation_for_verdict(s, &json!({"scan": scan, "body": model}));
    let measured = s.scan_deviation.take();
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
        scan_within: within_kept(s, &scan, plan.as_ref(), measured.as_deref(), &read).unwrap_or_else(|| num("within_all").unwrap_or(0.0)),
        model_within: num("model_within").unwrap_or(0.0),
        unseen: num("unseen"),
        // A region the user keeps as it is (don't touch) is not the model's to build.
        open_regions: plan
            .iter()
            .flat_map(|p| &p.regions)
            .filter(|r| r.state == RegionState::Open && r.role != Some(Role::Keep))
            .map(|r| r.name.clone())
            .collect(),
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
    let mut v = judge(&reading);
    // Sizes the user holds: measured on the model's exact faces where each is.
    if let Some(plan) = plan.as_ref() {
        for issue in held_sizes(s, plan, &model) {
            v.met = false;
            v.issues.push(issue);
        }
    }
    let mut next = Vec::new();
    for issue in &v.issues {
        let step = match issue.code {
            "out_of_tolerance" => Some(("scan.reconstruct_rest", "read and build what the model does not hold yet", json!({"body": scan}))),
            "hold_off" | "hold_unmeasured" => Some((
                "parameters.list",
                "set the size the region holds on the feature that makes it (parameters.change, or timeline.edit), then read the verdict again",
                json!({"filter": "feature"}),
            )),
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

/// Known Size (`scan.known_size`): the scan's size at the points given against the size the user
/// measured, and with `apply` the scan scaled to agree. A calliper reading is the cheapest truth
/// there is: one fixes a scanner's scale and names the design value a tolerance snap cannot.
pub(crate) fn known_size(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.known_size";
    let scan = scan_of(s, p, cmd)?;
    let value = p
        .get("value")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| bad(cmd, "`value` is the size measured on the part, in mm, above 0"))?;
    let fit = |s: &mut Session, id: &str, at: &Value| -> Result<Value> {
        super::super::find_command(id)
            .ok_or_else(|| EngineError::UnknownCommand(id.into()))?
            .run_nested(s, &json!({"body": scan, "point": at, "keep": false}))
    };
    let vec = |v: &Value| -> Option<[f64; 3]> {
        let a = v.as_array()?;
        Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
    };
    let (kind, measured) = match (p.get("between").filter(|v| !v.is_null()), p.get("diameter").filter(|v| !v.is_null())) {
        (Some(Value::Array(points)), None) if points.len() == 2 => {
            let (a, b) = (fit(s, "scan.fit_plane", &points[0])?, fit(s, "scan.fit_plane", &points[1])?);
            let plane = |r: &Value| Some((vec(r.pointer("/plane/point")?)?, vec(r.pointer("/plane/normal")?)?));
            let (Some((pa, na)), Some((pb, nb))) = (plane(&a), plane(&b)) else { return Err(bad(cmd, "no face was fitted at one of the points")) };
            let dot = na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2];
            if dot.abs() < 2.0_f64.to_radians().cos() {
                return Err(bad(
                    cmd,
                    format!(
                        "the two faces are {:.1}° from parallel: pick two faces that face apart, the size is between them",
                        dot.abs().clamp(-1.0, 1.0).acos().to_degrees()
                    ),
                ));
            }
            let d = (0..3).map(|k| (pb[k] - pa[k]) * na[k]).sum::<f64>().abs();
            ("between", d)
        }
        (None, Some(at)) => {
            let c = fit(s, "scan.fit_cylinder", at)?;
            ("diameter", c.pointer("/cylinder/diameter").and_then(Value::as_f64).ok_or_else(|| bad(cmd, "no round face was fitted at the point"))?)
        }
        _ => return Err(bad(cmd, "give `between` (two points, on two parallel faces) or `diameter` (a point on a round face), not both")),
    };
    if !(measured.is_finite() && measured > 1e-6) {
        return Err(bad(cmd, "the size on the scan there is 0: the points are on the same face"));
    }
    let factor = value / measured;
    let off = measured / value - 1.0;
    let apply = bool_(p, "apply") == Some(true);
    let note = if apply {
        if (factor - 1.0).abs() > MOST_SCALE {
            return Err(bad(
                cmd,
                format!(
                    "the scan measures {measured:.3} mm there, {:.1} % off the {value} mm given: more than a scanner's scale error; check the points, or the scan's unit (mesh.units)",
                    off.abs() * 100.0
                ),
            ));
        }
        let mesh = super::shown_mesh(s, &scan).ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for q in mesh.positions.iter() {
            for (k, v) in [q.x, q.y, q.z].into_iter().enumerate() {
                lo[k] = lo[k].min(v);
                hi[k] = hi[k].max(v);
            }
        }
        let middle: Vec<f64> = (0..3).map(|k| 0.5 * (lo[k] + hi[k])).collect();
        super::super::find_command("solid.scale")
            .ok_or_else(|| EngineError::UnknownCommand("solid.scale".into()))?
            .run_nested(s, &json!({"bodies": [scan], "factor": factor, "origin": middle}))
            .map_err(|e| bad(cmd, format!("the scan could not be scaled: {e}")))?;
        format!("the scan was scaled by {factor:.5} about its middle; read it again (scan.find_all, scan.to_cad) before building on it")
    } else if off.abs() <= 0.001 {
        format!("the scan agrees with the size measured ({:.2} %)", off * 100.0)
    } else {
        format!(
            "the scan is {:.2} % {} here; `\"apply\": true` scales it to agree (only within {:.0} %)",
            off.abs() * 100.0,
            if off > 0.0 { "large" } else { "small" },
            MOST_SCALE * 100.0
        )
    };
    to_value(ScanKnownSizeResult { scan, kind: kind.into(), measured, value, factor, off, applied: apply, note, param_warnings: Vec::new() })
}

/// The share of the scan within the tolerance of the model, the regions the user marked to ignore
/// left out (worn, damaged, a fixture: not what the model is to hold); `None` when the plan
/// ignores no region with an extent, or the reading can't be had.
fn within_kept(s: &Session, scan: &str, plan: Option<&ScanPlan>, map: Option<&super::DeviationMap>, read: &Value) -> Option<f64> {
    let mesh = super::shown_mesh(s, scan)?;
    let mut ignored = vec![false; mesh.triangles.len()];
    let mut any = false;
    for name in plan?.regions.iter().filter(|r| r.role == Some(Role::Ignore)).filter_map(|r| r.selection.as_deref()) {
        let Some(kept) = s.doc.scan_selections.iter().find(|k| k.name == name && k.body == scan && k.triangles as usize == mesh.triangles.len())
        else {
            continue;
        };
        for (t, on) in kept.flags().into_iter().enumerate() {
            if on && let Some(flag) = ignored.get_mut(t) {
                *flag = true;
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    let map = map?;
    let tolerance = read.get("tolerance").and_then(Value::as_f64)?;
    let far = read.get("far").and_then(Value::as_f64)?;
    let kept: Vec<[u32; 3]> = mesh.triangles.iter().zip(&ignored).filter(|(_, off)| !**off).map(|(t, _)| *t).collect();
    Some(solidcraft_scan::deviation::shares(&mesh.positions, &kept, &map.values, tolerance, far).within_all)
}

/// Commands that rewrite a scan's mesh (or scale or move it): an agent may not run them on a scan
/// whose plan keeps a region as it is, unless they work only on a selection away from it.
const REWRITES_SCAN: &[&str] = &[
    "mesh.units",
    "scan.clean_up",
    "scan.delete_selection",
    "scan.denoise",
    "scan.fill_holes",
    "scan.known_size",
    "scan.reduce",
    "scan.remesh",
    "scan.remove_and_fill",
    "scan.remove_floating",
    "scan.remove_markers",
    "scan.repair",
    "solid.scale",
];

/// Why an agent may not run `command` with `params`: it would change what the user marked
/// "don't touch" (a plan region with role keep): the scan under such a region (a command that
/// rewrites the scan, unless it works only on a selection apart from every kept region), or the
/// feature that built it (edited, deleted or suppressed). `None` when it may. The agent chat's
/// gate asks this of every request the agent sends, so the boundary is the app's, not the agent's.
pub fn kept_from_agent(s: &Session, command: &str, params: &Value) -> Option<String> {
    let kept: Vec<(&String, &Region)> = s
        .doc
        .scan_plans
        .iter()
        .flat_map(|(scan, plan)| plan.regions.iter().map(move |r| (scan, r)))
        .filter(|(_, r)| r.role == Some(Role::Keep))
        .collect();
    if kept.is_empty() {
        return None;
    }
    // A feature a kept region was built by: not edited, deleted or suppressed.
    if command.starts_with("timeline.") || command.starts_with("feature.") {
        let named: Vec<String> = ["feature", "features", "id", "ids", "name"]
            .iter()
            .filter_map(|k| params.get(*k))
            .flat_map(|v| match v {
                Value::Array(a) => a.clone(),
                v => vec![v.clone()],
            })
            .filter_map(|v| v.as_str().map(str::to_owned).or_else(|| v.as_u64().map(|n| n.to_string())))
            .collect();
        for (scan, r) in &kept {
            if let RegionState::Built { feature } = &r.state {
                let id = s.doc.find_feature(feature).map(|f| f.id.to_string());
                if named.iter().any(|n| n == feature || Some(n) == id.as_ref()) {
                    return Some(format!(
                        "the user marked region `{}` of `{scan}` don't touch: `{feature}`, which built it, may not be changed; ask the user first",
                        r.name
                    ));
                }
            }
        }
        return None;
    }
    if !REWRITES_SCAN.contains(&command) {
        return None;
    }
    // Which scans the command changes: `bodies` (Scale), else the one it names or defaults to.
    let scans: Vec<String> = match params.get("bodies").and_then(Value::as_array) {
        Some(list) => list.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        None => scan_of(s, params, command).ok().into_iter().collect(),
    };
    // Known Size changes the scan only when applied.
    if command == "scan.known_size" && bool_(params, "apply") != Some(true) {
        return None;
    }
    for scan in scans {
        let regions: Vec<&Region> = kept.iter().filter(|(k, _)| **k == scan).map(|(_, r)| *r).collect();
        if regions.is_empty() {
            continue;
        }
        // On a selection apart from every kept region's extent: the kept part is not touched.
        if bool_(params, "selected") == Some(true)
            && let Some(picked) = s.scan_selection.as_ref().filter(|k| k.scan == scan && k.count > 0 && k.current(s))
        {
            let overlaps = regions.iter().any(|r| {
                let Some(extent) = r.selection.as_deref().and_then(|n| s.doc.scan_selections.iter().find(|k| k.name == n && k.body == scan)) else {
                    // A kept region with no extent: its place alone is known, so nothing is assumed apart.
                    return true;
                };
                extent.flags().iter().zip(picked.selected.iter()).any(|(a, b)| *a && *b)
            });
            if !overlaps {
                continue;
            }
        }
        let names: Vec<&str> = regions.iter().map(|r| r.name.as_str()).collect();
        return Some(format!(
            "the user marked {} of `{scan}` don't touch: `{command}` would change the scan there; work on a selection apart from it, or ask the user first",
            names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    None
}

/// [`kept_from_agent`] of a control request: one command (`engine.execute`) or each step of a
/// batch (`engine.batch`), the first refused; command ids as the registry spells them.
pub fn kept_from_agent_request(s: &Session, method: &str, params: &Value) -> Option<String> {
    let step = |v: &Value| v.get("command").or_else(|| v.get("id")).and_then(Value::as_str).map(str::to_owned);
    let check = |id: String, p: &Value| {
        let id = crate::find_command(&id).map_or(id, |c| c.id.to_owned());
        kept_from_agent(s, &id, p)
    };
    match method {
        "engine.execute" | "command" => check(step(params)?, params.get("params").unwrap_or(&Value::Null)),
        "engine.batch" | "engine.script" => params
            .as_array()
            .or_else(|| params.get("commands").and_then(Value::as_array))?
            .iter()
            .take(10_000)
            .find_map(|v| check(step(v)?, v.get("params").unwrap_or(&Value::Null))),
        _ => None,
    }
}

/// Where a held size is on the part of `scan` selected now, from the shapes Find All reads there
/// (it reads the scan first when it has not): a round face (a cylinder, most of the selection) is
/// its diameter, one point; two opposite planes, one point on each. Empty when neither is plain.
fn hold_place(s: &mut Session, scan: &str) -> Result<Vec<[f64; 3]>> {
    let current = |s: &Session| s.scan_shapes.as_ref().is_some_and(|k| k.scan == scan && k.current(s));
    if !current(s) {
        super::super::find_command("scan.find_all")
            .ok_or_else(|| EngineError::UnknownCommand("scan.find_all".into()))?
            .run_nested(s, &json!({"body": scan, "show": true}))?;
    }
    let (Some(found), Some(picked), Some(mesh)) = (
        s.scan_shapes.clone().filter(|k| k.scan == scan),
        s.scan_selection.clone().filter(|k| k.scan == scan && k.count > 0),
        super::shown_mesh(s, scan),
    ) else {
        return Ok(Vec::new());
    };
    // The selected triangles by Find All region: (region, their area, their middles weighed by
    // area). By area, not by count: a face meshed coarser than its neighbour is no less of the part.
    let mut groups: std::collections::BTreeMap<u32, (f64, solidcore_record::Vec3)> = std::collections::BTreeMap::new();
    for (t, _) in picked.selected.iter().enumerate().filter(|(_, on)| **on) {
        let (Some(&r), Some(&[a, b, c])) = (found.regions.get(t), mesh.triangles.get(t)) else { continue };
        let q = |i: u32| mesh.positions.get(i as usize).copied();
        let (Some(a), Some(b), Some(c)) = (q(a), q(b), q(c)) else { continue };
        let area = 0.5 * (b - a).cross(c - a).len();
        let entry = groups.entry(r).or_insert((0.0, solidcore_record::Vec3::ZERO));
        entry.0 += area;
        entry.1 += (a + b + c) / 3.0 * area;
    }
    let mut groups: Vec<(u32, f64, solidcore_record::Vec3)> =
        groups.into_iter().filter(|(_, (area, _))| *area > 0.0).map(|(r, (area, sum))| (r, area, sum / area)).collect();
    groups.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = groups.iter().map(|g| g.1).sum();
    let shape = |r: u32| found.outcome.segments.get(r as usize).and_then(|seg| seg.shape);
    let at = |v: solidcore_record::Vec3| [v.x, v.y, v.z];
    if let Some(&(r, area, middle)) = groups.first()
        && matches!(shape(r), Some(solidcraft_scan::fit::Shape::Cylinder { .. }))
        && area * 2.0 >= total
    {
        // The point on the scan in the group nearest its middle: the middle of a bore is off it.
        let on = picked
            .selected
            .iter()
            .enumerate()
            .filter(|(t, on)| **on && found.regions.get(*t) == Some(&r))
            .filter_map(|(t, _)| mesh.triangles.get(t).and_then(|tri| mesh.positions.get(tri[0] as usize)).copied())
            .min_by(|a, b| (*a - middle).len().total_cmp(&(*b - middle).len()));
        return Ok(on.map(|q| vec![at(q)]).unwrap_or_default());
    }
    // Two planes facing apart, each a share of the selection.
    let planes: Vec<(solidcore_record::Vec3, solidcore_record::Vec3)> = groups
        .iter()
        .filter(|g| g.1 * 5.0 >= total)
        .filter_map(|g| match shape(g.0) {
            Some(solidcraft_scan::fit::Shape::Plane { normal, .. }) => Some((normal, g.2)),
            _ => None,
        })
        .collect();
    for (i, (na, a)) in planes.iter().enumerate() {
        for (nb, b) in planes.iter().skip(i + 1) {
            if na.dot(*nb) < -(5.0_f64.to_radians().cos()) {
                return Ok(vec![at(*a), at(*b)]);
            }
        }
    }
    Ok(Vec::new())
}

/// The sizes a plan's regions hold, each measured on `model`'s exact faces where it is (the
/// face nearest each point, solved): an issue for each the model does not hold within a hundredth
/// of a millimetre, or where no face of the right kind is near.
fn held_sizes(s: &Session, plan: &ScanPlan, model: &str) -> Vec<solidcraft_scan::verdict::Issue> {
    let state = s.model.state();
    let Some(body) = state.body(model) else { return Vec::new() };
    let reach = 5.0;
    let face = |q: [f64; 3]| {
        let p = solidcore_record::Vec3::new(q[0], q[1], q[2]);
        let (_, f) = body.body.nearest_face_within(p, reach)?;
        solidcraft_kernel::face_surface(&body.body, f)
    };
    let mut issues = Vec::new();
    for r in plan.regions.iter().filter(|r| r.role == Some(Role::Hold)) {
        let Some(want) = r.hold else { continue };
        let measured = match r.hold_at.as_slice() {
            [] => continue,
            [q] => match face(*q) {
                Some(solidcraft_kernel::FaceSurface::Cylinder { radius, .. }) => Some(2.0 * radius),
                _ => None,
            },
            [a, b, ..] => match (face(*a), face(*b)) {
                (
                    Some(solidcraft_kernel::FaceSurface::Plane { normal: na, point: pa }),
                    Some(solidcraft_kernel::FaceSurface::Plane { normal: nb, point: pb }),
                ) if solidcore_record::Vec3::from(na).dot(solidcore_record::Vec3::from(nb)) < -(2.0_f64.to_radians().cos()) => {
                    Some((pb - pa).dot(solidcore_record::Vec3::from(na)).abs())
                }
                _ => None,
            },
        };
        match measured {
            Some(m) if (m - want).abs() <= HOLD_WITHIN => {}
            Some(m) => issues.push(solidcraft_scan::verdict::Issue {
                code: "hold_off",
                detail: format!("`{}` holds {want} mm; the model has {m:.3} mm there", r.name),
            }),
            None => issues.push(solidcraft_scan::verdict::Issue {
                code: "hold_unmeasured",
                detail: format!(
                    "`{}` holds {want} mm, but the model has no {} near it to measure",
                    r.name,
                    if r.hold_at.len() == 1 { "round face" } else { "two opposite faces" }
                ),
            }),
        }
    }
    issues
}

/// How close a held size must be on the model, mm: it is a design value, not a scan's reading.
const HOLD_WITHIN: f64 = 0.01;
