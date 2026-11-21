//! How each Reconstruct step changed the fit of the body it builds to the scan: the body is
//! measured against the scan both ways, by area, before and after the step, and the reply says
//! whether the step brought it closer. An agent (or a person reading the reply) keeps a step that
//! helps and undoes one that does not, without a deviation map or a picture.

use serde_json::Value;

use super::{mesh_name, tolerance};
use crate::params::str_;
use crate::schema::scan_reconstruct::{ScanFitSample, ScanStepFit};
use crate::{Result, Session};

/// The most places read on the scan, and on the body, for each measurement.
const SAMPLE: usize = 100_000;
/// A change in the share explained (less the change in the share added) smaller than this is no
/// change.
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
    let gone = before.is_some() && s.model.state().body(&solid).is_none();
    let after = match sample(s, &scan, &solid) {
        Some(after) => after,
        // A cut that took the whole body away explains none of the scan.
        None if gone => ScanFitSample { within: 0.0, explained: 0.0, added: None, rms: 0.0, points: 0 },
        None => return Ok(reply),
    };
    let verdict = if gone { "worse" } else { verdict(before.as_ref(), &after) };
    let note = (verdict == "worse").then(|| {
        if gone {
            "this step took the whole body away: undo it (edit.undo)".to_owned()
        } else {
            "this step left the body farther from the scan: undo it (edit.undo) unless a later step needs it, or read the part a region at a time (scan.reconstruct_selection)"
                .to_owned()
        }
    });
    let fit = ScanStepFit { solid, before, after, verdict: verdict.into(), note };
    if let (Some(obj), Ok(fit)) = (reply.as_object_mut(), serde_json::to_value(fit)) {
        obj.insert("fit".into(), fit);
    }
    Ok(reply)
}

/// Whether the step brought the body closer: the share of the scan it explains, less the share of
/// its own surface the scan does not have, first; then the rms. Counted one way only, a step that
/// built a little more of the scan and much the scan does not have read as better (T-LESS 7: an
/// extrusion past the part).
pub(super) fn verdict(before: Option<&ScanFitSample>, after: &ScanFitSample) -> &'static str {
    let Some(before) = before else { return "first" };
    let added = match (before.added, after.added) {
        (Some(was), Some(now)) => now - was,
        _ => 0.0,
    };
    let explained = after.explained - before.explained - added;
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

/// How far from the body a part of the scan may be and still be explained by it: five times the
/// scan's tolerance, at least half a millimetre (a scan's own noise and the rounds of its sharp
/// edges stay within it).
pub(super) fn limit(tolerance: f64) -> f64 {
    0.5_f64.max(5.0 * tolerance)
}

/// `solid` measured against `scan` both ways: at most [`SAMPLE`] places spread evenly over the
/// scan's surface, each standing for its share of the area (every k-th point instead weighed a CAD
/// mesh's long thin triangles as a tiny one's), and as many over the solid's, for the share the scan
/// does not have; `None` when either is missing, the solid is a mesh or cannot be meshed.
pub(super) fn sample(s: &Session, scan: &str, solid: &str) -> Option<ScanFitSample> {
    use solidcraft_scan::deviation;
    let tolerance = tolerance::of(s, scan);
    let limit = limit(tolerance);
    let model = {
        let state = s.model.state();
        state.body(solid).filter(|b| !b.body.is_mesh())?.body.clone()
    };
    let scan_mesh = super::shown_mesh(s, scan)?;
    let (points, areas): (Vec<_>, Vec<f64>) = deviation::places(&scan_mesh, SAMPLE).into_iter().filter(|(p, _)| p.is_finite()).unzip();
    let total: f64 = areas.iter().sum();
    if points.is_empty() || !(total > 0.0) {
        return None;
    }
    let surface = model.mesh(&solidcraft_kernel::MeshTolerance::chord(tolerance / 4.0)).ok()?;
    let d = deviation::measure(&points, &surface, tolerance, 4.0 * limit);
    let share =
        |most: f64| d.values.iter().zip(&areas).filter(|(v, _)| v.is_finite() && f64::from(v.abs()) <= most).map(|(_, a)| a).sum::<f64>() / total;
    let (squares, reached) = d.values.iter().zip(&areas).filter(|(v, _)| v.is_finite()).fold((0.0, 0.0), |(sq, ar), (v, a)| {
        let v = f64::from(*v);
        (sq + v * v * a, ar + a)
    });
    let rms = if reached > 0.0 { (squares / reached).sqrt() } else { 0.0 };
    // The other way: the solid's surface measured to the scan.
    let added = {
        let places = deviation::places(&surface, SAMPLE);
        let tree = super::select::scan_tree(&scan_mesh);
        (!places.is_empty()).then(|| deviation::borne_out(&places, &tree, tolerance, limit, 4.0 * limit).beyond)
    };
    Some(ScanFitSample { within: share(tolerance), explained: share(limit), added, rms, points: points.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(explained: f64, rms: f64) -> ScanFitSample {
        ScanFitSample { within: 0.0, explained, added: None, rms, points: 10 }
    }

    fn adding(explained: f64, added: f64) -> ScanFitSample {
        ScanFitSample { within: 0.0, explained, added: Some(added), rms: 0.2, points: 10 }
    }

    /// A step that explains a little more of the scan but builds more the scan does not have is
    /// worse; one that takes away what the scan does not have, explaining as much, is better.
    #[test]
    fn a_step_that_builds_what_the_scan_does_not_have_is_worse() {
        assert_eq!(verdict(Some(&adding(0.80, 0.05)), &adding(0.81, 0.12)), "worse");
        assert_eq!(verdict(Some(&adding(0.80, 0.12)), &adding(0.80, 0.05)), "better");
        assert_eq!(verdict(Some(&adding(0.80, 0.05)), &adding(0.85, 0.05)), "better");
        assert_eq!(verdict(Some(&adding(0.80, 0.05)), &adding(0.80, 0.052)), "same");
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
