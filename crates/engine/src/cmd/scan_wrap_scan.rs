//! Wrap Scan (`scan.wrap_scan`): a whole scan wrapped in one network of patches fitted to it by
//! least squares (`solidcraft_scan::wrap_scan`), for parts no exact face describes. The patches are
//! sewn into one body (a solid when the scan is closed) and kept as STEP in a Scan Surface feature,
//! so the design opens anywhere. Design: `designs/A1-wrap-and-hybrid.md` in the scan handoff.

use serde_json::{Value, json};
use solidcraft_scan::handles;
use solidcraft_scan::wrap_planes;
use solidcraft_scan::wrap_scan::{self, Settings};

use super::{CommandSpec, mesh};
use crate::params::bad;
use crate::schema::scan_wrap_scan::{ScanWrapScanParams, ScanWrapScanResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const WRAP_CONTRACT: TypedContract<ScanWrapScanParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.wrap_scan", "Wrap Scan", crate::checked!("scan.wrap_scan", wrap_scan))
    .at("SCAN", "FREEFORM")
    .icon("scan_free_form")
    .contract(&WRAP_CONTRACT)
    .output(output_schema_for::<ScanWrapScanResult>)
    .params("body?: the scan (default: what is selected on a scan, else the only one); tolerance?: length (default 0.2 mm: how far the patches may lie from the scan); max_patches?: 300–60000 (default 12000; a scan that needs more is wrapped with this many, short of the tolerance); planes?: bool (default true: where Find All reads a plane over half a percent of the scan, fitting as well as the scan's noise lets a plane, one plane face in place of the patches there, bounded by the sides of the patches round it); close_tunnels?: bool (default true: the scan's small tunnels and bridges, a scanner's through thin walls, closed on the copy wrapped); tunnel_size?: length (default 12 median edges: the widest patch a tunnel is closed in): the whole scan reduced as far as the tolerance allows (checked both ways, detail kept where it misses), three patches laid on each triangle left (four on two laid as one quad), each side and each patch fitted to the scan by least squares (so noise averages out), sewn into one body: a solid when the scan is closed, else a surface; refused when that body cannot be drawn (faces too thin for the tessellator, as across a CAD export's long rounded edges: Reconstruct is for those); kept as a surface body `Wrap1`, … (one undo step); position only across the patches' sides (G0) → body, surface, solid, patches, coarse_triangles, plane_faces, tunnels_closed, tunnel_parts, rounds, met, capped, within, p99, max (the scan's points to the patches), tolerance, warning?, feature")];

/// A plane, in place of the patches on it, needs this share of the scan's area (Find All's
/// smaller planes are a few patches' worth: no face worth having).
const PLANE_SHARE: f64 = 0.005;
/// And it must be as flat as the scan's noise lets a plane be: its fit within this many times the
/// noise Find All read (or this share of the tolerance on a mesh with none). A machined face fits
/// so; a curved patch Find All reads as a plane only within the tolerance, snapped flat, would
/// crease the surface round it for nothing (and slow its meshing a hundredfold).
const PLANE_NOISE: f64 = 2.0;
const PLANE_FLOOR: f64 = 0.01;

/// A patch's 4 × 4 control points, u fastest.
fn net_of(f: &solidcraft_geom::bspline::NurbsSurface) -> [solidcore_record::Vec3; 16] {
    let pts = solidcraft_scan::surface_edit::points(f);
    std::array::from_fn(|k| pts.get(k).copied().unwrap_or(solidcore_record::Vec3::ZERO))
}

/// A plane face: its loops of the cubic sides the patches round it share.
fn plane_face(f: &wrap_planes::PlaneFace) -> solidcraft_kernel::FaceSpec {
    let chain = |s: &[solidcore_record::Vec3; 4]| solidcraft_kernel::EdgeSpec::Chain { degree: 3, ctrl: s.to_vec().into() };
    solidcraft_kernel::FaceSpec {
        loops: f.loops.iter().map(|l| l.iter().map(chain).collect()).collect(),
        surface: solidcraft_kernel::SurfSpec::Plane,
        probe: None,
    }
}

/// A patch as a face to sew: its bicubic net and its four sides (cubic chains, counter-clockwise
/// about its natural normal), which neighbouring patches share exactly.
fn patch_face(net: &[solidcore_record::Vec3; 16]) -> solidcraft_kernel::FaceSpec {
    let net = *net;
    let side = |idx: [usize; 4]| solidcraft_kernel::EdgeSpec::Chain { degree: 3, ctrl: idx.iter().map(|&i| net[i]).collect::<Vec<_>>().into() };
    solidcraft_kernel::FaceSpec {
        loops: vec![vec![side([0, 1, 2, 3]), side([3, 7, 11, 15]), side([15, 14, 13, 12]), side([12, 8, 4, 0])]],
        surface: solidcraft_kernel::SurfSpec::Bicubic { net: Box::new(net) },
        probe: None,
    }
}

/// Most patches allowed, and the fewest a cap may ask for.
const MOST_PATCHES: u64 = 60_000;
const FEWEST_PATCHES: u64 = 300;

fn wrap_scan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.wrap_scan";
    let on = mesh(s, p, cmd)?;
    let tolerance = match crate::params::expr(p, "tolerance") {
        Some(e) => s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tolerance: {e}")))?,
        None => 0.2,
    };
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(bad(cmd, "`tolerance` must be a length above 0"));
    }
    let max_patches = match p.get("max_patches").filter(|v| !v.is_null()) {
        None => 12_000,
        Some(v) => v
            .as_u64()
            .filter(|n| (FEWEST_PATCHES..=MOST_PATCHES).contains(n))
            .ok_or_else(|| bad(cmd, format!("`max_patches` must be a whole number from {FEWEST_PATCHES} to {MOST_PATCHES}")))?
            as usize,
    };
    let settings = Settings { tolerance, max_patches, ..Settings::default() };
    let with_planes = p.get("planes").filter(|v| !v.is_null()).map_or(Ok(true), |v| v.as_bool().ok_or_else(|| bad(cmd, "`planes` must be true or false")))?;
    let scan = on.scan();
    // Find All's planes: the scan is snapped onto them, so the patches laid there lie on them.
    let regions: Vec<wrap_planes::PlaneRegion> = if with_planes {
        let found = super::reconstruct::shapes(s, &json!({"body": on.body}), cmd, false)?;
        let total: f64 = found.outcome.segments.iter().map(|g| g.area).sum();
        let flat = (PLANE_NOISE * found.outcome.noise).max(PLANE_FLOOR * tolerance);
        found
            .outcome
            .segments
            .iter()
            .filter(|g| g.area >= PLANE_SHARE * total && g.rms <= flat)
            .filter_map(|g| match g.shape.as_ref() {
                Some(solidcraft_scan::fit::Shape::Plane { point, normal }) => {
                    Some(wrap_planes::PlaneRegion { point: *point, normal: *normal, triangles: g.triangles.clone() })
                }
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    let snapped = (!regions.is_empty()).then(|| wrap_planes::snapped(&scan, &regions, tolerance));
    let snapped_scan = snapped.as_deref().map(|positions| solidcraft_scan::Scan::new(positions, scan.triangles));
    let base = snapped_scan.as_ref().unwrap_or(&scan);
    // The scan's small tunnels closed (on the copy wrapped): they would keep the coarse mesh at full
    // resolution round them.
    let close_tunnels = p.get("close_tunnels").filter(|v| !v.is_null()).map_or(Ok(true), |v| v.as_bool().ok_or_else(|| bad(cmd, "`close_tunnels` must be true or false")))?;
    let tunnel_size = match crate::params::expr(p, "tunnel_size") {
        Some(e) => {
            let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tunnel_size: {e}")))?;
            if !(v.is_finite() && v > 0.0) {
                return Err(bad(cmd, "`tunnel_size` must be a length above 0"));
            }
            v
        }
        None => handles::DEFAULT_SIZE_EDGES * handles::median_edge(base.positions, base.triangles),
    };
    let closed = if close_tunnels { handles::close(base, &handles::Settings { max_size: tunnel_size, tolerance }) } else { None };
    let closed_scan = closed.as_ref().map(|c| solidcraft_scan::Scan::new(&c.positions, &c.triangles));
    let w = wrap_scan::wrap(closed_scan.as_ref().unwrap_or(base), &settings).map_err(|e| bad(cmd, format!("the scan could not be wrapped: {e}")))?;
    if w.faces.is_empty() {
        return Err(bad(cmd, "the scan could not be wrapped: no patch could be laid"));
    }
    // The patches share their sides exactly, so they are built straight into one body (each side
    // one edge, laid on its two patches by straight pcurves): no intersection, no gaps. The
    // patches on a plane are one plane face, bounded by the sides of the patches round it.
    let (kept, plane_faces) = wrap_planes::planes(&w.faces, &regions, scan.extent());
    let mut faces: Vec<solidcraft_kernel::FaceSpec> = kept.iter().map(patch_face).collect();
    faces.extend(plane_faces.iter().map(plane_face));
    let mut planes_note = None;
    let (sewn, plane_count) = match solidcraft_kernel::sew_network(&faces) {
        Ok(sewn) => (sewn, plane_faces.len()),
        Err(e) if !plane_faces.is_empty() => {
            planes_note = Some(format!("the plane faces could not be sewn ({e}): the wrap is patches only"));
            let all: Vec<solidcraft_kernel::FaceSpec> = w.faces.iter().map(|f| patch_face(&net_of(f))).collect();
            let sewn = solidcraft_kernel::sew_network(&all).map_err(|e| bad(cmd, format!("the {} patches could not be sewn: {e}", all.len())))?;
            (sewn, 0)
        }
        Err(e) => return Err(bad(cmd, format!("the {} patches could not be sewn: {e}", faces.len()))),
    };
    let patch_count = if plane_count > 0 { kept.len() } else { w.faces.len() };
    // A body the view and every mesh export can draw, or none: some faces (long thin ones across a
    // CAD export's rounded edges) are more than the tessellator can follow, and a body kept with
    // them would fail to show and could tie up an export.
    if let Err(e) = sewn.export_mesh(&solidcraft_io::MeshQuality::Standard.spec(sewn.size())) {
        return Err(bad(
            cmd,
            format!(
                "the wrap could not be drawn ({e}); a CAD export is built better by Reconstruct, a scan by a larger tolerance or fewer patches"
            ),
        ));
    }
    let step = solidcraft_kernel::step_export(&[&sewn], "SolidCraft").map_err(|e| bad(cmd, format!("the sewn patches could not be kept: {e}")))?;
    let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let name = (1..).map(|i| format!("Wrap{i}")).find(|n| !taken.contains(n)).unwrap_or_else(|| "Wrap".into());
    let kind = solidcraft_doc::FeatureKind::ScanSurface { file: "wrap".into(), step: step.into(), recipe: None };
    let feature = super::super::features::add_feature(s, &json!({ "body_name": name, "name": "Wrap Scan" }), kind)?;
    let solid = s.model.state().body(&name).is_some_and(|b| b.is_closed());
    let short = w.scan_to_patches.within < settings.share;
    let wanted = w.coarse.capped.then_some(w.coarse.wanted.max(w.faces.len()));
    let warning = short.then(|| {
        format!(
            "{:.1} % of the scan is within {tolerance} mm of the patches{}",
            100.0 * w.scan_to_patches.within,
            match wanted {
                Some(n) => format!(": the most patches bounded the wrap; the tolerance asks for about {n} (max_patches), or raise the tolerance"),
                None => String::new(),
            }
        )
    });
    let warning = match (warning, planes_note) {
        (Some(w), Some(n)) => Some(format!("{w}; {n}")),
        (w, n) => w.or(n),
    };
    // The verdict on the model, as scan.verdict gives it; a capped wrap's first next step is the
    // same wrap with the patches the tolerance asks for.
    let mut verdict = super::plan::verdict(s, &json!({"scan": on.body, "model": name}))?;
    if let (true, Some(n), Some(next)) = (short, wanted, verdict.get_mut("next").and_then(Value::as_array_mut)) {
        let most = n.min(MOST_PATCHES as usize);
        next.insert(
            0,
            json!({"command": cmd, "why": format!("the most patches held the wrap back: about {n} meet the tolerance"), "params": {"body": on.body, "tolerance": tolerance, "max_patches": most}}),
        );
        next.truncate(3);
    }
    let reply = ScanWrapScanResult {
        body: on.body,
        surface: name,
        solid,
        patches: patch_count,
        coarse_triangles: w.coarse.triangle_count(),
        plane_faces: plane_count,
        tunnels_closed: closed.as_ref().map_or(0, |c| c.handles_before.saturating_sub(c.handles_after)),
        tunnel_parts: closed.as_ref().map_or(0, |c| c.parts),
        rounds: w.coarse.rounds,
        met: w.coarse.met,
        capped: w.coarse.capped,
        within: w.scan_to_patches.within,
        p99: w.scan_to_patches.p99,
        max: w.scan_to_patches.max,
        tolerance,
        patches_wanted: wanted,
        warning,
        verdict,
        feature,
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
