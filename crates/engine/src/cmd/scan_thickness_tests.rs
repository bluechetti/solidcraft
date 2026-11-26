//! Wall Thickness on a hollow box: a box with a cavity, 40 × 30 × 20 outside and walls of 3.

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// A closed box from `lo` to `hi`, wound outward (or, with `inward`, the other way: a cavity).
fn slab(lo: Vec3, hi: Vec3, inward: bool, at: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    let triangles =
        [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    let triangles = triangles.iter().map(|t| if inward { [t[0] + at, t[2] + at, t[1] + at] } else { [t[0] + at, t[1] + at, t[2] + at] }).collect();
    ((0..8).map(p).collect(), triangles)
}

/// The hollow box inserted as a scan; its body's name.
fn hollow_box(s: &mut Session, inside_out: bool) -> String {
    let (mut positions, mut triangles) = slab(Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 30.0, 20.0), inside_out, 0);
    let (p, t) = slab(Vec3::new(3.0, 3.0, 3.0), Vec3::new(37.0, 27.0, 17.0), !inside_out, 8);
    positions.extend(p);
    triangles.extend(t);
    insert(s, &positions, &triangles, &format!("{inside_out}"))
}

/// A closed mesh inserted as a scan; its body's name.
fn insert(s: &mut Session, positions: &[Vec3], triangles: &[[u32; 3]], tag: &str) -> String {
    let mut stl = vec![0u8; 80];
    stl.extend((triangles.len() as u32).to_le_bytes());
    for t in triangles {
        stl.extend([0u8; 12]);
        for &i in t {
            let q = positions[i as usize];
            for c in [q.x, q.y, q.z] {
                stl.extend((c as f32).to_le_bytes());
            }
        }
        stl.extend([0u8; 2]);
    }
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-thickness-{}-{:?}-{tag}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("box.stl");
    std::fs::write(&path, stl).unwrap();
    let before: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh() && !before.contains(&b.name)).map(|b| b.name.clone()).unwrap()
}

#[test]
fn a_hollow_box_reads_three_millimetres_everywhere() {
    let mut s = Session::default();
    let body = hollow_box(&mut s, false);
    let r = run(&mut s, "scan.wall_thickness", json!({"body": body, "samples": 600, "below": 4, "up_to": 10}));
    // Along the box's solid rims a ray runs 30 or 40 mm through material: no wall at `up_to` 10.
    assert!(r["measured"].as_u64().unwrap() >= 400 && r["no_wall"].as_u64().unwrap() > 0, "{r}");
    for key in ["p05", "median", "p95", "mean"] {
        assert!((r[key].as_f64().unwrap() - 3.0).abs() < 1e-3, "{key}: {r}");
    }
    assert_eq!(r["looking_back"], 0, "{r}");
    assert!((r["share_below"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{r}");
    let thin = r["thin"].as_array().unwrap();
    assert!(!thin.is_empty() && thin.len() <= 20, "{r}");
    // The same places every time.
    assert_eq!(r, run(&mut s, "scan.wall_thickness", json!({"body": body, "samples": 600, "below": 4, "up_to": 10})));
}

#[test]
fn probes_read_where_they_are_put_and_up_to_leaves_out_the_thick() {
    let mut s = Session::default();
    let body = hollow_box(&mut s, false);
    let r = run(&mut s, "scan.wall_thickness", json!({"probes": [[20, 15, 20], [20, 15, 17], [0, 15, 10]]}));
    assert_eq!(r["body"], body);
    let probes = r["probes"].as_array().unwrap();
    for (i, want) in [(0, 3.0), (1, 3.0), (2, 3.0)] {
        assert!((probes[i]["reading"]["thickness"].as_f64().unwrap() - want).abs() < 1e-3, "{i}: {r}");
    }
    let across = probes[0]["reading"]["across"].as_array().unwrap();
    assert!((across[2].as_f64().unwrap() - 17.0).abs() < 1e-3, "the far side of the lid is the cavity's roof: {r}");
    // With probes given, no samples are read unless asked.
    assert!(r["measured"] == 0 && r.get("thinnest").is_none(), "{r}");
}

#[test]
fn flip_sends_the_rays_the_other_way() {
    let mut s = Session::default();
    hollow_box(&mut s, false);
    let along = run(&mut s, "scan.wall_thickness", json!({"samples": 200}));
    assert!((along["median"].as_f64().unwrap() - 3.0).abs() < 1e-3, "{along}");
    // Flipped, the rays leave the outside faces into nothing, and from the cavity's faces cross the cavity.
    let flipped = run(&mut s, "scan.wall_thickness", json!({"samples": 200, "flip": true}));
    assert!(flipped["median"].as_f64().unwrap() > 10.0 || flipped["measured"].as_u64().unwrap() < 100, "{flipped}");
    assert!(flipped["no_wall"].as_u64().unwrap() > 0, "{flipped}");
}

#[test]
fn hostile_parameters_are_refused_not_panicked_on() {
    let mut s = Session::default();
    hollow_box(&mut s, false);
    for p in [
        json!({"samples": 0}),
        json!({"samples": 1e9}),
        json!({"samples": 2.5}),
        json!({"samples": "many"}),
        json!({"probes": "here"}),
        json!({"probes": [[1, 2]]}),
        json!({"probes": [[1, 2, "x"]]}),
        json!({"up_to": -5}),
        json!({"below": 0}),
        json!({"up_to": "far"}),
    ] {
        assert!(s.execute("scan.wall_thickness", &p).is_err(), "{p}");
    }
    let empty = Session::default();
    let mut empty = empty;
    assert!(empty.execute("scan.wall_thickness", &json!({})).is_err());
}

#[test]
fn a_face_the_ray_only_grazes_is_not_the_other_side_of_a_wall() {
    // A right-angled wedge, 40 wide and 100 tall at its tall end: a ray up from the base meets the
    // slanted face at about 68 degrees off square (cosine 0.37), a ray along the tall side meets it square on.
    let p = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let positions = [p(0.0, 0.0, 0.0), p(40.0, 0.0, 0.0), p(40.0, 0.0, 100.0), p(0.0, 10.0, 0.0), p(40.0, 10.0, 0.0), p(40.0, 10.0, 100.0)];
    let triangles = [[0, 1, 2], [3, 5, 4], [0, 3, 1], [3, 4, 1], [1, 4, 2], [4, 5, 2], [0, 2, 3], [2, 5, 3]];
    let mut s = Session::default();
    insert(&mut s, &positions, &triangles, "wedge");
    let r = run(&mut s, "scan.wall_thickness", json!({"probes": [[30, 5, 0], [40, 5, 50]]}));
    let probes = r["probes"].as_array().unwrap();
    assert_eq!(probes[0]["miss"], "no_wall", "{r}");
    assert!((probes[1]["reading"]["thickness"].as_f64().unwrap() - 20.0).abs() < 1e-3, "{r}");
}
