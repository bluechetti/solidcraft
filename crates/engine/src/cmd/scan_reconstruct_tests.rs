//! Find All and Reconstruct, end to end: a part is modelled here, meshed finely as a scan would
//! be, inserted as a scan into a new design, and rebuilt from it as features.

use std::collections::HashMap;

use serde_json::{Value, json};
use solidcore_record::Vec3;

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

/// The solid `body` of `s`, meshed finely and cut `levels` times, inserted as the only body of a
/// new design: a scan of it.
fn scanned(s: &Session, body: &str, levels: u32, tag: &str) -> (Session, String) {
    let mesh = {
        let state = s.model.state();
        let b = state.body(body).unwrap_or_else(|| panic!("no {body}"));
        b.body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.005, angle: Some(4f64.to_radians()) }).unwrap()
    };
    let (positions, triangles) = subdivided(mesh.positions, mesh.triangles, levels);
    // Binary STL: a header, the count, then each triangle's normal (left zero), corners and a
    // spare word.
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
    let dir = std::env::temp_dir().join(format!("solidcraft-reconstruct-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.stl");
    std::fs::write(&path, stl).unwrap();
    let mut scan = Session::default();
    run(&mut scan, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    // The scan is in the design now; the file (over 200 MB for a fine part) would fill /tmp.
    let _ = std::fs::remove_dir_all(&dir);
    let name = scan.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (scan, name)
}

fn volume(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).and_then(|b| b.body.volume_at(0.01)).unwrap_or_else(|| panic!("no volume for {body}"))
}

/// A block 40 × 30 × 10 with its four upright edges rounded R 5: Find All finds its two ends,
/// four sides and four rounds; Reconstruct reads one prism, and Extrude It builds it back as a
/// sketch of lines and arcs extruded, the part's own volume.
#[test]
fn a_rounded_block_is_extruded_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.fillet", json!({"edges": [[0, 0, 5], [40, 0, 5], [40, 30, 5], [0, 30, 5]], "radius": 5}));
    let expected = (1200.0 - (4.0 - std::f64::consts::PI) * 25.0) * 10.0;
    // A mesh of curved faces holds a little less than they do.
    assert!((volume(&part, &body) - expected).abs() < 1e-3 * expected, "{}", volume(&part, &body));
    let (mut s, scan) = scanned(&part, &body, 3, "block");

    let found = run(&mut s, "scan.find_all", json!({}));
    assert_eq!(found["shown"], true);
    let kinds: Vec<&str> = found["shapes"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "plane").count(), 6, "{found}");
    assert_eq!(kinds.iter().filter(|k| **k == "cylinder").count(), 4, "{found}");
    assert!(found["in_shapes"].as_f64().unwrap() > 0.99, "{found}");
    assert!(s.scan_shapes.as_ref().is_some_and(|k| k.shown && k.current(&s)));

    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["prisms"].as_array().unwrap().len(), 1, "{read}");
    assert!((read["prisms"][0]["depth"].as_f64().unwrap() - 10.0).abs() < 0.05, "{read}");
    assert_eq!(read["proposals"][0]["command"], "scan.extrude_it", "{read}");

    let built = run(&mut s, "scan.extrude_it", json!({}));
    assert_eq!((built["lines"].as_u64(), built["arcs"].as_u64()), (Some(4), Some(4)), "{built}");
    assert_eq!(built["operation"], "new");
    let solid = built["solid"].as_str().unwrap().to_owned();
    assert_eq!(solid, format!("{scan} Reconstructed"));
    let v = volume(&s, &solid);
    assert!((v - expected).abs() < 0.005 * expected, "{v} against {expected}");
    // One undo takes it all back; the shapes found stay, the scan being as it was.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(&solid).is_none());
    assert!(s.scan_shapes.as_ref().is_some_and(|k| k.current(&s)));
    // Taking the colouring away keeps nothing shown.
    let off = run(&mut s, "scan.find_all", json!({"show": false}));
    assert_eq!(off["shown"], false);
    assert!(s.scan_shapes.as_ref().is_some_and(|k| !k.shown));
}

/// A shaft stepped from R 20 (25 long) to R 12 (15 long) with three blind holes Ø 6 × 12 drilled
/// along the radius into the wider part: Reconstruct reads it as turned with a ring of holes;
/// Revolve It builds the shaft, Cut It the first hole and Pattern It the other two. (A plain
/// cylinder, with one round face, reads as a disc extruded.)
#[test]
fn a_drilled_shaft_is_revolved_cut_and_patterned_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 20, "height": 25}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"radius": 12, "height": 15, "base": [0, 0, 25], "operation": "join", "targets": [body]}));
    run(
        &mut part,
        "solid.cylinder",
        json!({"radius": 3, "height": 13, "base": [21, 0, 12.5], "axis": [-1, 0, 0], "operation": "cut", "targets": [body], "name": "Hole"}),
    );
    run(&mut part, "solid.pattern.circular", json!({"features": ["Hole"], "axis": "Z", "count": 3}));
    let hole = std::f64::consts::PI * 9.0 * 12.0;
    let shaft = std::f64::consts::PI * (400.0 * 25.0 + 144.0 * 15.0);
    assert!((volume(&part, &body) - (shaft - 3.0 * hole)).abs() < 1e-3 * shaft, "{}", volume(&part, &body));
    let (mut s, scan) = scanned(&part, &body, 3, "shaft");

    let read = run(&mut s, "scan.reconstruct", json!({"body": scan}));
    assert!(read["turned"]["share"].as_f64().is_some_and(|share| share > 0.75), "{read}");
    let holes = read["holes"].as_array().unwrap();
    assert_eq!(holes.len(), 1, "the first of the ring, the rest by its pattern: {read}");
    assert_eq!(holes[0]["repeat"], 3, "{read}");
    assert_eq!(holes[0]["layout"], "circular", "{read}");
    assert!((holes[0]["diameter"].as_f64().unwrap() - 6.0).abs() < 0.05 && (holes[0]["depth"].as_f64().unwrap() - 12.0).abs() < 0.3, "{read}");
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands, ["scan.revolve_it", "scan.cut_it", "scan.pattern_it"], "{read}");

    let turned = run(&mut s, "scan.revolve_it", json!({}));
    let solid = turned["solid"].as_str().unwrap().to_owned();
    assert!((volume(&s, &solid) - shaft).abs() < 0.005 * shaft, "{} against {shaft}", volume(&s, &solid));
    run(&mut s, "scan.cut_it", json!({}));
    assert!((volume(&s, &solid) - (shaft - hole)).abs() < 0.005 * shaft, "{}", volume(&s, &solid));
    let patterned = run(&mut s, "scan.pattern_it", json!({}));
    assert_eq!(patterned["count"], 3);
    assert!((volume(&s, &solid) - (shaft - 3.0 * hole)).abs() < 0.005 * shaft, "{}", volume(&s, &solid));
}

/// With no scan, or nothing built yet, the tools say what to do first.
#[test]
fn reconstructs_tools_say_what_comes_first() {
    let mut s = Session::default();
    let none = s.execute("scan.find_all", &json!({})).unwrap_err().to_string();
    assert!(none.contains("no scan"), "{none}");
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 20, "width": 20, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let (mut s, _) = scanned(&part, &body, 3, "first");
    let cut = s.execute("scan.cut_it", &json!({})).unwrap_err().to_string();
    assert!(cut.contains("no round hole"), "{cut}");
    let beyond = s.execute("scan.extrude_it", &json!({"prism": 9})).unwrap_err().to_string();
    assert!(beyond.contains("`prism` is 0 to"), "{beyond}");
}

/// Find All computed on a copy of the design, as the window computes it beside itself, comes
/// back with the copy: the design colours the scan by it and Reconstruct reads it again for
/// nothing; a later copy starts with it.
#[test]
fn find_all_computed_on_a_copy_comes_back_with_it() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 20, "width": 20, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let (mut s, _) = scanned(&part, &body, 3, "copy");
    assert!(s.scan_shapes.is_none());
    let prepared = s.scratch().compute_commands(&[("scan.find_all".into(), json!({}))], None).unwrap();
    s.accept_commands(prepared).unwrap();
    let found = s.scan_shapes.clone().expect("what Find All read is kept");
    assert!(found.shown && found.current(&s));
    assert!(s.scratch().scan_shapes.is_some_and(|k| std::sync::Arc::ptr_eq(&k, &found)), "a copy starts with it");
    // Reading again on a copy finds it current: the same, not read afresh.
    let prepared = s.scratch().compute_commands(&[("scan.reconstruct".into(), json!({}))], None).unwrap();
    s.accept_commands(prepared).unwrap();
    assert!(s.scan_shapes.is_some_and(|k| std::sync::Arc::ptr_eq(&k, &found)));
}

/// Reconstruct proposes only steps that can run in order: a hole is cut once a step before it
/// builds the body or the body is built already, and a turned part whose profile cannot be read
/// is not revolved. (On T-LESS's scans it proposed Cut It first, refused for want of a body.)
#[test]
fn reconstruct_proposes_no_cut_with_nothing_to_cut() {
    use solidcraft_scan::fit::Shape;
    use solidcraft_scan::reconstruct::{Axis, Face, Hole, Reading, Revolve};
    let hole = Hole { face: 0, mouth: Vec3::new(0.0, 0.0, 10.0), bottom: Vec3::ZERO, radius: 3.0, repeat: None };
    let mut reading = Reading {
        faces: vec![Face { shape: Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius: 3.0 }, area: 180.0 }],
        same: vec![0],
        in_shapes: 0.4,
        prisms: Vec::new(),
        unread_prisms: 0,
        revolve: None,
        turned_unread: None,
        repeats: Vec::new(),
        plans: Vec::new(),
        holes: vec![hole],
        rounds: Default::default(),
        fillets: Vec::new(),
        chamfers: Vec::new(),
        size: 60.0,
    };
    assert!(super::proposals(&reading, false).is_empty());
    assert_eq!(super::proposals(&reading, true), vec![super::Proposal::Cut(0)]);
    reading.revolve = Some(Revolve { axis: Axis { point: Vec3::ZERO, direction: Vec3::Z }, members: vec![0], share: 1.0 });
    assert_eq!(super::proposals(&reading, false), vec![super::Proposal::Revolve, super::Proposal::Cut(0)]);
    reading.turned_unread = Some("a face between them is missing".into());
    assert!(super::proposals(&reading, false).is_empty());
}

/// A plain shaft R 20 × 40 with the same ring of holes at mid-height: one round face is a disc
/// extruded, so Extrude It builds it, its holes' floors not ending it; then Cut It and Pattern It.
#[test]
fn a_drilled_disc_is_extruded_cut_and_patterned_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 20, "height": 40}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(
        &mut part,
        "solid.cylinder",
        json!({"radius": 3, "height": 13, "base": [21, 0, 20], "axis": [-1, 0, 0], "operation": "cut", "targets": [body], "name": "Hole"}),
    );
    run(&mut part, "solid.pattern.circular", json!({"features": ["Hole"], "axis": "Z", "count": 3}));
    let (mut s, _) = scanned(&part, &body, 3, "disc");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert!(read.get("turned").is_none(), "{read}");
    let prisms = read["prisms"].as_array().unwrap();
    assert_eq!(prisms.len(), 1, "{read}");
    assert!((prisms[0]["depth"].as_f64().unwrap() - 40.0).abs() < 0.05, "{read}");
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands, ["scan.extrude_it", "scan.cut_it", "scan.pattern_it"], "{read}");
    let built = run(&mut s, "scan.extrude_it", json!({}));
    assert_eq!((built["lines"].as_u64(), built["arcs"].as_u64()), (Some(0), Some(2)), "one circle: {built}");
    let solid = built["solid"].as_str().unwrap().to_owned();
    let (shaft, hole) = (std::f64::consts::PI * 400.0 * 40.0, std::f64::consts::PI * 9.0 * 12.0);
    assert!((volume(&s, &solid) - shaft).abs() < 0.005 * shaft, "{}", volume(&s, &solid));
    run(&mut s, "scan.cut_it", json!({}));
    run(&mut s, "scan.pattern_it", json!({}));
    assert!((volume(&s, &solid) - (shaft - 3.0 * hole)).abs() < 0.005 * shaft, "{}", volume(&s, &solid));
}

/// A block 80 × 60 × 20 whose four top edges are rounded R 2: the rounds run across the prism
/// Reconstruct reads, so no profile holds them, and it proposes Fillet It after Extrude It. Fillet
/// It measures each round off the scan at the built block's edge and fillets the four at R 2, one
/// Fillet, the part's own volume back.
#[test]
fn rounds_across_a_prism_are_filleted_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.fillet", json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "radius": 2, "tangent_chain": false}));
    let expected = volume(&part, &body);
    let (mut s, _) = scanned(&part, &body, 2, "rounded-top");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands.first(), Some(&"scan.extrude_it"), "{read}");
    assert_eq!(commands.last(), Some(&"scan.fillet_it"), "{read}");
    let rounds = read["fillets"].as_array().unwrap();
    assert_eq!(rounds.len(), 4, "{read}");
    assert!(rounds.iter().all(|r| (r["radius"].as_f64().unwrap() - 2.0).abs() < 0.1), "{read}");
    for step in &read["proposals"].as_array().unwrap()[..commands.len() - 1] {
        run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
    }
    let solid = format!("{} Reconstructed", read["body"].as_str().unwrap());
    let sharp = volume(&s, &solid);
    let filleted = run(&mut s, "scan.fillet_it", json!({}));
    assert_eq!(filleted["set_aside"], 0, "{filleted}");
    let radii: Vec<f64> = filleted["fillets"].as_array().unwrap().iter().map(|f| f["radius"].as_f64().unwrap()).collect();
    assert_eq!(radii, [2.0], "{filleted}");
    let v = volume(&s, &solid);
    assert!(v < sharp && (v - expected).abs() < 0.002 * expected, "{v} against {expected} (sharp {sharp})");
}

/// A block 80 × 60 × 20 whose four top edges are chamfered 2 mm: the chamfers run across the
/// prism Reconstruct reads (they are no wall of it, and no profile holds them), so it proposes
/// Chamfer It after Extrude It, which cuts the four at 2 mm, one Chamfer, the part's own volume
/// back. Read before the chamfers were told apart, they made two side prisms of the block.
#[test]
fn chamfers_across_a_prism_are_chamfered_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.chamfer", json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "distance": 2, "tangent_chain": false}));
    let expected = volume(&part, &body);
    let (mut s, _) = scanned(&part, &body, 2, "chamfered-top");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands, ["scan.extrude_it", "scan.chamfer_it"], "{read}");
    assert_eq!(read["prisms"].as_array().unwrap().len(), 1, "{read}");
    let cuts = read["chamfers"].as_array().unwrap();
    assert_eq!(cuts.len(), 4, "{read}");
    assert!(cuts.iter().all(|c| (c["distance"].as_f64().unwrap() - 2.0).abs() < 0.1), "{read}");
    run(&mut s, "scan.extrude_it", read["proposals"][0]["params"].clone());
    let solid = format!("{} Reconstructed", read["body"].as_str().unwrap());
    let sharp = volume(&s, &solid);
    let chamfered = run(&mut s, "scan.chamfer_it", json!({}));
    assert_eq!(chamfered["set_aside"], 0, "{chamfered}");
    let distances: Vec<f64> = chamfered["chamfers"].as_array().unwrap().iter().map(|f| f["distance"].as_f64().unwrap()).collect();
    assert_eq!(distances.len(), 1, "{chamfered}");
    assert!((distances[0] - 2.0).abs() < 0.1, "{chamfered}");
    let v = volume(&s, &solid);
    assert!(v < sharp && (v - expected).abs() < 0.01 * expected, "{v} against {expected} (sharp {sharp})");
}

/// A block 60 × 40 × 10 with a turned boss on top, R 8 for 10 and coned in to R 5 over 3 more:
/// read whole, the boss is a disc extruded and its cone is lost (no prism has a cone for a wall).
/// The block built from the whole reading, then the boss selected and read alone (Reconstruct
/// Selection) is a turned part, and Revolve It on that reading joins it, cone and all: the part's
/// own volume.
#[test]
fn a_turned_boss_selected_on_a_block_is_revolved_onto_it() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"radius": 8, "height": 10, "base": [30, 20, 10], "operation": "join", "targets": [body]}));
    run(&mut part, "solid.cone", json!({"radius": 8, "top_radius": 5, "height": 3, "base": [30, 20, 20], "operation": "join", "targets": [body]}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 2, "boss-on-block");

    let whole = run(&mut s, "scan.reconstruct", json!({}));
    assert!(whole.get("turned").is_none(), "{whole}");
    assert_eq!(whole["proposals"][0]["command"], "scan.extrude_it", "{whole}");
    run(&mut s, "scan.extrude_it", json!({"prism": 0}));
    let solid = format!("{scan} Reconstructed");
    let block = volume(&s, &solid);
    assert!((block - 24000.0).abs() < 0.005 * 24000.0, "the block first: {block}");

    // Nothing selected: refused, and says how to go on.
    let none = s.execute("scan.reconstruct_selection", &json!({})).unwrap_err().to_string();
    assert!(none.contains("nothing is selected"), "{none}");
    // The boss: every triangle above the block's top.
    let boss: Vec<usize> = {
        let state = s.model.state();
        let (positions, triangles) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).unwrap();
        (0..triangles.len()).filter(|&t| triangles[t].iter().map(|&i| positions[i as usize].z).sum::<f64>() / 3.0 > 10.05).collect()
    };
    run(&mut s, "scan.select", json!({"body": scan, "triangles": boss}));
    let read = run(&mut s, "scan.reconstruct_selection", json!({}));
    assert!(read.get("turned").is_some(), "the boss alone is turned: {read}");
    let first = &read["proposals"][0];
    assert_eq!(first["command"], "scan.revolve_it", "{read}");
    assert_eq!(first["params"]["selected"], true, "{read}");
    let built = run(&mut s, "scan.revolve_it", first["params"].clone());
    assert_eq!(built["operation"], "join", "{built}");
    let v = volume(&s, &solid);
    assert!((v - expected).abs() < 0.005 * expected, "{v} against the part's {expected} (block {block})");
}

/// A flanged shaft, the flange R 30 × 8 with four bolt holes Ø 6 through it at R 22 and the shaft
/// R 10 × 30: a turned part with a ring of holes. The flange's walls (its round and the holes) also
/// make a slab along the axis; extruded, it would lie on the turned body's own faces (the kernel
/// refused that join, or left the body broken for the cuts after it). Reconstruct proposes the
/// revolve, the cut and the pattern only, and they build the part.
#[test]
fn a_flange_with_bolt_holes_is_revolved_and_cut_not_extruded_again() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 30, "height": 8}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"radius": 10, "height": 30, "base": [0, 0, 8], "operation": "join", "targets": [body]}));
    run(&mut part, "solid.cylinder", json!({"radius": 3, "height": 8, "base": [22, 0, 0], "operation": "cut", "targets": [body], "name": "Bolt"}));
    run(&mut part, "solid.pattern.circular", json!({"features": ["Bolt"], "axis": "Z", "count": 4}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 2, "flange");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands.first(), Some(&"scan.revolve_it"), "{read}");
    assert!(!commands.contains(&"scan.extrude_it"), "no slab of the turned body again: {read}");
    assert!(commands.contains(&"scan.cut_it") && commands.contains(&"scan.pattern_it"), "{read}");
    for step in read["proposals"].as_array().unwrap() {
        if step["command"] != "scan.fillet_it" {
            run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
        }
    }
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.005 * expected, "{v} against {expected}");
}
