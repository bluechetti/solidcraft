//! Scan to CAD (#8615): the tools of the Scan workspace. Placing a scan (standing it on a base,
//! resting it on the grid, centring it, laying it along its own axes, squaring it to its mirror
//! plane) works on a mesh body and is kept as an ordinary Move feature, and a mirror plane found
//! is kept as an ordinary construction plane, so a design opens anywhere. The tools not built yet are
//! registered with their place, icon and parameters so the tab shows its whole layout; the tab's
//! other working tools (Insert Mesh, Insert Point Cloud, Convert Mesh, Fit Curves to Mesh Section,
//! Section Analysis, Compare Bodies) live with their own areas.

use serde_json::{Value, json};
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::Scan;
use solidcraft_scan::base::{self, Base};
use solidcraft_scan::fit::{self, Fitted, Kind, Shape};
use solidcraft_scan::place::{self, Rigid};
use solidcraft_scan::symmetry::{self, Guide, Mirror};

use super::CommandSpec;
use crate::params::{bad, str_};
use crate::schema::scan::{
    ScanBase, ScanBasesParams, ScanBasesResult, ScanDeviationParams, ScanDeviationResult, ScanFitParams, ScanFitResult, ScanFitSurfaceParams,
    ScanFitSurfaceResult, ScanFittedCylinder, ScanFittedPlane, ScanFittedSphere, ScanMirror, ScanPlaceParams, ScanPlaceResult, ScanSquareParams,
    ScanStandParams, ScanSymmetryParams, ScanSymmetryResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};
use solidcore_record::Vec3;

const BASES_CONTRACT: TypedContract<ScanBasesParams> = TypedContract::new();
const PLACE_CONTRACT: TypedContract<ScanPlaceParams> = TypedContract::new();
const STAND_CONTRACT: TypedContract<ScanStandParams> = TypedContract::new();
const SYMMETRY_CONTRACT: TypedContract<ScanSymmetryParams> = TypedContract::new();
const SQUARE_CONTRACT: TypedContract<ScanSquareParams> = TypedContract::new();
const FIT_CONTRACT: TypedContract<ScanFitParams> = TypedContract::new();
const DEVIATION_CONTRACT: TypedContract<ScanDeviationParams> = TypedContract::new();
const SURFACE_CONTRACT: TypedContract<ScanFitSurfaceParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    planned!("scan.repair", "Repair Mesh", "8615")
        .at("SCAN", "CLEAN")
        .icon("mesh_body")
        .params("planned: body: mesh body name (fixes flipped normals, non-manifold edges, slivers and duplicate triangles) → repaired counts"),
    planned!("scan.fill_holes", "Fill Holes", "8615")
        .at("SCAN", "CLEAN")
        .icon("surface_patch")
        .params("planned: body: mesh body name; max_edges?: largest hole to fill, in boundary edges (default 300); fair?: bool (default true) → filled, left_open"),
    planned!("scan.reduce", "Reduce Mesh", "8615")
        .at("SCAN", "CLEAN")
        .icon("scale")
        .params("planned: body: mesh body name; ratio?: share of triangles kept (0…1) or triangles?: count; max_error?: mm → triangles, mean_error, max_error"),
    planned!("scan.remesh", "Remesh", "8615")
        .at("SCAN", "CLEAN")
        .icon("grid")
        .params("planned: body: mesh body name; edge_length?: mm (default from the scan's spacing) → triangles, deviation"),
    planned!("scan.mesh_from_points", "Mesh from Points", "8615")
        .at("SCAN", "CLEAN")
        .icon("dots")
        .params("planned: cloud: point cloud id or name; depth?: detail (5…12, default 8); name? → body"),
    CommandSpec::new("scan.bases", "Scan Bases", crate::checked!("scan.bases", bases))
        .icon("list")
        .noundo()
        .contract(&BASES_CONTRACT)
        .output(output_schema_for::<ScanBasesResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one) → body, bases: [{kind: flat|resting|axis, normal, anchor, area, share, tip_degrees, rms}], steadiest first, at most 8"),
    CommandSpec::new("scan.stand_on_base", "Stand on Base", crate::checked!("scan.stand_on_base", stand_on_base))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&STAND_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); point?: [x,y,z] on the scan (grow the flat face there and stand on it; a curved wall is refused; default: the point picked on the scan); base?: index into scan.bases; with neither, the steadiest base, or pressed again the next one → body, moved, base, base_index, base_count, feature (the Move)"),
    CommandSpec::new("scan.rest_on_grid", "Rest on Grid", crate::checked!("scan.rest_on_grid", rest_on_grid))
        .at("SCAN", "ALIGN")
        .icon("move")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): up or down until its lowest point is at Z = 0 → body, moved, feature (the Move)"),
    CommandSpec::new("scan.center", "Center on Origin", crate::checked!("scan.center", center))
        .at("SCAN", "ALIGN")
        .icon("origin")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): sideways until the middle of its box is over the origin, its height kept → body, moved, feature (the Move)"),
    CommandSpec::new("scan.quick_align", "Quick Align", crate::checked!("scan.quick_align", quick_align))
        .at("SCAN", "ALIGN")
        .icon("axis")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): laid along its own directions, longest along X and thinnest along Z, the middle of its surface at the origin; a scan too round to have a longest direction is refused → body, moved, feature (the Move)"),
    CommandSpec::new("scan.symmetry", "Find Symmetry", crate::checked!("scan.symmetry", find_symmetry))
        .at("SCAN", "ALIGN")
        .icon("mirror")
        .contract(&SYMMETRY_CONTRACT)
        .output(output_schema_for::<ScanSymmetryResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); guide?: {from, to: [x,y,z], toward?: [x,y,z] (the way you looked, default [0,0,1])}: a line along the rough mirror plane, to find that one; keep?: bool (default true: kept as the construction plane `Symmetry Plane`, to sketch on); a scan that is no mirror image is refused → body, mirror: {point, normal, rms, coverage, symmetric}, plane (the construction plane)"),
    CommandSpec::new("scan.square_to_symmetry", "Square to Symmetry", crate::checked!("scan.square_to_symmetry", square_to_symmetry))
        .at("SCAN", "ALIGN")
        .icon("c_symmetry")
        .contract(&SQUARE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); guide?: {from, to: [x,y,z], toward?: [x,y,z]}: the smallest turn that lays its mirror plane along YZ, then sideways to X = 0, so the origin's YZ plane is the mirror plane; a scan that is no mirror image is refused → body, moved, mirror, feature (the Move)"),
    CommandSpec::new("scan.fit_plane", "Fit Plane", crate::checked!("scan.fit_plane", fit_plane))
        .at("SCAN", "FIT")
        .icon("plane")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: mm (default 0.15 % of the scan's size); keep?: bool (default true: kept as a construction plane) → body, plane: {point, normal}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_cylinder", "Fit Cylinder", crate::checked!("scan.fit_cylinder", fit_cylinder))
        .at("SCAN", "FIT")
        .icon("cylinder")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: mm (default 0.15 % of the scan's size); keep?: bool (default true: kept as a construction axis) → body, cylinder: {point, axis, radius, diameter, length}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_sphere", "Fit Sphere", crate::checked!("scan.fit_sphere", fit_sphere))
        .at("SCAN", "FIT")
        .icon("sphere")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: length (default 0.15 % of the scan's size); keep?: bool (default true: its centre kept as a construction point) → body, sphere: {center, radius, diameter}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_surface", "Fit Surface", crate::checked!("scan.fit_surface", fit_surface))
        .at("SCAN", "FREEFORM")
        .icon("surface")
        .contract(&SURFACE_CONTRACT)
        .output(output_schema_for::<ScanFitSurfaceResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan, the middle of the area (default: the point picked on it); radius?: length (default a tenth of the scan's size); controls?: 4…30 control points each way (default 8); smoothing?: 0…1 (default 0.1): a smooth B-spline surface over the area, kept as a surface body (one undo step); an area that wraps round is refused → body, surface, rms, max, points, controls, feature"),
    CommandSpec::new("scan.deviation", "Scan Deviation", crate::checked!("scan.deviation", deviation))
        .at("SCAN", "CHECK")
        .icon("curvature_map")
        .noundo()
        .contract(&DEVIATION_CONTRACT)
        .output(output_schema_for::<ScanDeviationResult>)
        .params("scan?: mesh body (default: the selected or only one); body?: the model (default: the selected or only solid or surface body); tolerance?: length (default 0.1 mm); limit?: length (default 0.5 mm); show?: bool (false takes the map away): colours the scan by its distance from the model in the viewport, a view and not an undo step → shown, scan, body, rms, mean, max_above, max_below, within, measured, unmatched, tolerance, limit"),
];

/// A move smaller than this (mm, at the scan's reach) leaves the scan where it is.
const NEGLIGIBLE: f64 = 1e-3;

/// A scan's triangles, copied out of the evaluated model so the session can then move it.
struct Mesh {
    body: String,
    positions: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
}

impl Mesh {
    fn scan(&self) -> Scan<'_> {
        Scan::new(&self.positions, &self.triangles)
    }

    /// The farthest a point of the scan is from the origin, for judging a move negligible.
    fn reach(&self) -> f64 {
        self.positions.iter().map(|p| p.len()).fold(1.0, f64::max)
    }
}

/// The mesh body a scan command works on: `body`, else the one selected mesh body, else the
/// design's only mesh body.
fn mesh(s: &Session, p: &Value, cmd: &str) -> Result<Mesh> {
    let state = s.model.state();
    let named = str_(p, "body").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let name = match named {
        Some(name) => name,
        None => {
            let mut selected: Vec<&str> = s
                .selection
                .iter()
                .filter_map(|x| match x {
                    Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .filter(|name| state.body(name).is_some_and(|b| b.body.is_mesh()))
                .collect();
            // Several points picked on one scan are one scan.
            selected.sort_unstable();
            selected.dedup();
            let meshes: Vec<&str> = state.bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.as_str()).collect();
            match (selected.as_slice(), meshes.as_slice()) {
                ([one], _) | ([], [one]) => (*one).to_owned(),
                ([], []) => return Err(bad(cmd, "the design has no scan: insert one with Insert Mesh")),
                ([], _) => return Err(bad(cmd, "select the scan (a mesh body) to work on")),
                _ => return Err(bad(cmd, "select one scan (mesh body)")),
            }
        }
    };
    let body = state.body(&name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
    let (positions, triangles) = body.body.triangle_mesh().ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    Ok(Mesh { body: name, positions: positions.to_vec(), triangles: triangles.to_vec() })
}

/// Move the scan by `by` as one Move feature; `None` when the move is negligible.
fn move_by(s: &mut Session, mesh: &Mesh, by: &Rigid) -> Result<Option<Value>> {
    if !(by.translation.is_finite() && by.rotation.iter().flatten().all(|x| x.is_finite())) {
        return Err(EngineError::Other("the move came out undefined".into()));
    }
    if by.is_negligible(NEGLIGIBLE, mesh.reach()) {
        return Ok(None);
    }
    let t = by.translation;
    let mut p = json!({"bodies": [mesh.body], "translate": [t.x, t.y, t.z]});
    if let Some((axis, angle)) = by.axis_angle() {
        p["axis"] = json!([axis.x, axis.y, axis.z]);
        p["angle"] = json!(format!("{:.12} deg", angle.to_degrees()));
    }
    let mover = super::find_command("solid.move").ok_or_else(|| EngineError::UnknownCommand("solid.move".into()))?;
    mover.run_nested(s, &p).map(Some)
}

fn base_reply(b: &Base) -> ScanBase {
    ScanBase {
        kind: b.kind.name().into(),
        normal: [b.normal.x, b.normal.y, b.normal.z],
        anchor: [b.anchor.x, b.anchor.y, b.anchor.z],
        area: b.area,
        share: b.share,
        tip_degrees: b.tip.to_degrees(),
        rms: b.rms,
    }
}

fn place_reply(body: String, base: Option<&Base>, feature: Option<Value>) -> Result<Value> {
    let reply = ScanPlaceResult {
        body,
        moved: feature.is_some(),
        base: base.map(base_reply),
        base_index: None,
        base_count: None,
        mirror: None,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn mirror_reply(m: &Mirror) -> ScanMirror {
    ScanMirror {
        point: [m.point.x, m.point.y, m.point.z],
        normal: [m.normal.x, m.normal.y, m.normal.z],
        rms: m.rms,
        coverage: m.coverage,
        symmetric: m.is_symmetric(),
    }
}

/// `guide`, if given: `{from, to, toward?}`.
fn guide(p: &Value, cmd: &str) -> Result<Option<Guide>> {
    let Some(g) = p.get("guide").filter(|v| !v.is_null()) else { return Ok(None) };
    let point = |key: &str, required: bool| -> Result<Option<Vec3>> {
        match g.get(key).filter(|v| !v.is_null()) {
            Some(v) => crate::params::vec3(v).map(Some).ok_or_else(|| bad(cmd, format!("`guide.{key}` must be a point [x, y, z] of numbers"))),
            None if required => Err(bad(cmd, format!("`guide.{key}` is required"))),
            None => Ok(None),
        }
    };
    if !g.is_object() {
        return Err(bad(cmd, "`guide` must be {from, to, toward?}"));
    }
    let (Some(from), Some(to)) = (point("from", true)?, point("to", true)?) else {
        return Err(bad(cmd, "`guide` must be {from, to, toward?}"));
    };
    Ok(Some(Guide { from, to, toward: point("toward", false)?.unwrap_or(Vec3::Z) }))
}

/// A guide from two points picked on the scan: the line between them, seen square to the scan's
/// surface there (the plane through a line drawn along the middle of a part stands up from it).
fn picked_guide(s: &Session, mesh: &Mesh) -> Option<Guide> {
    let picks: Vec<Vec3> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Face { body, point, .. } | Sel::Edge { body, point, .. } if *body == mesh.body => Some(*point),
            _ => None,
        })
        .collect();
    let (&from, &to) = (picks.first()?, picks.last()?);
    if from.dist(to) < 1e-6 {
        return None;
    }
    let scan = mesh.scan();
    let normal_at = |p: Vec3| {
        let [a, b, c] = scan.corners(base::nearest_triangle(&scan, p)?)?;
        (b - a).cross(c - a).normalized()
    };
    let (n1, n2) = (normal_at(from)?, normal_at(to)?);
    // Either winding: the two normals are made to agree before they are averaged.
    let toward = (n1 + if n1.dot(n2) < 0.0 { -n2 } else { n2 }).normalized()?;
    Some(Guide { from, to, toward })
}

/// The scan's mirror plane, near `guide` when one is given; refused when the scan is no mirror image.
fn mirror_of(mesh: &Mesh, guide: Option<&Guide>, cmd: &str) -> Result<Mirror> {
    let scan = mesh.scan();
    let found = match guide {
        Some(g) => symmetry::find_guided(&scan, g).ok_or_else(|| bad(cmd, "the guide marks no plane: draw a line with some length, not end-on"))?,
        None => symmetry::find(&scan).ok_or_else(|| bad(cmd, "the scan is too small to look for a mirror plane"))?,
    };
    if !found.is_symmetric() {
        return Err(bad(
            cmd,
            format!(
                "the scan is not a mirror image: only {:.0} % of it matches its reflection (typically {:.2} mm off); draw a guide along the plane to look near it",
                100.0 * found.coverage,
                found.rms
            ),
        ));
    }
    Ok(found)
}

fn find_symmetry(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.symmetry";
    let guide = guide(p, cmd)?;
    let keep = crate::params::bool_(p, "keep").unwrap_or(true);
    let mesh = mesh(s, p, cmd)?;
    let guide = guide.or_else(|| picked_guide(s, &mesh));
    let found = mirror_of(&mesh, guide.as_ref(), cmd)?;
    let plane = if keep {
        // Three points of the plane, a quarter of the scan apart, make an ordinary construction plane.
        let (u, v) = found.normal.any_perp().normalized().map(|u| (u, found.normal.cross(u))).unwrap_or((Vec3::X, Vec3::Y));
        let reach = (found.size / 4.0).max(1.0);
        let (a, b, c) = (found.point, found.point + u * reach, found.point + v * reach);
        let maker =
            super::find_command("construct.plane.three_points").ok_or_else(|| EngineError::UnknownCommand("construct.plane.three_points".into()))?;
        let at = |q: Vec3| json!({"point": [q.x, q.y, q.z]});
        Some(maker.run_nested(s, &json!({"a": at(a), "b": at(b), "c": at(c), "name": "Symmetry Plane"}))?)
    } else {
        None
    };
    let reply = ScanSymmetryResult { body: mesh.body, mirror: mirror_reply(&found), plane, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn square_to_symmetry(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.square_to_symmetry";
    let guide = guide(p, cmd)?;
    let mesh = mesh(s, p, cmd)?;
    let guide = guide.or_else(|| picked_guide(s, &mesh));
    let found = mirror_of(&mesh, guide.as_ref(), cmd)?;
    let by = symmetry::square_to(&found).ok_or_else(|| bad(cmd, "the mirror plane has no direction"))?;
    let feature = move_by(s, &mesh, &by)?;
    let reply = ScanPlaceResult {
        body: mesh.body,
        moved: feature.is_some(),
        base: None,
        base_index: None,
        base_count: None,
        mirror: Some(mirror_reply(&found)),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn bases(s: &mut Session, p: &Value) -> Result<Value> {
    let mesh = mesh(s, p, "scan.bases")?;
    let found = base::bases(&mesh.scan());
    let reply = ScanBasesResult { body: mesh.body, bases: found.iter().map(base_reply).collect(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn stand_on_base(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.stand_on_base";
    // The parameters first, so a malformed call is told so before anything else.
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let index = match p.get("base").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(cmd, "`base` must be a whole number from 0"))?),
        None => None,
    };
    let mesh = mesh(s, p, cmd)?;
    let scan = mesh.scan();
    // A point given or picked on the scan grows the face there; else a base by number, or the
    // next one after the base the scan was last stood on by pressing again.
    let point = point.or_else(|| if index.is_none() { picked_point(s, &mesh.body) } else { None });
    let (chosen, numbered) = match point {
        Some(point) => {
            let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
            (base::grow(&scan, seed).map_err(|e| bad(cmd, e.to_string()))?, None)
        }
        None => {
            let found = base::bases(&scan);
            let count = found.len();
            let i = match index {
                Some(i) => usize::try_from(i).unwrap_or(usize::MAX),
                // The next base while the scan still stands on the last one (nothing has moved it).
                None => match &s.scan_base_cycle {
                    Some((body, last)) if *body == mesh.body && count > 0 && stands_on(&scan, found.get(*last), mesh.reach()) => (last + 1) % count,
                    _ => 0,
                },
            };
            let base = found.get(i).copied().ok_or_else(|| bad(cmd, format!("`base` must be below {count}, the number of bases found")))?;
            (base, Some((i, count)))
        }
    };
    let stand = place::stand(&scan, chosen.normal, chosen.anchor).ok_or_else(|| bad(cmd, "that base has no direction to stand on"))?;
    let feature = move_by(s, &mesh, &stand)?;
    s.scan_base_cycle = numbered.map(|(i, _)| (mesh.body.clone(), i));
    let reply = ScanPlaceResult {
        body: mesh.body,
        moved: feature.is_some(),
        base: Some(base_reply(&chosen)),
        base_index: numbered.map(|(i, _)| i),
        base_count: numbered.map(|(_, n)| n),
        mirror: None,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// What the Stand on Base dialog shows before it stands the scan: the base and the scan's points to
/// mark on it (where the part would touch the ground, or the face grown from the pick).
#[derive(Clone, Debug)]
pub struct BasePreview {
    pub body: String,
    pub base: ScanBase,
    /// Which of the scan's bases it is, when it is not a face grown from a pick.
    pub index: Option<usize>,
    /// How many bases the scan has.
    pub count: usize,
    pub marked: Vec<[f32; 3]>,
}

/// The base `scan.stand_on_base` would stand the scan `body` on (the dialog's preview): the face
/// grown from `point` when there is one, else base number `index` (wrapped round the bases).
pub fn base_preview(s: &Session, body: Option<&str>, point: Option<Vec3>, index: usize) -> Result<BasePreview> {
    let cmd = "scan.stand_on_base";
    let mesh = mesh(s, &json!({ "body": body }), cmd)?;
    let scan = mesh.scan();
    let found = base::bases(&scan);
    let count = found.len();
    let (chosen, index, triangles) = match point {
        Some(point) => {
            let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
            let (grown, triangles) = base::grow_face(&scan, seed).map_err(|e| bad(cmd, e.to_string()))?;
            (grown, None, triangles)
        }
        None => {
            let i = if count == 0 { 0 } else { index % count };
            let chosen = *found.get(i).ok_or_else(|| bad(cmd, "the scan has no base to stand on"))?;
            (chosen, Some(i), base::touching(&scan, &chosen))
        }
    };
    // Each point once, however many of its triangles are marked.
    let mut seen = vec![false; mesh.positions.len()];
    let mut marked = Vec::new();
    for &t in &triangles {
        for &v in mesh.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), mesh.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                marked.push([p.x as f32, p.y as f32, p.z as f32]);
            }
        }
    }
    Ok(BasePreview { body: mesh.body, base: base_reply(&chosen), index, count, marked })
}

/// Whether the scan already stands on `base`: standing it there would not move it.
fn stands_on(scan: &Scan, base: Option<&Base>, reach: f64) -> bool {
    base.and_then(|b| place::stand(scan, b.normal, b.anchor)).is_some_and(|m| m.is_negligible(NEGLIGIBLE, reach))
}

/// A placing move: `how` gives the move for the scan, or `None` with the reason in `refusal`.
fn place_by(s: &mut Session, p: &Value, cmd: &str, how: fn(&Scan) -> Option<Rigid>, refusal: &str) -> Result<Value> {
    let mesh = mesh(s, p, cmd)?;
    let by = how(&mesh.scan()).ok_or_else(|| bad(cmd, refusal))?;
    let feature = move_by(s, &mesh, &by)?;
    place_reply(mesh.body, None, feature)
}

fn rest_on_grid(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.rest_on_grid", place::rest_on_grid, "the scan has no points")
}

fn center(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.center", place::center_on_grid, "the scan has no points")
}

fn quick_align(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.quick_align", place::quick_align, "the scan is too round to have a longest direction to lay along X")
}

/// A scan's deviation from a model, measured by `scan.deviation`, for the viewport to colour:
/// one value per position of the scan's mesh (see [`solidcraft_scan::deviation::Deviation`]).
/// It stands for the design as it was at `revision`.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviationMap {
    pub scan: String,
    pub body: String,
    pub revision: u64,
    pub values: Vec<f32>,
    pub tolerance: f64,
    pub limit: f64,
    pub rms: f64,
    pub within: f64,
}

impl DeviationMap {
    /// Whether it still stands for the design: nothing has changed since it was measured.
    pub fn current(&self, s: &Session) -> bool {
        self.revision == s.revision
    }

    /// Where a point's value sits on the colour scale, -1 to 1 (0 within the tolerance); `None` for
    /// a point the model leaves out.
    pub fn scale(&self, value: f32) -> Option<f64> {
        solidcraft_scan::deviation::scale(value, self.tolerance, self.limit)
    }
}

/// A length parameter in mm, or `default`; above 0.
fn length(s: &Session, p: &Value, key: &str, default: f64, cmd: &str) -> Result<f64> {
    let Some(e) = crate::params::expr(p, key) else { return Ok(default) };
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !(v.is_finite() && v > 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a length above 0")));
    }
    Ok(v)
}

fn deviation(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.deviation";
    let reply = |r: ScanDeviationResult| serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()));
    let empty = ScanDeviationResult {
        shown: false,
        scan: None,
        body: None,
        rms: None,
        mean: None,
        max_above: None,
        max_below: None,
        within: None,
        measured: None,
        unmatched: None,
        tolerance: None,
        limit: None,
        param_warnings: Vec::new(),
    };
    if crate::params::bool_(p, "show") == Some(false) {
        s.scan_deviation = None;
        return reply(empty);
    }
    let tolerance = length(s, p, "tolerance", 0.1, cmd)?;
    let limit = length(s, p, "limit", 0.5, cmd)?;
    if !(limit > tolerance) {
        return Err(bad(cmd, "`limit` must be more than `tolerance`"));
    }
    let scan_name = str_(p, "scan").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let mesh = mesh(s, &json!({ "body": scan_name }), cmd)?;
    let state = s.model.state();
    let model_name = match str_(p, "body").filter(|b| !b.trim().is_empty()) {
        Some(name) => name.to_owned(),
        None => {
            let solid = |name: &str| state.body(name).is_some_and(|b| !b.body.is_mesh());
            let selected: Vec<&str> = s
                .selection
                .iter()
                .filter_map(|x| match x {
                    Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .filter(|n| solid(n))
                .collect();
            let all: Vec<&str> = state.bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.as_str()).collect();
            match (selected.as_slice(), all.as_slice()) {
                ([one], _) | ([], [one]) => (*one).to_owned(),
                ([], []) => return Err(bad(cmd, "the design has no model to measure the scan against: build one first")),
                _ => return Err(bad(cmd, "select the model (a solid or surface body) to measure the scan against")),
            }
        }
    };
    let model = state.body(&model_name).ok_or_else(|| bad(cmd, format!("no body `{model_name}`")))?;
    if model.body.is_mesh() {
        return Err(bad(cmd, format!("`{model_name}` is a mesh: measure the scan against a solid or surface body")));
    }
    // Meshed within a quarter of the tolerance, as Compare Bodies does, so the mesh agrees with
    // the surface well within what is measured.
    let surface = model.body.mesh(&solidcraft_kernel::MeshTolerance::chord(tolerance / 4.0)).map_err(|e| bad(cmd, format!("`{model_name}`: {e}")))?;
    let d = solidcraft_scan::deviation::measure(&mesh.positions, &surface, tolerance, 4.0 * limit);
    if d.measured == 0 {
        return Err(bad(cmd, format!("the scan is nowhere near `{model_name}`: no point is within {:.3} mm of it", 4.0 * limit)));
    }
    s.scan_deviation = Some(std::sync::Arc::new(DeviationMap {
        scan: mesh.body.clone(),
        body: model_name.clone(),
        revision: s.revision,
        values: d.values,
        tolerance,
        limit,
        rms: d.rms,
        within: d.within,
    }));
    reply(ScanDeviationResult {
        shown: true,
        scan: Some(mesh.body),
        body: Some(model_name),
        rms: Some(d.rms),
        mean: Some(d.mean),
        max_above: Some(d.max_above),
        max_below: Some(d.max_below),
        within: Some(d.within),
        measured: Some(d.measured),
        unmatched: Some(d.unmatched),
        tolerance: Some(tolerance),
        limit: Some(limit),
        ..empty
    })
}

fn fit_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fit_surface";
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let controls = match p.get("controls").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| (4..=30).contains(n)).ok_or_else(|| bad(cmd, "`controls` must be a whole number from 4 to 30"))? as usize,
        None => 8,
    };
    let smoothing = match p.get("smoothing").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`smoothing` must be a number from 0 to 1"))?,
        None => 0.1,
    };
    let mesh = mesh(s, p, cmd)?;
    let scan = mesh.scan();
    let radius = length(s, p, "radius", (scan.extent() / 10.0).max(1e-3), cmd)?;
    let point =
        point.or_else(|| picked_point(s, &mesh.body)).ok_or_else(|| bad(cmd, "pick a point on the scan first, in the middle of the area to fit"))?;
    let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = solidcraft_scan::surface::fit_surface(&scan, seed, radius, controls, smoothing).map_err(|e| bad(cmd, e.to_string()))?;
    // The surface as a sheet, kept as STEP in an Import feature: the design reopens anywhere.
    let ts = solidcraft_geom::tspline::TSpline::from_nurbs(&fitted.surface).map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let sheet = solidcraft_kernel::tspline_sheet(&ts).map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let step = solidcraft_kernel::step_export(&[&sheet], "SolidCraft").map_err(|e| bad(cmd, format!("the surface: {e}")))?;
    let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let name = (1..).map(|i| format!("Surface{i}")).find(|n| !taken.contains(n)).unwrap_or_else(|| "Surface".into());
    let kind = solidcraft_doc::FeatureKind::Import {
        file: format!("{} (fitted surface)", mesh.body),
        step: step.into(),
        components: Vec::new(),
        skip: Vec::new(),
    };
    let feature = super::features::add_feature(s, &json!({ "body_name": name, "name": "Fit Surface" }), kind)?;
    let reply = ScanFitSurfaceResult {
        body: mesh.body,
        surface: name,
        rms: fitted.rms,
        max: fitted.max,
        points: fitted.points,
        controls,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The point picked on the scan `body`: a picked face or edge of it.
fn picked_point(s: &Session, body: &str) -> Option<Vec3> {
    s.selection.iter().rev().find_map(|x| match x {
        Sel::Face { body: b, point, .. } | Sel::Edge { body: b, point, .. } if b == body => Some(*point),
        _ => None,
    })
}

fn fit_plane(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_plane", Kind::Plane)
}

fn fit_cylinder(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_cylinder", Kind::Cylinder)
}

fn fit_sphere(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_sphere", Kind::Sphere)
}

fn fit_shape(s: &mut Session, p: &Value, cmd: &str, kind: Kind) -> Result<Value> {
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let tolerance = match crate::params::expr(p, "tolerance") {
        Some(e) => {
            let t = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tolerance: {e}")))?;
            if !(t.is_finite() && t > 0.0) {
                return Err(bad(cmd, "`tolerance` must be a length above 0"));
            }
            Some(t)
        }
        None => None,
    };
    let keep = crate::params::bool_(p, "keep").unwrap_or(true);
    let mesh = mesh(s, p, cmd)?;
    let point = point.or_else(|| picked_point(s, &mesh.body)).ok_or_else(|| bad(cmd, "pick a point on the scan first, on the face to fit"))?;
    let scan = mesh.scan();
    let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = fit::fit(&scan, seed, kind, tolerance).map_err(|e| bad(cmd, e.to_string()))?;
    let feature = if keep { Some(keep_shape(s, &fitted, mesh.scan().extent())?) } else { None };
    let at = |v: Vec3| [v.x, v.y, v.z];
    let (mut plane, mut cylinder, mut sphere) = (None, None, None);
    match fitted.shape {
        Shape::Plane { normal, .. } => plane = Some(ScanFittedPlane { point: at(fitted.middle), normal: at(normal) }),
        Shape::Cylinder { axis, radius, .. } => {
            cylinder = Some(ScanFittedCylinder {
                point: at(fitted.middle),
                axis: at(axis),
                radius,
                diameter: 2.0 * radius,
                length: 2.0 * fitted.half_length,
            })
        }
        Shape::Sphere { center, radius } => sphere = Some(ScanFittedSphere { center: at(center), radius, diameter: 2.0 * radius }),
    }
    let reply = ScanFitResult {
        body: mesh.body,
        plane,
        cylinder,
        sphere,
        triangles: fitted.triangles.len(),
        area: fitted.area,
        rms: fitted.rms,
        max: fitted.max,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Keeps a fitted shape as an ordinary construction feature: a plane through three of its points,
/// an axis through two, or a sphere's centre as a point.
fn keep_shape(s: &mut Session, fitted: &Fitted, extent: f64) -> Result<Value> {
    let at = |q: Vec3| json!({"point": [q.x, q.y, q.z]});
    let run = |s: &mut Session, id: &str, p: Value| -> Result<Value> {
        super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
    };
    let reach = (extent / 8.0).max(1.0);
    match fitted.shape {
        Shape::Plane { normal, .. } => {
            let u = normal.any_perp().normalized().unwrap_or(Vec3::X);
            let v = normal.cross(u);
            let m = fitted.middle;
            run(s, "construct.plane.three_points", json!({"a": at(m), "b": at(m + u * reach), "c": at(m + v * reach)}))
        }
        Shape::Sphere { center, .. } => run(s, "construct.point.vertex", json!({"point": at(center)})),
        Shape::Cylinder { axis, .. } => {
            let half = fitted.half_length.max(reach / 2.0);
            run(s, "construct.axis.two_points", json!({"a": at(fitted.middle - axis * half), "b": at(fitted.middle + axis * half)}))
        }
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
