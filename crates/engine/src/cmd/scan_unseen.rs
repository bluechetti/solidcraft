//! Surface the scanner never saw on a scan (`solidcraft_scan::unseen`), worked out from where the
//! scan came from:
//!
//! - a scan its Scan Clean steps changed: against the scan as it was before them (the steps fill
//!   holes, take parts off and fill them, remesh); a Move after the steps, or the component it is
//!   placed in, carries that scan along: the move is read off the steps' last mesh and the scan as
//!   it shows, which have the same triangles;
//! - a mirror fill (Complete by Mirroring): all of it;
//! - a surface made from points (Mesh from Points): against the points of its cloud;
//! - a scan nothing has changed: none of it.
//!
//! Each reading is kept for the scan as it shows, so fits and the deviation map ask again for free.

use std::sync::{Arc, Mutex, Weak};

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_scan::Scan;
use solidcraft_scan::unseen::{self, Unseen};

use crate::params::bad;
use crate::schema::scan::{ScanUnseenPatch, ScanUnseenResult};
use crate::{EngineError, Result, Session};

/// Where a scan's surface came from, as far as what it saw goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// As the scanner saw it: nothing changed it.
    AsScanned,
    /// Scan Clean steps changed it: read against the scan before them.
    Cleaned,
    /// Made of the scan's reflection: all of it unseen.
    MirrorFill,
    /// Made from a point cloud: read against its points.
    MeshFromPoints,
    /// Changed since its steps in a way that is not a move (a later feature reshaped it): not read.
    Changed,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::AsScanned => "as_scanned",
            Source::Cleaned => "cleaned",
            Source::MirrorFill => "mirror_fill",
            Source::MeshFromPoints => "mesh_from_points",
            Source::Changed => "changed",
        }
    }
}

/// The last reading: the scan's mesh as it shows (held weakly) and what was read.
type Kept = (Weak<solidcraft_geom::Mesh>, Source, Option<Arc<Unseen>>);

static LAST: Mutex<Option<Kept>> = Mutex::new(None);

/// The label a Mesh Import that made a body was given, when it was one.
fn import_label(s: &Session, body: &str) -> Option<String> {
    let made_by = s.model.state().body(body)?.feature;
    match &s.doc.feature(made_by)?.kind {
        solidcraft_doc::FeatureKind::MeshImport { file, .. } => Some(file.clone()),
        _ => None,
    }
}

/// The surface of scan `body` its scanner never saw, triangle by triangle of the scan as it shows,
/// and where the scan came from. `None` for a scan nothing changed or one that cannot be read.
pub fn unseen_of(s: &Session, body: &str) -> Result<(Source, Option<Arc<Unseen>>)> {
    let cmd = "scan.unseen";
    let shown = super::shown_mesh(s, body).ok_or_else(|| bad(cmd, format!("`{body}` is not a scan (a mesh body)")))?;
    if let Ok(last) = LAST.lock()
        && let Some((mesh, source, found)) = last.as_ref()
        && mesh.upgrade().is_some_and(|m| Arc::ptr_eq(&m, &shown))
    {
        return Ok((*source, found.clone()));
    }
    let (source, found) = read(s, body, &shown);
    let found = found.map(Arc::new);
    if let Ok(mut last) = LAST.lock() {
        *last = Some((Arc::downgrade(&shown), source, found.clone()));
    }
    Ok((source, found))
}

fn read(s: &Session, body: &str, shown: &solidcraft_geom::Mesh) -> (Source, Option<Unseen>) {
    let now = Scan::new(&shown.positions, &shown.triangles);
    let label = import_label(s, body).unwrap_or_default();
    if label.ends_with("(mirror fill)") {
        return (Source::MirrorFill, Some(unseen::all(&now)));
    }
    if let Some(cloud) = label.strip_suffix(" (mesh from points)") {
        let Some(cloud) = s.doc.point_clouds.iter().find(|c| c.name == cloud) else { return (Source::Changed, None) };
        let points: Vec<Vec3> = (0..cloud.len()).filter_map(|i| cloud.world_point(i)).collect();
        let reach = unseen::REACH_EDGES * solidcraft_scan::mirror_fill::typical_edge(&now);
        return (Source::MeshFromPoints, Some(unseen::against_points(&now, &points, reach)));
    }
    let state = s.model.state();
    let Some((base, latest)) = state.scan_before_cleaning(body) else { return (Source::AsScanned, None) };
    let (Some((base_positions, base_triangles)), Some(latest)) = (base.triangle_mesh(), latest) else { return (Source::Changed, None) };
    // The steps' last mesh and the scan as it shows have the same triangles: the move between them
    // (a later Move, the component's placing) carries the scan before the steps along.
    if latest.triangles != shown.triangles || latest.positions.len() != shown.positions.len() {
        return (Source::Changed, None);
    }
    let count = latest.positions.len();
    let step = (count / 2000).max(1);
    let picks: Vec<usize> = (0..count).step_by(step).collect();
    let from: Vec<Vec3> = picks.iter().map(|&i| latest.positions[i]).collect();
    let to: Vec<Vec3> = picks.iter().map(|&i| shown.positions[i]).collect();
    let moved = match solidcraft_scan::align::by_pairs(&from, &to) {
        Ok(m) if m.rms <= 1e-6 * (1.0 + now.extent()) => m.moved,
        // Too few points to tell a move by (a scan of a triangle or two): read as it lies.
        Err(_) if count < 3 => solidcraft_scan::place::Rigid::shift(Vec3::ZERO),
        _ => return (Source::Changed, None),
    };
    let seen_positions: Vec<Vec3> = base_positions.iter().map(|p| moved.apply(*p)).collect();
    let seen = Scan::new(&seen_positions, base_triangles);
    (Source::Cleaned, Some(unseen::against_scan(&now, &seen, None)))
}

/// A warning for a reading that leans on unseen surface: `share` of its area.
pub fn warning(share: f64, what: &str) -> Option<String> {
    (share >= 0.01).then(|| {
        format!(
            "{:.0} % of the area {what} is surface the scanner never saw (filled, mirrored or made from sparse points): check it against the part, or choose scanned surface",
            100.0 * share
        )
    })
}

/// `scan.unseen`: what of a scan the scanner never saw. Changes nothing.
pub(super) fn unseen_command(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.unseen";
    let body = super::mesh_body_name(s, crate::params::str_(p, "body"), cmd)?;
    let (source, found) = unseen_of(s, &body)?;
    let at = |q: Vec3| [q.x, q.y, q.z];
    let reply = match found {
        Some(u) => ScanUnseenResult {
            body,
            source: source.name().to_owned(),
            triangles: u.flags.iter().filter(|f| **f).count(),
            area: u.area,
            share: u.share(),
            patches: u.patches.iter().map(|q| ScanUnseenPatch { centre: at(q.centre), area: q.area, triangles: q.triangles }).collect(),
            param_warnings: Vec::new(),
        },
        None => ScanUnseenResult {
            body,
            source: source.name().to_owned(),
            triangles: 0,
            area: 0.0,
            share: 0.0,
            patches: Vec::new(),
            param_warnings: Vec::new(),
        },
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
