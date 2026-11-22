//! Tools on a scan's mesh as a whole (#8615), after SurfaceCraft's: `scan.register` lays one scan
//! onto another, a second pass onto the first, and keeps the move as an ordinary Move of it;
//! `scan.mesh_from_points` makes a mesh body from a point cloud (screened Poisson); `scan.remesh`
//! makes a scan again as one even surface, fusing other passes into it; `scan.denoise` smooths a
//! scan's noise away, its sharp edges kept.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_doc::scan_clean::{CleanStep, Report};
use solidcraft_scan::register::{self, Settings};
use solidcraft_scan::{Scan, denoise, poisson};

use super::{CommandSpec, Mesh, mesh, move_by};
use crate::params::{bad, bool_, num, str_};
use crate::schema::scan_mesh::{
    ScanDenoiseParams, ScanDenoiseResult, ScanMeshFromPointsParams, ScanMeshFromPointsResult, ScanRegisterParams, ScanRegisterResult,
    ScanRemeshParams, ScanRemeshResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};
pub use solidcraft_scan::denoise::{MOST_PASSES, Stage, Study};

const REGISTER_CONTRACT: TypedContract<ScanRegisterParams> = TypedContract::new();
const FROM_POINTS_CONTRACT: TypedContract<ScanMeshFromPointsParams> = TypedContract::new();
const REMESH_CONTRACT: TypedContract<ScanRemeshParams> = TypedContract::new();
const DENOISE_CONTRACT: TypedContract<ScanDenoiseParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.mesh_from_points", "Mesh from Points", crate::checked!("scan.mesh_from_points", mesh_from_points))
        .at("SCAN", "CLEAN")
        .icon("dots")
        .contract(&FROM_POINTS_CONTRACT)
        .output(output_schema_for::<ScanMeshFromPointsResult>)
        .params("cloud?: point cloud id or name (default: the only one); depth?: 5–12 (the finest grid is 2^depth cells across; default: about the points' spacing, and no finer than twice the layer of points is thick, so passes merged apart make one surface); trim?: 0–1 (default 0.5: where the points are sparse the surface stops; 0 keeps it closed); name?: the body's name: a mesh body made from the points by screened Poisson reconstruction (their normals estimated and turned to agree where the cloud states none), one undo step → body, triangles, vertices, depth, trim, points, points_used, estimated_normals, cell, spacing, trimmed, rms, p95, max, thickness, warning? (the points lie in a layer thicker than they are apart: passes that do not lie on each other), feature"),
    CommandSpec::new("scan.denoise", "Denoise", crate::checked!("scan.denoise", denoise_scan))
        .at("SCAN", "CLEAN")
        .icon("c_smooth")
        .contract(&DENOISE_CONTRACT)
        .output(output_schema_for::<ScanDenoiseResult>)
        .params("body?: the scan (default: the selected or only one); strength?: passes 0.05–8 (default: the strength its noise and shrinkage suggest, refused when smoothing would not lower its error); area: point?: [x,y,z] with grow?: true|degrees or radius?: mm, saved?: kept selection, selected?: true (what is selected; a selection is never taken unasked), or area?: {triangles, runs} as the step keeps it (default: the whole scan; outside it nothing moves); then: the faces' normals filtered bilaterally (faces across a sharp edge barely mix) and the vertices moved square to them, the open rim kept put, kept as a Scan Clean step in the timeline (its settings only; in_place?: bool writes the scan back instead), one undo step → body, strength, suggested, noise, roughness, rms_moved, max_moved, feature"),
    CommandSpec::new("scan.remesh", "Remesh", crate::checked!("scan.remesh", remesh_scan))
        .at("SCAN", "CLEAN")
        .icon("grid")
        .contract(&REMESH_CONTRACT)
        .output(output_schema_for::<ScanRemeshResult>)
        .params("body?: the scan (default: the selected or only one); with?: [other passes, laid on it] fused in; cell?: length (default the scan's typical edge: the new mesh's edge); sharp?: bool (default false: each vertex where the planes of its cell meet, so sharp edges and corners stay sharp); offset?: length or expression (default 0: the surface made that far off the scan, positive the way its faces point, negative into it, following the design's parameters: a liner, a socket, a nest's clearance; the whole scan only); shell?: length or expression (a hollow part that thick: the surface at the offset and, inside it, one that much farther in, facing into the hollow, joined along the scan's open rims, closed and printable: a socket, a liner, a cover; the whole scan only); area: point?: [x,y,z] with grow?: true|degrees or radius?: mm, saved?: kept selection, selected?: true, or area?: {triangles, runs} (default the whole scan; within an area its triangles are made again as even triangles of about cell on the scan, the rest and the area's edge kept, sharp keeping its sharp edges; not with `with`): the scan made again as one even surface from the signed distances of the scans in a narrow band of a grid (overlaps averaged), kept as a Scan Clean step in the timeline (its settings only; in_place?: bool writes the scan back instead), one undo step → body, with, before, triangles, vertices, cell, sharp, new_rms, new_max, scan_rms, scan_p95, uncovered, feature"),
    CommandSpec::new("scan.register", "Align to Scan", crate::checked!("scan.register", register_scan))
    .at("SCAN", "ALIGN")
    .icon("align")
    .contract(&REGISTER_CONTRACT)
    .output(output_schema_for::<ScanRegisterResult>)
    .params("body?: the scan to move (default: the selected mesh body, else with two scans the one inserted last); to?: the scan to lay it on (default: the other, when there are two); search?: bool (default true: from the ways their principal axes and matched surface features lie on each other too, not only from where it lies); ignored?: share 0–0.6 (default 0.2: how much of it may have nothing under it): laid on the other by point-to-plane ICP both ways, trimmed and robust, one undo step kept as a Move → body, to, moved, rms_before, rms_after, matched, start, converged, rival?, feature"),
];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// The scans of the design, in the order they were made.
fn scans(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// The scan to move and the scan to lay it on.
fn pair(s: &Session, p: &Value, cmd: &str) -> Result<(String, String)> {
    let all = scans(s);
    let named = |k: &str| str_(p, k).filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let selected: Vec<String> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.clone()),
            _ => None,
        })
        .filter(|n| all.contains(n))
        .collect();
    let moving = match (named("body"), selected.first(), all.as_slice()) {
        (Some(body), _, _) => body,
        (None, Some(one), _) => one.clone(),
        (None, None, [only]) => only.clone(),
        (None, None, [_, second]) => second.clone(),
        (None, None, []) => return Err(bad(cmd, "the design has no scan: insert two passes with Insert Mesh")),
        (None, None, _) => return Err(bad(cmd, "select the scan to move onto another")),
    };
    let others: Vec<&String> = all.iter().filter(|n| **n != moving).collect();
    let to = match (named("to"), others.as_slice()) {
        (Some(to), _) => to,
        (None, [one]) => (*one).clone(),
        (None, []) => return Err(bad(cmd, "the design has one scan: insert the other pass to lay it on")),
        (None, _) => return Err(bad(cmd, "name the scan to lay it on with `to`")),
    };
    if to == moving {
        return Err(bad(cmd, "a scan cannot be laid onto itself"));
    }
    Ok((moving, to))
}

pub(super) fn register_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.register";
    let (moving, to) = pair(s, p, cmd)?;
    let ignored = num(p, "ignored").unwrap_or(0.2);
    if !(0.0..=0.6).contains(&ignored) {
        return Err(bad(cmd, "`ignored` is a share from 0 to 0.6"));
    }
    let settings = Settings { search: bool_(p, "search").unwrap_or(true), ignored };
    let mesh_of = |name: &str| mesh(s, &json!({ "body": name }), cmd);
    let (scan, other): (Mesh, Mesh) = (mesh_of(&moving)?, mesh_of(&to)?);
    let found = register::register(&scan.scan(), &other.scan(), &settings).map_err(|e| bad(cmd, e.to_string()))?;
    let feature = move_by(s, &scan, &found.moved)?;
    reply(ScanRegisterResult {
        body: moving,
        to,
        moved: feature.is_some(),
        rms_before: found.rms_before,
        rms_after: found.rms_after,
        matched: found.matched,
        rms_matched: found.rms_matched,
        start: found.start.to_string(),
        converged: found.converged,
        note: found.note(),
        rival: found.rival,
        feature,
        param_warnings: Vec::new(),
    })
}

pub(super) fn remesh_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.remesh";
    let body = super::mesh_body_name(s, str_(p, "body"), cmd)?;
    let with = crate::params::string_list_or_default(p, "with");
    let cell = match crate::params::expr(p, "cell") {
        Some(_) => Some(super::length(s, p, "cell", 1.0, cmd)?),
        None => None,
    };
    let sharp = bool_(p, "sharp").unwrap_or(false);
    // Kept as written, so a step whose offset names a parameter follows it; worked out now to say
    // what it is and refuse what is no length.
    let (offset, offset_mm) = match crate::params::expr(p, "offset") {
        Some(e) => {
            let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("offset: {e}")))?;
            if !v.is_finite() {
                return Err(bad(cmd, "`offset` must be a length"));
            }
            (Some(solidcraft_doc::Expr::new(e)), v)
        }
        None => (None, 0.0),
    };
    let shell = match crate::params::expr(p, "shell") {
        Some(e) => {
            let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("shell: {e}")))?;
            if !(v.is_finite() && v > 0.0) {
                return Err(bad(cmd, "`shell` is the wall's thickness: a length above 0"));
            }
            Some(solidcraft_doc::Expr::new(e))
        }
        None => None,
    };
    let area = super::flatten::step_area(s, p, &body, cmd)?;
    if area.is_some() && shell.is_some() {
        return Err(bad(cmd, "`shell` makes the whole scan hollow, not an area: give one or the other"));
    }
    if area.is_some() && !with.is_empty() {
        return Err(bad(cmd, "`with` fuses other scans in over the whole scan, not an area: give one or the other"));
    }
    if area.is_some() && offset.is_some() {
        return Err(bad(cmd, "`offset` makes the whole scan again that far off it, not an area: give one or the other"));
    }
    let step = CleanStep::Remesh { cell, sharp, offset, shell, with: with.iter().cloned().map(Into::into).collect(), area };
    let done = super::clean::apply(s, p, &body, step, cmd)?;
    let (vertices, triangles) = (done.vertices, done.triangles);
    let feature = done.feature.ok_or_else(|| bad(cmd, "the step was not added"))?;
    let Report::Remesh { before, cell, new_rms, new_max, scan_rms, scan_p95, uncovered } = done.report else {
        return Err(bad(cmd, "the step did something else"));
    };
    reply(ScanRemeshResult {
        body,
        with,
        before,
        triangles,
        vertices,
        cell,
        sharp,
        offset: offset_mm,
        new_rms,
        new_max,
        scan_rms,
        scan_p95,
        uncovered,
        feature,
        param_warnings: Vec::new(),
    })
}

/// The scan Denoise would smooth (`body`, else the selected or only one): its name and its mesh
/// (shared, not copied), for a dialog to study off the window thread.
pub fn denoise_mesh(s: &Session, body: Option<&str>) -> Result<(String, std::sync::Arc<solidcraft_geom::Mesh>)> {
    let name = super::mesh_body_name(s, body, "scan.denoise")?;
    let kept = super::shown_mesh(s, &name).ok_or_else(|| bad("scan.denoise", format!("`{name}` is not a scan (a mesh body)")))?;
    Ok((name, kept))
}

/// What smoothing would do for a mesh: each pass measured, and the strength suggested.
pub fn denoise_study(positions: &[Vec3], triangles: &[[u32; 3]]) -> Option<Study> {
    denoise_study_within(positions, triangles, None)
}

/// [`denoise_study`] of an area (one flag a triangle; the whole mesh with `None`).
pub fn denoise_study_within(positions: &[Vec3], triangles: &[[u32; 3]], area: Option<&[bool]>) -> Option<Study> {
    denoise::study_within(&Scan::new(positions, triangles), area)
}

pub(super) fn denoise_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.denoise";
    let body = super::mesh_body_name(s, str_(p, "body"), cmd)?;
    let strength = num(p, "strength");
    if strength.is_some_and(|k| !(0.0..=denoise::MOST_PASSES as f64).contains(&k)) {
        return Err(bad(cmd, "`strength` is a number of passes from 0 to 8"));
    }
    // Asked to smooth by nothing: a mistake in the call, not a scan with nothing to do.
    if strength == Some(0.0) {
        return Err(bad(cmd, "a `strength` of 0 smooths nothing: give 0.05 to 8 passes"));
    }
    let area = super::flatten::step_area(s, p, &body, cmd)?;
    let done = super::clean::apply(s, p, &body, CleanStep::Denoise { strength, area }, cmd)?;
    let Report::Denoise { strength, suggested, noise, roughness, rms_moved, max_moved } = done.report else {
        return Err(bad(cmd, "the step did something else"));
    };
    reply(ScanDenoiseResult {
        body,
        strength,
        suggested,
        noise,
        roughness,
        rms_moved,
        max_moved,
        feature: done.feature,
        changed: done.changed(),
        note: done.nothing,
        param_warnings: Vec::new(),
    })
}

/// The cloud `p` names (or the only one): its name, its points in the design, and the normals it
/// states, turned with it.
pub(super) fn cloud_points(s: &Session, p: &Value, cmd: &str) -> Result<(String, Vec<Vec3>, Option<Vec<Vec3>>)> {
    let clouds = &s.doc.point_clouds;
    let cloud = match p.get("cloud").filter(|v| !v.is_null()) {
        Some(Value::Number(n)) => clouds.iter().find(|c| c.id.to_string() == n.to_string()),
        Some(Value::String(x)) => clouds.iter().find(|c| c.id.to_string() == *x || c.name == *x),
        Some(_) => return Err(bad(cmd, "`cloud` must be an id or a name")),
        None => match clouds.as_slice() {
            [one] => Some(one),
            [] => return Err(bad(cmd, "the design has no point cloud: insert one with Point Clouds")),
            _ => return Err(bad(cmd, "name the point cloud with `cloud`")),
        },
    };
    let cloud = cloud.ok_or_else(|| bad(cmd, "no such point cloud"))?;
    let points: Vec<Vec3> = (0..cloud.len()).filter_map(|i| cloud.world_point(i)).collect();
    // Normals the file states, turned with the cloud.
    let column = |name: &str| cloud.complex.column(name, solidcraft_doc::point_cloud::On::Point).map(|c| c.values.0.clone());
    let normals = match (column("nx"), column("ny"), column("nz")) {
        (Some(x), Some(y), Some(z)) if x.len() == cloud.len() && y.len() == cloud.len() && z.len() == cloud.len() => {
            let at = |v: Vec3| solidcraft_doc::apply_point(&cloud.transform, v);
            let base = at(cloud.origin);
            Some(
                (0..cloud.len())
                    .map(|i| at(cloud.origin + Vec3::new(f64::from(x[i]), f64::from(y[i]), f64::from(z[i]))) - base)
                    .map(|n| solidcraft_geom::Vec3Geom::normalized(n).unwrap_or(Vec3::ZERO))
                    .collect(),
            )
        }
        _ => None,
    };
    Ok((cloud.name.clone(), points, normals))
}

pub(super) fn mesh_from_points(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.mesh_from_points";
    let (cloud, points, normals) = cloud_points(s, p, cmd)?;
    if points.len() < 16 {
        return Err(bad(cmd, format!("`{cloud}` has too few points to make a surface of")));
    }
    let trim = num(p, "trim").unwrap_or(poisson::DEFAULT_TRIM);
    if !(0.0..=1.0).contains(&trim) {
        return Err(bad(cmd, "`trim` is a share from 0 to 1"));
    }
    let spacing = poisson::typical_spacing(&points);
    let thickness = poisson::layer_thickness(&points, spacing);
    let layered = thickness > poisson::LAYERED * spacing;
    let asked = p.get("depth").filter(|v| !v.is_null()).is_some();
    let depth = match p.get("depth").filter(|v| !v.is_null()) {
        Some(v) => {
            let d = v
                .as_u64()
                .filter(|d| (u64::from(poisson::LEAST_DEPTH)..=u64::from(poisson::MOST_DEPTH)).contains(d))
                .ok_or_else(|| bad(cmd, format!("`depth` is a whole number from {} to {}", poisson::LEAST_DEPTH, poisson::MOST_DEPTH)))?;
            d as u32
        }
        None => {
            let (lo, hi) = points
                .iter()
                .fold((points[0], points[0]), |(lo, hi), q| (solidcraft_geom::Vec3Geom::min(lo, *q), solidcraft_geom::Vec3Geom::max(hi, *q)));
            let d = hi - lo;
            poisson::Settings::suggested_for(d.x.max(d.y).max(d.z), spacing, thickness).depth
        }
    };
    // A suggested depth too fine for the node budget steps down until it fits; one asked for is refused.
    let mut depth = depth;
    let made = loop {
        match poisson::reconstruct(&points, normals.as_deref(), &poisson::Settings { depth, trim }) {
            Err(poisson::PoissonError::TooFine) if !asked && depth > poisson::LEAST_DEPTH => depth -= 1,
            other => break other.map_err(|e| bad(cmd, e.to_string()))?,
        }
    };
    let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let name = match str_(p, "name").filter(|n| !n.trim().is_empty()) {
        Some(n) => n.to_owned(),
        None => std::iter::once(format!("{cloud} Mesh"))
            .chain((2..).map(|i| format!("{cloud} Mesh {i}")))
            .find(|n| !taken.contains(n))
            .unwrap_or_else(|| "Mesh".into()),
    };
    let data = solidcraft_doc::MeshData {
        name: name.clone(),
        positions: made.positions.iter().flat_map(|q| [q.x, q.y, q.z]).collect::<Vec<f64>>().into(),
        triangles: made.triangles.iter().flatten().copied().collect::<Vec<u32>>().into(),
        corner_normals: Default::default(),
        color: None,
    };
    let kind = solidcraft_doc::FeatureKind::MeshImport { file: format!("{cloud} (mesh from points)"), meshes: vec![data] };
    let feature = super::super::features::add_feature(s, &json!({ "body_name": name, "name": "Mesh from Points" }), kind)?;
    reply(ScanMeshFromPointsResult {
        body: name,
        triangles: made.triangles.len(),
        vertices: made.positions.len(),
        depth: made.settings.depth,
        trim: made.settings.trim,
        points: made.points,
        points_used: made.points_used,
        estimated_normals: made.estimated_normals,
        cell: made.cell,
        spacing: made.spacing,
        trimmed: made.trimmed,
        rms: made.distances.rms,
        p95: made.distances.p95,
        max: made.distances.max,
        thickness,
        warning: layered.then(|| {
            format!(
                "the points lie in a layer {thickness:.2} mm thick, {:.1} times as far as they are apart: passes merged without lying on each other; the surface is made through the middle of the layer with cells of {:.2} mm, align the passes (Align All Scans) before merging them for a finer one",
                thickness / spacing.max(1e-12),
                made.cell
            )
        }),
        feature,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_mesh_tests.rs"]
mod tests;
