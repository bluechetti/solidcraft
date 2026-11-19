//! `cargo xtask scan-bench [--full] [--bless] [--cases a,b] [--bin PATH] [--scorer CMD] [--out DIR]`:
//! the scan-to-CAD accuracy benchmark. Every part in `tests/scan-cases/` has an exact answer; each
//! is meshed, made into a scan with a fixed seed (noise, and with `--full` missing surface, a
//! pose, stray specks), built back by the scan commands a person would run
//! (`crates/engine/examples/scan_bench.rs`) and held to the bars of `tests/scan-bench.toml`.
//!
//! A run is one of:
//!   pass          built, within every bar, and Scan Deviation said it passes
//!   fail          built, outside a bar, and Scan Deviation said it does not pass
//!   refused       nothing was built: the commands refused
//!   SILENT WRONG  built, outside a bar, and Scan Deviation passed it: none is allowed
//!   missed pass   built and within every bar, though Scan Deviation did not pass it
//!
//! The numbers of every run are compared with `tests/scan-bench.lock.json` (`--bless` writes it):
//! the benchmark is seeded, and a number that moves is a change in what SolidCraft builds. It
//! writes `target/parity/report.json` and `docs/parity/report.md`, and fails on a silently wrong
//! run, a crashed one, or a number that differs from the lock.
//!
//! `--scorer CMD` adds a second judge that shares no code with SolidCraft: CMD is run as
//! `CMD <run folder> <bar.json> <expected.json>` and prints `{"pass": bool, "failed": [..]}`; a
//! run then passes only when both judges pass it (a run the bars already fail is not sent to it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const CASES: &str = "tests/scan-cases";
const CONFIG: &str = "tests/scan-bench.toml";
const LOCK: &str = "tests/scan-bench.lock.json";
const MATRIX: &str = "tests/parity/matrix.toml";
const REPORT: &str = "docs/parity/report.md";

/// One way a case is scanned.
struct Level {
    /// Its name in a run's folder and in the lock (`s0.02`, `s0.02-missing-10`).
    name: String,
    sigma: f64,
    flags: Vec<String>,
    /// The tier's name in the report.
    tier: String,
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let (mut full, mut bless, mut only, mut bin, mut scorer) = (false, false, None, None, None);
    let mut out = root.join("target/parity");
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--full" => full = true,
            "--bless" => bless = true,
            "--cases" => only = Some(args.next().ok_or("--cases needs a list, a,b")?.split(',').map(str::to_owned).collect::<Vec<_>>()),
            "--bin" => bin = Some(PathBuf::from(args.next().ok_or("--bin needs the scan_bench program")?)),
            "--scorer" => scorer = Some(args.next().ok_or("--scorer needs a command")?.to_string()),
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a folder")?),
            other => return Err(format!("scan-bench: unknown argument `{other}`")),
        }
    }
    let text = std::fs::read_to_string(root.join(CONFIG)).map_err(|e| format!("{CONFIG}: {e}"))?;
    let config: toml::Value = toml::from_str(&text).map_err(|e| format!("{CONFIG}: {e}"))?;
    let config = serde_json::to_value(config).map_err(|e| format!("{CONFIG}: {e}"))?;
    let levels = levels(&config, full)?;
    let bin = match bin {
        Some(bin) => bin,
        None => built(root)?,
    };
    let mut cases: Vec<String> = std::fs::read_dir(root.join(CASES))
        .map_err(|e| format!("{CASES}: {e}"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("recipe.json").exists())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| only.as_ref().is_none_or(|o| o.contains(name)))
        .collect();
    cases.sort();
    if cases.is_empty() {
        return Err("scan-bench: no case to run".into());
    }

    let mut rows = Vec::new();
    for case in &cases {
        for level in &levels {
            let row = one(root, &config, &bin, scorer.as_deref(), &out, case, level)?;
            println!(
                "{case:16} {:20} {:13} {}",
                level.name,
                row["status"].as_str().unwrap_or("?"),
                row["failed"].as_array().map(|f| f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default()
            );
            rows.push(row);
        }
    }

    let count = |status: &str| rows.iter().filter(|r| r["status"] == status).count();
    let summary = json!({
        "runs": rows.len(), "pass": count("pass"), "fail": count("fail"), "refused": count("refused"),
        "silent_wrong": count("SILENT WRONG"), "missed_pass": count("missed pass"), "crashed": count("CRASHED"),
    });
    let locked: BTreeMap<String, Value> = rows.iter().map(|r| (format!("{}/{}", text_of(&r["case"]), text_of(&r["level"])), r["numbers"].clone())).collect();
    let lock_path = root.join(LOCK);
    let before: BTreeMap<String, Value> = std::fs::read_to_string(&lock_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let moved: Vec<String> = locked.iter().filter(|(key, numbers)| before.get(*key).is_some_and(|was| was != *numbers)).map(|(key, _)| key.clone()).collect();
    let new: Vec<String> = locked.keys().filter(|key| !before.contains_key(*key)).cloned().collect();
    if bless {
        // Runs not made this time (the other tier, other cases) keep their numbers.
        let mut all = before.clone();
        all.extend(locked.clone());
        write(&lock_path, &(pretty(&json!(all))? + "\n"))?;
    }

    let report = json!({
        "sha": git(root, &["rev-parse", "--short", "HEAD"]),
        "dirty": !git(root, &["status", "--porcelain"]).is_empty(),
        "tier": if full { "full" } else { "fast" },
        "second_judge": scorer,
        "summary": summary,
        "moved_from_lock": moved,
        "not_in_lock": new,
        "rows": rows,
    });
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    write(&out.join("report.json"), &(pretty(&report)? + "\n"))?;
    write(&root.join(REPORT), &markdown(root, &config, &report))?;
    println!(
        "{} runs: {} pass, {} fail, {} refused, {} silently wrong, {} missed pass, {} crashed",
        rows.len(),
        count("pass"),
        count("fail"),
        count("refused"),
        count("SILENT WRONG"),
        count("missed pass"),
        count("CRASHED")
    );
    let mut wrong = Vec::new();
    if count("SILENT WRONG") > 0 {
        wrong.push(format!("{} run(s) came back wrong and marked good", count("SILENT WRONG")));
    }
    if count("CRASHED") > 0 {
        wrong.push(format!("{} run(s) crashed", count("CRASHED")));
    }
    if !bless && !moved.is_empty() {
        wrong.push(format!("numbers differ from {LOCK} (--bless takes them, with the reason in the commit): {}", moved.join(", ")));
    }
    if !bless && !new.is_empty() {
        wrong.push(format!("runs not in {LOCK} (--bless adds them): {}", new.join(", ")));
    }
    if wrong.is_empty() { Ok(()) } else { Err(format!("scan-bench: {}", wrong.join("; "))) }
}

/// The ways each case is scanned: the fast tier is every noise level whose features must be
/// exact; the full tier adds the rest and the `[[variant]]`s.
fn levels(config: &Value, full: bool) -> Result<Vec<Level>, String> {
    let sigmas: Vec<f64> = config["levels"]["sigma"].as_array().ok_or("scan-bench.toml: levels.sigma")?.iter().filter_map(Value::as_f64).collect();
    let exact = |sigma: f64| {
        config["prismatic"]["bars"].as_array().into_iter().flatten().any(|bar| bar["sigma"].as_f64() == Some(sigma) && bar["features"] == "exact")
    };
    let mut levels = Vec::new();
    for sigma in sigmas {
        if full || exact(sigma) {
            levels.push(Level { name: format!("s{sigma}"), sigma, flags: Vec::new(), tier: "noise".into() });
        }
    }
    if full {
        for variant in config["variant"].as_array().into_iter().flatten() {
            let name = variant["name"].as_str().ok_or("scan-bench.toml: a variant has no name")?;
            let sigma = variant["sigma"].as_f64().ok_or("scan-bench.toml: a variant has no sigma")?;
            let flags = variant["flags"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
            levels.push(Level { name: format!("s{sigma}-{name}"), sigma, flags, tier: name.to_owned() });
        }
    }
    Ok(levels)
}

/// The benchmark program, built with the `ci` profile.
fn built(root: &Path) -> Result<PathBuf, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(root)
        .args(["build", "--profile", "ci", "-p", "solidcraft-engine", "--example", "scan_bench"])
        .status()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err("scan-bench: the benchmark program did not build".into());
    }
    let target = std::env::var("CARGO_TARGET_DIR").map_or_else(|_| root.join("target"), PathBuf::from);
    Ok(target.join("ci/examples/scan_bench"))
}

/// One case at one level: rebuilt, measured and judged.
fn one(root: &Path, config: &Value, bin: &Path, scorer: Option<&str>, out: &Path, case: &str, level: &Level) -> Result<Value, String> {
    let folder = root.join(CASES).join(case);
    let dir = out.join(format!("{case}-{}", level.name));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for old in ["model.step", "result.json"] {
        let _ = std::fs::remove_file(dir.join(old));
    }
    let number = |key: &str| config["levels"][key].to_string();
    let done = Command::new(bin)
        .arg(&folder)
        .args(["--out", &dir.to_string_lossy(), "--sigma", &level.sigma.to_string(), "--seed", &number("seed"), "--edge", &number("edge")])
        .args(&level.flags)
        .output()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let mut row = json!({"case": case, "level": level.name, "tier": level.tier, "sigma": level.sigma, "flags": level.flags});
    let Some(result) = std::fs::read(dir.join("result.json")).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else {
        let said = String::from_utf8_lossy(&done.stderr);
        row["status"] = json!("CRASHED");
        row["failed"] = json!([said.lines().last().unwrap_or("no result")]);
        row["numbers"] = json!({"status": "CRASHED"});
        return Ok(row);
    };
    let class = result["class"].as_str().unwrap_or("prismatic");
    let expected: Value = std::fs::read(folder.join("expected.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null);
    let ran: Vec<&str> = result["steps"].as_array().into_iter().flatten().filter(|s| s["ok"] == true).filter_map(|s| s["command"].as_str()).filter(|c| c.ends_with("_it")).collect();
    let in_app = &result["in_app"];
    row["ran"] = json!(ran);
    row["refused"] = result["refused"].clone();
    row["in_app_passes"] = in_app["passes"].clone();
    row["why_not"] = in_app["why_not"].clone();
    row["seconds"] = result["seconds"].clone();
    if result["built"].is_null() {
        row["status"] = json!("refused");
        row["failed"] = json!([]);
        row["numbers"] = json!({"status": "refused"});
        return Ok(row);
    }
    let bar = config[class]["bars"].as_array().into_iter().flatten().find(|bar| bar["sigma"].as_f64() == Some(level.sigma)).cloned().unwrap_or(Value::Null);
    let has = |flag: &str| level.flags.iter().any(|f| f == flag);
    let checks = checks(config, &bar, &result, &expected, has("--missing"), has("--scale"));
    let mut failed: Vec<String> = checks.iter().filter(|(_, c)| c["ok"] == false).map(|(name, _)| name.clone()).collect();
    let own = failed.is_empty();
    // The second judge can only fail what the bars passed: a run already outside a bar keeps its
    // result whatever it says, and a badly wrong model is the slowest for it to measure.
    if let Some(command) = scorer.filter(|_| own) {
        let mut held = bar.clone();
        held["tolerance"] = json!((config[class]["tolerance_sigmas"].as_f64().unwrap_or(3.0) * level.sigma).max(config[class]["tolerance_least"].as_f64().unwrap_or(0.1)));
        held["edge_allowance"] = json!(config["levels"]["edge_allowance"].as_f64().unwrap_or(0.0) * config["levels"]["edge"].as_f64().unwrap_or(0.0));
        held["against_truth"] = json!(has("--missing"));
        held["placed"] = json!(has("--turn"));
        write(&dir.join("bar.json"), &pretty(&held)?)?;
        let mut words = command.split_whitespace();
        let second = words.next().and_then(|program| {
            Command::new(program).args(words).arg(&dir).arg(dir.join("bar.json")).arg(folder.join("expected.json")).output().ok()
        });
        let verdict: Value = second.as_ref().and_then(|o| serde_json::from_slice(&o.stdout).ok()).unwrap_or(Value::Null);
        match verdict["pass"].as_bool() {
            Some(true) => {}
            Some(false) => failed.extend(verdict["failed"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|f| format!("second judge: {f}"))),
            None => failed.push("second judge: gave no verdict".into()),
        }
        row["second_judge"] = verdict;
    }
    let passes = in_app["passes"] == true;
    let status = |within: bool| match (within, passes) {
        (true, true) => "pass",
        (true, false) => "missed pass",
        (false, true) => "SILENT WRONG",
        (false, false) => "fail",
    };
    row["status"] = json!(status(failed.is_empty()));
    // What the lock holds: SolidCraft's own verdict (the same with or without a second judge)
    // and each measured value, to a nanometre. Not the times.
    let values: BTreeMap<&String, Value> = checks.iter().map(|(name, c)| (name, json!(c["value"].as_f64().map(|v| (v * 1e6).round() / 1e6)))).collect();
    row["numbers"] = json!({"status": status(own), "in_app_passes": passes, "ran": row["ran"], "values": values});
    row["failed"] = json!(failed);
    row["checks"] = json!(checks);
    Ok(row)
}

/// Each bar the run is held to: its measured value, the bar, and whether it holds.
fn checks(config: &Value, bar: &Value, result: &Value, expected: &Value, missing: bool, scaled: bool) -> BTreeMap<String, Value> {
    let (in_app, truth) = (&result["in_app"], &result["against_truth"]);
    let mut checks = BTreeMap::new();
    let mut hold = |name: &str, value: Option<f64>, limit: Option<f64>, at_least: bool| {
        if let (Some(value), Some(limit)) = (value, limit) {
            let ok = if at_least { value >= limit } else { value <= limit };
            checks.insert(name.to_owned(), json!({"value": value, "bar": limit, "ok": ok}));
        }
    };
    let farthest = |of: &Value| match (of["max_above"].as_f64(), of["max_below"].as_f64()) {
        (Some(above), Some(below)) => Some(above.abs().max(below.abs())),
        _ => None,
    };
    let (within, rms, max) = (bar["within"].as_f64(), bar["rms"].as_f64(), bar["max"].as_f64());
    hold("scan to model: within", in_app["within_all"].as_f64(), within, true);
    hold("scan to model: rms", in_app["rms"].as_f64(), rms, false);
    hold("scan to model: max", farthest(in_app), max, false);
    // A scan with surface missing cannot bear the model out there; the truth does, below.
    if !missing {
        let allowance = config["levels"]["edge_allowance"].as_f64().unwrap_or(0.0) * config["levels"]["edge"].as_f64().unwrap_or(0.0);
        hold("model to scan: within", in_app["model_within"].as_f64(), within, true);
        hold("model to scan: max", in_app["model_max"].as_f64(), max.map(|m| m + allowance), false);
    }
    if !scaled && let (Some(volume), Some(right)) = (truth["volume"].as_f64(), expected["volume"].as_f64()) {
        hold("volume", Some((volume - right).abs() / right), bar["volume"].as_f64(), false);
    }
    // The part itself against the model, both ways: nowhere farther off than a feature may be
    // out of place.
    hold("part to model: max", farthest(truth), bar["position"].as_f64(), false);
    hold("model to part: max", truth["model_max"].as_f64(), bar["position"].as_f64(), false);
    checks
}

/// `docs/parity/report.md`: the benchmark's table, and the capability matrix's counts.
fn markdown(root: &Path, config: &Value, report: &Value) -> String {
    let mut md = String::from("<!-- Generated by `cargo xtask scan-bench`; do not edit. -->\n\n# Scan to CAD: what is proven\n\n");
    md += &format!(
        "Made at `{}`{} by `cargo xtask scan-bench{}`.\n\n",
        text_of(&report["sha"]),
        if report["dirty"] == true { " with uncommitted changes" } else { "" },
        if report["tier"] == "full" { " --full" } else { "" }
    );
    md += "No head-to-head run against another product exists: nothing here says SolidCraft's output matches any other program's. \
           Every number is against a part whose exact answer is known.\n\n";
    md += "## Accuracy benchmark\n\n";
    md += &format!(
        "Each part in `{CASES}/` is built from its recipe, meshed, made into a scan (triangles about {} mm, seed {}) and built back \
         by the scan commands a person would run, following Survey's advice, with nothing tuned for the case. The bars are in `{CONFIG}`.\n\n",
        config["levels"]["edge"], config["levels"]["seed"]
    );
    let s = &report["summary"];
    md += &format!(
        "**{} runs: {} pass, {} fail, {} refused, {} silently wrong, {} built right but not proven in-app.**\n\n",
        s["runs"], s["pass"], s["fail"], s["refused"], s["silent_wrong"], s["missed_pass"]
    );
    md += "- pass: built, within every bar, and Scan Deviation passed it\n- fail: built, outside a bar, and Scan Deviation said so\n\
           - refused: the commands refused and nothing was built\n- SILENT WRONG: outside a bar and Scan Deviation passed it (none is allowed)\n\
           - missed pass: within every bar, but Scan Deviation did not pass it (a scan with surface missing cannot prove the model there)\n\n";
    md += &match report["second_judge"].as_str() {
        Some(judge) => format!("Judges: SolidCraft's own measurements against the scan and the true part, and a second program that shares no code with it (`{judge}`).\n\n"),
        None => "Judge: SolidCraft's own measurements against the scan and against the true part. No second, independent judge was used in this run.\n\n".to_owned(),
    };
    md += "| Part | Scan | Result | Commands run | Outside its bar |\n|---|---|---|---|---|\n";
    for row in report["rows"].as_array().into_iter().flatten() {
        let list = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let outside: Vec<String> = row["failed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|name| match (row["checks"][name]["value"].as_f64(), row["checks"][name]["bar"].as_f64()) {
                (Some(value), Some(bar)) => format!("{name} {value:.3} (bar {bar})"),
                _ => name.to_owned(),
            })
            .collect();
        md += &format!("| {} | {} | {} | {} | {} |\n", text_of(&row["case"]), text_of(&row["level"]), text_of(&row["status"]), list(&row["ran"]), outside.join("; "));
    }
    for (key, title) in [("moved_from_lock", "Numbers that differ from the lock"), ("not_in_lock", "Runs not yet in the lock")] {
        let list: Vec<&str> = report[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !list.is_empty() {
            md += &format!("\n{title} (`{LOCK}`): {}.\n", list.join(", "));
        }
    }
    if let Some(matrix) = std::fs::read_to_string(root.join(MATRIX)).ok().and_then(|text| toml::from_str::<toml::Value>(&text).ok()) {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for row in matrix.get("row").and_then(|r| r.as_array()).into_iter().flatten() {
            *counts.entry(row.get("status").and_then(|s| s.as_str()).unwrap_or("?").to_owned()).or_default() += 1;
        }
        md += &format!("\n## Capabilities\n\nFrom `{MATRIX}` (each row with its commands, tests and notes in `docs/parity/matrix.md`):\n\n| Status | Capabilities |\n|---|---|\n");
        for (status, count) in counts {
            md += &format!("| {status} | {count} |\n");
        }
    }
    md
}

fn text_of(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
}

fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn git(root: &Path, args: &[&str]) -> String {
    Command::new("git").current_dir(root).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default()
}
