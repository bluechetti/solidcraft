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
