//! Fit Patches (`scan.fit_patches`, #8615), after SurfaceCraft's grid, block and quad layouts: a
//! region of the scan (what is selected, the piece clicked, or as far as a radius round the
//! click) fitted with a bicubic surface placed by the region's own flattening, so a region that
//! curves round fits too; in blocks, cut exactly into rows and columns of faces sewn into one
//! body; or as a quad whose edges follow the region's edge. The surface is kept as STEP in a
//! Scan Surface feature, so a design opens anywhere.

use serde_json::{Value, json};
use solidcraft_scan::patches::{self, Layout, Patches};

use super::{CommandSpec, mesh};
use crate::params::{bad, num, str_};
use crate::schema::scan_patches::{ScanPatchesParams, ScanPatchesResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const PATCHES_CONTRACT: TypedContract<ScanPatchesParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.fit_patches", "Fit Patches", crate::checked!("scan.fit_patches", fit_patches))
    .at("SCAN", "FREEFORM")
    .icon("scan_fit_patches")
    .contract(&PATCHES_CONTRACT)
    .output(output_schema_for::<ScanPatchesResult>)
    .params("body?: the scan (default: what is selected on a scan, else the picked or only one); the region: triangles?: [indices] | point?: [x,y,z] (the piece there) with radius?: length (default: what is selected, else the point picked); layout?: grid (default: one patch over the region's flattened box) | blocks (that patch cut exactly into `rows` × `columns` faces, sewn into one body) | quads (one patch whose edges follow the region's edge, its corners where the region's are); controls?: 4–30 or [u, v] (default 10: control points each way over the whole); rows?, columns?: blocks (default 2 × 2); smoothing?: 0–1 (default 0.1): fitted in all three directions to the region's points, each starting on the base surface the region fits best (its plane, a cylinder round it, or its flattening, so a region that curves round fits too) and moved to its foot on the surface between rounds; kept as a surface body `Patches1`, … (one undo step); a surface that folds, pinches or passes through itself is kept and said so (warning), and blocks that cannot be sewn because of it are refused → body, surface, layout, patches, rows, columns, controls, rms, max, points, folds, pinches, crossings, warning?, feature")];

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

/// What `p` would fit: the scan, the layout and controls, and the patches.
pub fn fitted(s: &Session, p: &Value, cmd: &str) -> Result<(String, Layout, [usize; 2], Patches)> {
    let body = str_(p, "body").map(str::to_owned).or_else(|| super::select::selected(s).map(|k| k.scan.clone()));
    let on = mesh(s, &json!({ "body": body }), cmd)?;
    let region = super::flatten::region(s, p, &on, cmd)?;
    let whole = |key: &str, low: u64, high: u64, default: u64| -> Result<usize> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(default as usize),
            Some(v) => v
                .as_u64()
                .filter(|n| (low..=high).contains(n))
                .map(|n| n as usize)
                .ok_or_else(|| bad(cmd, format!("`{key}` must be a whole number from {low} to {high}"))),
        }
    };
    let layout = match str_(p, "layout").unwrap_or("grid") {
        "grid" => Layout::Grid,
        "blocks" => Layout::Blocks { rows: whole("rows", 1, 8, 2)?, columns: whole("columns", 1, 8, 2)? },
        "quads" => Layout::Quads,
        other => return Err(bad(cmd, format!("`layout` is grid, blocks or quads, not `{other}`"))),
    };
    let controls = match p.get("controls").filter(|v| !v.is_null()) {
        None => [10, 10],
        Some(Value::Array(a)) if a.len() == 2 => {
            let one = |v: &Value| v.as_u64().filter(|n| (4..=30).contains(n)).map(|n| n as usize);
            match (one(&a[0]), one(&a[1])) {
                (Some(u), Some(v)) => [u, v],
                _ => return Err(bad(cmd, "`controls` must be 4 to 30, or [u, v] of them")),
            }
        }
        Some(v) => {
            let n = v.as_u64().filter(|n| (4..=30).contains(n)).ok_or_else(|| bad(cmd, "`controls` must be 4 to 30, or [u, v] of them"))? as usize;
            [n, n]
        }
    };
    let smoothing = num(p, "smoothing").unwrap_or(0.1);
    if !(0.0..=1.0).contains(&smoothing) {
        return Err(bad(cmd, "`smoothing` is 0 to 1"));
    }
    let found = patches::fit(&on.scan(), &region, layout, controls, smoothing).map_err(|e| bad(cmd, e.to_string()))?;
    Ok((on.body, layout, controls, found))
}

fn fit_patches(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fit_patches";
    let (body, layout, controls, found) = fitted(s, p, cmd)?;
    // Fitted to the part of the scan under a click: the recipe keeps how, so `scan.refit` can fit it
    // again. (A part chosen by selecting is not in the parameters, so it cannot be made again.)
    let recipe = p.get("point").filter(|v| !v.is_null()).map(|_| {
        let mut params = p.clone();
        params["body"] = json!(body);
        super::refit::recipe(&body, "scan.fit_patches", params, &found.surfaces)
    });
    let (surface, feature) = if let [one] = found.surfaces.as_slice() {
        super::surface::keep_surface(s, one, "Patches", "Fit Patches", "patches", recipe, cmd)?
    } else {
        // The blocks sewn into one body along the cuts they share.
        let sheets = found
            .surfaces
            .iter()
            .map(|f| solidcraft_kernel::nurbs_sheet(f).map_err(|e| bad(cmd, format!("a patch: {e}"))))
            .collect::<Result<Vec<_>>>()?;
        let refs: Vec<&_> = sheets.iter().collect();
        // A surface that folds over itself has blocks whose sides do not meet where their
        // neighbours' do: said so, rather than in the kernel's words.
        let sewn = solidcraft_kernel::stitch(&refs, 0.0).map_err(|e| {
            if found.trouble.iter().sum::<usize>() > 0 {
                bad(
                    cmd,
                    format!(
                        "the blocks could not be sewn into one body: the surface folds over itself ({}), so the blocks' sides do not meet; take a smaller region (more control points do not stop it folding)",
                        super::surface::trouble_text(found.trouble)
                    ),
                )
            } else {
                bad(cmd, format!("the blocks could not be sewn: {e}"))
            }
        })?;
        let step = solidcraft_kernel::step_export(&[&sewn], "SolidCraft").map_err(|e| bad(cmd, format!("the patches: {e}")))?;
        let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
        let name = (1..).map(|i| format!("Patches{i}")).find(|n| !taken.contains(n)).unwrap_or_else(|| "Patches".into());
        let kind = solidcraft_doc::FeatureKind::ScanSurface { file: "patches".into(), step: step.into(), recipe };
        let reply = super::super::features::add_feature(s, &json!({ "body_name": name, "name": "Fit Patches" }), kind)?;
        (name, reply)
    };
    reply(ScanPatchesResult {
        body,
        surface,
        layout: match layout {
            Layout::Grid => "grid",
            Layout::Blocks { .. } => "blocks",
            Layout::Quads => "quads",
        }
        .into(),
        patches: found.surfaces.len(),
        rows: found.rows,
        columns: found.columns,
        controls,
        rms: found.rms,
        max: found.max,
        points: found.points,
        folds: found.trouble[0],
        pinches: found.trouble[1],
        crossings: found.trouble[2],
        warning: super::surface::fold_warning(found.trouble),
        feature,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "scan_patches_tests.rs"]
mod tests;
