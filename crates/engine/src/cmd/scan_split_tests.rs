//! Split Mesh on a box scan: by a plane, a fitted shape, another body, a 3D curve and a sketch,
//! keeping one side, both as scans, an envelope's slab, or only the crossing.

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// A closed box from the origin to `size`, each side a grid of `n` × `n` squares, wound outward.
fn grid_box(size: [f64; 3], n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
        let (u, v) = match (axis, high) {
            (0, false) => (2, 1),
            (0, true) => (1, 2),
            (1, false) => (0, 2),
            (1, true) => (2, 0),
            (2, false) => (1, 0),
            _ => (0, 1),
        };
        let base = positions.len() as u32;
        for j in 0..=n {
            for i in 0..=n {
                let mut c = [0.0; 3];
                if high {
                    c[axis] = size[axis];
                }
                c[u] = size[u] * f64::from(i) / f64::from(n);
                c[v] = size[v] * f64::from(j) / f64::from(n);
                positions.push(Vec3::new(c[0], c[1], c[2]));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let k = base + j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
    }
    (positions, triangles)
}

/// The box inserted as a scan; its body's name.
fn scan(s: &mut Session) -> String {
    let (positions, triangles) = grid_box([40.0, 30.0, 20.0], 20);
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
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-split-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("block.stl");
    std::fs::write(&path, stl).unwrap();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap()
}

fn mesh_of(s: &Session, body: &str) -> (Vec<Vec3>, usize) {
    s.model.state().body(body).and_then(|b| b.body.triangle_mesh()).map(|(p, t)| (p.to_vec(), t.len())).unwrap()
}

fn scans(s: &Session) -> Vec<String> {
    s.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// Cut across by a plane, both kept: the scan keeps the top, a new scan the bottom; one undo
/// takes it all back.
#[test]
fn a_plane_splits_the_scan_into_two_scans_in_one_undo_step() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let before = mesh_of(&s, &body).1;
    let r = run(&mut s, "scan.split", json!({"plane": {"origin": [0, 0, 8], "normal": [0, 0, 1]}}));
    assert_eq!(r["keep"], "both");
    assert_eq!(r["sides"], json!(["behind", "in_front"]));
    assert_eq!((r["runs"].as_u64(), r["closed_runs"].as_u64()), (Some(1), Some(1)));
    assert!((r["length"].as_f64().unwrap() - 140.0).abs() < 1e-6, "{r}");
    let other = r["made"][0]["body"].as_str().unwrap().to_owned();
    assert_eq!(other, format!("{body} Behind"));
    let (top, _) = mesh_of(&s, &body);
    let (bottom, _) = mesh_of(&s, &other);
    assert!(top.iter().all(|q| q.z >= 8.0 - 1e-6) && bottom.iter().all(|q| q.z <= 8.0 + 1e-6));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(scans(&s), vec![body.clone()], "one undo takes the new scan away");
    assert_eq!(mesh_of(&s, &body).1, before, "and puts the scan back");
}

/// A construction plane up the box, the side behind it kept: written back into the scan, no new
/// scan made.
#[test]
fn one_side_kept_is_written_back_into_the_scan() {
    let mut s = Session::default();
    let body = scan(&mut s);
    run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 12, "name": "Cut Here"}));
    let r = run(&mut s, "scan.split", json!({"body": body, "plane": "Cut Here", "keep": "back"}));
    assert!(r.get("made").is_none());
    assert_eq!(r["cutter"], "plane Cut Here");
    let (kept, _) = mesh_of(&s, &body);
    assert!(kept.iter().all(|q| q.z <= 12.0 + 1e-6));
    assert!(kept.iter().any(|q| (q.z - 12.0).abs() < 1e-6), "the cut's edge is on the plane");
    assert_eq!(scans(&s).len(), 1);
}

#[test]
fn a_cylinder_cuts_a_disc_out_of_the_top_and_bottom_and_an_envelope_makes_a_slab() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let r = run(&mut s, "scan.split", json!({"shape": {"kind": "cylinder", "point": [20, 15, 0], "axis": [0, 0, 1], "radius": 6}}));
    assert_eq!(r["sides"], json!(["inside", "outside"]));
    assert_eq!(r["runs"].as_u64(), Some(2));
    assert_eq!(r["made"][0]["body"].as_str(), Some(format!("{body} Inside").as_str()));

    // A plane fitted where the top was clicked cuts nothing off: the top lies on it. One fitted
    // to the side wall at x = 40 is the box's own face too; fitted shapes cut as given ones do.
    let mut s = Session::default();
    scan(&mut s);
    let e = s.execute("scan.split", &json!({"shape": {"kind": "plane", "at": [20, 15, 20]}})).unwrap_err().to_string();
    assert!(e.contains("does not cut"), "{e}");

    let mut s = Session::default();
    let body = scan(&mut s);
    let r = run(&mut s, "scan.split", json!({"plane": {"origin": [0, 0, 10], "normal": [0, 0, 1]}, "envelope": 1}));
    let made: Vec<&str> = r["made"].as_array().unwrap().iter().map(|m| m["side"].as_str().unwrap()).collect();
    assert_eq!(made, ["behind", "slab"]);
    let slab = r["made"][1]["body"].as_str().unwrap();
    assert!(mesh_of(&s, slab).0.iter().all(|q| (q.z - 10.0).abs() <= 0.5 + 1e-6));
    assert_eq!(s.selection, vec![crate::Sel::Body { name: slab.to_owned() }], "the slab is chosen, ready to delete");
    let _ = body;
}

#[test]
fn another_body_cuts_the_scan_where_it_reaches() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let made = run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [10, 10, 15]}));
    let cutter = made["created_bodies"][0].as_str().unwrap().to_owned();
    let r = run(&mut s, "scan.split", json!({"body": body, "cutter": cutter, "keep": "outside"}));
    assert_eq!(r["runs"].as_u64(), Some(1), "one ring where the box goes through the top: {r}");
    // The square the box covers on the top is gone.
    let (kept, _) = mesh_of(&s, &body);
    assert!(!kept.iter().any(|q| (q.z - 20.0).abs() < 1e-6 && q.x > 10.5 && q.x < 19.5 && q.y > 10.5 && q.y < 19.5));
}

#[test]
fn a_closed_curve_on_the_top_with_an_offset_cuts_only_under_it() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let square = json!([[5, 5, 20], [15, 5, 20], [15, 15, 20], [5, 15, 20]]);
    let r = run(&mut s, "scan.split", json!({"curve": {"points": square, "closed": true}, "keep": "outside"}));
    assert_eq!(r["runs"].as_u64(), Some(2), "right through: the top and the bottom");
    run(&mut s, "edit.undo", json!({}));
    let r = run(&mut s, "scan.split", json!({"curve": {"points": square, "closed": true}, "keep": "outside", "offset": 1}));
    assert_eq!(r["runs"].as_u64(), Some(1), "held to the curve: the top only");
    let (kept, _) = mesh_of(&s, &body);
    assert!(kept.iter().any(|q| q.z < 1e-6 && q.x > 6.0 && q.x < 14.0 && q.y > 6.0 && q.y < 14.0), "the bottom under it stays");
}

#[test]
fn a_sketch_circle_cuts_and_only_the_crossing_can_be_kept_as_curves() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let before = mesh_of(&s, &body).1;
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cutter"}));
    run(&mut s, "sketch.circle.center", json!({"center": [20, 15], "radius": 6}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "scan.split", json!({"sketch": "Cutter", "keep": "curves"}));
    assert_eq!(r["curves"].as_array().map(Vec::len), Some(2), "a circle on the top and one on the bottom: {r}");
    assert_eq!(mesh_of(&s, &body).1, before, "the scan untouched");
    let sketch = s.doc.features.iter().find(|f| f.name == super::SKETCH_NAME).map(|f| f.id).expect("Split Curves");
    let state = s.model.state();
    let wires = &state.sketch(sketch).unwrap().sketch.wires;
    for w in wires {
        let off = w.pts.iter().map(|q| ((q.x - 20.0).hypot(q.y - 15.0) - 6.0).abs()).fold(0.0, f64::max);
        assert!(off < 0.05, "each curve lies on the circle: {off}");
    }
}

#[test]
fn bad_cutters_are_refused_with_the_reason() {
    let mut s = Session::default();
    let body = scan(&mut s);
    for (p, why) in [
        (json!({}), "give a cutter"),
        (json!({"plane": "XY", "shape": {"kind": "sphere", "center": [0, 0, 0], "radius": 1}}), "one cutter"),
        (json!({"plane": {"origin": [0, 0, 50], "normal": [0, 0, 1]}}), "does not cut"),
        (json!({"shape": {"kind": "cylinder", "point": [0, 0, 0], "axis": [0, 0, 0], "radius": 1}}), "direction"),
        (json!({"cutter": body}), "itself"),
        (json!({"plane": "XY", "keep": "sideways"}), "`keep`"),
        (json!({"plane": "XY", "envelope": -1}), "envelope"),
    ] {
        let e = s.execute("scan.split", &p).expect_err(&p.to_string()).to_string();
        assert!(e.contains(why), "{p}: {e}");
    }
}

/// The box's top selected: parted along its edge, both kept, the top is a scan of its own.
#[test]
fn what_is_selected_is_parted_into_a_scan_of_its_own() {
    let mut s = Session::default();
    let body = scan(&mut s);
    let tops: Vec<usize> = {
        let state = s.model.state();
        let (p, t) = state.body(&body).and_then(|b| b.body.triangle_mesh()).unwrap();
        t.iter().enumerate().filter(|(_, tri)| tri.iter().all(|&v| (p[v as usize].z - 20.0).abs() < 1e-6)).map(|(i, _)| i).collect()
    };
    run(&mut s, "scan.select", json!({"body": body, "triangles": tops}));
    let r = run(&mut s, "scan.split", json!({"selection": true}));
    assert_eq!(r["cutter"], "selection");
    assert_eq!(r["sides"], json!(["selected", "rest"]));
    assert_eq!((r["runs"].as_u64(), r["closed_runs"].as_u64()), (Some(1), Some(1)), "{r}");
    let top = r["made"][0]["body"].as_str().unwrap().to_owned();
    assert_eq!(top, format!("{body} Selected"));
    assert!(mesh_of(&s, &top).0.iter().all(|q| (q.z - 20.0).abs() < 1e-6));
    assert!(
        !mesh_of(&s, &body).0.iter().any(|q| (q.z - 20.0).abs() < 1e-6 && q.x > 1.0 && q.x < 39.0 && q.y > 1.0 && q.y < 29.0),
        "the top left the scan"
    );
}

/// All of a piece selected (as a brush on a floating bit of the YCB mug's scan takes it whole):
/// there is no edge inside the scan to part along, and the refusal says so, not that a cutter
/// misses.
#[test]
fn a_whole_piece_selected_has_no_edge_to_part_along() {
    let mut s = Session::default();
    scan(&mut s);
    run(&mut s, "scan.select", json!({"all": true}));
    let e = s.execute("scan.split", &json!({"selection": true})).expect_err("all of it selected").to_string();
    assert!(e.contains("whole pieces"), "{e}");
}
