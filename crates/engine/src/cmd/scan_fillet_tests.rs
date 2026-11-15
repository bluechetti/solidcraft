//! Fillet from Scan on a scan of a block whose top front edge is rounded at 3 mm, measured
//! against the sharp block it was modelled as.

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// The mesh with every triangle cut into four `levels` times, so its points spread as a scan's.
fn subdivided(mut positions: Vec<Vec3>, mut triangles: Vec<[u32; 3]>, levels: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    for _ in 0..levels {
        let mut middles: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        let mut middle = |a: u32, b: u32, positions: &mut Vec<Vec3>| {
            *middles.entry((a.min(b), a.max(b))).or_insert_with(|| {
                positions.push((positions[a as usize] + positions[b as usize]) * 0.5);
                (positions.len() - 1) as u32
            })
        };
        let mut next = Vec::with_capacity(triangles.len() * 4);
        for [a, b, c] in triangles {
            let (ab, bc, ca) = (middle(a, b, &mut positions), middle(b, c, &mut positions), middle(c, a, &mut positions));
            next.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        triangles = next;
    }
    (positions, triangles)
}

/// A scan of the block with its top front edge rounded at 3 mm, inserted into `s`.
fn rounded_scan(s: &mut Session) -> String {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Rounded"}));
    run(&mut part, "solid.fillet", json!({"edges": [[30, 0, 20]], "radius": 3, "body": "Rounded"}));
    let mesh = {
        let state = part.model.state();
        state.body("Rounded").unwrap().body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.005, angle: Some(3f64.to_radians()) }).unwrap()
    };
    let (positions, triangles) = subdivided(mesh.positions, mesh.triangles, 2);
    let mut stl = vec![0u8; 80];
    stl.extend((triangles.len() as u32).to_le_bytes());
    for t in &triangles {
        stl.extend([0u8; 12]);
        for &i in t {
            let p = positions[i as usize];
            for c in [p.x, p.y, p.z] {
                stl.extend((c as f32).to_le_bytes());
            }
        }
        stl.extend([0u8; 2]);
    }
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-fillet-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rounded.stl");
    std::fs::write(&path, stl).unwrap();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap()
}

/// The round is measured at 3 mm from the scan; applied, the model's edge is filleted at it in
/// one undo step.
#[test]
fn the_round_the_scan_shows_is_measured_and_filleted() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Part"}));
    let scan = rounded_scan(&mut s);
    let r = run(&mut s, "scan.fillet", json!({"edges": [[30, 0, 20]], "body": "Part", "scan": scan, "apply": false}));
    assert!((r["fitted"].as_f64().unwrap() - 3.0).abs() < 0.1, "{r}");
    assert_eq!(r["radius"].as_f64(), Some(3.0));
    assert!(r["sharp_rms"].as_f64().unwrap() > 3.0 * r["rms"].as_f64().unwrap(), "{r}");
    assert!(r.get("feature").is_none(), "measuring alone changes nothing");
    let faces = s.model.state().body("Part").map(|b| b.body.face_count());
    let r = run(&mut s, "scan.fillet", json!({"edges": [[30, 0, 20]], "body": "Part", "scan": scan}));
    assert!(r.get("feature").is_some(), "{r}");
    assert_eq!(s.model.state().body("Part").map(|b| b.body.face_count()), faces.map(|f| f + 1), "a round face more");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.model.state().body("Part").map(|b| b.body.face_count()), faces);
    // A sharp edge of the scan shows no round.
    let e = s.execute("scan.fillet", &json!({"edges": [[30, 40, 20]], "body": "Part", "scan": scan, "apply": false})).unwrap_err().to_string();
    assert!(e.contains("sharp") || e.contains("no better"), "{e}");
}

/// A tap on the scan's round reads 3 mm from the scan alone, finds the model's edge along it,
/// and with `apply` fillets that edge in one undo step; a tap on a flat face finds no round.
#[test]
fn a_tap_on_the_scan_reads_the_round_and_fillets_its_edge() {
    let mut s = Session::default();
    let scan = rounded_scan(&mut s);
    let tap = [30.0, 3.0 - 3.0 * std::f64::consts::FRAC_PI_4.cos(), 17.0 + 3.0 * std::f64::consts::FRAC_PI_4.sin()];
    let r = run(&mut s, "scan.radius", json!({"at": tap, "scan": scan}));
    assert_eq!(r["radius"].as_f64(), Some(3.0), "{r}");
    assert_eq!(r["convex"].as_bool(), Some(true), "{r}");
    assert!((r["swept"].as_f64().unwrap() - 90.0).abs() < 10.0, "{r}");
    assert!(r.get("edge").is_none(), "no model yet: {r}");
    let e = s.execute("scan.radius", &json!({"at": tap, "scan": scan, "apply": true})).unwrap_err().to_string();
    assert!(e.contains("no edge"), "{e}");
    run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Part"}));
    let r = run(&mut s, "scan.radius", json!({"at": tap, "scan": scan}));
    assert_eq!(r["edge"]["body"].as_str(), Some("Part"), "{r}");
    let faces = s.model.state().body("Part").map(|b| b.body.face_count());
    let r = run(&mut s, "scan.radius", json!({"at": tap, "scan": scan, "apply": true}));
    assert!(r.get("feature").is_some(), "{r}");
    assert_eq!(s.model.state().body("Part").map(|b| b.body.face_count()), faces.map(|f| f + 1), "a round face more");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.model.state().body("Part").map(|b| b.body.face_count()), faces);
    assert!(s.execute("scan.radius", &json!({"at": [30, 20, 20], "scan": scan})).is_err(), "the flat top is no round");
}
