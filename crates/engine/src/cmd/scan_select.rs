//! Selections of a scan's triangles (#8615), after SurfaceCraft's Select tool, Curve from
//! Selection Edge and Select Inside Curve: `scan.select` selects by a brush, a polygon round what
//! is seen, a smooth region, a shape Find All found, a kept selection or by number, and grows,
//! shrinks or inverts what is selected; `scan.select_inside_curve` selects inside a closed curve
//! drawn on the scan; `scan.keep_selection` and `scan.delete_selection` keep selections in the
//! design; `scan.curve_from_selection` lays the selection's outline on the scan as closed 3D
//! sketch splines.
//!
//! What is selected is a view of the scan, as Find All's colouring is: held by the session, not
//! part of the design and not an undo step. It follows the scan while its triangles are the same
//! ones (a Move or Denoise keeps it) and is dropped when they are not (Remesh, Reduce). The scan
//! tools take it only while its scan is in view ([`selected`]): one hidden since gives way, as
//! the scan tools' own choice of scan does. Each selection made has its own [`stamp`], so a
//! dialog's preview knows to look again when it changes (it is not in the design's revision).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_scan::Scan;
use solidcraft_scan::curve::{self, Laid};
use solidcraft_scan::selection::{Topology, View};

use super::{CommandSpec, length, mesh_name};
use crate::params::{bad, bool_, str_, string_list_or_default};
use crate::schema::scan_select::{
    ScanCurveFromSelectionParams, ScanCurveFromSelectionResult, ScanDeleteSelectionParams, ScanDeleteSelectionResult, ScanKeepSelectionParams,
    ScanKeepSelectionResult, ScanSelectInsideParams, ScanSelectParams, ScanSelectResult, ScanUnexplainedPiece,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const SELECT_CONTRACT: TypedContract<ScanSelectParams> = TypedContract::new();
const INSIDE_CONTRACT: TypedContract<ScanSelectInsideParams> = TypedContract::new();
const KEEP_CONTRACT: TypedContract<ScanKeepSelectionParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<ScanDeleteSelectionParams> = TypedContract::new();
const EDGE_CONTRACT: TypedContract<ScanCurveFromSelectionParams> = TypedContract::new();

/// The most places a brush or a polygon takes, and rings a grow or shrink.
const MOST_PLACES: usize = 10_000;
const MOST_RINGS: u64 = 1000;

/// How sharply a smooth region may bend between neighbouring triangles unless told, in degrees.
pub const REGION_ANGLE: f64 = 30.0;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.select", "Select on Scan", crate::checked!("scan.select", select))
        .at("SCAN", "CLEAN")
        .icon("select")
        .contract(&SELECT_CONTRACT)
        .output(output_schema_for::<ScanSelectResult>)
        .params("body?: the scan (default: the one selected on, else the selected or only one); mode?: replace|add|remove (default replace); and one of: brush: {points: [[x,y,z], …], radius: length} (the triangles within the radius of each place, reached across the scan from the one under it); polygon: {points: [[x,y,z], …] (its corners as the view draws them), direction: [x,y,z] (the way the view looks), eye?: [x,y,z] (a perspective view's), through?: bool (default false: only what is seen)}; region: {point, angle?: degrees (default 30), until?: facing|crease} (facing, the default: grown while the scan faces within the angle of the way it faces there, a face up to where its rounds turn away; crease: grown while it turns less than the angle from one triangle to the next, up to its creases, a whole hole wall or a fillet with the faces it runs into); shape: n | {point, like?: bool} (a shape Find All found; with like, every shape like it: cylinders and spheres of its radius, cones of its half angle, tori of its radii, planes facing its way); saved: name (a kept selection); triangles: [n, …]; all: true; none: true; invert: true; grow: rings; shrink: rings; unexplained: solid | {body: solid, piece?: n (default 0, the largest), grow?: rings} (the part of the scan the solid built from it does not explain yet: its triangles farther from the solid than five times the scan's tolerance, at least 0.5 mm, in pieces of 50 triangles or more; one piece selected, nothing when the solid explains the whole scan; read it with scan.reconstruct_selection to build what is missing). A view of the scan, not an undo step → body, triangles, area, of, changed, pieces? [{piece, triangles, area, side: missing|extra, deepest, point}], limit?"),
    CommandSpec::new("scan.select_inside_curve", "Select Inside Curve", crate::checked!("scan.select_inside_curve", select_inside))
        .icon("boundary_fill")
        .contract(&INSIDE_CONTRACT)
        .output(output_schema_for::<ScanSelectResult>)
        .params("body?; sketch?: the 3D sketch (default the last `Scan Curves`); curves?: [ids end to end] (default its last curve), closed; mode?: replace|add|remove (default replace): the triangles inside the curve drawn on the scan, reached across the scan without crossing it, so the far side of a part is not taken. A view, not an undo step → body, triangles, area, of, changed"),
    CommandSpec::new("scan.keep_selection", "Keep Selection", crate::checked!("scan.keep_selection", keep))
        .icon("checkpoint")
        .contract(&KEEP_CONTRACT)
        .output(output_schema_for::<ScanKeepSelectionResult>)
        .params("name?: (default `Selection n`, the first number free; one of that name is replaced): what is selected on the scan kept in the design, opened again with scan.select {saved: name}; one undo step → name, body, triangles, replaced"),
    CommandSpec::new("scan.delete_selection", "Delete Kept Selection", crate::checked!("scan.delete_selection", delete))
        .icon("delete")
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<ScanDeleteSelectionResult>)
        .params("name: a kept selection, taken out of the design; one undo step → name, left"),
    CommandSpec::new("scan.curve_from_selection", "Curve from Selection Edge", crate::checked!("scan.curve_from_selection", curve_from_selection))
        .at("SCAN", "SKETCH")
        .icon("boundary")
        .contract(&EDGE_CONTRACT)
        .output(output_schema_for::<ScanCurveFromSelectionResult>)
        .params("every?: bool (default false: the longest outline alone; true: each hole's too, holes of fewer than six triangle sides left out); smooth?: length smoothed over (default twice the scan's typical triangle side; the shrinking taken back); tolerance?: length (default the scan's typical triangle side, as Curve on Scan's); sketch?: the 3D sketch (default the one being edited, else `Scan Curves`): the outline of what is selected on the scan, smoothed, laid on the scan and kept as closed 3D sketch splines; one undo step → body, curves, sketch, fit_points, max, skipped"),
];

/// What is selected on a scan.
#[derive(Debug)]
pub struct ScanSelected {
    pub scan: String,
    /// One flag a triangle, how many are set and their area.
    pub selected: Arc<Vec<bool>>,
    pub count: usize,
    pub area: f64,
    /// This selection's own number, never given to another while the program runs.
    pub stamp: u64,
    /// The scan's triangles' fingerprint, and the storage last found holding them (asked every
    /// frame at no cost; another storage is checked by the fingerprint, then remembered).
    key: u64,
    held: Mutex<[usize; 2]>,
}

/// The next selection's [`ScanSelected::stamp`] (0 stands for none).
static NEXT_STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl ScanSelected {
    /// Whether it is of `scan`'s triangles as they are now.
    pub fn current(&self, s: &Session) -> bool {
        let state = s.model.state();
        let Some((_, triangles)) = state.body(&self.scan).and_then(|b| b.body.triangle_mesh()) else { return false };
        let at = [triangles.as_ptr() as usize, triangles.len()];
        let Ok(mut held) = self.held.lock() else { return false };
        if *held == at {
            return true;
        }
        if triangles.len() == self.selected.len() && triangles_key(triangles) == self.key {
            *held = at;
            return true;
        }
        false
    }
}

/// A fingerprint of a mesh's triangles: their count and corners, folded.
fn triangles_key(triangles: &[[u32; 3]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ triangles.len() as u64;
    for t in triangles {
        h ^= u64::from(t[0]) | (u64::from(t[1]) << 21) | (u64::from(t[2]) << 42);
        h = h.wrapping_mul(0x0000_0100_0000_01b3).rotate_left(5);
    }
    h
}

/// What is selected now, if it is of a scan as it is and that scan is in view: what the scan
/// tools take and the viewport draws.
pub fn selected(s: &Session) -> Option<Arc<ScanSelected>> {
    held(s).filter(|k| s.body_visible(&k.scan))
}

/// What is selected now, if it is of a scan as it is, shown or hidden: what the selection
/// commands add to, take from and keep.
fn held(s: &Session) -> Option<Arc<ScanSelected>> {
    s.scan_selection.clone().filter(|k| k.current(s))
}

/// The [`ScanSelected::stamp`] of what is [`selected`] now, 0 with nothing: a dialog's preview
/// keyed on it looks again whenever the selection changes, is hidden or goes.
pub fn stamp(s: &Session) -> u64 {
    selected(s).map_or(0, |k| k.stamp)
}

/// The last scan's topology, and the storage and fingerprint of the mesh it was built from.
#[allow(clippy::type_complexity)]
static TOPOLOGY: Mutex<Option<([usize; 4], u64, Arc<Topology>)>> = Mutex::new(None);

fn mesh_key(positions: &[Vec3], triangles: &[[u32; 3]]) -> u64 {
    let mut h = triangles_key(triangles) ^ (positions.len() as u64).rotate_left(17);
    for p in positions {
        for v in [p.x, p.y, p.z] {
            h ^= v.to_bits();
            h = h.wrapping_mul(0x0000_0100_0000_01b3).rotate_left(7);
        }
    }
    h
}

/// The last scan's triangle tree, and the storage of the points and triangles it was built over:
/// Select's topology and Curve on Scan's projector share it (a scan's tree is as large as the
/// scan, and each built its own).
#[allow(clippy::type_complexity)]
static TREE: Mutex<Option<([usize; 4], Arc<solidcraft_geom::compare::TriangleTree>)>> = Mutex::new(None);

/// The triangle tree over a scan's `mesh` (as its body keeps it), built once while the mesh is
/// the same, reading its corners from the mesh rather than a copy of its own.
pub fn scan_tree(mesh: &Arc<solidcraft_geom::Mesh>) -> Arc<solidcraft_geom::compare::TriangleTree> {
    let (positions, triangles) = (&mesh.positions, &mesh.triangles);
    let held = [positions.as_ptr() as usize, positions.len(), triangles.as_ptr() as usize, triangles.len()];
    if let Ok(cache) = TREE.lock()
        && let Some((_, tree)) = cache.as_ref().filter(|(h, _)| *h == held)
    {
        return tree.clone();
    }
    let built = Arc::new(solidcraft_geom::compare::TriangleTree::over(mesh.clone()));
    if let Ok(mut cache) = TREE.lock() {
        *cache = Some((held, built.clone()));
    }
    built
}

pub use solidcraft_scan::selection::{Snap, SnapKind};

/// The scans a click's snap may build the topology of on the spot (triangles): larger ones snap
/// only once a selection tool has built it, so a hover never stalls the window.
pub const SNAP_BUILD_LIMIT: usize = 300_000;

/// The topology of the scan `body` for a click to snap to: the one already built, or built now
/// when the scan is small enough ([`SNAP_BUILD_LIMIT`]); `None` for a larger scan nothing has
/// built it for.
pub fn topology_for_snap(s: &Session, body: &str) -> Option<Arc<Topology>> {
    let mesh = super::shown_mesh(s, body)?;
    let held = [mesh.positions.as_ptr() as usize, mesh.positions.len(), mesh.triangles.as_ptr() as usize, mesh.triangles.len()];
    let ready = TOPOLOGY.lock().ok().and_then(|c| c.as_ref().filter(|(h, ..)| *h == held).map(|(_, _, t)| t.clone()));
    ready.or_else(|| {
        let small = mesh.triangles.len() <= SNAP_BUILD_LIMIT;
        small.then(|| topology(s, body, "scan.snap").ok()).flatten()
    })
}

/// The topology of the scan `body`, built once while the scan is as it was.
pub fn topology(s: &Session, body: &str, cmd: &str) -> Result<Arc<Topology>> {
    // Where the scan shows (see `shown_mesh`): what is grown, painted and cut is where it is drawn.
    let mesh = super::shown_mesh(s, body).ok_or_else(|| bad(cmd, format!("`{body}` is not a scan")))?;
    let (positions, triangles) = (mesh.positions.as_slice(), mesh.triangles.as_slice());
    let held = [positions.as_ptr() as usize, positions.len(), triangles.as_ptr() as usize, triangles.len()];
    let mut cache = TOPOLOGY.lock().map_err(|_| EngineError::Other("the scan's topology is unavailable".into()))?;
    if let Some((_, _, t)) = cache.as_ref().filter(|(h, ..)| *h == held) {
        return Ok(t.clone());
    }
    let key = mesh_key(positions, triangles);
    if let Some((_, _, t)) = cache.as_ref().filter(|(_, k, _)| *k == key) {
        let t = t.clone();
        *cache = Some((held, key, t.clone()));
        return Ok(t);
    }
    let tree = scan_tree(&mesh);
    let built = Arc::new(Topology::over(mesh, tree));
    *cache = Some((held, key, built.clone()));
    Ok(built)
}

/// The scan a selection command works on: `body`, else the one already selected on, else the
/// selected or only mesh body.
fn scan_name(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    if str_(p, "body").is_none_or(|b| b.trim().is_empty()) {
        // A kept selection opened again is of its own scan, whichever is selected on now.
        if let Some(kept) = str_(p, "saved").and_then(|name| s.doc.scan_selections.iter().find(|k| k.name == name)) {
            return Ok(kept.body.clone());
        }
        if let Some(k) = selected(s) {
            return Ok(k.scan.clone());
        }
    }
    mesh_name(s, p, cmd)
}

/// How the triangles found change what is selected.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Replace,
    Add,
    Remove,
}

fn mode(p: &Value, cmd: &str) -> Result<Mode> {
    match str_(p, "mode") {
        None | Some("replace") => Ok(Mode::Replace),
        Some("add") => Ok(Mode::Add),
        Some("remove") => Ok(Mode::Remove),
        Some(m) => Err(bad(cmd, format!("unknown mode `{m}` (replace, add or remove)"))),
    }
}

fn points(v: &Value, cmd: &str, what: &str, least: usize) -> Result<Vec<Vec3>> {
    let list = v.as_array().ok_or_else(|| bad(cmd, format!("`{what}` must list places [[x, y, z], …]")))?;
    if list.len() < least || list.len() > MOST_PLACES {
        return Err(bad(cmd, format!("`{what}` takes {least} to {MOST_PLACES} places")));
    }
    list.iter().map(|q| crate::params::vec3(q).ok_or_else(|| bad(cmd, format!("a place in `{what}` must be [x, y, z] of numbers")))).collect()
}

/// Set what is selected on `scan` to `flags`, saying how it came out.
fn set(s: &mut Session, scan: &str, flags: Vec<bool>, cmd: &str) -> Result<Value> {
    let state = s.model.state();
    let (_, triangles) = state.body(scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
    let key = triangles_key(triangles);
    let held = [triangles.as_ptr() as usize, triangles.len()];
    let of = triangles.len();
    drop(state);
    let topology = topology(s, scan, cmd)?;
    let count = flags.iter().filter(|f| **f).count();
    let area = flags.iter().enumerate().filter(|(_, f)| **f).map(|(t, _)| topology.area(t)).sum();
    let before = self::held(s).filter(|k| k.scan == scan);
    let changed = before.as_ref().is_none_or(|k| *k.selected != flags);
    // The same triangles selected again keep their stamp, so an open preview is not redone.
    let stamp = match before.as_ref().filter(|_| !changed) {
        Some(k) => k.stamp,
        None => NEXT_STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    };
    s.scan_selection = (count > 0)
        .then(|| Arc::new(ScanSelected { scan: scan.to_owned(), selected: Arc::new(flags), count, area, stamp, key, held: Mutex::new(held) }));
    let changed = changed && !(before.is_none() && count == 0);
    let reply =
        ScanSelectResult { body: scan.to_owned(), triangles: count, area, of, changed, pieces: None, limit: None, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// What is selected on `scan` now (none when on another scan or a changed one), with `found`
/// put to it as `mode` says.
fn combined(s: &Session, scan: &str, count: usize, found: &[bool], mode: Mode) -> Vec<bool> {
    let base = held(s).filter(|k| k.scan == scan && k.selected.len() == count).map(|k| (*k.selected).clone());
    match (mode, base) {
        (Mode::Replace, _) | (Mode::Add, None) => found.to_vec(),
        (Mode::Remove, None) => vec![false; count],
        (Mode::Add, Some(mut b)) => {
            for (x, f) in b.iter_mut().zip(found) {
                *x |= *f;
            }
            b
        }
        (Mode::Remove, Some(mut b)) => {
            for (x, f) in b.iter_mut().zip(found) {
                *x &= !*f;
            }
            b
        }
    }
}

pub(super) fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.select";
    let ways = ["brush", "polygon", "region", "shape", "saved", "triangles", "all", "none", "invert", "grow", "shrink", "unexplained"];
    let given: Vec<&str> = ways.iter().copied().filter(|k| p.get(*k).is_some_and(|v| !v.is_null() && *v != json!(false))).collect();
    let [way] = given[..] else {
        return Err(bad(cmd, format!("give one way to select: {}", ways.join(", "))));
    };
    let scan = scan_name(s, p, cmd)?;
    let mode = mode(p, cmd)?;
    let topology = topology(s, &scan, cmd)?;
    let count = topology.triangle_count();
    let current =
        || held(s).filter(|k| k.scan == scan && k.selected.len() == count).map(|k| (*k.selected).clone()).unwrap_or_else(|| vec![false; count]);
    let v = &p[way];
    if way == "unexplained" {
        let (found, pieces, limit) = unexplained(s, &scan, v, &topology, cmd)?;
        let flags = combined(s, &scan, count, &found, mode);
        let mut reply = set(s, &scan, flags, cmd)?;
        if let Some(obj) = reply.as_object_mut() {
            obj.insert("pieces".into(), serde_json::to_value(pieces).map_err(|e| EngineError::Other(e.to_string()))?);
            obj.insert("limit".into(), json!(limit));
        }
        return Ok(reply);
    }
    let flags = match way {
        "all" => vec![true; count],
        "none" => vec![false; count],
        "invert" => current().into_iter().map(|f| !f).collect(),
        "grow" | "shrink" => {
            let rings = v
                .as_u64()
                .filter(|n| (1..=MOST_RINGS).contains(n))
                .ok_or_else(|| bad(cmd, format!("`{way}` is a number of rings, 1 to {MOST_RINGS}")))?;
            let mut flags = current();
            topology.rings(&mut flags, if way == "grow" { rings as i32 } else { -(rings as i32) });
            flags
        }
        _ => {
            let found = match way {
                "brush" => {
                    let places = points(&v["points"], cmd, "brush.points", 1)?;
                    let radius = length(s, v, "radius", 0.0, cmd)?;
                    if !(radius > 0.0) {
                        return Err(bad(cmd, "`brush.radius` must be a length above 0"));
                    }
                    let mut found = vec![false; count];
                    for place in places {
                        for (f, b) in found.iter_mut().zip(topology.brush(place, radius)) {
                            *f |= b;
                        }
                    }
                    found
                }
                "polygon" => {
                    let ring = points(&v["points"], cmd, "polygon.points", 3)?;
                    let direction = crate::params::vec3(&v["direction"]).ok_or_else(|| bad(cmd, "`polygon.direction` must be [x, y, z]"))?;
                    let eye = match v.get("eye").filter(|e| !e.is_null()) {
                        Some(e) => Some(crate::params::vec3(e).ok_or_else(|| bad(cmd, "`polygon.eye` must be [x, y, z]"))?),
                        None => None,
                    };
                    if solidcraft_geom::Vec3Geom::normalized(direction).is_none() {
                        return Err(bad(cmd, "`polygon.direction` must not be zero"));
                    }
                    let through = bool_(v, "through").unwrap_or(false);
                    let shown = super::shown_mesh(s, &scan).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
                    topology.seen_inside(&Scan::new(&shown.positions, &shown.triangles), &ring, &View { direction, eye }, through)
                }
                "region" => {
                    let point = crate::params::vec3(&v["point"]).ok_or_else(|| bad(cmd, "`region.point` must be [x, y, z]"))?;
                    let angle = v.get("angle").and_then(Value::as_f64).unwrap_or(REGION_ANGLE);
                    if !(1.0..=90.0).contains(&angle) {
                        return Err(bad(cmd, "`region.angle` is 1 to 90 degrees"));
                    }
                    let (seed, _) = topology.nearest(point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
                    match v.get("until").and_then(Value::as_str) {
                        None | Some("facing") => topology.region(seed, angle.to_radians()),
                        Some("crease") => topology.smooth_region(seed, angle.to_radians()),
                        Some(_) => return Err(bad(cmd, "`region.until` is facing or crease")),
                    }
                }
                "shape" => shape_flags(s, &scan, v, &topology, cmd)?,
                "saved" => {
                    let name = v.as_str().ok_or_else(|| bad(cmd, "`saved` is the name of a kept selection"))?;
                    let kept =
                        s.doc.scan_selections.iter().find(|k| k.name == name).ok_or_else(|| bad(cmd, format!("no kept selection `{name}`")))?;
                    if kept.body != scan {
                        return Err(bad(cmd, format!("`{name}` is of `{}`, not `{scan}`", kept.body)));
                    }
                    if kept.triangles as usize != count {
                        return Err(bad(
                            cmd,
                            format!("`{scan}` has other triangles than when `{name}` was kept: its triangles are made again (Remesh, Reduce)"),
                        ));
                    }
                    kept.flags()
                }
                _ => {
                    let list = v.as_array().ok_or_else(|| bad(cmd, "`triangles` must list triangle numbers"))?;
                    let mut found = vec![false; count];
                    for t in list {
                        let t = t
                            .as_u64()
                            .filter(|t| (*t as usize) < count)
                            .ok_or_else(|| bad(cmd, format!("the scan's triangles are numbered 0 to {}", count.saturating_sub(1))))?;
                        found[t as usize] = true;
                    }
                    found
                }
            };
            combined(s, &scan, count, &found, mode)
        }
    };
    set(s, &scan, flags, cmd)
}

/// Pieces smaller than this many triangles are left out of what a solid leaves unexplained: a
/// sharp edge's rounding or a stray speck, too little for Reconstruct to read.
const LEAST_PIECE: usize = 50;

/// The triangles of `scan` the solid `v` names does not explain (their middles farther from it
/// than [`super::step_fit::limit`]), in pieces largest first; the piece asked for (the largest
/// by default), grown by the rings asked for, is what is selected. Nothing is selected when the
/// solid explains the whole scan.
fn unexplained(s: &Session, scan: &str, v: &Value, topology: &Topology, cmd: &str) -> Result<(Vec<bool>, Vec<ScanUnexplainedPiece>, f64)> {
    let (solid, piece, grow) = match v {
        Value::String(b) => (b.clone(), 0, 0),
        Value::Object(_) => {
            let solid =
                str_(v, "body").filter(|b| !b.trim().is_empty()).ok_or_else(|| bad(cmd, "`unexplained.body` is the solid built from the scan"))?;
            let number = |key: &str, most: u64| -> Result<u64> {
                match v.get(key).filter(|n| !n.is_null()) {
                    None => Ok(0),
                    Some(n) => n.as_u64().filter(|n| *n <= most).ok_or_else(|| bad(cmd, format!("`unexplained.{key}` is a number, 0 to {most}"))),
                }
            };
            (solid.to_owned(), number("piece", u64::from(u32::MAX))? as usize, number("grow", MOST_RINGS)? as i32)
        }
        _ => return Err(bad(cmd, "`unexplained` is the solid's name or {body, piece?, grow?}")),
    };
    if solid == scan {
        return Err(bad(cmd, "`unexplained` names the solid built from the scan, not the scan itself"));
    }
    let tolerance = super::tolerance::of(s, scan);
    let limit = super::step_fit::limit(tolerance);
    let surface = {
        let state = s.model.state();
        let model = state.body(&solid).ok_or_else(|| bad(cmd, format!("there is no body `{solid}`")))?;
        if model.body.is_mesh() {
            return Err(bad(cmd, format!("`{solid}` is a mesh: give the solid built from the scan (Reconstruct builds `{scan} Reconstructed`)")));
        }
        model
            .body
            .mesh(&solidcraft_kernel::MeshTolerance::chord(tolerance / 4.0))
            .map_err(|e| bad(cmd, format!("`{solid}` cannot be measured against the scan: {e}")))?
    };
    let count = topology.triangle_count();
    let middles: Vec<Vec3> = (0..count).map(|t| topology.middle(t)).collect();
    let distances = solidcraft_scan::deviation::measure(&middles, &surface, tolerance, f64::INFINITY).values;
    let off: Vec<bool> = distances.iter().map(|d| !(f64::from(d.abs()) <= limit)).collect();
    let pieces: Vec<Vec<usize>> = topology.pieces(&off).into_iter().filter(|p| p.len() >= LEAST_PIECE).collect();
    let described = pieces
        .iter()
        .enumerate()
        .map(|(k, piece)| {
            let area: f64 = piece.iter().map(|&t| topology.area(t)).sum();
            let weighed = piece.iter().fold(Vec3::ZERO, |m, &t| m + topology.middle(t) * topology.area(t));
            let centre = if area > 0.0 { weighed * (1.0 / area) } else { topology.middle(piece[0]) };
            let near = piece.iter().copied().min_by(|&a, &b| {
                solidcraft_geom::Vec3Geom::len2(topology.middle(a) - centre).total_cmp(&solidcraft_geom::Vec3Geom::len2(topology.middle(b) - centre))
            });
            let point = topology.middle(near.unwrap_or(piece[0]));
            let signed: f64 = piece.iter().map(|&t| f64::from(distances[t]) * topology.area(t)).filter(|d| d.is_finite()).sum();
            let deepest = piece.iter().map(|&t| f64::from(distances[t].abs())).filter(|d| d.is_finite()).fold(0.0, f64::max);
            ScanUnexplainedPiece {
                piece: k,
                triangles: piece.len(),
                area,
                side: if signed >= 0.0 { "missing" } else { "extra" }.to_owned(),
                deepest,
                point: [point.x, point.y, point.z],
            }
        })
        .collect();
    let mut flags = vec![false; count];
    if !pieces.is_empty() {
        let chosen = pieces.get(piece).ok_or_else(|| bad(cmd, format!("`{solid}` leaves {} pieces unexplained, numbered from 0", pieces.len())))?;
        for &t in chosen {
            flags[t] = true;
        }
        if grow > 0 {
            topology.rings(&mut flags, grow);
        }
    }
    Ok((flags, described, limit))
}

/// The triangles of a shape Find All found on `scan`: by its number, or the one under `{point}`.
fn shape_flags(s: &Session, scan: &str, v: &Value, topology: &Topology, cmd: &str) -> Result<Vec<bool>> {
    let shapes = s
        .scan_shapes
        .clone()
        .filter(|k| k.scan == scan && k.current(s))
        .ok_or_else(|| bad(cmd, "run Find All on the scan first: its shapes are what `shape` names"))?;
    let region = match v {
        Value::Number(n) => {
            let k = n.as_u64().ok_or_else(|| bad(cmd, "`shape` is a shape's number"))? as usize;
            if shapes.outcome.segments.get(k).is_none_or(|seg| seg.shape.is_none()) {
                return Err(bad(cmd, format!("Find All found no shape {k}")));
            }
            k as u32
        }
        _ => {
            let point = crate::params::vec3(&v["point"]).ok_or_else(|| bad(cmd, "`shape` is a number or {point: [x, y, z]}"))?;
            let (t, _) = topology.nearest(point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
            *shapes.regions.get(t).ok_or_else(|| bad(cmd, "Find All's regions do not match the scan"))?
        }
    };
    // With `like`, every shape Find All found that is like that one (holes of its size, faces
    // facing its way: Shape::like).
    let like = v.get("like").and_then(Value::as_bool).unwrap_or(false);
    if !like {
        return Ok(shapes.regions.iter().map(|r| *r == region).collect());
    }
    let Some(first) = shapes.outcome.segments.get(region as usize).and_then(|seg| seg.shape) else {
        return Err(bad(cmd, "the scan there is freeform, no shape: there is nothing like it to find"));
    };
    let alike: Vec<bool> = shapes.outcome.segments.iter().map(|seg| seg.shape.is_some_and(|sh| first.like(&sh))).collect();
    Ok(shapes.regions.iter().map(|r| alike.get(*r as usize).copied().unwrap_or(false)).collect())
}

pub(super) fn select_inside(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.select_inside_curve";
    let scan = scan_name(s, p, cmd)?;
    let mode = mode(p, cmd)?;
    let (_, _, path) = super::section::along_path(s, p.get("sketch"), &string_list_or_default(p, "curves"), cmd)?;
    let topology = topology(s, &scan, cmd)?;
    let reach = super::curve::size(s, &scan) / 100.0 + 3.0 * topology.edge;
    let found = topology.inside_curve(&path, reach).map_err(|e| bad(cmd, e.to_string()))?;
    let flags = combined(s, &scan, topology.triangle_count(), &found, mode);
    set(s, &scan, flags, cmd)
}

pub(super) fn keep(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.keep_selection";
    let k = held(s).ok_or_else(|| bad(cmd, "nothing is selected on a scan: select some of it first"))?;
    let name = match str_(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.chars().count() > solidcraft_doc::scan_selection::MAX_NAME => return Err(bad(cmd, "the name is too long")),
        Some(n) => n.to_owned(),
        None => (1..).map(|n| format!("Selection {n}")).find(|n| !s.doc.scan_selections.iter().any(|k| k.name == *n)).unwrap_or_default(),
    };
    let kept = solidcraft_doc::scan_selection::ScanSelection::from_flags(&name, &k.scan, &k.selected);
    let replaced = s.doc.scan_selections.iter().any(|x| x.name == name);
    if !replaced && s.doc.scan_selections.len() >= solidcraft_doc::scan_selection::MAX_SELECTIONS {
        return Err(bad(cmd, "the design keeps as many selections as it can: delete one first"));
    }
    let triangles = kept.count();
    let doc = s.doc_mut();
    match doc.scan_selections.iter_mut().find(|x| x.name == name) {
        Some(old) => *old = kept,
        None => doc.scan_selections.push(kept),
    }
    let reply = ScanKeepSelectionResult { name, body: k.scan.clone(), triangles, replaced, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

pub(super) fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.delete_selection";
    let name = str_(p, "name").ok_or_else(|| bad(cmd, "`name` is the kept selection to delete"))?.to_owned();
    if !s.doc.scan_selections.iter().any(|k| k.name == name) {
        return Err(bad(cmd, format!("no kept selection `{name}`")));
    }
    s.doc_mut().scan_selections.retain(|k| k.name != name);
    let reply = ScanDeleteSelectionResult { name, left: s.doc.scan_selections.len(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The outlines Curve from Selection Edge would keep, for a dialog's preview: the scan, and each
/// smoothed outline laid on the scan with the fit points of its curve and how far it strays.
pub fn edge_preview(s: &Session, every: bool, smooth: Option<f64>, tolerance: f64) -> Result<(String, Vec<(Vec<Vec3>, curve::Fitted)>, usize)> {
    let cmd = "scan.curve_from_selection";
    let k = selected(s).ok_or_else(|| bad(cmd, "nothing is selected on a scan: select the region whose edge to follow"))?;
    let topology = topology(s, &k.scan, cmd)?;
    let width = smooth.unwrap_or(2.0 * topology.edge);
    let reach = super::curve::size(s, &k.scan) / 100.0 + 3.0 * topology.edge;
    let shown = super::shown_mesh(s, &k.scan).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let (lines, skipped) =
        topology.edge_lines(&Scan::new(&shown.positions, &shown.triangles), &k.selected, every, width, reach).map_err(|e| bad(cmd, e.to_string()))?;
    let fitted = lines
        .into_iter()
        .map(|path| {
            let n = path.len() - 1;
            let at = vec![0, n / 4, n / 2, 3 * n / 4, n];
            let laid = Laid { picks: Vec::new(), at, path, closed: true };
            let fit = curve::fit(&laid, tolerance);
            (laid.path, fit)
        })
        .collect();
    Ok((k.scan.clone(), fitted, skipped))
}

pub(super) fn curve_from_selection(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.curve_from_selection";
    let tolerance = match crate::params::expr(p, "tolerance") {
        Some(_) => length(s, p, "tolerance", 0.0, cmd)?,
        None => {
            let k = selected(s).ok_or_else(|| bad(cmd, "nothing is selected on a scan: select the region whose edge to follow"))?;
            super::curve::default_tolerance(s, Some(&k.scan))?
        }
    };
    let smooth = match crate::params::expr(p, "smooth") {
        Some(_) => Some(length(s, p, "smooth", 1.0, cmd)?),
        None => None,
    };
    let (scan, outlines, skipped) = edge_preview(s, bool_(p, "every").unwrap_or(false), smooth, tolerance)?;
    let (sketch, opened) = super::curve::open_sketch(s, p, cmd)?;
    let mut curves = Vec::with_capacity(outlines.len());
    let mut drawn: Result<()> = Ok(());
    for (_, fitted) in &outlines {
        let points: Vec<[f64; 3]> = fitted.fit.iter().map(|q| [q.x, q.y, q.z]).collect();
        match super::super::find_command("sketch.spline3d")
            .ok_or_else(|| EngineError::UnknownCommand("sketch.spline3d".into()))
            .and_then(|c| c.run_nested(s, &json!({"sketch": sketch, "points": points})))
        {
            Ok(made) => curves.push(made.get("curves").and_then(|c| c.get(0)).and_then(Value::as_str).unwrap_or_default().to_owned()),
            Err(e) => {
                drawn = Err(e);
                break;
            }
        }
    }
    if opened {
        super::super::find_command("sketch.finish").ok_or_else(|| EngineError::UnknownCommand("sketch.finish".into()))?.run_nested(s, &json!({}))?;
    }
    drawn?;
    let reply = ScanCurveFromSelectionResult {
        body: scan,
        curves,
        sketch: json!(sketch),
        fit_points: outlines.iter().map(|(_, f)| f.fit.len()).collect(),
        max: outlines.iter().map(|(_, f)| f.max).fold(0.0, f64::max),
        skipped,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
#[path = "scan_select_tests.rs"]
mod tests;
