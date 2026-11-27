//! `scan.select` on a point cloud (`cloud`): the points selected by a brush, a box, all, none or
//! the inverse or the selection grown or shrunk, held by the session as a scan's selected
//! triangles are: a view of the cloud, not part of the design and not an undo step. The fits
//! take it (`scan.fit_*` with the same `cloud` and no `point`). It follows the cloud while its
//! points are the same count; a cloud inserted again or cut down gives it up.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;

use super::{MOST_PLACES, MOST_RINGS, Mode, NEXT_STAMP, mode, points};
use crate::params::bad;
use crate::schema::scan_select::{ScanDeleteSelectionResult, ScanKeepSelectionResult, ScanSelectResult};
use crate::{EngineError, Result, Session};

/// What is selected on a point cloud.
#[derive(Debug)]
pub struct CloudSelected {
    /// The cloud's id, and its name.
    pub id: u64,
    pub cloud: String,
    /// One flag a point.
    pub selected: Arc<Vec<bool>>,
    pub count: usize,
    /// This selection's own number, as [`super::ScanSelected::stamp`].
    pub stamp: u64,
}

/// What is selected on the cloud `id` of `points` points now, if it is of the cloud as it is.
pub fn selected(s: &Session, id: u64, points: usize) -> Option<Arc<CloudSelected>> {
    s.cloud_selection.clone().filter(|k| k.id == id && k.selected.len() == points)
}

pub(super) fn select(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let ways = ["brush", "polygon", "box", "saved", "all", "none", "invert", "grow", "shrink"];
    for other in ["region", "shape", "triangles", "unexplained", "body"] {
        if p.get(other).is_some_and(|v| !v.is_null() && *v != json!(false)) {
            return Err(bad(cmd, format!("`{other}` is for a scan's triangles: with `cloud`, give one of {}", ways.join(", "))));
        }
    }
    let given: Vec<&str> = ways.iter().copied().filter(|k| p.get(*k).is_some_and(|v| !v.is_null() && *v != json!(false))).collect();
    let [way] = given[..] else {
        return Err(bad(cmd, format!("give one way to select points: {}", ways.join(", "))));
    };
    let mode = mode(p, cmd)?;
    let cloud = super::super::mesh::find_cloud(s, p, cmd)?;
    let (id, name, count) = (cloud.id, cloud.name.clone(), cloud.len());
    let cloud_name = name.as_str();
    let places: Vec<Vec3> = (0..count).filter_map(|i| cloud.world_point(i)).collect();
    let before = selected(s, id, count);
    let current = || before.as_ref().map(|k| (*k.selected).clone()).unwrap_or_else(|| vec![false; count]);
    let v = &p[way];
    let found = match way {
        "all" => vec![true; count],
        "none" => vec![false; count],
        "invert" => current().into_iter().map(|f| !f).collect(),
        "saved" => {
            let name = v.as_str().ok_or_else(|| bad(cmd, "`saved` is the name of a kept selection"))?;
            let kept = s
                .doc
                .cloud_selections
                .iter()
                .find(|k| k.name == name)
                .ok_or_else(|| bad(cmd, format!("no kept selection `{name}` of a point cloud")))?;
            if kept.body != cloud_name || kept.triangles as usize != count {
                return Err(bad(cmd, format!("the kept selection `{name}` is of `{}` as it was, not of `{cloud_name}` as it is now", kept.body)));
            }
            kept.flags()
        }
        "grow" | "shrink" => {
            let rings = v
                .as_u64()
                .filter(|n| (1..=MOST_RINGS).contains(n))
                .ok_or_else(|| bad(cmd, format!("`{way}` is a number of rings, 1 to {MOST_RINGS}")))?;
            // A ring is as wide as `step`, else twice the cloud's point spacing.
            let step = match crate::params::expr(p, "step") {
                Some(e) => s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("step: {e}")))?,
                None => 2.0 * solidcraft_scan::poisson::typical_spacing(&places),
            };
            if !(step.is_finite() && step > 0.0) {
                return Err(bad(cmd, "`step` is how wide a ring is, a length above 0 (the cloud's points have no spacing to take it from)"));
            }
            solidcraft_scan::selection::points_grown(&places, &current(), step * rings as f64, way == "grow")
        }
        "polygon" => {
            let (ring, view, through) = super::polygon_of(v, cmd)?;
            solidcraft_scan::selection::points_seen_inside(&places, &ring, &view, through)
        }
        "box" => {
            let corner =
                |key: &str| crate::params::vec3(&v[key]).ok_or_else(|| bad(cmd, format!("`box.{key}` must be a point [x, y, z] of numbers")));
            let (lo, hi) = (corner("min")?, corner("max")?);
            if lo.x > hi.x || lo.y > hi.y || lo.z > hi.z {
                return Err(bad(cmd, "`box.min` must be below `box.max` on every axis"));
            }
            places.iter().map(|q| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y && q.z >= lo.z && q.z <= hi.z).collect()
        }
        _ => {
            let at = points(&v["points"], cmd, "brush.points", 1)?;
            debug_assert!(at.len() <= MOST_PLACES);
            let radius = match crate::params::expr(v, "radius") {
                Some(e) => s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("brush.radius: {e}")))?,
                None => return Err(bad(cmd, "`brush.radius` is the brush's radius, a length above 0")),
            };
            if !(radius.is_finite() && radius > 0.0) {
                return Err(bad(cmd, "`brush.radius` must be a length above 0"));
            }
            solidcraft_scan::selection::points_near(&places, &at, radius)
        }
    };
    let flags: Vec<bool> = match (mode, way) {
        // These say what is selected whole, whichever the mode.
        (_, "all" | "none" | "invert" | "grow" | "shrink") => found,
        (Mode::Replace, _) => found,
        (Mode::Add, _) => current().into_iter().zip(&found).map(|(a, b)| a | *b).collect(),
        (Mode::Remove, _) => current().into_iter().zip(&found).map(|(a, b)| a & !*b).collect(),
    };
    let total = flags.iter().filter(|f| **f).count();
    let changed = before.as_ref().is_none_or(|k| *k.selected != flags) && !(before.is_none() && total == 0);
    let stamp = match before.as_ref().filter(|_| !changed) {
        Some(k) => k.stamp,
        None => NEXT_STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    };
    s.cloud_selection = (total > 0).then(|| Arc::new(CloudSelected { id, cloud: name.clone(), selected: Arc::new(flags), count: total, stamp }));
    let reply = ScanSelectResult {
        body: name,
        triangles: 0,
        area: 0.0,
        of: count,
        points: Some(total),
        changed,
        pieces: None,
        limit: None,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Keep what is selected on a point cloud in the design (`scan.keep_selection`): the cloud `p`
/// names, else the one a selection is held on.
pub(super) fn keep(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let pick = match p.get("cloud").filter(|v| !v.is_null()) {
        Some(_) => p.clone(),
        None => json!({"cloud": s.cloud_selection.as_ref().map(|k| k.id)}),
    };
    let cloud = super::super::mesh::find_cloud(s, &pick, cmd)?;
    let (id, cloud_name, count) = (cloud.id, cloud.name.clone(), cloud.len());
    let held = selected(s, id, count).ok_or_else(|| bad(cmd, "nothing is selected on the point cloud: select some of its points first"))?;
    let name = match crate::params::str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.chars().count() > solidcraft_doc::scan_selection::MAX_NAME => return Err(bad(cmd, "the name is too long")),
        Some(n) => n.to_owned(),
        None => (1..).map(|n| format!("Selection {n}")).find(|n| !s.doc.cloud_selections.iter().any(|k| k.name == *n)).unwrap_or_default(),
    };
    let kept = solidcraft_doc::scan_selection::ScanSelection::from_flags(&name, &cloud_name, &held.selected);
    let replaced = s.doc.cloud_selections.iter().any(|x| x.name == name);
    if !replaced && s.doc.cloud_selections.len() >= solidcraft_doc::scan_selection::MAX_SELECTIONS {
        return Err(bad(cmd, "the design keeps as many selections as it can: delete one first"));
    }
    let points = kept.count();
    let doc = s.doc_mut();
    match doc.cloud_selections.iter_mut().find(|x| x.name == name) {
        Some(old) => *old = kept,
        None => doc.cloud_selections.push(kept),
    }
    let reply = ScanKeepSelectionResult { name, body: cloud_name, triangles: 0, points: Some(points), replaced, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Take a kept selection of a point cloud out of the design (`scan.delete_selection` with `cloud`).
pub(super) fn delete(s: &mut Session, name: &str, cmd: &str) -> Result<Value> {
    if !s.doc.cloud_selections.iter().any(|k| k.name == name) {
        return Err(bad(cmd, format!("no kept selection `{name}` of a point cloud")));
    }
    s.doc_mut().cloud_selections.retain(|k| k.name != name);
    let reply = ScanDeleteSelectionResult { name: name.to_owned(), left: s.doc.cloud_selections.len(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
