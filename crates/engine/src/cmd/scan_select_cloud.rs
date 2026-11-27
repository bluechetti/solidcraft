//! `scan.select` on a point cloud (`cloud`): the points selected by a brush, a box, all, none or
//! the inverse, held by the session as a scan's selected triangles are: a view of the cloud, not
//! part of the design and not an undo step. The fits take it (`scan.fit_*` with the same `cloud`
//! and no `point`). It follows the cloud while its points are the same count; a cloud inserted
//! again or cut down gives it up.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;

use super::{MOST_PLACES, Mode, NEXT_STAMP, mode, points};
use crate::params::bad;
use crate::schema::scan_select::ScanSelectResult;
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
    let ways = ["brush", "polygon", "box", "all", "none", "invert"];
    for other in ["region", "shape", "saved", "triangles", "grow", "shrink", "unexplained", "body"] {
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
    let places: Vec<Vec3> = (0..count).filter_map(|i| cloud.world_point(i)).collect();
    let before = selected(s, id, count);
    let current = || before.as_ref().map(|k| (*k.selected).clone()).unwrap_or_else(|| vec![false; count]);
    let v = &p[way];
    let found = match way {
        "all" => vec![true; count],
        "none" => vec![false; count],
        "invert" => current().into_iter().map(|f| !f).collect(),
        "polygon" => {
            let (ring, view, through) = super::polygon_of(v, cmd)?;
            solidcraft_scan::selection::points_seen_inside(&places, &ring, &view, through)
        }
        "box" => {
            let corner = |key: &str| {
                crate::params::vec3(&v[key]).ok_or_else(|| bad(cmd, format!("`box.{key}` must be a point [x, y, z] of numbers")))
            };
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
        (_, "all" | "none" | "invert") => found,
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
    s.cloud_selection =
        (total > 0).then(|| Arc::new(CloudSelected { id, cloud: name.clone(), selected: Arc::new(flags), count: total, stamp }));
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
