//! The scan mesh tools on scans made here: a part modelled, meshed finely and inserted as scans.

use std::collections::HashMap;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// The mesh with every triangle cut into four `levels` times, the cuts shared along each edge.
fn subdivided(mut positions: Vec<Vec3>, mut triangles: Vec<[u32; 3]>, levels: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    for _ in 0..levels {
        let mut middles: HashMap<(u32, u32), u32> = HashMap::new();
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

/// A block with a boss at one corner (no turn of it is itself), meshed finely: positions and triangles.
pub(super) fn part_mesh() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"radius": 6, "height": 12, "base": [12, 10, 20], "operation": "join", "targets": [body]}));
    let state = part.model.state();
    let mesh = state.body(&body).unwrap().body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.01, angle: Some(4f64.to_radians()) }).unwrap();
    subdivided(mesh.positions, mesh.triangles, 3)
}

/// `positions` turned by `angle` about the unit `axis` through the origin, then moved.
pub(super) fn turned(positions: &[Vec3], axis: Vec3, angle: f64, shift: Vec3) -> Vec<Vec3> {
    let (s, c) = angle.sin_cos();
    positions.iter().map(|p| *p * c + axis.cross(*p) * s + axis * (axis.dot(*p) * (1.0 - c)) + shift).collect()
}

/// The mesh inserted as a scan named after `name`; its body's name.
pub(super) fn insert(s: &mut Session, positions: &[Vec3], triangles: &[[u32; 3]], name: &str) -> String {
    let mut stl = vec![0u8; 80];
    stl.extend((triangles.len() as u32).to_le_bytes());
    for t in triangles {
        stl.extend([0u8; 12]);
        for &i in t {
            let p = positions[i as usize];
            for c in [p.x, p.y, p.z] {
                stl.extend((c as f32).to_le_bytes());
            }
        }
        stl.extend([0u8; 2]);
    }
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-mesh-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.stl"));
    std::fs::write(&path, stl).unwrap();
    let before: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh() && !before.contains(&b.name)).map(|b| b.name.clone()).unwrap()
}

fn positions_of(s: &Session, body: &str) -> Vec<Vec3> {
    s.model.state().body(body).and_then(|b| b.body.triangle_mesh()).map(|(p, _)| p.to_vec()).unwrap()
}

/// A second pass turned half round and moved far off is laid back onto the first as one Move;
/// undone, it lies where it came.
#[test]
fn a_second_pass_is_laid_onto_the_first_as_a_move() {
    let (positions, triangles) = part_mesh();
    let axis = Vec3::new(-0.4, 0.2, 0.9).normalized().unwrap();
    let moved = turned(&positions, axis, 2.6, Vec3::new(140.0, -60.0, 35.0));
    let mut s = Session::default();
    let first = insert(&mut s, &positions, &triangles, "first");
    let second = insert(&mut s, &moved, &triangles, "second");
    let came = positions_of(&s, &second);
    let reply = run(&mut s, "scan.register", json!({}));
    assert_eq!((reply["body"].as_str(), reply["to"].as_str()), (Some(second.as_str()), Some(first.as_str())), "{reply}");
    assert_eq!(reply["moved"], true);
    assert!(reply["matched"].as_f64().unwrap() > 0.99 && reply["rms_after"].as_f64().unwrap() < 1e-3, "{reply}");
    let now = positions_of(&s, &second);
    let worst = now.iter().zip(positions_of(&s, &first)).map(|(a, b)| a.dist(b)).fold(0.0, f64::max);
    // STL keeps single precision, so each pass is exact to about 1e-5 mm of where it was.
    assert!(worst < 1e-2, "{worst}");
    run(&mut s, "edit.undo", json!({}));
    assert!(positions_of(&s, &second).iter().zip(&came).all(|(a, b)| a.dist(*b) < 1e-9), "undone, it lies where it came");
}

#[test]
fn align_to_scan_says_which_scans_it_needs() {
    let mut s = Session::default();
    let none = s.execute("scan.register", &json!({})).unwrap_err().to_string();
    assert!(none.contains("no scan"), "{none}");
    let (positions, triangles) = part_mesh();
    insert(&mut s, &positions, &triangles, "only");
    let one = s.execute("scan.register", &json!({})).unwrap_err().to_string();
    assert!(one.contains("one scan"), "{one}");
    let shares = s.execute("scan.register", &json!({"to": "nothing", "ignored": 0.9})).unwrap_err().to_string();
    assert!(shares.contains("`ignored`"), "{shares}");
}
