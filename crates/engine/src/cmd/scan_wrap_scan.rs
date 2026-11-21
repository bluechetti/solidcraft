//! Wrap Scan (`scan.wrap_scan`): a whole scan wrapped in one network of patches fitted to it by
//! least squares (`solidcraft_scan::wrap_scan`), for parts no exact face describes. The patches are
//! sewn into one body (a solid when the scan is closed) and kept as STEP in a Scan Surface feature,
//! so the design opens anywhere. Design: `designs/A1-wrap-and-hybrid.md` in the scan handoff.

use serde_json::{Value, json};
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
    .params("body?: the scan (default: what is selected on a scan, else the only one); tolerance?: length (default 0.2 mm: how far the patches may lie from the scan); max_patches?: 300–60000 (default 12000; a scan that needs more is wrapped with this many, short of the tolerance): the whole scan reduced as far as the tolerance allows (checked both ways, detail kept where it misses), three patches laid on each triangle left (four on two laid as one quad), each side and each patch fitted to the scan by least squares (so noise averages out), sewn into one body: a solid when the scan is closed, else a surface; refused when that body cannot be drawn (faces too thin for the tessellator, as across a CAD export's long rounded edges: Reconstruct is for those); kept as a surface body `Wrap1`, … (one undo step); position only across the patches' sides (G0) → body, surface, solid, patches, coarse_triangles, rounds, met, capped, within, p99, max (the scan's points to the patches), tolerance, warning?, feature")];

/// A patch as a face to sew: its bicubic net and its four sides (cubic chains, counter-clockwise
/// about its natural normal), which neighbouring patches share exactly.
fn patch_face(f: &solidcraft_geom::bspline::NurbsSurface) -> solidcraft_kernel::FaceSpec {
    let pts = solidcraft_scan::surface_edit::points(f);
    let net: [solidcore_record::Vec3; 16] = std::array::from_fn(|k| pts.get(k).copied().unwrap_or(solidcore_record::Vec3::ZERO));
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
    let w = wrap_scan::wrap(&on.scan(), &settings).map_err(|e| bad(cmd, format!("the scan could not be wrapped: {e}")))?;
    if w.faces.is_empty() {
        return Err(bad(cmd, "the scan could not be wrapped: no patch could be laid"));
    }
    // The patches share their sides exactly, so they are built straight into one body (each side
    // one edge, laid on its two patches by straight pcurves): no intersection, no gaps.
    let faces: Vec<solidcraft_kernel::FaceSpec> = w.faces.iter().map(patch_face).collect();
    let sewn = solidcraft_kernel::sew_network(&faces).map_err(|e| bad(cmd, format!("the {} patches could not be sewn: {e}", faces.len())))?;
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
        patches: w.faces.len(),
        coarse_triangles: w.coarse.triangle_count(),
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
