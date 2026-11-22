//! A cut through the scan, and the model beside it, read as data (`scan.section`): the outlines fitted
//! to each and how far the scan's cut lies from the model's. Agents in 2026-11-21's trials read
//! sections by drawing one (Section to Sketch), inspecting the sketch and undoing it: 55 to 116 of
//! their calls; this is one call that makes nothing.

use serde_json::Value;
use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
use solidcraft_scan::chain::{Outline, Piece};
use solidcraft_scan::section::Run;

use super::{CommandSpec, mesh_name, tolerance};
use crate::params::{bad, str_};
use crate::schema::scan_section_read::{ScanCutGaps, ScanCutOutline, ScanCutParams, ScanCutPiece, ScanCutPlace, ScanCutPlane, ScanCutResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanCutParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.section", "Scan Section", crate::checked!("scan.section", cut))
    .icon("fit_section")
    .contract(&CONTRACT)
    .output(output_schema_for::<ScanCutResult>)
    .params("scan? (or body?): mesh body (default: the selected or only one); plane: XY|XZ|YZ (offset?: mm along its normal: Z, -Y, X) or {origin: [x,y,z], normal: [x,y,z]}; model?: the solid or surface body (default: the design's shown solid, when there is one; false: the scan alone); tolerance?: mm the outlines are fitted to (default: the scan's tolerance, no finer than its noise): the scan, and the model, cut by the plane and read as data, making nothing → scan, model?, plane {origin, x, y, normal} (outlines are in its x, y), tolerance, scan_outlines and model_outlines [{closed, pieces [{kind: line|arc|circle, from, to, center?, radius?, ccw?}]}], gaps? {max, mean, within, worst [{at, distance}]} (how far the scan's cut lies from the model's)")];

/// The most scan cut points the gaps are measured at, and the most places named.
const MOST_POINTS: usize = 20_000;
const MOST_PLACES: usize = 5;

fn to_value<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| EngineError::Other(e.to_string()))
}

fn plane_of(p: &Value, cmd: &str) -> Result<Plane> {
    let offset = match p.get("offset").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|o| o.is_finite()).ok_or_else(|| bad(cmd, "`offset` is a number of mm"))?,
        None => 0.0,
    };
    match p.get("plane") {
        Some(Value::String(name)) => {
            super::section::standard_plane(name.trim(), offset).ok_or_else(|| bad(cmd, "`plane` is XY, XZ, YZ or {origin, normal}"))
        }
        Some(o @ Value::Object(_)) => {
            let origin = o
                .get("origin")
                .and_then(crate::params::vec3)
                .filter(|q| q.is_finite())
                .ok_or_else(|| bad(cmd, "`plane.origin` is a point [x, y, z]"))?;
            let normal = o
                .get("normal")
                .and_then(crate::params::vec3)
                .and_then(|n| n.normalized())
                .ok_or_else(|| bad(cmd, "`plane.normal` is a direction [x, y, z]"))?;
            let x = if normal.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let x = (x - normal * x.dot(normal)).normalized().ok_or_else(|| bad(cmd, "`plane.normal` is a direction [x, y, z]"))?;
            Ok(Plane { origin: origin + normal * offset, x, y: normal.cross(x) })
        }
        _ => Err(bad(cmd, "`plane` is XY, XZ, YZ or {origin, normal}")),
    }
}

fn outlines(runs: &[Run], tolerance: f64) -> Vec<ScanCutOutline> {
    let length = |r: &Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    runs.iter()
        .filter(|r| length(r) > 10.0 * tolerance)
        .filter_map(|r| solidcraft_scan::chain::chain(&r.points, r.closed, tolerance).map(|o| (o, r.closed)))
        .map(|(o, closed)| ScanCutOutline { closed, pieces: pieces(&o) })
        .collect()
}

fn pieces(o: &Outline) -> Vec<ScanCutPiece> {
    let pt = |v: Vec2| [v.x, v.y];
    match o {
        // A whole circle: from and to its point along the plane's x from the centre.
        Outline::Circle { center, radius } => {
            let at = pt(*center + Vec2::new(*radius, 0.0));
            vec![ScanCutPiece { kind: "circle".into(), from: at, to: at, center: Some(pt(*center)), radius: Some(*radius), ccw: Some(true) }]
        }
        Outline::Chain(pieces) => pieces
            .iter()
            .map(|p| match p {
                Piece::Line { from, to } => ScanCutPiece { kind: "line".into(), from: pt(*from), to: pt(*to), center: None, radius: None, ccw: None },
                Piece::Arc { center, radius, from, to, ccw } => ScanCutPiece {
                    kind: "arc".into(),
                    from: pt(*from),
                    to: pt(*to),
                    center: Some(pt(*center)),
                    radius: Some(*radius),
                    ccw: Some(*ccw),
                },
            })
            .collect(),
    }
}

/// The runs with every step longer than `step` split evenly: a scan made of long triangles (a
/// decimated one, a CAD model brought in as an STL) cuts into a few points far apart, where the
/// fitter reads arcs through what are lines, and the gaps would be measured at the corners only.
fn even(runs: Vec<Run>, step: f64) -> Vec<Run> {
    runs.into_iter()
        .map(|r| {
            let n = r.points.len();
            let last = if r.closed { n } else { n.saturating_sub(1) };
            let mut points = Vec::with_capacity(n);
            let mut triangles = Vec::with_capacity(n);
            for k in 0..n {
                let (Some(&a), Some(&t)) = (r.points.get(k), r.triangles.get(k)) else { continue };
                points.push(a);
                triangles.push(t);
                let Some(&b) = r.points.get((k + 1) % n).filter(|_| k < last) else { continue };
                // at most a thousand pieces a step: a step a whole part long on a hostile plane
                let parts = ((a.dist(b) / step).ceil() as usize).clamp(1, 1000);
                for i in 1..parts {
                    points.push(a + (b - a) * (i as f64 / parts as f64));
                    triangles.push(t);
                }
            }
            Run { points, triangles, closed: r.closed }
        })
        .collect()
}

/// The distance from `p` to the nearest of the polylines `runs`.
fn nearest(p: Vec2, runs: &[Run]) -> f64 {
    let seg = |a: Vec2, b: Vec2| {
        let ab = b - a;
        let t = if ab.len2() > 0.0 { ((p - a).dot(ab) / ab.len2()).clamp(0.0, 1.0) } else { 0.0 };
        p.dist(a + ab * t)
    };
    runs.iter()
        .flat_map(|r| {
            let n = r.points.len();
            let last = if r.closed { n } else { n.saturating_sub(1) };
            (0..last).filter_map(move |k| Some((*r.points.get(k)?, *r.points.get((k + 1) % n)?)))
        })
        .map(|(a, b)| seg(a, b))
        .fold(f64::INFINITY, f64::min)
}

fn cut(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.section";
    let named = match (str_(p, "scan"), str_(p, "body")) {
        (Some(a), Some(b)) if a != b => return Err(bad(cmd, format!("`scan` (`{a}`) and `body` (`{b}`) name different scans: give one"))),
        (a, b) => a.or(b),
    };
    let scan = mesh_name(s, &serde_json::json!({ "body": named }), cmd)?;
    let plane = plane_of(p, cmd)?;
    let runs = super::with_shown(s, &scan, |pos, tri| solidcraft_scan::section::section(&Scan::new(pos, tri), &plane))
        .ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
    if runs.is_empty() {
        return Err(bad(cmd, format!("the plane misses `{scan}`: no cut")));
    }
    let extent = runs
        .iter()
        .flat_map(|r| r.points.iter())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q.x.min(q.y)), hi.max(q.x.max(q.y))));
    let extent = (extent.1 - extent.0).max(0.0);
    let tolerance = match p.get("tolerance").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|t| t.is_finite() && *t > 0.0).ok_or_else(|| bad(cmd, "`tolerance` is a length above 0 (mm)"))?,
        None => tolerance::of(s, &scan).max(super::section::noise_tolerance(&runs)),
    };
    let step = (extent / 50.0).max(2.0 * tolerance);
    let runs = even(runs, step);
    // The model: named, or the design's one shown solid; `false` for none.
    let model = match p.get("model") {
        Some(Value::Bool(false)) => None,
        Some(Value::String(name)) if !name.trim().is_empty() => {
            let state = s.model.state();
            let b = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
            if b.body.is_mesh() {
                return Err(bad(cmd, format!("`{name}` is a mesh: give the model, a solid or surface body")));
            }
            Some(name.clone())
        }
        None | Some(Value::Null) => {
            let state = s.model.state();
            let solids: Vec<String> = state.bodies.iter().filter(|b| !b.body.is_mesh() && s.body_visible(&b.name)).map(|b| b.name.clone()).collect();
            match solids.as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            }
        }
        Some(_) => return Err(bad(cmd, "`model` is a body's name, or false")),
    };
    // The model's cut, through its surface meshed within a quarter of the tolerance (as Deviation
    // measures it): its outlines fitted as the scan's are, closer.
    let model_runs: Vec<Run> = match &model {
        Some(name) => {
            let state = s.model.state();
            let body = state.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
            let mesh = body
                .body
                .mesh(&solidcraft_kernel::MeshTolerance::chord((tolerance / 4.0).min(0.01)))
                .map_err(|e| bad(cmd, format!("`{name}`: {e}")))?;
            even(solidcraft_scan::section::section(&Scan::new(&mesh.positions, &mesh.triangles), &plane), step)
        }
        None => Vec::new(),
    };
    let gaps = model.as_ref().filter(|_| !model_runs.is_empty()).map(|_| {
        let points: Vec<Vec2> = runs.iter().flat_map(|r| r.points.iter().copied()).collect();
        let step = (points.len() / MOST_POINTS).max(1);
        let measured: Vec<(Vec2, f64)> = points.iter().step_by(step).map(|&q| (q, nearest(q, &model_runs))).collect();
        let n = measured.len().max(1) as f64;
        let max = measured.iter().map(|(_, d)| *d).fold(0.0, f64::max);
        let mean = measured.iter().map(|(_, d)| d).sum::<f64>() / n;
        let within = measured.iter().filter(|(_, d)| *d <= tolerance).count() as f64 / n;
        // The worst places, each a tenth of the cut's size from those named before.
        let apart = 0.1 * extent;
        let mut sorted = measured.clone();
        sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut worst: Vec<(Vec2, f64)> = Vec::new();
        for (q, d) in sorted {
            if d <= tolerance || worst.len() >= MOST_PLACES {
                break;
            }
            if worst.iter().all(|(w, _)| w.dist(q) > apart) {
                worst.push((q, d));
            }
        }
        ScanCutGaps {
            max,
            mean,
            within,
            worst: worst
                .into_iter()
                .map(|(q, d)| {
                    let w = plane.origin + plane.x * q.x + plane.y * q.y;
                    ScanCutPlace { at: [w.x, w.y, w.z], distance: d }
                })
                .collect(),
        }
    });
    let normal = plane.x.cross(plane.y);
    let at = |v: Vec3| [v.x, v.y, v.z];
    to_value(ScanCutResult {
        model_outlines: outlines(&model_runs, (tolerance / 10.0).max(1e-4)),
        scan_outlines: outlines(&runs, tolerance),
        plane: ScanCutPlane { origin: at(plane.origin), x: at(plane.x), y: at(plane.y), normal: at(normal) },
        tolerance,
        gaps,
        scan,
        model,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    /// A block 40 x 30 x 10 scanned and modelled 1 mm short in X: cut at z 5, the scan's cut is one
    /// closed outline of four lines, 40 by 30; the model's 39 by 30; the gaps reach 1 mm (the scan's
    /// end 1 mm past the model's), most of the cut within the tolerance, the worst place on that end.
    #[test]
    fn a_cut_reads_both_outlines_and_where_they_part() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-section-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("block.stl");
        // a closed box 40 x 30 x 10, each side two triangles, as a scan
        let c = |x: f64, y: f64, z: f64| [x * 40.0, y * 30.0, z * 10.0];
        let quads = [
            [c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.), c(1., 0., 0.)],
            [c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)],
            [c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.), c(0., 0., 1.)],
            [c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)],
            [c(0., 0., 0.), c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.)],
            [c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)],
        ];
        let mut text = String::from("solid b\n");
        for q in quads {
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                text.push_str("facet normal 0 0 0\nouter loop\n");
                for v in t {
                    text.push_str(&format!("vertex {} {} {}\n", v[0], v[1], v[2]));
                }
                text.push_str("endloop\nendfacet\n");
            }
        }
        text.push_str("endsolid b\n");
        std::fs::write(&path, text).unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        s.execute("file.insert_mesh", &json!({"path": path})).unwrap();
        s.execute("solid.box", &json!({"length": 39, "width": 30, "height": 10})).unwrap();
        let features = s.doc.features.len();
        let r = s.execute("scan.section", &json!({"plane": "XY", "offset": 5, "tolerance": 0.1})).unwrap();
        assert_eq!(s.doc.features.len(), features, "a cut makes nothing");
        let scan = r["scan_outlines"].as_array().unwrap();
        assert_eq!(scan.len(), 1, "{r}");
        let lines = scan[0]["pieces"].as_array().unwrap();
        assert!(scan[0]["closed"] == true && lines.len() == 4 && lines.iter().all(|p| p["kind"] == "line"), "{r}");
        let span = |o: &serde_json::Value, k: usize| {
            let xs: Vec<f64> = o["pieces"].as_array().unwrap().iter().map(|p| p["from"][k].as_f64().unwrap()).collect();
            xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - xs.iter().cloned().fold(f64::INFINITY, f64::min)
        };
        assert!((span(&scan[0], 0) - 40.0).abs() < 1e-6 && (span(&scan[0], 1) - 30.0).abs() < 1e-6, "{r}");
        let model = &r["model_outlines"][0];
        assert!((span(model, 0) - 39.0).abs() < 1e-3 && (span(model, 1) - 30.0).abs() < 1e-3, "{r}");
        let gaps = &r["gaps"];
        assert!((gaps["max"].as_f64().unwrap() - 1.0).abs() < 1e-6, "{r}");
        let within = gaps["within"].as_f64().unwrap();
        assert!(within > 0.5 && within < 1.0, "{r}");
        let worst = gaps["worst"][0]["at"].as_array().unwrap();
        assert!((worst[0].as_f64().unwrap() - 40.0).abs() < 1e-6 && (worst[2].as_f64().unwrap() - 5.0).abs() < 1e-6, "{r}");
        // `model: false` cuts the scan alone; a plane it cannot read is refused.
        let alone = s.execute("scan.section", &json!({"plane": "XY", "offset": 5, "model": false})).unwrap();
        assert!(alone.get("gaps").is_none() && alone["model_outlines"].as_array().unwrap().is_empty(), "{alone}");
        assert!(s.execute("scan.section", &json!({"plane": "XW"})).is_err());
        let missed = s.execute("scan.section", &json!({"plane": "XY", "offset": 50})).unwrap_err().to_string();
        assert!(missed.contains("misses"), "{missed}");
    }
}
