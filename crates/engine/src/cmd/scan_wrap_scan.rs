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
    .params("body?: the scan (default: what is selected on a scan, else the only one); tolerance?: length (default 0.2 mm: how far the patches may lie from the scan); max_patches?: 300–60000 (default 12000; a scan that needs more is wrapped with this many, short of the tolerance): the whole scan reduced as far as the tolerance allows (checked both ways, detail kept where it misses), three patches laid on each triangle left, each side and each patch fitted to the scan by least squares (so noise averages out), sewn into one body: a solid when the scan is closed, else a surface; refused when that body cannot be drawn (faces too thin for the tessellator, as across a CAD export's long rounded edges: Reconstruct is for those); kept as a surface body `Wrap1`, … (one undo step); position only across the patches' sides (G0) → body, surface, solid, patches, coarse_triangles, rounds, met, capped, within, p99, max (the scan's points to the patches), tolerance, warning?, feature")];

/// Patches sewn at a time: the kernel keeps every sheet it is given as a shell of its own until it
/// sews them, and a record holds at most 1,024 shells. The wrap gives its patches in an order that
/// keeps neighbours together, so each run is one connected piece; the runs are then sewn together.
const RUN: usize = 512;

/// The sheets sewn into one body, a run of [`RUN`] at a time, then the runs (again in runs while
/// there are more than that).
fn sewn_in_runs(sheets: &[solidcraft_kernel::Body]) -> solidcraft_kernel::Result<solidcraft_kernel::Body> {
    let mut level: Vec<solidcraft_kernel::Body> = Vec::new();
    for run in sheets.chunks(RUN) {
        let refs: Vec<_> = run.iter().collect();
        level.push(solidcraft_kernel::stitch(&refs, 0.0)?);
    }
    while level.len() > 1 {
        let mut next = Vec::new();
        for run in level.chunks(RUN) {
            let refs: Vec<_> = run.iter().collect();
            next.push(if refs.len() == 1 { run[0].clone() } else { solidcraft_kernel::stitch(&refs, 0.0)? });
        }
        level = next;
    }
    level.pop().ok_or_else(|| solidcraft_kernel::KernelError::Invalid("nothing to sew".into()))
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
    let sheets: Vec<_> = w
        .faces
        .iter()
        .map(|f| solidcraft_kernel::nurbs_sheet(f).map_err(|e| bad(cmd, format!("a patch could not be made into a face: {e}"))))
        .collect::<Result<_>>()?;
    let sewn = sewn_in_runs(&sheets).map_err(|e| bad(cmd, format!("the {} patches could not be sewn: {e}", sheets.len())))?;
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
    let warning = short.then(|| {
        format!(
            "{:.1} % of the scan is within {tolerance} mm of the patches{}",
            100.0 * w.scan_to_patches.within,
            if w.coarse.capped { ": the most patches bounded the wrap; raise max_patches or the tolerance" } else { "" }
        )
    });
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
        warning,
        feature,
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
