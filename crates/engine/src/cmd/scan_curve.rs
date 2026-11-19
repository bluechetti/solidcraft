//! Drawing curves on the scan (`scan.curve_on_scan`): points clicked along the scan, the smooth
//! line through them laid on it, kept as an ordinary 3D sketch spline through the fewest points
//! that keep it within the tolerance of the scan. The curves go in the 3D sketch being edited, or
//! else in a 3D sketch named `Scan Curves`, made the first time; so they are sketch geometry to
//! sweep along, loft through or project, and a design opens anywhere.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::curve::{self, Fitted, Laid, Projector};

use super::{length, mesh_name};
use crate::params::bad;
use crate::schema::scan::{ScanCurveResult, ScanLineRound, ScanLineStretch, ScanMeasureLineResult};
use crate::{EngineError, Result, Session};

/// The 3D sketch the curves go in when none is being edited.
pub const SKETCH_NAME: &str = "Scan Curves";

/// The default tolerance for the scan `body` (or the selected or only one): how far the curve may
/// stray from the line laid on it, as fine as its triangles (see [`curve::default_tolerance`]).
pub fn default_tolerance(s: &Session, body: Option<&str>) -> Result<f64> {
    let cmd = "scan.curve_on_scan";
    let state = s.model.state();
    let name = mesh_name(s, &json!({ "body": body }), cmd)?;
    drop(state);
    super::with_shown(s, &name, |p, t| curve::default_tolerance(&solidcraft_scan::Scan::new(p, t)))
        .ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))
}

/// The projector last built: for which scan (its name and where its points are held, which
/// changes whenever the scan does) and the projector.
type Cached = (String, usize, usize, Arc<Projector>);
static PROJECTOR: Mutex<Option<Cached>> = Mutex::new(None);

/// The scan `body` (or the selected or only one) ready to put points on: built once per scan and
/// kept while the scan is unchanged.
pub fn projector(s: &Session, body: Option<&str>, cmd: &str) -> Result<(String, Arc<Projector>)> {
    let state = s.model.state();
    let name = mesh_name(s, &json!({ "body": body }), cmd)?;
    drop(state);
    // Where the scan shows: a curve drawn on a scan in a placed component lies on it there.
    let mesh = super::shown_mesh(s, &name).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let key = (mesh.positions.as_ptr() as usize, mesh.positions.len());
    if let Ok(cached) = PROJECTOR.lock()
        && let Some((n, ptr, len, p)) = cached.as_ref()
        && *n == name
        && (*ptr, *len) == key
    {
        return Ok((name, p.clone()));
    }
    // The scan's own points and triangles and the tree Select shares: nothing copied.
    let tree = super::select::scan_tree(&mesh);
    let built = Arc::new(Projector::sharing(mesh, tree));
    if let Ok(mut cached) = PROJECTOR.lock() {
        *cached = Some((name.clone(), key.0, key.1, built.clone()));
    }
    Ok((name, built))
}

/// How far from the scan a clicked point may be: a hundredth of the scan's size, at least five of
/// its triangles.
/// How big the scan `body` is: the diagonal of its bounds, in mm (0 for no scan).
pub fn size(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).and_then(|b| b.body.triangle_mesh()).map_or(0.0, |(p, t)| solidcraft_scan::Scan::new(p, t).extent())
}

fn pick_reach(on: &Projector, s: &Session, body: &str) -> f64 {
    let size = size(s, body);
    (size / 100.0).max(5.0 * on.edge).max(1e-3)
}

/// The line through `picks` laid on the scan and the curve that would be kept, for a dialog's
/// preview.
pub fn preview(s: &Session, body: Option<&str>, picks: &[Vec3], closed: bool, tolerance: f64) -> Result<(String, Laid, Fitted)> {
    let cmd = "scan.curve_on_scan";
    let (name, on) = projector(s, body, cmd)?;
    let laid = curve::lay(&on, picks, closed, pick_reach(&on, s, &name)).map_err(|e| bad(cmd, e.to_string()))?;
    let fitted = curve::fit(&laid, tolerance);
    Ok((name, laid, fitted))
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// Opens the sketch the curve goes in for drawing: the one named, the 3D sketch being edited, the
/// design's `Scan Curves`, or a new `Scan Curves`. Its id, and whether it was opened here (to
/// finish it again after).
pub(super) fn open_sketch(s: &mut Session, p: &Value, cmd: &str) -> Result<(u64, bool)> {
    let three_d = |s: &Session, id: u64| super::super::sketch_store::read(&s.doc, id).is_ok_and(|sk| sk.view.three_d);
    let named = match p.get("sketch").filter(|v| !v.is_null()) {
        Some(_) => Some(super::super::sketch_store::target(s, p, cmd)?),
        None => None,
    };
    let wanted = named.or_else(|| s.active_sketch.filter(|id| three_d(s, *id))).or_else(|| {
        let kept = s.doc.features.iter().rev().find(|f| f.name == SKETCH_NAME && matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. }));
        kept.map(|f| f.id).filter(|id| three_d(s, *id))
    });
    match wanted {
        Some(id) if s.active_sketch == Some(id) => Ok((id, false)),
        Some(id) => {
            if s.active_sketch.is_some() {
                return Err(bad(cmd, "finish the sketch being edited first, or draw in it"));
            }
            run(s, "sketch.edit", json!({"sketch": id}))?;
            run(s, "sketch.options", json!({"sketch": id, "sketch_3d": true}))?;
            Ok((id, true))
        }
        None => {
            if s.active_sketch.is_some() {
                return Err(bad(cmd, "the sketch being edited is not a 3D sketch: turn on 3D Sketch, or finish it"));
            }
            let made = run(s, "sketch.create", json!({"plane": "XY", "name": SKETCH_NAME}))?;
            let id = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch for the curves was not made"))?;
            run(s, "sketch.options", json!({"sketch": id, "sketch_3d": true}))?;
            Ok((id, true))
        }
    }
}

pub(super) fn curve_on_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.curve_on_scan";
    let picks: Vec<Vec3> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`points` must list the points clicked on the scan, [[x, y, z], …]"))?
        .iter()
        .map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, "a point must be [x, y, z] of numbers")))
        .collect::<Result<_>>()?;
    if picks.len() > curve::MAX_FIT {
        return Err(bad(cmd, format!("at most {} points", curve::MAX_FIT)));
    }
    let closed = crate::params::bool_(p, "closed").unwrap_or(false);
    let body = crate::params::str_(p, "body").map(str::to_owned);
    let tolerance = match crate::params::expr(p, "tolerance") {
        Some(_) => length(s, p, "tolerance", 0.0, cmd)?,
        None => default_tolerance(s, body.as_deref())?,
    };
    let (name, laid, fitted) = preview(s, body.as_deref(), &picks, closed, tolerance)?;
    let on = projector(s, Some(&name), cmd)?.1;
    let (sketch, opened) = open_sketch(s, p, cmd)?;
    let points: Vec<[f64; 3]> = fitted.fit.iter().map(|q| [q.x, q.y, q.z]).collect();
    let drawn = run(s, "sketch.spline3d", json!({"sketch": sketch, "points": points}));
    if opened {
        run(s, "sketch.finish", json!({}))?;
    }
    let drawn = drawn?;
    // How far the kept curve is from the scan itself, along its own samples.
    let off = solidcraft_geom::catmull_rom::samples(&fitted.fit).iter().filter_map(|q| on.nearest(*q).map(|(_, d)| d)).fold(0.0, f64::max);
    let length: f64 = laid.path.windows(2).map(|w| w[0].dist(w[1])).sum();
    let reply = ScanCurveResult {
        body: name,
        curve: drawn.get("curves").and_then(|c| c.get(0)).and_then(Value::as_str).unwrap_or_default().to_owned(),
        sketch: json!(sketch),
        closed: laid.closed,
        points: laid.picks.len(),
        fit_points: fitted.fit.len(),
        length,
        max: fitted.max,
        off_scan: off,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The most points of the ways a measured line's stretches take that a reply gives.
const MOST_PATH_POINTS: usize = 200_000;

/// Measure a line on the scan: its length along the surface, and how that compares with the
/// straight lines between its dots. Changes nothing.
pub(super) fn measure_line(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.measure_line";
    let point = |v: &Value| crate::params::vec3(v).filter(|q| q.is_finite()).ok_or_else(|| bad(cmd, "a point must be [x, y, z] of numbers"));
    let dots: Vec<Vec3> = p
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`points` must list the dots on the scan, [[x, y, z], …]"))?
        .iter()
        .map(point)
        .collect::<Result<_>>()?;
    let view = match p.get("view").filter(|v| !v.is_null()) {
        Some(v) => Some(point(v)?).filter(|v| v.len() > 0.0),
        None => None,
    };
    let with_path = crate::params::bool_(p, "path").unwrap_or(false);
    let body = crate::params::str_(p, "body").map(str::to_owned);
    let (name, on) = projector(s, body.as_deref(), cmd)?;
    let mesh = super::shown_mesh(s, &name).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let scan = solidcraft_scan::Scan::new(&mesh.positions, &mesh.triangles);
    let line = solidcraft_scan::surface_line::measure(&scan, &on, &dots, view).map_err(|e| bad(cmd, e.to_string()))?;
    let xyz = |q: Vec3| [q.x, q.y, q.z];
    let mut budget = MOST_PATH_POINTS;
    let stretches = line
        .stretches
        .iter()
        .map(|st| {
            let path = (with_path && st.path.len() <= budget).then(|| {
                budget -= st.path.len();
                st.path.iter().map(|q| xyz(*q)).collect()
            });
            ScanLineStretch { length: st.length(), followed: st.followed, path }
        })
        .collect();
    let reply = ScanMeasureLineResult {
        body: name,
        dots: line.dots.iter().map(|q| xyz(*q)).collect(),
        length: line.along,
        straight: line.straight,
        under_read: line.along - line.straight,
        worst_gap: line.worst_gap,
        rms_gap: line.rms_gap,
        ends: line.ends,
        delta: xyz(line.delta),
        stretches,
        not_followed: line.not_followed,
        round: line.round.map(|r| ScanLineRound { radius: r.radius, centre: xyz(r.centre), turned: r.turned, within: r.within }),
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
