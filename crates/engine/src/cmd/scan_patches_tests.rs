//! Fit Patches on a sheet bent a third of the way round a cylinder: too curved for one height
//! field, it fits as one patch, as blocks sewn into one body, and as a quad on its own edges.

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn bent_scan(s: &mut Session) -> String {
    let n = 30u32;
    let at = |i: u32, j: u32| {
        let a = f64::from(i) / f64::from(n) * 2.1 - 1.05;
        Vec3::new(20.0 * a.sin(), f64::from(j), 20.0 * (1.0 - a.cos()))
    };
    let mut stl = String::from("solid s\n");
    for j in 0..30 {
        for i in 0..n {
            for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                stl.push_str("facet normal 0 0 0\nouter loop\n");
                for q in tri {
                    stl.push_str(&format!("vertex {} {} {}\n", q.x, q.y, q.z));
                }
                stl.push_str("endloop\nendfacet\n");
            }
        }
    }
    stl.push_str("endsolid s\n");
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-patches-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bent.stl");
    std::fs::write(&path, stl).unwrap();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap()
}

#[test]
fn a_curving_scan_fits_as_a_grid_as_blocks_and_as_a_quad() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let r = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0]}));
    assert_eq!((r["layout"].as_str(), r["patches"].as_u64()), (Some("grid"), Some(1)));
    assert!(r["rms"].as_f64().unwrap() < 0.05, "{r}");
    // A clean surface: no folds, pinches or crossings, and nothing to warn of.
    assert_eq!((r["folds"].as_u64(), r["pinches"].as_u64(), r["crossings"].as_u64()), (Some(0), Some(0), Some(0)), "{r}");
    assert!(r.get("warning").is_none(), "{r}");
    let one = r["surface"].as_str().unwrap().to_owned();
    assert_eq!(s.model.state().body(&one).map(|b| b.body.face_count()), Some(1));
    let r = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "layout": "blocks", "rows": 2, "columns": 3}));
    assert_eq!(r["patches"], 6, "{r}");
    let blocks = r["surface"].as_str().unwrap().to_owned();
    assert_eq!(s.model.state().body(&blocks).map(|b| b.body.face_count()), Some(6), "six faces sewn into one body");
    let r = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "layout": "quads", "controls": 8}));
    assert!(r["rms"].as_f64().unwrap() < 0.1, "{r}");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(r["surface"].as_str().unwrap()).is_none(), "one undo step");
    let e = s.execute("scan.fit_patches", &json!({"body": scan, "point": [0, 15, 0], "layout": "rings"})).unwrap_err().to_string();
    assert!(e.contains("`layout`"), "{e}");
}

/// With a tolerance and no control points, the fit takes as few as bring it within half of it: a
/// loose tolerance few, a tight one more and closer; control points given win over it.
#[test]
fn a_tolerance_chooses_the_control_points() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let loose = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "tolerance": 1.0}));
    let tight = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "tolerance": 0.002}));
    let controls = |r: &Value| r["controls"][0].as_u64().unwrap();
    assert!(loose["rms"].as_f64().unwrap() <= 0.5 && controls(&loose) == 6, "{loose}");
    assert!(controls(&tight) > controls(&loose) && tight["rms"].as_f64() < loose["rms"].as_f64(), "{loose} {tight}");
    let given = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "tolerance": 1.0, "controls": 12}));
    assert_eq!(given["controls"], json!([12, 12]), "{given}");
    let e = s.execute("scan.fit_patches", &json!({"body": scan, "point": [0, 15, 0], "tolerance": 0})).unwrap_err().to_string();
    assert!(e.contains("`tolerance`"), "{e}");
}

/// A surface that folds is said to, in words a person can act on.
#[test]
fn a_folded_surface_is_told_of_in_plain_words() {
    use super::super::surface::{fold_warning, trouble_text};
    assert_eq!(fold_warning([0, 0, 0]), None);
    assert_eq!(trouble_text([128, 0, 722]), "128 folds, 722 crossings");
    assert_eq!(trouble_text([1, 1, 0]), "1 fold, 1 pinch");
    let w = fold_warning([3, 0, 0]).unwrap();
    assert!(w.contains("folds over itself (3 folds)") && w.contains("smaller region"), "{w}");
}

/// A part chosen by selecting is fitted again like one chosen by a click: the recipe keeps its
/// triangles with how many the scan had, `scan.refit` fits them again, and once the scan has been
/// reduced (its triangles numbered anew) it says the part is not the same instead of fitting
/// other triangles. A click's Radius keeps the region to what is that near.
#[test]
fn a_selected_part_is_fitted_again_until_the_scan_is_renumbered() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let half: Vec<usize> = {
        let state = s.model.state();
        let (positions, triangles) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).unwrap();
        (0..triangles.len()).filter(|&t| triangles[t].iter().map(|&i| positions[i as usize].y).sum::<f64>() / 3.0 < 15.0).collect()
    };
    run(&mut s, "scan.select", json!({"body": scan, "triangles": half}));
    let made = run(&mut s, "scan.fit_patches", json!({}));
    let feature = made["feature"]["feature"].as_u64().unwrap();
    let recipe = crate::cmd::scan::refit::recipe_of(&s, feature).expect("a part selected is kept as a recipe too");
    assert_eq!(recipe.params["triangles"].as_array().map(Vec::len), Some(half.len()), "{}", recipe.params);
    assert!(recipe.params["of_triangles"].as_u64().unwrap() > half.len() as u64);
    let again = run(&mut s, "scan.refit", json!({"surface": made["surface"]}));
    assert_eq!(again["faces"], 1, "{again}");

    // Reduced, the same numbers are other triangles: refused in words.
    run(&mut s, "scan.reduce", json!({"ratio": 0.5}));
    let refused = s.execute("scan.refit", &json!({"surface": made["surface"]})).unwrap_err().to_string();
    assert!(refused.contains("select it again"), "{refused}");
}

/// A click's `radius` keeps the region to what is that near it.
#[test]
fn a_clicks_radius_bounds_the_region() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let whole = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0]}));
    let near = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0, 15, 0], "radius": 6}));
    assert!(near["points"].as_u64().unwrap() < whole["points"].as_u64().unwrap() / 2, "{near} against {whole}");
}
