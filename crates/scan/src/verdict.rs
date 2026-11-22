//! Whether a model of a scan is done: rules over numbers the engine gathers (a Scan Deviation read
//! both ways, the plan's target and regions, the timeline). "Met" only when every rule holds;
//! otherwise "not done", with each open issue and the worst places. A route that answers `ok` leaves
//! the judging to whoever reads it, and part of a part gets handed over as finished.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

/// What the verdict reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The share (0 to 1) of the scan's surface, and of the model's, each must have within the
    /// tolerance of the other.
    pub target: f64,
    /// Whether the design keeps a plan for the scan (without one, the default target is read).
    pub planned: bool,
    /// The share of the scan's surface within the tolerance of the model, the points too far to
    /// measure counted as not within.
    pub scan_within: f64,
    /// The share of the model's surface with the scan within the tolerance of it.
    pub model_within: f64,
    /// The share of the scan the scanner saw too poorly to read (none: all of it read).
    pub unseen: Option<f64>,
    /// The plan's regions not yet built, skipped or inferred.
    pub open_regions: Vec<String>,
    /// Features of the timeline that failed to build, and those after the history marker.
    pub failed: Vec<String>,
    pub rolled_back: Option<String>,
    /// Places beyond the tolerance, worst first: where, how far, and whether the model has
    /// material there the scan does not (`extra`) or lacks it.
    pub places: Vec<Place>,
    /// The area (mm²) a place beyond the tolerance must have to be a feature the model has wrong,
    /// not noise (Deviation's: a square this many of the scan's triangles' edges wide); 0 for none.
    pub feature_area: f64,
    /// The places the plan leaves out on purpose: each skipped or inferred region's point and how
    /// far round it the region reaches. A feature-sized place within one is accounted for.
    pub accounted: Vec<(Vec3, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub at: Vec3,
    pub depth: f64,
    pub area: f64,
    pub extra: bool,
}

/// One reason the model is not done.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub met: bool,
    pub issues: Vec<Issue>,
    /// The worst places, at most [`MOST_PLACES`].
    pub places: Vec<Place>,
}

/// The most places a verdict names.
pub const MOST_PLACES: usize = 3;

fn percent(share: f64) -> String {
    format!("{:.1} %", 100.0 * share)
}

/// The verdict on `r`.
pub fn judge(r: &Reading) -> Verdict {
    let mut issues = Vec::new();
    if !(r.scan_within >= r.target) {
        issues.push(Issue {
            code: "out_of_tolerance",
            detail: format!("{} of the scan is within the tolerance of the model; the target is {}", percent(r.scan_within), percent(r.target)),
        });
    }
    if !(r.model_within >= r.target) {
        issues.push(Issue {
            code: "not_on_the_scan",
            detail: format!(
                "{} of the model has the scan within the tolerance of it; the target is {} (material the part does not have, or faces the scanner did not see)",
                percent(r.model_within),
                percent(r.target)
            ),
        });
    }
    if !r.open_regions.is_empty() {
        issues.push(Issue { code: "region_open", detail: format!("regions not yet built, skipped or inferred: {}", r.open_regions.join(", ")) });
    }
    if !r.failed.is_empty() {
        issues.push(Issue { code: "feature_failed", detail: format!("features that failed to build: {}", r.failed.join(", ")) });
    }
    if let Some(note) = &r.rolled_back {
        issues.push(Issue { code: "rolled_back", detail: note.clone() });
    }
    // A place beyond the tolerance a feature wide is a feature the model has wrong, however much
    // of the scan is within it overall (agent trial r3-19: met at 95.7 % with a 96 mm² patch 1.8 mm
    // off), unless a region the plan skips or infers holds it.
    let wrong: Vec<&Place> = r
        .places
        .iter()
        .filter(|p| r.feature_area > 0.0 && p.area >= r.feature_area)
        .filter(|p| !r.accounted.iter().any(|(at, reach)| at.dist(p.at) <= reach.max(p.area.sqrt())))
        .collect();
    if let Some(worst) = wrong.iter().max_by(|a, b| (a.depth * a.area).total_cmp(&(b.depth * b.area))) {
        issues.push(Issue {
            code: "feature_off",
            detail: format!(
                "{} place(s) a feature wide beyond the tolerance; the worst {:.0} mm², {:.2} mm {} at [{:.1}, {:.1}, {:.1}] (model it, or mark the region skipped or inferred with a reason)",
                wrong.len(),
                worst.area,
                worst.depth,
                if worst.extra { "extra" } else { "missing" },
                worst.at.x,
                worst.at.y,
                worst.at.z
            ),
        });
    }
    let met = issues.is_empty();
    if !r.planned {
        issues.push(Issue {
            code: "no_plan",
            detail: format!("the scan has no plan: read against the default target {} (scan.plan sets the process and target)", percent(r.target)),
        });
    }
    if let Some(unseen) = r.unseen.filter(|u| *u > 0.0) {
        issues.push(Issue {
            code: "unseen",
            detail: format!("{} of the scan was seen too poorly to read: the model there is not checked by it", percent(unseen)),
        });
    }
    let mut places = r.places.clone();
    places.sort_by(|a, b| (b.depth * b.area).total_cmp(&(a.depth * a.area)));
    places.truncate(MOST_PLACES);
    Verdict { met, issues, places }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done() -> Reading {
        Reading { target: 0.9, planned: true, scan_within: 0.95, model_within: 0.97, ..Reading::default() }
    }

    fn codes(v: &Verdict) -> Vec<&str> {
        v.issues.iter().map(|i| i.code).collect()
    }

    /// Both ways at or past the target, every region accounted for, nothing failed: met.
    #[test]
    fn both_ways_past_the_target_with_nothing_open_is_met() {
        let v = judge(&done());
        assert!(v.met && v.issues.is_empty(), "{v:?}");
    }

    /// The scan held well but the model holding much the scan does not: not done (one way only,
    /// it would pass).
    #[test]
    fn a_model_with_material_the_scan_lacks_is_not_done() {
        let v = judge(&Reading { model_within: 0.7, ..done() });
        assert!(!v.met);
        assert_eq!(codes(&v), ["not_on_the_scan"]);
        assert!(v.issues[0].detail.contains("70.0 %"), "{v:?}");
    }

    /// 99 % within but one region open, a failed feature, a rolled-back design: not done, each
    /// named.
    #[test]
    fn an_open_region_or_a_failed_feature_is_not_done() {
        let r = Reading {
            open_regions: vec!["Inside".into()],
            failed: vec!["Fillet3".into()],
            rolled_back: Some("rolled back".into()),
            scan_within: 0.99,
            ..done()
        };
        let v = judge(&r);
        assert!(!v.met);
        assert_eq!(codes(&v), ["region_open", "feature_failed", "rolled_back"]);
        assert!(v.issues[0].detail.contains("Inside") && v.issues[1].detail.contains("Fillet3"));
    }

    /// No plan and some of the scan unseen are said, but do not by themselves make it not done.
    #[test]
    fn no_plan_and_unseen_are_said_but_do_not_block() {
        let v = judge(&Reading { planned: false, unseen: Some(0.12), ..done() });
        assert!(v.met, "{v:?}");
        assert_eq!(codes(&v), ["no_plan", "unseen"]);
    }

    /// A feature-sized place beyond the tolerance is not done, whatever the shares (trial r3-19);
    /// one a skipped or inferred region holds is accounted for; a place smaller than a feature is noise.
    #[test]
    fn a_feature_sized_place_off_is_not_done_unless_the_plan_accounts_for_it() {
        let place = Place { at: Vec3::new(2.1, 0.8, -18.3), depth: 1.8, area: 96.0, extra: false };
        let r = Reading { places: vec![place], feature_area: 40.0, ..done() };
        let v = judge(&r);
        assert!(!v.met && codes(&v) == ["feature_off"], "{v:?}");
        assert!(v.issues[0].detail.contains("96 mm²") && v.issues[0].detail.contains("missing"), "{v:?}");
        let held = Reading { accounted: vec![(Vec3::new(3.0, 0.0, -17.0), 5.0)], ..r.clone() };
        assert!(judge(&held).met, "a skipped region round it holds it");
        let small = Reading { feature_area: 200.0, ..r };
        assert!(judge(&small).met, "smaller than a feature");
    }

    /// The places named are the worst three by depth times area.
    #[test]
    fn the_worst_three_places_are_named() {
        let place = |depth: f64, area: f64| Place { at: Vec3::new(depth, 0.0, 0.0), depth, area, extra: false };
        let r = Reading { places: vec![place(1.0, 10.0), place(5.0, 1.0), place(0.5, 100.0), place(2.0, 20.0)], ..done() };
        let depths: Vec<f64> = judge(&r).places.iter().map(|p| p.depth).collect();
        assert_eq!(depths, [0.5, 2.0, 1.0]);
    }
}
