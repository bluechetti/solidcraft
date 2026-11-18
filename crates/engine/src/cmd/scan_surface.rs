//! Editing a fitted surface by its control points (`scan.edit_surface`): the surface Fit Surface
//! made (one B-spline face, kept as STEP in a Scan Surface feature) has its control points moved, and
//! those round them or all the others fitted to the scan again. The feature's STEP is
//! replaced, so an edit is one undo step and the design opens anywhere, with no format change.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcore_record::brep as b;
use solidcraft_geom::Vec3Geom as _;
use solidcraft_geom::bspline::{HPoint, NurbsSurface};
pub use solidcraft_scan::merge::Side;
use solidcraft_scan::net;
pub use solidcraft_scan::net::{Line, NetError};
use solidcraft_scan::place::Rigid;
use solidcraft_scan::surface_edit;

use super::{length, mesh};
use crate::params::{bad, str_};
use crate::schema::scan::{ScanEditNetResult, ScanEditSurfaceResult};
use crate::{EngineError, Result, Sel, Session};

/// The most scan points a refit uses: enough to pin a net of 30 × 30, few enough to refit as a
/// point is dragged.
pub const REFIT_POINTS: usize = 8_000;

/// A surface that can be edited, as it now is.
#[derive(Clone, Debug)]
pub struct EditableSurface {
    /// The surface body and the feature that keeps it.
    pub body: String,
    pub feature: u64,
    /// Its first face (its only one, but for a Free Form of several faces).
    pub surface: NurbsSurface,
    /// Every face: one, or a Free Form's network of faces that share whole sides.
    pub faces: Vec<NurbsSurface>,
    /// How later features have moved it since it was imported, if they have: an edit is written
    /// back moved the other way, so they move it to where it was edited.
    pub placed: Option<Rigid>,
}

impl EditableSurface {
    /// Whether it is a network of several faces.
    pub fn is_network(&self) -> bool {
        self.faces.len() > 1
    }
}

/// The surface's faces as NURBS surfaces: one B-spline face, or several cubic ones that make a Free
/// Form's network (they share whole sides). `None` for anything else.
fn faces_of(record: &b::Brep) -> Option<Vec<NurbsSurface>> {
    let faces: Vec<NurbsSurface> = record.faces.iter().map(|f| nurbs_of_surface(&f.surface)).collect::<Option<_>>()?;
    match faces.len() {
        0 => None,
        1 => Some(faces),
        _ => solidcraft_scan::network::joins(&faces).ok().filter(|j| !j.seams.is_empty()).map(|_| faces),
    }
}

/// A B-spline record surface (rational or not) as a NURBS surface, on the same (u, v); `None` for
/// any other kind of surface.
pub fn nurbs_of_surface(surface: &b::Surface) -> Option<NurbsSurface> {
    let (u_knots, v_knots, rows): (&Vec<f64>, &Vec<f64>, Vec<Vec<HPoint>>) = match surface {
        b::Surface::Spline { u_knots, v_knots, points } => {
            (u_knots, v_knots, points.iter().map(|row| row.iter().map(|p| HPoint::weighted(Vec3::new(p[0], p[1], p[2]), 1.0)).collect()).collect())
        }
        b::Surface::RationalSpline { u_knots, v_knots, homogeneous } => {
            (u_knots, v_knots, homogeneous.iter().map(|row| row.iter().map(|h| HPoint { x: h[0], y: h[1], z: h[2], w: h[3] }).collect()).collect())
        }
        _ => return None,
    };
    // The record's rows run along u (`rows[i][j]`, i along u); the surface's points go u fastest.
    let nu = rows.len();
    let nv = rows.first()?.len();
    if nu == 0 || nv == 0 || rows.iter().any(|r| r.len() != nv) {
        return None;
    }
    let degree = [u_knots.len().checked_sub(nu + 1)?, v_knots.len().checked_sub(nv + 1)?];
    let ctrl = (0..nv).flat_map(|j| rows.iter().map(move |row| row[j])).collect();
    NurbsSurface::from_homogeneous(degree, [u_knots.clone(), v_knots.clone()], ctrl).ok()
}

/// The surface body named, or the selected one, or the design's only one: a body of one B-spline
/// face, as it was imported or only moved since (nothing later has reshaped it).
pub fn editable(s: &Session, name: Option<&str>, cmd: &str) -> Result<EditableSurface> {
    let state = s.model.state();
    let usable = |name: &str| -> Option<EditableSurface> {
        let body = state.body(name).filter(|b| !b.body.is_mesh())?;
        // A Scan Surface feature, or an Import feature in a design from before there was one.
        let is_import = |f: &&solidcraft_doc::Feature| {
            matches!(f.kind, solidcraft_doc::FeatureKind::ScanSurface { .. } | solidcraft_doc::FeatureKind::Import { .. })
        };
        // The feature that made it, or, once a later feature has moved it, the one that named it.
        let feature = s
            .doc
            .features
            .iter()
            .find(|f| f.id == body.feature)
            .filter(is_import)
            .or_else(|| s.doc.features.iter().filter(is_import).rfind(|f| f.body_names.iter().any(|n| n == name)))?;
        let (solidcraft_doc::FeatureKind::ScanSurface { step, .. } | solidcraft_doc::FeatureKind::Import { step, .. }) = &feature.kind else {
            return None;
        };
        let faces = faces_of(&solidcraft_kernel::brep_record(&body.body).ok()?)?;
        // The file's own surface is this one, or this one moved: writing it back changes nothing else.
        let imported = solidcraft_kernel::step_import_shared(step).ok()?;
        let [only] = imported.bodies.as_slice() else { return None };
        let original = faces_of(&solidcraft_kernel::brep_record(&only.body).ok()?)?;
        if original.len() != faces.len() {
            return None;
        }
        let placed = if original == faces {
            None
        } else {
            let from: Vec<Vec3> = original.iter().flat_map(surface_edit::points).collect();
            let to: Vec<Vec3> = faces.iter().flat_map(surface_edit::points).collect();
            let by = surface_edit::placement(&from, &to, [from.len(), 1])?;
            // Only moved: the same knots and weights, its points where the move puts them.
            let moved = original.iter().zip(&faces).all(|(o, f)| surface_edit::with_points(o, &surface_edit::points(f)).as_ref() == Some(f));
            moved.then_some(())?;
            Some(by)
        };
        let surface = faces[0].clone();
        Some(EditableSurface { body: name.to_owned(), feature: feature.id, surface, faces, placed })
    };
    let named = name.filter(|n| !n.trim().is_empty());
    if let Some(name) = named {
        if state.body(name).is_none() {
            return Err(bad(cmd, format!("no body `{name}`")));
        }
        return usable(name).ok_or_else(|| {
            bad(
                cmd,
                format!(
                    "`{name}` is not a surface whose control points can be edited: one B-spline face, as Fit Surface makes it, or a Free Form's faces, not reshaped since"
                ),
            )
        });
    }
    let selected: Vec<&str> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    if let Some(found) = selected.iter().find_map(|n| usable(n)) {
        return Ok(found);
    }
    // With none chosen, the one made last (a Free Form just made is the one to edit).
    let all: Vec<EditableSurface> = state.bodies.iter().filter_map(|b| usable(&b.name)).collect();
    all.into_iter().max_by_key(|e| e.feature).ok_or_else(|| bad(cmd, "the design has no surface to edit: make one with Fit Surface or New Free Form"))
}

/// The scan points under a surface, for refitting and for how far it is from the scan.
#[derive(Clone, Debug)]
pub struct Covered {
    pub scan: String,
    pub points: Arc<Vec<Vec3>>,
}

/// The default distance a scan point may be from the surface to be under it: a tenth of the control
/// net's size, at least a millimetre, so the scan under a point pulled well off it still counts.
pub fn default_reach(surface: &NurbsSurface) -> f64 {
    let net = surface_edit::points(surface);
    let Some(first) = net.first().copied() else { return 1.0 };
    let (lo, hi) = net.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    ((hi - lo).len() / 10.0).max(1.0)
}

/// The scan points the surface covers (on the scan `scan`, or the selected or only one).
pub fn covered(s: &Session, scan: Option<&str>, surface: &NurbsSurface, reach: f64, cmd: &str) -> Result<Covered> {
    let mesh = mesh(s, &json!({ "body": scan }), cmd)?;
    let points = surface_edit::covered(&mesh.scan(), surface, reach, REFIT_POINTS);
    Ok(Covered { scan: mesh.body, points: Arc::new(points) })
}

/// The most scan points the colouring of a surface's deviation shows.
pub const SHADE_POINTS: usize = 40_000;

/// The scan (`scan`, or the selected or only one) under `faces`, for colouring it by how far it is
/// from them as they are edited: one look over the scan for all the faces.
pub fn shade(s: &Session, scan: Option<&str>, faces: &[NurbsSurface], cmd: &str) -> Result<(String, solidcraft_scan::network::Shade)> {
    let mesh = mesh(s, &json!({ "body": scan }), cmd)?;
    let reach = faces.iter().map(default_reach).fold(1.0, f64::max);
    let shade = solidcraft_scan::network::shade(&mesh.scan(), faces, reach, SHADE_POINTS);
    Ok((mesh.body, shade))
}

/// A short print of where scan `scan`'s points are: how many, and a few spread over them. It
/// changes when the scan is moved, turned or edited, so a dialog colouring the scan under a
/// surface knows to look for the scan under it again; `None` when there is no such scan.
pub fn scan_print(s: &Session, scan: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    super::with_shown(s, scan, |positions, triangles| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (positions.len(), triangles.len()).hash(&mut h);
        const SAMPLES: usize = 64;
        let step = positions.len() / SAMPLES + 1;
        for p in positions.iter().step_by(step) {
            [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()].hash(&mut h);
        }
        h.finish()
    })
}

/// The network maths a dialog edits a Free Form of several faces with, as the commands do.
pub use solidcraft_scan::network;
/// Symmetry for a Free Form, as Edit Surface keeps it.
pub use solidcraft_scan::symmetric;
/// Shapes points are held to, refinement fixes and patterns, as Edit Surface offers them.
pub use solidcraft_scan::{constrain, face_options, pattern};

/// The surface as STEP text: one face, the whole of it (its knots may be uneven or repeated).
pub fn surface_step(surface: &NurbsSurface, cmd: &str) -> Result<String> {
    let sheet = solidcraft_kernel::nurbs_sheet(surface)
        .map_err(|e| super::refusal::refused(cmd, "the surface could not be made into a face", &e.to_string()))?;
    solidcraft_kernel::step_export(&[&sheet], "SolidCraft").map_err(|e| super::refusal::refused(cmd, "the surface could not be kept", &e.to_string()))
}

/// Keeps `surface` as a surface body of its own, named `<base>1`, `<base>2`, … (the first free),
/// in a Scan Surface feature called `feature`, whose file reads `file`: one undo step, and a design
/// that opens anywhere. The body's name and the feature's reply.
pub fn keep_surface(
    s: &mut Session,
    surface: &NurbsSurface,
    base: &str,
    feature: &str,
    file: &str,
    recipe: Option<solidcraft_doc::scan_recipe::ScanRecipe>,
    cmd: &str,
) -> Result<(String, Value)> {
    let step = surface_step(surface, cmd)?;
    let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let name = (1..).map(|i| format!("{base}{i}")).find(|n| !taken.contains(n)).unwrap_or_else(|| base.into());
    let kind = solidcraft_doc::FeatureKind::ScanSurface { file: file.into(), step: step.into(), recipe };
    let reply = super::super::features::add_feature(s, &json!({ "body_name": name, "name": feature }), kind)?;
    Ok((name, reply))
}

/// `surface` where it was before later features moved the surface, as an edit is written back.
pub(super) fn unmoved(target: &EditableSurface, surface: &NurbsSurface, cmd: &str) -> Result<NurbsSurface> {
    match &target.placed {
        Some(by) => {
            let back = by.inverse();
            let net: Vec<Vec3> = surface_edit::points(surface).iter().map(|p| back.apply(*p)).collect();
            surface_edit::with_points(surface, &net).ok_or_else(|| bad(cmd, "the surface's place could not be undone"))
        }
        None => Ok(surface.clone()),
    }
}

/// Writes `surface` back into the feature that keeps the surface: one undo step.
pub(super) fn write_back(s: &mut Session, target: &EditableSurface, surface: &NurbsSurface, cmd: &str) -> Result<()> {
    let step = surface_step(&unmoved(target, surface, cmd)?, cmd)?;
    write_step(s, target, step, cmd)
}

/// Writes `faces` back into the feature that keeps the surface: one face as it is, a
/// network's faces sewn into one body along the sides they share. One undo step.
pub(super) fn write_faces(s: &mut Session, target: &EditableSurface, faces: &[NurbsSurface], cmd: &str) -> Result<()> {
    match faces {
        [] => Err(bad(cmd, "a surface needs a face")),
        [one] => write_back(s, target, one, cmd),
        many => {
            let unmoved: Vec<NurbsSurface> = many.iter().map(|f| unmoved(target, f, cmd)).collect::<Result<_>>()?;
            write_step(s, target, network_step(&unmoved, cmd)?, cmd)
        }
    }
}

/// A network's faces as STEP text: sewn into one body along the sides they share.
pub fn network_step(faces: &[NurbsSurface], cmd: &str) -> Result<String> {
    let sheets: Vec<_> = faces
        .iter()
        .map(|f| {
            solidcraft_kernel::nurbs_sheet(f).map_err(|e| super::refusal::refused(cmd, "a face could not be made into a surface", &e.to_string()))
        })
        .collect::<Result<_>>()?;
    let refs: Vec<_> = sheets.iter().collect();
    let sewn = solidcraft_kernel::stitch(&refs, 0.0)
        .map_err(|e| super::refusal::refused(cmd, "the faces could not be sewn into one surface", &e.to_string()))?;
    solidcraft_kernel::step_export(&[&sewn], "SolidCraft").map_err(|e| super::refusal::refused(cmd, "the faces could not be kept", &e.to_string()))
}

/// Writes `step` into the feature that keeps the surface, and works the design out again.
pub(super) fn write_step(s: &mut Session, target: &EditableSurface, step: String, cmd: &str) -> Result<()> {
    let feature = s.doc_mut().feature_mut(target.feature).ok_or_else(|| bad(cmd, "the surface's feature is gone"))?;
    if let solidcraft_doc::FeatureKind::ScanSurface { step: kept, .. } | solidcraft_doc::FeatureKind::Import { step: kept, .. } = &mut feature.kind {
        *kept = step.into();
    }
    s.refresh();
    match s.model.result(target.feature).and_then(|r| r.error.clone()) {
        Some(e) => Err(EngineError::Other(e)),
        None => Ok(()),
    }
}

/// A length (an expression, negative allowed) under `key`, `None` when it is not given.
fn signed_length(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Option<f64>> {
    if p.get(key).is_none_or(Value::is_null) {
        return Ok(None);
    }
    let e = crate::params::expr(p, key).ok_or_else(|| bad(cmd, format!("`{key}` must be a length")))?;
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !v.is_finite() {
        return Err(bad(cmd, format!("`{key}` must be a finite length")));
    }
    Ok(Some(v))
}

/// A control point `[u, v]` by its place in the net, checked against the net's size.
fn index(v: &Value, count: [usize; 2], cmd: &str, what: &str) -> Result<usize> {
    let pair = v.as_array().filter(|a| a.len() == 2).and_then(|a| Some([a[0].as_u64()?, a[1].as_u64()?]));
    let [i, j] = pair.ok_or_else(|| bad(cmd, format!("`{what}` must be [u, v], whole numbers from 0")))?;
    let (i, j) = (usize::try_from(i).unwrap_or(usize::MAX), usize::try_from(j).unwrap_or(usize::MAX));
    if i >= count[0] || j >= count[1] {
        return Err(bad(cmd, format!("`{what}` [{i}, {j}] is outside the net of {} × {} control points", count[0], count[1])));
    }
    Ok(j * count[0] + i)
}

/// A whole net as `{count: [u, v], knots: [[…], […]], points: [[x, y, z], …], degree?: [u, v]}`
/// (u fastest), as a dialog that changed the net's shape sends it.
fn net_param(v: &Value, cmd: &str) -> Result<NurbsSurface> {
    let pair = |key: &str| -> Option<[usize; 2]> {
        let a = v.get(key)?.as_array()?;
        let n = |k: usize| a.get(k).and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok());
        (a.len() == 2).then_some([n(0)?, n(1)?])
    };
    let count = pair("count").ok_or_else(|| bad(cmd, "`net.count` must be [u, v], whole numbers"))?;
    let degree = match v.get("degree").filter(|d| !d.is_null()) {
        Some(_) => pair("degree").ok_or_else(|| bad(cmd, "`net.degree` must be [u, v], whole numbers"))?,
        None => [3, 3],
    };
    let knots_of = |k: usize| -> Option<Vec<f64>> { v.get("knots")?.get(k)?.as_array()?.iter().map(Value::as_f64).collect() };
    let knots = [knots_of(0), knots_of(1)];
    let [Some(ku), Some(kv)] = knots else { return Err(bad(cmd, "`net.knots` must be two lists of numbers")) };
    let points: Vec<Vec3> = v
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`net.points` must list the control points [x, y, z], u fastest"))?
        .iter()
        .map(|q| crate::params::vec3(q).ok_or_else(|| bad(cmd, "a point of `net.points` must be [x, y, z] of numbers")))
        .collect::<Result<_>>()?;
    if count[0].saturating_mul(count[1]) != points.len() || count.iter().any(|c| *c > solidcraft_scan::net::MOST) {
        return Err(bad(cmd, "`net.points` must hold count[0] × count[1] points (at most 48 each way)"));
    }
    NurbsSurface::new(degree, [ku, kv], &points, &vec![1.0; points.len()]).map_err(|e| bad(cmd, format!("`net`: {e}")))
}

/// The scan points under `faces` and how far they are from them, root mean square and largest:
/// each face's covered points measured to it (`None` with no scan under them).
pub fn faces_distance(s: &Session, scan: Option<&str>, faces: &[NurbsSurface], cmd: &str) -> Option<(String, usize, f64, f64)> {
    let (mut name, mut count, mut squares, mut max) = (None, 0usize, 0.0, 0.0f64);
    for face in faces {
        let Ok(c) = covered(s, scan, face, default_reach(face), cmd) else { continue };
        if c.points.is_empty() {
            continue;
        }
        let (rms, m) = surface_edit::distance(face, &c.points);
        squares += rms * rms * c.points.len() as f64;
        count += c.points.len();
        max = max.max(m);
        name = Some(c.scan);
    }
    Some((name?, count, (squares / count.max(1) as f64).sqrt(), max))
}

/// A whole network of faces, as `faces` gives it: each a net as `net` is, the faces one piece.
fn faces_param(v: &Value, cmd: &str) -> Result<Vec<NurbsSurface>> {
    let list = v.as_array().filter(|a| !a.is_empty()).ok_or_else(|| bad(cmd, "`faces` must list the faces, each {count, knots, points, degree?}"))?;
    if list.len() > solidcraft_scan::network::MOST_FACES {
        return Err(bad(cmd, format!("`faces` lists at most {} faces", solidcraft_scan::network::MOST_FACES)));
    }
    let faces: Vec<NurbsSurface> = list.iter().map(|f| net_param(f, cmd)).collect::<Result<_>>()?;
    if faces.len() > 1 {
        let joins = solidcraft_scan::network::joins(&faces).map_err(|e| bad(cmd, e.to_string()))?;
        if !solidcraft_scan::network::connected(&faces, &joins) {
            return Err(bad(cmd, "the faces must make one piece: each shares a whole side with another"));
        }
    }
    Ok(faces)
}

/// The reply of an edit that wrote `faces` back, joined as `joins` says.
pub(super) fn faces_reply(
    s: &Session,
    target: &EditableSurface,
    faces: &[NurbsSurface],
    joins: &solidcraft_scan::network::Joins,
    moved: usize,
    cmd: &str,
) -> Result<Value> {
    let measured = if s.model.state().bodies.iter().any(|b| b.body.is_mesh()) { faces_distance(s, None, faces, cmd) } else { None };
    let reply = ScanEditSurfaceResult {
        surface: target.body.clone(),
        controls: faces.first().map_or([0, 0], NurbsSurface::count),
        moved,
        carried: 0,
        refitted: 0,
        scan: measured.as_ref().map(|m| m.0.clone()),
        points: measured.as_ref().map(|m| m.1),
        rms: measured.as_ref().map(|m| m.2),
        max: measured.as_ref().map(|m| m.3),
        faces: (faces.len() > 1).then_some(faces.len()),
        creases: (faces.len() > 1).then(|| solidcraft_scan::network::creased(joins).iter().filter(|c| **c).count()),
        sewn: None,
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Shared edges of a Free Form creased (`crease`) or made smooth again (`uncrease`): each named by
/// a point near it or by its ends; the faces held tangent across it or not from now on, and the
/// points near it fitted to the scan again, so they take the scan's edge, or round it over.
fn creased_edges(s: &mut Session, target: &EditableSurface, p: &Value, cmd: &str) -> Result<Value> {
    use solidcraft_scan::network;
    if !target.is_network() {
        return Err(bad(
            cmd,
            format!(
                "`{}` is one face: a crease needs a shared edge of a Free Form of several faces (Edit Net's crease creases a row or column of one face)",
                target.body
            ),
        ));
    }
    let faces = target.faces.clone();
    let mut joins = super::network_edit::joins_for(s, target, &faces, cmd)?;
    let reach = length(s, p, "reach", faces.iter().map(default_reach).fold(1.0, f64::max), cmd)?;
    if !(reach.is_finite() && reach > 0.0) {
        return Err(bad(cmd, "`reach` must be a length above 0"));
    }
    let (pairs, symmetry) = super::network_edit::edit_pairs(s, target, &joins, p, cmd)?;
    let mut picks: Vec<(bool, usize)> = Vec::new();
    for (key, crease) in [("crease", true), ("uncrease", false)] {
        let Some(list) = p.get(key).filter(|v| !v.is_null()) else { continue };
        let list = list
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 256)
            .ok_or_else(|| bad(cmd, format!("`{key}` must list 1 to 256 edges, each {{at: [x, y, z]}} or {{edge: [[x, y, z], [x, y, z]]}}")))?;
        for (k, pick) in list.iter().enumerate() {
            let at = pick.get("at").filter(|v| !v.is_null());
            let edge = pick.get("edge").filter(|v| !v.is_null());
            let seam = match (at, edge) {
                (Some(at), None) => {
                    let near = crate::params::vec3(at).ok_or_else(|| bad(cmd, format!("`{key}[{k}].at` must be a point [x, y, z] of numbers")))?;
                    let (seam, off) = network::seam_near(&faces, &joins, near).ok_or_else(|| bad(cmd, "the Free Form has no shared edge"))?;
                    if off > reach {
                        return Err(bad(
                            cmd,
                            format!("`{key}[{k}].at` is {off:.2} mm from the nearest shared edge, more than the reach ({reach:.2} mm)"),
                        ));
                    }
                    seam
                }
                (None, Some(edge)) => {
                    let ends = edge
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or_else(|| bad(cmd, format!("`{key}[{k}].edge` must be two points [x, y, z]")))?;
                    let (Some(a), Some(b)) = (crate::params::vec3(&ends[0]), crate::params::vec3(&ends[1])) else {
                        return Err(bad(cmd, format!("`{key}[{k}].edge` must be two points [x, y, z] of numbers")));
                    };
                    network::seam_between(&faces, &joins, a, b, reach).ok_or_else(|| {
                        bad(cmd, format!("`{key}[{k}].edge` is not the ends of a shared edge of the Free Form (within the reach, {reach:.2} mm)"))
                    })?
                }
                _ => return Err(bad(cmd, format!("`{key}[{k}]` names its edge by `at` (a point near it) or by `edge` (its two ends): one of them"))),
            };
            if picks.iter().any(|(_, i)| *i == seam) {
                return Err(bad(cmd, format!("`{key}[{k}]` names an edge already named in this call")));
            }
            let chain = match pick.get("chain").filter(|v| !v.is_null()) {
                None => false,
                Some(v) => v.as_bool().ok_or_else(|| bad(cmd, format!("`{key}[{k}].chain` must be true or false")))?,
            };
            // The edge, the edges it runs on into through the grid's corners, and with Symmetry
            // their mirror images.
            let mut named = if chain { network::crease_chain(&faces, &joins, seam) } else { vec![seam] };
            if let Some(pairs) = &pairs {
                let twins: Vec<usize> = named
                    .iter()
                    .filter_map(|&i| network::seam_ends(&faces, &joins, i))
                    .filter_map(|(a, b)| network::seam_between(&faces, &joins, pairs.plane.reflect(a), pairs.plane.reflect(b), 1e-6))
                    .collect();
                named.extend(twins);
            }
            for i in named {
                if !picks.iter().any(|(_, j)| *j == i) {
                    picks.push((crease, i));
                }
            }
        }
    }
    if picks.is_empty() {
        return Err(bad(cmd, "`crease` or `uncrease` must name an edge"));
    }
    let scan: Vec<Vec3> = shade(s, str_(p, "scan"), &faces, cmd).map(|(_, sh)| sh.points.iter().map(|u| u.at).collect()).unwrap_or_default();
    let mut current = faces;
    let held = vec![false; joins.len()];
    for (crease, seam) in picks {
        joins = if crease { network::without_tangency(&joins, seam) } else { network::with_tangency(&current, &joins, seam) };
        if let Some(next) = network::refit_near_seam(&current, &joins, seam, &scan, reach, &held) {
            current = next;
        }
        if let Some(pairs) = &pairs {
            current = pairs.kept(&current, &joins).unwrap_or(current);
        }
    }
    write_faces(s, target, &current, cmd)?;
    super::network_edit::keep(s, target, &current, &joins, symmetry);
    faces_reply(s, target, &current, &joins, 0, cmd)
}

/// A smoothing brush dabbed on the surface (`brush`): at each place, the points on the surface
/// within the radius are evened out, more toward the middle, as many passes as asked, kept on the
/// scan unless `snap` is off.
fn brushed_points(s: &mut Session, target: &EditableSurface, v: &Value, p: &Value, cmd: &str) -> Result<Value> {
    use solidcraft_scan::network;
    let places = v
        .get("points")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= 10_000)
        .ok_or_else(|| bad(cmd, "`brush.points` must list 1 to 10000 places [x, y, z]"))?;
    let places: Vec<Vec3> = places
        .iter()
        .enumerate()
        .map(|(k, q)| crate::params::vec3(q).ok_or_else(|| bad(cmd, format!("`brush.points[{k}]` must be a point [x, y, z] of numbers"))))
        .collect::<Result<_>>()?;
    if crate::params::expr(v, "radius").is_none() {
        return Err(bad(cmd, "`brush.radius` must be a length: how far round each place the brush reaches"));
    }
    let radius = length(s, v, "radius", 1.0, cmd)?;
    if !(radius.is_finite() && radius > 0.0) {
        return Err(bad(cmd, "`brush.radius` must be a length above 0"));
    }
    let strength = match v.get("strength").filter(|x| !x.is_null()) {
        None => 0.5,
        Some(x) => {
            x.as_f64().filter(|f| f.is_finite() && *f > 0.0 && *f <= 1.0).ok_or_else(|| bad(cmd, "`brush.strength` must be above 0 and at most 1"))?
        }
    };
    let passes = match v.get("passes").filter(|x| !x.is_null()) {
        None => 4,
        Some(x) => x.as_u64().filter(|n| (1..=100).contains(n)).ok_or_else(|| bad(cmd, "`brush.passes` must be a whole number, 1 to 100"))?,
    };
    if places.len() as u64 * passes > 2000 {
        return Err(bad(cmd, "`brush`: at most 2000 passes in all (places × passes)"));
    }
    let snap = match v.get("snap").filter(|x| !x.is_null()) {
        None => true,
        Some(x) => x.as_bool().ok_or_else(|| bad(cmd, "`brush.snap` must be true or false"))?,
    };
    let mut faces = target.faces.clone();
    let joins = super::network_edit::joins_for(s, target, &faces, cmd)?;
    let reach = length(s, p, "reach", faces.iter().map(default_reach).fold(1.0, f64::max), cmd)?;
    if !(reach.is_finite() && reach > 0.0) {
        return Err(bad(cmd, "`reach` must be a length above 0"));
    }
    let (pairs, symmetry) = super::network_edit::edit_pairs(s, target, &joins, p, cmd)?;
    let scan: Vec<Vec3> = if snap {
        let (_, sh) = shade(s, str_(p, "scan"), &faces, cmd)?;
        sh.points.iter().map(|u| u.at).collect()
    } else {
        Vec::new()
    };
    let all = vec![true; faces.len()];
    let mut touched: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (k, place) in places.iter().enumerate() {
        let dots = network::dots(&faces, &joins);
        let under: Vec<(usize, f64)> =
            dots.iter().enumerate().map(|(i, d)| (i, d.at.dist(*place))).filter(|(_, d)| *d <= radius).map(|(i, d)| (i, 1.0 - d / radius)).collect();
        if under.is_empty() {
            return Err(bad(cmd, format!("`brush.points[{k}]` has no point of the surface within the radius ({radius:.2} mm) of it")));
        }
        touched.extend(under.iter().map(|u| u.0));
        // With Symmetry the brush works on both sides at once.
        let under = match &pairs {
            Some(pairs) => {
                let mut both = under.clone();
                for &(p, w) in &under {
                    if let Some(&t) = pairs.twin.get(p).filter(|t| !under.iter().any(|u| u.0 == **t)) {
                        both.push((t, w));
                    }
                }
                both
            }
            None => under,
        };
        for _ in 0..passes {
            let goals = if scan.is_empty() { Vec::new() } else { network::feet(&faces, &scan, reach, &all) };
            match network::brushed(&faces, &joins, &under, &goals, strength) {
                Some(next) => faces = next,
                None => {
                    return Err(bad(cmd, format!("`brush.points[{k}]`: nothing under the brush there may move (the surface's own corners stay)")));
                }
            }
            if let Some(pairs) = &pairs {
                faces = pairs.kept(&faces, &joins).unwrap_or(faces);
            }
        }
    }
    write_faces(s, target, &faces, cmd)?;
    super::network_edit::keep(s, target, &faces, &joins, symmetry);
    faces_reply(s, target, &faces, &joins, touched.len(), cmd)
}

/// Points on the surface moved (`dots`): each the surface at a control point's place on it,
/// taken where it goes; the points round it hold the surface through every other dot, or follow.
fn moved_dots(s: &mut Session, target: &EditableSurface, list: &Value, p: &Value, cmd: &str) -> Result<Value> {
    use solidcraft_scan::network;
    let list = list.as_array().filter(|a| !a.is_empty()).ok_or_else(|| bad(cmd, "`dots` must list {point: [u, v], face?, to | along}"))?;
    let hold = match str_(p, "neighbours").unwrap_or("hold") {
        "hold" => true,
        "follow" => false,
        other => return Err(bad(cmd, format!("`neighbours` must be hold or follow, not `{other}`"))),
    };
    let faces = &target.faces;
    let joins = super::network_edit::joins_for(s, target, faces, cmd)?;
    let dots = network::dots(faces, &joins);
    let mut moves: Vec<(usize, Vec3)> = Vec::new();
    for (k, d) in list.iter().enumerate() {
        let id = match d.get("at").filter(|v| !v.is_null()) {
            Some(v) => {
                if d.get("point").is_some_and(|v| !v.is_null()) {
                    return Err(bad(cmd, format!("`dots[{k}]` names its point by `at` or by `point`, not both")));
                }
                let near = crate::params::vec3(v).ok_or_else(|| bad(cmd, format!("`dots[{k}].at` must be a point [x, y, z] of numbers")))?;
                (0..dots.len())
                    .min_by(|a, b| dots[*a].at.dist(near).total_cmp(&dots[*b].at.dist(near)))
                    .ok_or_else(|| bad(cmd, "the surface has no points"))?
            }
            None => {
                let face = match d.get("face").filter(|v| !v.is_null()) {
                    Some(v) => v
                        .as_u64()
                        .and_then(|f| usize::try_from(f).ok())
                        .filter(|f| *f < faces.len())
                        .ok_or_else(|| bad(cmd, format!("`dots[{k}].face` must be a face of the surface, 0 to {}", faces.len() - 1)))?,
                    None => 0,
                };
                let at = index(d.get("point").unwrap_or(&Value::Null), faces[face].count(), cmd, &format!("dots[{k}].point"))?;
                joins.of[face][at]
            }
        };
        let dot = dots[id];
        let to = match (d.get("to").filter(|v| !v.is_null()), d.get("along").filter(|v| !v.is_null())) {
            (Some(to), None) => crate::params::vec3(to).ok_or_else(|| bad(cmd, format!("`dots[{k}].to` must be a point [x, y, z] of numbers")))?,
            (None, Some(_)) => {
                let e = crate::params::expr(d, "along").ok_or_else(|| bad(cmd, format!("`dots[{k}].along` must be a length")))?;
                let length = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("dots[{k}].along: {e}")))?;
                if !length.is_finite() {
                    return Err(bad(cmd, format!("`dots[{k}].along` must be a finite length")));
                }
                let n = network::dot_normal(faces, &dot).ok_or_else(|| bad(cmd, "the surface has no direction at that point"))?;
                dot.at + n * length
            }
            _ => return Err(bad(cmd, format!("`dots[{k}]` needs one of `to` (a point) or `along` (a length along the surface's normal)"))),
        };
        moves.retain(|m| m.0 != id);
        moves.push((id, to));
    }
    // With a symmetry plane (given, or the one the design keeps): each dot's twin goes to the
    // mirror image, and the result is made an exact mirror image again.
    let (pairs, symmetry) = super::network_edit::edit_pairs(s, target, &joins, p, cmd)?;
    if let Some(pairs) = &pairs {
        moves = pairs.moves(&moves);
    }
    let made =
        network::dots_moved(faces, &joins, &moves, network::DOT_RING, hold, &[]).ok_or_else(|| bad(cmd, "the points could not be moved there"))?;
    let made = match &pairs {
        Some(pairs) => pairs.kept(&made, &joins).unwrap_or(made),
        None => made,
    };
    write_faces(s, target, &made, cmd)?;
    super::network_edit::keep(s, target, &made, &joins, symmetry);
    faces_reply(s, target, &made, &joins, moves.len(), cmd)
}

pub(super) fn edit_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.edit_surface";
    let target = editable(s, str_(p, "surface"), cmd)?;
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    // The edits of a Free Form as a whole, each given alone.
    const WHOLE: [&str; 6] = ["brush", "smooth", "delete_face", "weld", "keep_half", "crease"];
    const POINTS: [&str; 8] = ["points", "moves", "hold", "net", "slide", "dots", "faces", "kept_edges"];
    let whole: Vec<&str> = WHOLE.iter().copied().filter(|k| given(k) || (*k == "crease" && given("uncrease"))).collect();
    if let Some(op) = whole.first()
        && (whole.len() > 1 || POINTS.iter().any(|k| given(k)) || str_(p, "refit").is_some_and(|r| r != "none"))
    {
        return Err(bad(
            cmd,
            format!(
                "`{op}` edits a Free Form as a whole: give it alone (with `reach`, `scan` or `symmetry`), without points, moves, hold, net, slide, dots, faces or a refit, and without another of brush, smooth, delete_face, weld, keep_half or crease"
            ),
        ));
    }
    if given("kept_edges") && !given("faces") {
        return Err(bad(cmd, "`kept_edges` goes with `faces`: the edges creased or held smooth by hand among the faces given"));
    }
    if let Some(v) = p.get("smooth").filter(|v| !v.is_null()) {
        return super::network_edit::smoothed(s, &target, v, p, cmd);
    }
    if let Some(v) = p.get("delete_face").filter(|v| !v.is_null()) {
        return super::network_edit::deleted_face(s, &target, v, p, cmd);
    }
    if let Some(v) = p.get("weld").filter(|v| !v.is_null()) {
        return super::network_edit::welded_corner(s, &target, v, p, cmd);
    }
    if let Some(v) = p.get("keep_half").filter(|v| !v.is_null()) {
        return super::network_edit::kept_half(s, &target, v, cmd);
    }
    // A smoothing brush dabbed on the surface.
    if let Some(v) = p.get("brush").filter(|v| !v.is_null()) {
        if ["points", "moves", "hold", "net", "slide", "dots", "faces", "crease", "uncrease"].iter().any(|k| given(k))
            || str_(p, "refit").is_some_and(|r| r != "none")
        {
            return Err(bad(
                cmd,
                "`brush` smooths points on the surface: give it alone, without points, moves, hold, net, slide, dots, faces, crease, uncrease or a refit",
            ));
        }
        return brushed_points(s, &target, v, p, cmd);
    }
    // Shared edges creased or made smooth again.
    if given("crease") || given("uncrease") {
        if ["points", "moves", "hold", "net", "slide", "dots", "faces"].iter().any(|k| given(k)) || str_(p, "refit").is_some_and(|r| r != "none") {
            return Err(bad(
                cmd,
                "`crease` and `uncrease` name shared edges of a Free Form: give them without points, moves, hold, net, slide, dots, faces or a refit",
            ));
        }
        return creased_edges(s, &target, p, cmd);
    }
    // Every face as it now is: a network as the dialog edited it, faces added.
    if let Some(v) = p.get("faces").filter(|v| !v.is_null()) {
        if ["points", "moves", "hold", "net", "slide", "dots"].iter().any(|k| given(k)) || str_(p, "refit").is_some_and(|r| r != "none") {
            return Err(bad(cmd, "`faces` gives every face as it is: give it alone"));
        }
        let made = faces_param(v, cmd)?;
        // The edges creased or held smooth by hand among them, as given, else as the design keeps them.
        let joins = match p.get("kept_edges").filter(|v| !v.is_null()) {
            Some(k) => {
                let creases = super::network_edit::edge_list(k, "creases", cmd)?;
                let smooths = super::network_edit::edge_list(k, "smooths", cmd)?;
                solidcraft_scan::network::joins_keeping(&made, &creases, &smooths, solidcraft_scan::network::ends_tolerance(&made))
                    .map_err(|e| bad(cmd, e.to_string()))?
            }
            None => super::network_edit::joins_for(s, &target, &made, cmd)?,
        };
        // The symmetry plane kept with them: as given (`none` drops it), else as kept, while the
        // faces are still a mirror image in it.
        let (plane, named) = super::network_edit::wanted_symmetry(s, &target, p, cmd)?;
        let symmetry = match plane {
            Some(pl) if super::network_edit::pairs_in(&made, &joins, pl).is_some() => named.then_some(Some(pl)),
            Some(_) if named => return Err(bad(cmd, format!("`symmetry`: {}", solidcraft_scan::symmetric::MirrorError::NotMirror))),
            Some(_) => Some(None),
            None => named.then_some(None),
        };
        write_faces(s, &target, &made, cmd)?;
        super::network_edit::keep(s, &target, &made, &joins, symmetry);
        return faces_reply(s, &target, &made, &joins, 0, cmd);
    }
    if let Some(list) = p.get("dots").filter(|v| !v.is_null()) {
        if ["points", "moves", "hold", "net", "slide"].iter().any(|k| given(k)) || str_(p, "refit").is_some_and(|r| r != "none") {
            return Err(bad(cmd, "`dots` moves points on the surface: give it without points, moves, hold, net, slide or a refit"));
        }
        return moved_dots(s, &target, list, p, cmd);
    }
    if given("symmetry") {
        return Err(bad(
            cmd,
            "`symmetry` goes with an edit of a Free Form of several faces (`dots`, `faces`, `brush`, `smooth`, `crease`, `delete_face`, `weld`): the plane it is kept a mirror image in",
        ));
    }
    if target.is_network() {
        return Err(bad(
            cmd,
            format!(
                "`{}` is a Free Form of {} faces: move points on it with `dots`, or give every face with `faces`",
                target.body,
                target.faces.len()
            ),
        ));
    }
    // A whole new net: the surface as it is sent, nothing else done to it.
    if let Some(v) = p.get("net").filter(|v| !v.is_null()) {
        if ["points", "moves", "hold"].iter().any(|k| p.get(*k).is_some_and(|v| !v.is_null())) || str_(p, "refit").is_some_and(|r| r != "none") {
            return Err(bad(cmd, "`net` replaces the whole net: give it alone, without points, moves, hold or a refit"));
        }
        let made = net_param(v, cmd)?;
        write_back(s, &target, &made, cmd)?;
        let reply = ScanEditSurfaceResult {
            surface: target.body,
            controls: made.count(),
            moved: 0,
            carried: 0,
            refitted: 0,
            scan: None,
            points: None,
            rms: None,
            max: None,
            faces: None,
            creases: None,
            sewn: None,
            feature: target.feature,
            param_warnings: Vec::new(),
        };
        return serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()));
    }
    let count = target.surface.count();
    let mut net = surface_edit::points(&target.surface);
    let mut moved = vec![false; net.len()];
    let falloff = signed_length(s, p, "falloff", cmd)?.unwrap_or(0.0);
    if falloff < 0.0 {
        return Err(bad(cmd, "`falloff` must be a length of 0 or more"));
    }
    let slide = p.get("slide").filter(|v| !v.is_null());
    // The whole net, as a dialog that previewed it sends it.
    if let Some(all) = p.get("points").filter(|v| !v.is_null()) {
        if falloff > 0.0 || slide.is_some() {
            return Err(bad(cmd, "`points` gives the whole net as it goes: give it without `falloff` or `slide`"));
        }
        let all = all.as_array().ok_or_else(|| bad(cmd, "`points` must be a list of points [x, y, z]"))?;
        if all.len() != net.len() {
            return Err(bad(cmd, format!("`points` must hold all {} control points ({} × {}, u fastest)", net.len(), count[0], count[1])));
        }
        for (k, v) in all.iter().enumerate() {
            let q = crate::params::vec3(v).ok_or_else(|| bad(cmd, format!("`points[{k}]` must be a point [x, y, z] of numbers")))?;
            moved[k] = q.dist(net[k]) > 1e-9;
            net[k] = q;
        }
    }
    // The points held when refitting, and never carried by a falloff.
    let mut hold_flags = vec![false; net.len()];
    if let Some(hold) = p.get("hold").filter(|v| !v.is_null()) {
        let hold = hold.as_array().ok_or_else(|| bad(cmd, "`hold` must be a list of [u, v]"))?;
        for (k, h) in hold.iter().enumerate() {
            hold_flags[index(h, count, cmd, &format!("hold[{k}]"))?] = true;
        }
    }
    // Moves of single points, the points round them carried along with a falloff.
    let mut carried = vec![false; net.len()];
    if let Some(moves) = p.get("moves").filter(|v| !v.is_null()) {
        if slide.is_some() {
            return Err(bad(cmd, "give `moves` or `slide`, not both"));
        }
        let moves = moves.as_array().ok_or_else(|| bad(cmd, "`moves` must be a list of {point: [u, v], to: [x, y, z]} or {point, along}"))?;
        let mut list: Vec<(usize, Vec3)> = Vec::with_capacity(moves.len());
        for (k, m) in moves.iter().enumerate() {
            let at = index(m.get("point").unwrap_or(&Value::Null), count, cmd, &format!("moves[{k}].point"))?;
            let to = match (m.get("to").filter(|v| !v.is_null()), m.get("along").filter(|v| !v.is_null())) {
                (Some(to), None) => {
                    crate::params::vec3(to).ok_or_else(|| bad(cmd, format!("`moves[{k}].to` must be a point [x, y, z] of numbers")))?
                }
                (None, Some(_)) => {
                    let e = crate::params::expr(m, "along").ok_or_else(|| bad(cmd, format!("`moves[{k}].along` must be a length")))?;
                    let d = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("moves[{k}].along: {e}")))?;
                    if !d.is_finite() {
                        return Err(bad(cmd, format!("`moves[{k}].along` must be a finite length")));
                    }
                    let n = surface_edit::normal_at(&target.surface, at).ok_or_else(|| bad(cmd, "the surface has no direction at that point"))?;
                    net[at] + n * d
                }
                _ => return Err(bad(cmd, format!("`moves[{k}]` needs one of `to` (a point) or `along` (a length along the surface's normal)"))),
            };
            list.retain(|(i, _)| *i != at);
            list.push((at, to));
        }
        let (made, by) = surface_edit::with_falloff(&net, &list, &hold_flags, falloff);
        net = made;
        carried = by;
        for (at, _) in &list {
            moved[*at] = true;
        }
    }
    // Points slid along the surface, the points round them carried with a falloff.
    if let Some(slide) = slide {
        let points = slide
            .get("points")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| bad(cmd, "`slide.points` must list the control points [u, v] to slide"))?;
        let mut chosen = Vec::with_capacity(points.len());
        for (k, v) in points.iter().enumerate() {
            chosen.push(index(v, count, cmd, &format!("slide.points[{k}]"))?);
        }
        let along = [signed_length(s, slide, "u", cmd)?.unwrap_or(0.0), signed_length(s, slide, "v", cmd)?.unwrap_or(0.0)];
        if along == [0.0, 0.0] {
            return Err(bad(cmd, "`slide` needs a length along `u` or `v` (or both)"));
        }
        let steps = surface_edit::slide_steps(&target.surface, along);
        let mut weights = surface_edit::falloff_weights(&net, &chosen, &hold_flags, falloff);
        for &c in &chosen {
            // A point chosen slides all the way, unless it is held.
            weights[c] = if hold_flags[c] { 0.0 } else { 1.0 };
        }
        let slid = surface_edit::slid(&target.surface, &weights, |i| steps.get(i).copied().unwrap_or((0.0, 0.0)))
            .ok_or_else(|| bad(cmd, "the surface could not be evaluated to slide its points"))?;
        for (k, (q, w)) in slid.into_iter().zip(&weights).enumerate() {
            if *w > 0.0 && q.dist(net[k]) > 1e-12 {
                net[k] = q;
                if chosen.contains(&k) { moved[k] = true } else { carried[k] = true }
            }
        }
    }
    // Held when refitting: the points moved, those held and those a falloff carried.
    let held: Vec<bool> = moved.iter().zip(&hold_flags).zip(&carried).map(|((m, h), c)| *m || *h || *c).collect();
    let refit = str_(p, "refit").unwrap_or("none");
    let free: Vec<bool> = match refit {
        "none" => vec![false; net.len()],
        "all" => held.iter().map(|h| !h).collect(),
        "around" => {
            // The ring of two round each point moved or held, but not those points themselves.
            let mut near = vec![false; net.len()];
            for (k, _) in held.iter().enumerate().filter(|(_, h)| **h) {
                for (n, r) in surface_edit::around(count, k % count[0], k / count[0], 2).into_iter().enumerate() {
                    near[n] |= r;
                }
            }
            near.iter().zip(&held).map(|(n, h)| *n && !h).collect()
        }
        other => return Err(bad(cmd, format!("`refit` must be none, around or all, not `{other}`"))),
    };
    if !moved.iter().any(|m| *m) && !carried.iter().any(|c| *c) && !free.iter().any(|f| *f) {
        return Err(bad(cmd, "nothing to change: move a control point, or refit"));
    }
    let edited = surface_edit::with_points(&target.surface, &net).ok_or_else(|| bad(cmd, "a control point is not a finite place"))?;
    let reach = length(s, p, "reach", default_reach(&target.surface), cmd)?;
    let wants_scan = free.iter().any(|f| *f) || p.get("scan").is_some_and(|v| !v.is_null());
    let under = if wants_scan || s.model.state().bodies.iter().any(|b| b.body.is_mesh()) {
        match covered(s, str_(p, "scan"), &target.surface, reach, cmd) {
            Ok(c) => Some(c),
            Err(e) if wants_scan => return Err(e),
            Err(_) => None,
        }
    } else {
        None
    };
    let result = if free.iter().any(|f| *f) {
        let points = under.as_ref().map(|c| c.points.clone()).unwrap_or_default();
        if points.is_empty() {
            return Err(bad(cmd, "no scan point lies under the surface to refit it to (a larger reach takes in more)"));
        }
        let held: Vec<bool> = free.iter().map(|f| !f).collect();
        surface_edit::refit(&edited, &points, &held).ok_or_else(|| bad(cmd, "the points to refit have no scan under them"))?
    } else {
        edited
    };
    write_back(s, &target, &result, cmd)?;
    let (rms, max) = match &under {
        Some(c) if !c.points.is_empty() => {
            let (r, m) = surface_edit::distance(&result, &c.points);
            (Some(r), Some(m))
        }
        _ => (None, None),
    };
    let reply = ScanEditSurfaceResult {
        surface: target.body,
        controls: count,
        moved: moved.iter().filter(|m| **m).count(),
        carried: carried.iter().filter(|c| **c).count(),
        refitted: free.iter().filter(|f| **f).count(),
        scan: under.as_ref().map(|c| c.scan.clone()),
        points: under.as_ref().map(|c| c.points.len()),
        rms,
        max,
        faces: None,
        creases: None,
        sewn: None,
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// How a fitted surface is wrong, for a reader: "128 folds, 722 crossings" (only the kinds found).
pub fn trouble_text([folds, pinches, crossings]: [usize; 3]) -> String {
    [(folds, "fold", "folds"), (pinches, "pinch", "pinches"), (crossings, "crossing", "crossings")]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What to tell someone about a surface a fit made that folds, pinches or passes through itself
/// (by Surface Check): `None` when it does none of them.
pub fn fold_warning(trouble: [usize; 3]) -> Option<String> {
    (trouble.iter().sum::<usize>() > 0).then(|| {
        format!(
            "the surface folds over itself ({}): the region is too intricate for one surface; take a smaller region (more control points do not stop it folding)",
            trouble_text(trouble)
        )
    })
}

/// The surface maths a dialog previews an edit with, as the command does it.
pub use solidcraft_scan::surface_edit::{
    around, distance, falloff_weight, falloff_weights, foot, greville_places, normal_at, points as control_points, refit, slid, slide_steps,
    with_falloff, with_points,
};

/// Surface Check on a surface as a dialog shows it while it is edited, and the grid Surface Check
/// itself uses (a dialog checks on it too, so the two count the same).
pub use solidcraft_scan::surface_check::{Check as SurfaceCheck, GRID as SURFACE_CHECK_GRID, Trouble, check_whole as check_surface};

/// A line of a net from `{row: j}` or `{column: i}`.
fn line_param(v: &Value, cmd: &str) -> Result<Line> {
    let n = |key: &str| v.get(key).filter(|x| !x.is_null()).map(|x| x.as_u64().and_then(|n| usize::try_from(n).ok()));
    match (n("row"), n("column")) {
        (Some(Some(j)), None) => Ok(Line::Row(j)),
        (None, Some(Some(i))) => Ok(Line::Column(i)),
        _ => Err(bad(cmd, "`line` must be {row: j} or {column: i}, a whole number from 0")),
    }
}

/// A line as a reply: `{row: j}` or `{column: i}`.
pub fn line_reply(line: Line) -> Value {
    match line {
        Line::Row(j) => json!({ "row": j }),
        Line::Column(i) => json!({ "column": i }),
    }
}

pub(super) fn edit_net(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.edit_net";
    let op = str_(p, "op").ok_or_else(|| bad(cmd, "`op` must be refine, split, duplicate, remove, crease, uncrease or extend"))?;
    let line = match p.get("line").filter(|v| !v.is_null()) {
        Some(v) => Some(line_param(v, cmd)?),
        None => None,
    };
    let need = |l: Option<Line>| l.ok_or_else(|| bad(cmd, format!("`{op}` needs `line`: {{row: j}} or {{column: i}}")));
    let target = editable(s, str_(p, "surface"), cmd)?;
    let fail = |e: solidcraft_scan::net::NetError| bad(cmd, e.to_string());
    if target.is_network() {
        if op != "refine" || str_(p, "in").is_some_and(|i| i != "both") {
            return Err(bad(
                cmd,
                format!("`{}` is a Free Form of {} faces: its net is changed as a whole (refine, both ways)", target.body, target.faces.len()),
            ));
        }
        let made = solidcraft_scan::network::refined(&target.faces).map_err(fail)?;
        write_faces(s, &target, &made, cmd)?;
        let reply = ScanEditNetResult {
            surface: target.body,
            controls: made.first().map_or([0, 0], NurbsSurface::count),
            line: None,
            creases: Vec::new(),
            feature: target.feature,
            param_warnings: Vec::new(),
        };
        return serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()));
    }
    let (made, new_line) = match op {
        "refine" => {
            let (u, v) = match str_(p, "in").unwrap_or("both") {
                "u" => (true, false),
                "v" => (false, true),
                "both" => (true, true),
                other => return Err(bad(cmd, format!("`in` must be u, v or both, not `{other}`"))),
            };
            (net::refine(&target.surface, u, v).map_err(fail)?, None)
        }
        "split" => {
            let (m, l) = net::split(&target.surface, need(line)?).map_err(fail)?;
            (m, Some(l))
        }
        "duplicate" => {
            let after = crate::params::bool_(p, "after").unwrap_or(true);
            let (m, l) = net::duplicate(&target.surface, need(line)?, after).map_err(fail)?;
            (m, Some(l))
        }
        "remove" => (net::remove(&target.surface, need(line)?).map_err(fail)?, None),
        "crease" => (net::crease(&target.surface, need(line)?).map_err(fail)?, line),
        "uncrease" => (net::uncrease(&target.surface, need(line)?).map_err(fail)?, line),
        "extend" => {
            let distance = length(s, p, "distance", 10.0, cmd)?;
            let (m, l) = net::extend(&target.surface, need(line)?, distance).map_err(fail)?;
            (m, Some(l))
        }
        other => return Err(bad(cmd, format!("`op` must be refine, split, duplicate, remove, crease, uncrease or extend, not `{other}`"))),
    };
    write_back(s, &target, &made, cmd)?;
    let reply = ScanEditNetResult {
        surface: target.body,
        controls: made.count(),
        line: new_line.map(line_reply),
        creases: net::creases(&made).into_iter().map(line_reply).collect(),
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The net tools, as the dialog applies them to its own net (the same as `scan.edit_net`).
pub fn split_line(s: &NurbsSurface, line: Line) -> std::result::Result<(NurbsSurface, Line), NetError> {
    net::split(s, line)
}

pub fn duplicate_line(s: &NurbsSurface, line: Line, after: bool) -> std::result::Result<(NurbsSurface, Line), NetError> {
    net::duplicate(s, line, after)
}

pub fn remove_line(s: &NurbsSurface, line: Line) -> std::result::Result<NurbsSurface, NetError> {
    net::remove(s, line)
}

pub fn crease_line(s: &NurbsSurface, line: Line) -> std::result::Result<NurbsSurface, NetError> {
    net::crease(s, line)
}

pub fn uncrease_line(s: &NurbsSurface, line: Line) -> std::result::Result<NurbsSurface, NetError> {
    net::uncrease(s, line)
}

pub fn refine_net(s: &NurbsSurface) -> std::result::Result<NurbsSurface, NetError> {
    net::refine(s, true, true)
}

pub fn creases(s: &NurbsSurface) -> Vec<Line> {
    net::creases(s)
}
