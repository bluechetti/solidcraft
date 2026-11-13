//! How each Reconstruct step changed the fit of the body it builds to the scan: the body is
//! measured against a sample of the scan's points before and after the step, and the reply says
//! whether the step brought it closer. An agent (or a person reading the reply) keeps a step that
//! helps and undoes one that does not, without a deviation map or a picture.

use serde_json::Value;

use super::{mesh_name, tolerance};
use crate::params::str_;
use crate::schema::scan_reconstruct::{ScanFitSample, ScanStepFit};
use crate::{Result, Session};

/// The most scan points read for each measurement.
const SAMPLE: usize = 100_000;
/// A change in the share explained smaller than this is no change.
const EXPLAINED_STEP: f64 = 0.005;
/// With the share explained the same, a change in rms smaller than this share is no change.
const RMS_STEP: f64 = 0.05;

/// Run `step`, a Reconstruct step building into a solid, and add `fit` to its reply: the solid
/// measured against the scan before and after. A step that fails fails as it would alone; a fit
/// that cannot be measured is left out.
pub(super) fn with_fit(s: &mut Session, p: &Value, cmd: &str, step: fn(&mut Session, &Value) -> Result<Value>) -> Result<Value> {
    let scan = mesh_name(s, p, "").ok();
    let target = scan
        .as_deref()
        .map(|scan| str_(p, "into").filter(|b| !b.trim().is_empty()).map_or_else(|| super::reconstruct::built_name(scan), str::to_owned));
    let before = match (&scan, &target) {
        (Some(scan), Some(target)) => sample(s, scan, target),
        _ => None,
    };
    let mut reply = step(s, p).map_err(|e| super::refusal::reconstruct_refusal(cmd, e))?;
    let Some(scan) = scan else { return Ok(reply) };
    let solid = reply.get("solid").and_then(Value::as_str).map(str::to_owned).or(target);
    let Some(solid) = solid else { return Ok(reply) };
    let Some(after) = sample(s, &scan, &solid) else { return Ok(reply) };
    let verdict = verdict(before.as_ref(), &after);
    let note = (verdict == "worse").then(|| {
        "this step left the body farther from the scan: undo it (edit.undo) unless a later step needs it, or read the part a region at a time (scan.reconstruct_selection)"
            .to_owned()
    });
    let fit = ScanStepFit { solid, before, after, verdict: verdict.into(), note };
    if let (Some(obj), Ok(fit)) = (reply.as_object_mut(), serde_json::to_value(fit)) {
        obj.insert("fit".into(), fit);
    }
    Ok(reply)
}

/// Whether the step brought the body closer: the share of the scan explained first, then the rms.
fn verdict(before: Option<&ScanFitSample>, after: &ScanFitSample) -> &'static str {
    let Some(before) = before else { return "first" };
    let explained = after.explained - before.explained;
    if explained > EXPLAINED_STEP {
        "better"
    } else if explained < -EXPLAINED_STEP {
        "worse"
    } else if after.rms < before.rms * (1.0 - RMS_STEP) {
        "better"
    } else if after.rms > before.rms * (1.0 + RMS_STEP) {
        "worse"
    } else {
        "same"
    }
}

/// `solid` measured against at most [`SAMPLE`] of `scan`'s points, evenly spread through them;
/// `None` when either is missing, the solid is a mesh or cannot be meshed.
pub(super) fn sample(s: &Session, scan: &str, solid: &str) -> Option<ScanFitSample> {
    let tolerance = tolerance::of(s, scan);
    let limit = 0.5_f64.max(5.0 * tolerance);
    let state = s.model.state();
    let model = state.body(solid).filter(|b| !b.body.is_mesh())?;
    let (positions, _) = state.body(scan)?.body.triangle_mesh()?;
    let step = (positions.len() / SAMPLE).max(1);
    let points: Vec<_> = positions.iter().step_by(step).copied().filter(|p| p.is_finite()).collect();
    if points.is_empty() {
        return None;
    }
    let surface = model.body.mesh(&solidcraft_kernel::MeshTolerance::chord(tolerance / 4.0)).ok()?;
    let d = solidcraft_scan::deviation::measure(&points, &surface, tolerance, 4.0 * limit);
    let n = d.values.len() as f64;
    let share = |most: f64| d.values.iter().filter(|v| v.is_finite() && f64::from(v.abs()) <= most).count() as f64 / n;
    Some(ScanFitSample { within: share(tolerance), explained: share(limit), rms: d.rms, points: d.values.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(explained: f64, rms: f64) -> ScanFitSample {
        ScanFitSample { within: 0.0, explained, rms, points: 10 }
    }

    /// More of the scan explained is better whatever the rms; the same share is judged by rms.
    #[test]
    fn a_step_is_judged_by_the_share_explained_then_the_rms() {
        assert_eq!(verdict(None, &at(0.5, 1.0)), "first");
        assert_eq!(verdict(Some(&at(0.5, 0.2)), &at(0.6, 0.4)), "better");
        assert_eq!(verdict(Some(&at(0.6, 0.2)), &at(0.5, 0.1)), "worse");
        assert_eq!(verdict(Some(&at(0.6, 0.4)), &at(0.601, 0.2)), "better");
        assert_eq!(verdict(Some(&at(0.6, 0.2)), &at(0.601, 0.4)), "worse");
        assert_eq!(verdict(Some(&at(0.6, 0.2)), &at(0.601, 0.201)), "same");
    }
}
