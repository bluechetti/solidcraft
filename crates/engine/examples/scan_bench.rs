//! One case of the scan-to-CAD benchmark (`cargo xtask scan-bench`): a part with an exact answer
//! is built from its recipe, meshed, made into a scan with a fixed seed, and built back through
//! the scan commands a person would run, with nothing tuned for the case.
//!
//!   scan_bench <tests/scan-cases/NAME> --out DIR [--sigma 0.02] [--seed N] [--edge 0.5]
//!              [--missing 0.1] [--specks 0.001] [--turn DEGREES] [--scale K]
//!              [--truth-tolerance 0.05]
//!
//! Writes into DIR: truth.step (the part), scan.stl (what the scan commands were given),
//! placed.stl (the scan where the rebuilt model stands), model.step (what was built, when
//! anything was) and result.json: every command run with its time and its refusal or warning,
//! Scan Deviation's own verdict against the scan, and what the built model measures against the
//! truth (its volume, and the part's fine mesh against it both ways, where the scan was left
//! where the part was drawn). Holding those to the bars is the benchmark's, not this program's:
//! it only rebuilds, measures and records.

use std::{collections::BTreeMap, error::Error, path::PathBuf, time::Instant};

use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_testkit::scan_degrade::{Degrade, Scan, degraded};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// A binary STL's triangles as a mesh, corners at exactly the same place made one point.
fn read_stl(bytes: &[u8]) -> Result<Scan> {
    let count = bytes.get(80..84).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or("a short STL")?;
    let mut scan = Scan::default();
    let mut seen: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    for k in 0..count {
        let at = 84 + 50 * k + 12;
        let mut corners = [0u32; 3];
        for (c, corner) in corners.iter_mut().enumerate() {
            let mut p = [0f32; 3];
            for (axis, value) in p.iter_mut().enumerate() {
                let from = at + 12 * c + 4 * axis;
                let b = bytes.get(from..from + 4).ok_or("a short STL")?;
                *value = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            *corner = *seen.entry(key).or_insert_with(|| {
                scan.points.push([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]);
                (scan.points.len() - 1) as u32
            });
        }
        if corners[0] != corners[1] && corners[1] != corners[2] && corners[2] != corners[0] {
            scan.triangles.push(corners);
        }
    }
    Ok(scan)
}

/// What was run, in order.
#[derive(Default)]
struct Log {
    steps: Vec<Value>,
}

impl Log {
    /// Run `command`, record it (its time, its refusal, the warnings its reply carries) and give
    /// its reply when it ran.
    fn run(&mut self, s: &mut Session, command: &str, params: Value) -> Option<Value> {
        let start = Instant::now();
        let outcome = s.execute(command, &params);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut step = json!({"command": command, "params": params, "ms": ms, "ok": outcome.is_ok()});
        match &outcome {
            Ok(reply) => {
                for key in ["warning", "warnings", "note", "why_not", "param_warnings"] {
                    if let Some(said) = reply.get(key).filter(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty())) {
                        step[key] = said.clone();
                    }
                }
                if let Some(fit) = reply.get("fit") {
                    step["fit"] = fit.clone();
                }
            }
            Err(refusal) => step["refusal"] = json!(refusal.to_string()),
        }
        self.steps.push(step);
        outcome.ok()
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let case = PathBuf::from(args.first().ok_or("usage: scan_bench <case folder> --out DIR [--sigma S] [--seed N] ...")?);
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let number = |name: &str, default: f64| -> Result<f64> { Ok(flag(name).map(|v| v.parse::<f64>()).transpose()?.unwrap_or(default)) };
    let out = PathBuf::from(flag("--out").ok_or("--out DIR is needed")?);
    std::fs::create_dir_all(&out)?;
    let how = Degrade {
        seed: number("--seed", 20_261_118.0)? as u64,
        edge: number("--edge", 0.5)?,
        sigma: number("--sigma", 0.02)?,
        missing: number("--missing", 0.0)?,
        specks: number("--specks", 0.0)?,
        angle: number("--turn", 0.0)?.to_radians(),
        axis: [0.6, -0.3, 0.74],
        shift: if flag("--turn").is_some() { [40.0, -25.0, 60.0] } else { [0.0; 3] },
        scale: number("--scale", 1.0)?,
        ..Degrade::default()
    };
    let recipe: Value = serde_json::from_slice(&std::fs::read(case.join("recipe.json"))?)?;
    let path = |name: &str| out.join(name).to_string_lossy().into_owned();

    // The part, exact, and a fine mesh of it.
    let mut truth = Session::default();
    for step in recipe["commands"].as_array().ok_or("the recipe has no commands")? {
        let command = step["command"].as_str().ok_or("a recipe step has no command")?;
        truth.execute(command, &step["params"]).map_err(|e| format!("the recipe's {command}: {e}"))?;
    }
    truth.execute("file.export_step", &json!({"path": path("truth.step")}))?;
    // Fine: curved faces within 5e-5 of the part's size, a few microns, well under the noise.
    truth.execute("file.export_stl", &json!({"path": path("truth.stl"), "quality": "fine"}))?;
    let scan = degraded(&read_stl(&std::fs::read(out.join("truth.stl"))?)?, &how);
    std::fs::write(out.join("scan.stl"), scan.stl())?;

    // Built back, as a person would: read, tidy, place when it lies askew, find its shapes, take
    // what Reconstruct proposes one step at a time and undo a step that makes the fit worse.
    let mut s = Session::default();
    let mut log = Log::default();
    let started = Instant::now();
    let inserted = log.run(&mut s, "file.insert_mesh", json!({"path": path("scan.stl")}));
    let body = inserted.as_ref().and_then(|r| r["bodies"][0].as_str()).map(str::to_owned);
    let mut built = None;
    let mut in_app = Value::Null;
    let mut advice = Value::Null;
    let mut against_truth = Value::Null;
    let truth_tolerance = number("--truth-tolerance", 0.05)?;
    if let Some(body) = &body {
        // Survey reads the scan and names what to run first, each with its parameters: its
        // tidying and its tolerance are taken as it gives them, and its placing when the scan
        // lies askew. Nothing here is chosen for the case.
        let survey = log.run(&mut s, "scan.survey", json!({}));
        advice = survey.as_ref().map(|r| json!({"noise": r["noise"], "tolerance": r["tolerance"], "next": r["next"].as_array().map(|n| n.iter().map(|x| x["command"].clone()).collect::<Vec<_>>())})).unwrap_or(Value::Null);
        let tidying = ["scan.repair", "scan.remove_floating", "scan.denoise", "scan.fill_holes", "scan.tolerance"];
        let placing = ["scan.stand_on_base", "scan.quick_align"];
        for step in survey.iter().flat_map(|r| r["next"].as_array().into_iter().flatten()) {
            let Some(command) = step["command"].as_str() else { continue };
            if tidying.contains(&command) || (flag("--turn").is_some() && placing.contains(&command)) {
                log.run(&mut s, command, step["params"].clone());
            }
        }
        log.run(&mut s, "scan.find_all", json!({}));
        if let Some(read) = log.run(&mut s, "scan.reconstruct", json!({})) {
            for step in read["proposals"].as_array().into_iter().flatten() {
                let Some(command) = step["command"].as_str() else { continue };
                let reply = log.run(&mut s, command, step["params"].clone());
                if reply.as_ref().is_some_and(|r| r["fit"]["verdict"] == "worse") {
                    log.run(&mut s, "edit.undo", json!({}));
                }
            }
        }
        let model = format!("{body} Reconstructed");
        // What the body leaves of the scan unexplained, built piece by piece.
        log.run(&mut s, "scan.reconstruct_rest", json!({}));
        if let Some(reply) = log.run(&mut s, "scan.deviation", json!({"scan": body, "body": model})) {
            in_app = reply;
            if log.run(&mut s, "file.export_step", json!({"path": path("model.step"), "bodies": [model]})).is_some() {
                built = Some(model);
            }
        }
        log.run(&mut s, "file.export_stl", json!({"path": path("placed.stl"), "bodies": [body]}));
        if let Some(model) = &built {
            let volume = log.run(&mut s, "inspect.measure", json!({"bodies": [model]})).map(|r| r["total"]["volume_mm3"].clone());
            against_truth = json!({"volume": volume});
            // The part's own fine mesh against the model, both ways: only where the model stands
            // where the part was drawn.
            if flag("--turn").is_none() && how.scale == 1.0 {
                let read = log.run(&mut s, "file.insert_mesh", json!({"path": path("truth.stl"), "units": "mm"}));
                if let Some(part) = read.as_ref().and_then(|r| r["bodies"][0].as_str()) {
                    log.run(&mut s, "scan.tolerance", json!({"scan": part, "tolerance": truth_tolerance}));
                    if let Some(reply) = log.run(&mut s, "scan.deviation", json!({"scan": part, "body": model})) {
                        for key in [
                            "within_all",
                            "rms",
                            "max_above",
                            "max_below",
                            "max_above_at",
                            "max_below_at",
                            "model_within",
                            "model_covered",
                            "model_max",
                            "model_farthest_at",
                            "tolerance",
                        ] {
                            against_truth[key] = reply[key].clone();
                        }
                    }
                }
            }
        }
        log.run(&mut s, "scan.deviation", json!({"show": false}));
    }
    let result = json!({
        "case": recipe["id"],
        "class": recipe["class"],
        "degrade": {"seed": how.seed, "edge": how.edge, "sigma": how.sigma, "missing": how.missing, "specks": how.specks, "turn_degrees": how.angle.to_degrees(), "scale": how.scale},
        "scan": {"points": scan.points.len(), "triangles": scan.triangles.len()},
        "built": built,
        "seconds": started.elapsed().as_secs_f64(),
        "survey": advice,
        "in_app": in_app,
        "against_truth": against_truth,
        "refused": log.steps.iter().filter(|s| s["ok"] == false).map(|s| s["command"].clone()).collect::<Vec<_>>(),
        "steps": log.steps,
    });
    std::fs::write(out.join("result.json"), serde_json::to_string_pretty(&result)? + "\n")?;
    println!(
        "{} sigma {}: {}",
        recipe["id"],
        how.sigma,
        if result["built"].is_null() { "nothing built".to_owned() } else { format!("built, in-app passes: {}", result["in_app"]["passes"]) }
    );
    Ok(())
}
