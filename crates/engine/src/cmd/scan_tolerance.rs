//! The tolerance of a scan (`scan.tolerance`): how far off its surface still counts as on it, kept
//! in the design per scan. Scan Deviation, Free Form, Edit Surface, Fit Patches and Flat Template
//! all colour and report against it, so it is set once and every tool answers "is this still
//! faithful?" the same way. [`of`] is what they ask.

use serde_json::Value;
use solidcraft_doc::scan_tolerance::{DEFAULT_MM, MAX_NAME, MAX_SCANS, valid};

use super::{CommandSpec, mesh_name};
use crate::params::{bad, bool_};
use crate::schema::scan_tolerance::{ScanToleranceParams, ScanToleranceResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanToleranceParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.tolerance", "Scan Tolerance", crate::checked!("scan.tolerance", tolerance))
        .icon("curvature_map")
        .contract(&CONTRACT)
        .output(output_schema_for::<ScanToleranceResult>)
        .params("scan? (or body?): mesh body (default: the selected or only one); tolerance?: length above 0; reset?: bool (back to the default, 0.1 mm): how far off the scan's surface still counts as on it, kept in the design for that scan and used by Scan Deviation, Free Form, Edit Surface, Fit Patches and Flat Template; with neither tolerance nor reset it changes nothing; one undo step when it changes → scan, tolerance, set, changed"),
];

/// A scan's tolerance in mm: the one set for it, else the default.
pub fn of(s: &Session, scan: &str) -> f64 {
    s.doc.scan_tolerances.get(scan).copied().filter(|t| valid(*t)).unwrap_or(DEFAULT_MM)
}

/// The tolerance a scan with `noise` (mm) is read against so that a faithful fit reads as on it:
/// three times the noise, rounded up to a hundredth of a millimetre, and never under the default.
pub fn for_noise(noise: f64) -> f64 {
    let three = (300.0 * noise).ceil() / 100.0;
    if three.is_finite() && valid(three) { three.max(DEFAULT_MM) } else { DEFAULT_MM }
}

/// The noise of the scan `scan` in mm (Survey Scan's reading of it), `None` for a body that is not
/// a scan or too small to read. Reads the whole scan: on a large one, call it beside the window.
pub fn noise(s: &Session, scan: &str) -> Option<f64> {
    let state = s.model.state();
    let (positions, triangles) = state.body(scan)?.body.triangle_mesh()?;
    solidcraft_scan::survey::noise(&solidcraft_scan::Scan::new(positions, triangles)).filter(|n| n.is_finite() && *n >= 0.0)
}

/// Whether `scan` has a tolerance of its own.
pub fn is_set(s: &Session, scan: &str) -> bool {
    s.doc.scan_tolerances.contains_key(scan)
}

fn tolerance(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.tolerance";
    // `scan`, or `body` as every other scan command names it.
    let named = match (crate::params::str_(p, "scan"), crate::params::str_(p, "body")) {
        (Some(a), Some(b)) if a != b => return Err(bad(cmd, format!("`scan` (`{a}`) and `body` (`{b}`) name different scans: give one"))),
        (a, b) => a.or(b),
    };
    let scan = mesh_name(s, &serde_json::json!({ "body": named }), cmd)?;
    let reset = bool_(p, "reset") == Some(true);
    let given = crate::params::expr(p, "tolerance");
    if reset && given.is_some() {
        return Err(bad(cmd, "give a `tolerance` or `reset`, not both"));
    }
    let before = of(s, &scan);
    let was_set = is_set(s, &scan);
    if reset {
        if was_set {
            s.doc_mut().scan_tolerances.remove(&scan);
        }
    } else if let Some(e) = given {
        let mm = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tolerance: {e}")))?;
        if !valid(mm) {
            return Err(bad(cmd, "`tolerance` must be a length above 0"));
        }
        if scan.chars().count() > MAX_NAME || (!was_set && s.doc.scan_tolerances.len() >= MAX_SCANS) {
            return Err(bad(cmd, "the design keeps as many scan tolerances as it can"));
        }
        if !(was_set && (mm - before).abs() <= f64::EPSILON * before.abs()) {
            s.doc_mut().scan_tolerances.insert(scan.clone(), mm);
        }
    }
    let now = of(s, &scan);
    let reply = ScanToleranceResult {
        set: is_set(s, &scan),
        changed: now != before || is_set(s, &scan) != was_set,
        tolerance: now,
        scan,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: serde_json::Value) -> serde_json::Value {
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// A scan with no tolerance reads the default; one set is kept in the design, undone with one
    /// undo, read by Scan Deviation as its default and taken back by reset; nonsense is refused.
    #[test]
    fn a_scan_keeps_one_tolerance_set_undone_and_reset() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-tol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tri.stl");
        std::fs::write(&path, "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid t\n")
            .unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        run(&mut s, "file.insert_mesh", json!({"path": path}));
        let read = run(&mut s, "scan.tolerance", json!({}));
        assert_eq!((read["tolerance"].as_f64(), read["set"].as_bool(), read["changed"].as_bool()), (Some(0.1), Some(false), Some(false)));
        let set = run(&mut s, "scan.tolerance", json!({"tolerance": 0.25}));
        assert_eq!((set["tolerance"].as_f64(), set["set"].as_bool(), set["changed"].as_bool()), (Some(0.25), Some(true), Some(true)));
        assert_eq!(super::of(&s, set["scan"].as_str().unwrap()), 0.25);
        let again = run(&mut s, "scan.tolerance", json!({"tolerance": 0.25}));
        assert_eq!(again["changed"].as_bool(), Some(false), "the same value changes nothing");
        for bad in [json!({"tolerance": 0}), json!({"tolerance": -1}), json!({"tolerance": 0.1, "reset": true})] {
            assert!(s.execute("scan.tolerance", &bad).is_err(), "{bad}");
        }
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(run(&mut s, "scan.tolerance", json!({}))["set"].as_bool(), Some(false), "one undo takes it back");
        run(&mut s, "scan.tolerance", json!({"tolerance": 0.3}));
        let reset = run(&mut s, "scan.tolerance", json!({"reset": true}));
        assert_eq!((reset["tolerance"].as_f64(), reset["set"].as_bool()), (Some(0.1), Some(false)));
    }

    /// A noisy scan's tolerance is three times its noise, rounded up; a clean one keeps the
    /// default, and nonsense noise gives the default.
    #[test]
    fn the_tolerance_for_a_noise_is_three_times_it() {
        assert_eq!(super::for_noise(0.1), 0.3);
        assert_eq!(super::for_noise(0.0412), 0.13);
        assert_eq!(super::for_noise(0.001), 0.1, "never under the default");
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(super::for_noise(bad), 0.1, "{bad}");
        }
    }
}
