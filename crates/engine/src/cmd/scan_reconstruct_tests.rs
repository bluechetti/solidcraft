//! Find All and Reconstruct, end to end: a part is modelled here, meshed finely as a scan would
//! be, inserted as a scan into a new design, and rebuilt from it as features.

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
    // Its profile is dimensioned from the sketch's origin, as one drawn by hand: nothing free.
    let profile = run(&mut s, "sketch.inspect", json!({"sketch": built["sketch"].clone()}));
    assert_eq!(profile["dof"].as_u64(), Some(0), "{profile}");
    // Its sizes read as designed and placed from the part's origin: the straight sides 30 and 20,
    // the rounds R 5, the corners at whole millimetres from the origin, each with its measure.
    let sizes: Vec<f64> = s
        .doc
        .params
        .iter()
        .filter(|q| q.model && q.comment.starts_with("measured "))
        .map(|q| s.doc.eval(&q.expr, solidcraft_doc::expr::Kind::Length).unwrap())
        .collect();
    for size in [30.0, 20.0, 5.0] {
        assert!(sizes.iter().any(|v| (v - size).abs() < 1e-9), "{size} in {sizes:?}");
    }
    assert!(sizes.iter().all(|v| (v - v.round()).abs() < 1e-9), "{sizes:?}");
    // One undo takes it all back; the shapes found stay, the scan being as it was.
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(&solid).is_none());
    assert!(s.scan_shapes.as_ref().is_some_and(|k| k.current(&s)));
    // Taking the colouring away keeps nothing shown.
    let off = run(&mut s, "scan.find_all", json!({"show": false}));
    assert_eq!(off["shown"], false);
    assert!(s.scan_shapes.as_ref().is_some_and(|k| !k.shown));
}

/// A block 40 × 30 × 10 with two holes Ø 8 through it: the prism's profile holds the holes as
/// circles inside its outline, and Extrude It extrudes the outline with the holes open, not the
/// discs inside them as well.
#[test]
fn holes_through_a_prism_stay_open_when_it_is_extruded() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    for x in [10, 30] {
        run(&mut part, "solid.hole", json!({"position": [x, 15, 10], "diameter": 8}));
    }
    let expected = 40.0 * 30.0 * 10.0 - 2.0 * std::f64::consts::PI * 16.0 * 10.0;
    assert!((volume(&part, &body) - expected).abs() < 1e-3 * expected, "{}", volume(&part, &body));
    let (mut s, _) = scanned(&part, &body, 3, "holed-block");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["proposals"][0]["command"], "scan.extrude_it", "{read}");
    let built = run(&mut s, "scan.extrude_it", json!({}));
    let v = volume(&s, built["solid"].as_str().unwrap());
    assert!((v - expected).abs() < 0.005 * expected, "{v} against {expected}: {read} {built}");
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
    let hole = Hole { face: 0, mouth: Vec3::new(0.0, 0.0, 10.0), bottom: Vec3::ZERO, radius: 3.0, repeat: None, form: None };
    let mut reading = Reading {
        faces: vec![Face { shape: Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius: 3.0 }, area: 180.0 }],
        same: vec![0],
        in_shapes: 0.4,
        prisms: Vec::new(),
        unread_prisms: 0,
        revolve: None,
        partly_turned: None,
        spokes: None,
        turned_unread: None,
        repeats: Vec::new(),
        plans: Vec::new(),
        holes: vec![hole],
        rounds: Default::default(),
        fillets: Vec::new(),
        chamfers: Vec::new(),
        shell: None,
        size: 60.0,
        prism_repeats: Vec::new(),
        coil: None,
        pipe: None,
        loft: None,
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
    assert_eq!(built["fit"]["verdict"], "first", "the first step makes the body: {built}");
    let cut = run(&mut s, "scan.cut_it", json!({}));
    assert_eq!(cut["fit"]["verdict"], "better", "a hole the scan has brings the body closer: {cut}");
    let pattern = run(&mut s, "scan.pattern_it", json!({}));
    assert_eq!(pattern["fit"]["verdict"], "better", "{pattern}");
    let before = pattern["fit"]["before"]["explained"].as_f64().unwrap();
    let after = pattern["fit"]["after"]["explained"].as_f64().unwrap();
    assert!(after > before && after > 0.95, "{pattern}");
    assert!((volume(&s, &solid) - (shaft - 3.0 * hole)).abs() < 0.005 * shaft, "{}", volume(&s, &solid));
    // The same hole cut again changes nothing.
    let again = run(&mut s, "scan.cut_it", json!({"hole": 0}));
    assert_eq!(again["fit"]["verdict"], "same", "{again}");
    assert!(again["fit"].get("note").is_none(), "{again}");
    // No round to fillet is nothing to do, not a failure: the body is as it was, and a batch of
    // steps with Fillet It among them goes on.
    let before = volume(&s, &solid);
    let none = run(&mut s, "scan.fillet_it", json!({}));
    assert_eq!((none["changed"].as_bool(), none["fillets"].as_array().map(Vec::len)), (Some(false), Some(0)), "{none}");
    assert!(none["note"].as_str().is_some_and(|n| n.contains("reads no round")), "{none}");
    assert_eq!(none["fit"]["verdict"], "same", "{none}");
    assert_eq!(volume(&s, &solid), before);
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

/// A moulded housing: an 80 × 60 × 30 box, its sides drafted 2° in toward the top, its upright
/// corners R 6, shelled 2 mm open at the bottom, its outer top rim R 3. Reconstruct reads one
/// drafted prism off the outside (the inside set aside as the shell's), walls 2 mm thick open at
/// the bottom and the rim's rounds; Extrude It, Shell It and Fillet It build it back.
#[test]
fn a_drafted_shelled_housing_is_extruded_shelled_and_filleted_back_from_its_scan() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 30, "center": [0, 0, 15]}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(
        &mut part,
        "solid.draft",
        json!({"faces": [[40, 0, 15], [-40, 0, 15], [0, 30, 15], [0, -30, 15]], "angle": "2 deg", "neutral": "XY", "pull": [0, 0, 1]}),
    );
    let lean = 15.0 * 2f64.to_radians().tan();
    let (x, y) = (40.0 - lean, 30.0 - lean);
    run(&mut part, "solid.fillet", json!({"edges": [[x, y, 15], [-x, y, 15], [x, -y, 15], [-x, -y, 15]], "radius": 6}));
    run(&mut part, "solid.shell", json!({"faces": [[0, 0, 0]], "thickness": 2}));
    run(&mut part, "solid.fillet", json!({"edges": [[0, 30.0 - 2.0 * lean, 30]], "radius": 3}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 2, "housing");

    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["prisms"].as_array().unwrap().len(), 1, "{read}");
    let prism = &read["prisms"][0];
    assert!((prism["draft"].as_f64().unwrap() - 2.0).abs() < 0.05, "drafted 2°: {read}");
    assert_eq!(prism["direction"], json!([0.0, 0.0, 1.0]), "along the grid's Z: {read}");
    let shell = &read["shell"];
    assert!((shell["thickness"].as_f64().unwrap() - 2.0).abs() < 0.02, "{read}");
    assert_eq!(shell["open"].as_array().unwrap().len(), 1, "open at the bottom only: {read}");
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands, ["scan.extrude_it", "scan.shell_it", "scan.fillet_it"], "{read}");
    for step in read["proposals"].as_array().unwrap() {
        let done = run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
        assert_ne!(done["fit"]["verdict"], "worse", "{done}");
        if step["command"] == "scan.extrude_it" {
            assert_eq!((done["lines"].as_u64(), done["arcs"].as_u64()), (Some(4), Some(0)), "the profile is drawn sharp: {done}");
        }
    }
    // Its draft and depth read as designed: 2° and 30, not the hundredths the scan measures.
    let extrude = s.doc.features.iter().find(|f| f.name.starts_with("Extrude It")).map(|f| serde_json::to_string(f).unwrap()).unwrap();
    assert!(extrude.contains("\"-2 deg\"") && extrude.contains("30"), "{extrude}");
    // So does the wall: 2 mm, not the 2.00002 the scan's two skins measure.
    let shell = s.doc.features.iter().find(|f| f.name == "Shell It").map(|f| serde_json::to_string(f).unwrap()).unwrap();
    assert!(shell.contains("\"2 mm\""), "{shell}");
    // The corners, drafted and then rounded with one radius: extruded sharp and filleted R 6, not
    // sketched as arcs that would taper with the walls.
    let names: Vec<&str> = s.doc.features.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"Extrude It 1 Rounds"), "one Fillet for the four corners' one radius: {names:?}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.01 * expected, "{v} against the housing's {expected}");
}

/// A plate 100 × 60 × 12 drilled as a designer drills one: two counterbores (Ø 6.6, Ø 11 × 6.5),
/// a countersink (Ø 5.5, Ø 10.4 at 90°), a tapped M6 and a blind Ø 8 × 8 with its drill point.
/// Reconstruct reads the plate as one prism (no counterbore step or blind hole's end cuts it into
/// slabs) and each hole as a Hole feature makes it; Extrude It and Hole It build the plate back.
#[test]
fn a_plate_with_counterbores_countersinks_and_blind_holes_is_drilled_back_with_hole_it() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 100, "width": 60, "height": 12, "center": [0, 0, 6]}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    for y in [-15, 15] {
        run(&mut part, "solid.hole", json!({"position": [-35, y, 12], "diameter": 6.6, "type": "counterbore", "cb_diameter": 11, "cb_depth": 6.5}));
    }
    run(&mut part, "solid.hole", json!({"position": [0, 15, 12], "diameter": 5.5, "type": "countersink", "cs_diameter": 10.4, "cs_angle": 90}));
    run(&mut part, "solid.hole", json!({"position": [35, 15, 12], "thread": "M6"}));
    run(&mut part, "solid.hole", json!({"position": [35, -15, 12], "diameter": 8, "depth": 8, "type": "drilled"}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 2, "drilled-plate");

    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["prisms"].as_array().unwrap().len(), 1, "one plate: {read}");
    assert!((read["prisms"][0]["depth"].as_f64().unwrap() - 12.0).abs() < 0.05, "{read}");
    let forms: Vec<&str> = read["holes"].as_array().unwrap().iter().filter_map(|h| h["form"]["kind"].as_str()).collect();
    for kind in ["counterbore", "countersink", "tapped", "drilled"] {
        assert!(forms.contains(&kind), "{kind}: {read}");
    }
    let counterbore = read["holes"].as_array().unwrap().iter().find(|h| h["form"]["kind"] == "counterbore").unwrap();
    assert_eq!((counterbore["form"]["diameter"].as_f64(), counterbore["form"]["cb_diameter"].as_f64()), (Some(6.6), Some(11.0)), "{read}");
    assert_eq!(counterbore["form"]["cb_depth"].as_f64(), Some(6.5), "{read}");
    let blind = read["holes"].as_array().unwrap().iter().find(|h| h["form"]["kind"] == "drilled").unwrap();
    assert_eq!((blind["form"]["depth"].as_f64(), blind["form"]["through"].as_bool()), (Some(8.0), Some(false)), "{read}");
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert_eq!(commands[0], "scan.extrude_it", "{read}");
    assert_eq!(commands.iter().filter(|c| **c == "scan.hole_it").count(), 5, "{read}");
    let mut log = Vec::new();
    for step in read["proposals"].as_array().unwrap() {
        let done = run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
        assert_ne!(done["fit"]["verdict"], "worse", "{done}");
        log.push(format!("{} {} {}", step["params"], volume(&s, &format!("{scan} Reconstructed")), done["feature"]));
    }
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.003 * expected, "{v} against the plate's {expected}: {log:#?} {} {}", read["holes"][2], read["holes"][3]);
}

/// Runs every step Reconstruct proposes on `s`, none worse; the commands run.
fn build_all(s: &mut Session) -> Vec<String> {
    let read = run(s, "scan.reconstruct", json!({}));
    let mut done = Vec::new();
    for step in read["proposals"].as_array().unwrap() {
        let command = step["command"].as_str().unwrap();
        let built = run(s, command, step["params"].clone());
        assert_ne!(built["fit"]["verdict"], "worse", "{built} {read}");
        done.push(command.to_owned());
    }
    done
}

/// A Ø 16 bar up 50, round a quarter bend R 30 and along 60: Find All finds its two straight runs
/// and the bend; Reconstruct reads one bar along a flat path, and Pipe It sweeps it back (not two
/// prisms extruded along the runs).
#[test]
fn a_bent_bar_is_swept_back_along_its_path_with_pipe_it() {
    let mut part = Session::default();
    run(&mut part, "sketch.create", json!({"plane": "XZ", "name": "Path"}));
    run(&mut part, "sketch.line", json!({"points": [[0, 0], [0, -50]]}));
    run(&mut part, "sketch.arc.center_point", json!({"center": [30, -50], "start": [0, -50], "end": [30, -80]}));
    run(&mut part, "sketch.line", json!({"points": [[30, -80], [90, -80]]}));
    run(&mut part, "sketch.finish", json!({}));
    let made = run(&mut part, "solid.pipe", json!({"path_sketch": "Path", "path": ["l1", "a1", "l2"], "diameter": 16}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 1, "bent-bar");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert!((read["pipe"]["diameter"].as_f64().unwrap() - 16.0).abs() < 0.05, "{read}");
    assert_eq!((read["pipe"]["lines"].as_u64(), read["pipe"]["arcs"].as_u64()), (Some(2), Some(1)), "{read}");
    assert_eq!(build_all(&mut s), ["scan.pipe_it"], "{read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.01 * expected, "{v} against the bar's {expected}");
}

/// A spring: a Ø 4 wire wound Ø 30, 5 turns at 8 a turn. Find All fits short stretches of it as
/// tori; Reconstruct reads the coil off them, and Coil It winds it back.
#[test]
fn a_spring_is_wound_back_with_coil_it() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.coil", json!({"diameter": 30, "revolutions": 5, "pitch": 8, "section_size": 4, "section": "circular"}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 0, "spring");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let coil = &read["coil"];
    assert_eq!((coil["diameter"].as_f64(), coil["wire"].as_f64(), coil["pitch"].as_f64()), (Some(30.0), Some(4.0), Some(8.0)), "{read}");
    assert_eq!((coil["turns"].as_f64(), coil["clockwise"].as_bool()), (Some(5.0), Some(false)), "{read}");
    assert_eq!(build_all(&mut s), ["scan.coil_it"], "{read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.02 * expected, "{v} against the spring's {expected}");
}

/// A blade lofted through three sections, each narrower and turned further than the one below:
/// no prism, turned part, pipe or coil, so Reconstruct reads sections lofted between its flat
/// ends, and Loft It lofts them back.
#[test]
fn a_twisted_blade_is_lofted_back_with_loft_it() {
    let mut part = Session::default();
    let section = |chord: f64, turn: f64| -> Vec<[f64; 2]> {
        (0..16)
            .map(|k| {
                let t = std::f64::consts::TAU * f64::from(k) / 16.0;
                let (x, y) = (0.5 * chord * t.cos(), 0.12 * chord * t.sin());
                let a = turn.to_radians();
                [x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos()]
            })
            .collect()
    };
    for (k, (z, chord, turn)) in [(0, 60.0, 0.0), (50, 50.0, 15.0), (100, 40.0, 30.0)].into_iter().enumerate() {
        run(&mut part, "sketch.create", json!({"plane": "XY", "offset": z, "name": format!("Section{k}")}));
        run(&mut part, "sketch.spline.fit_point", json!({"points": section(chord, turn), "periodic": true}));
        run(&mut part, "sketch.finish", json!({}));
    }
    let made = run(&mut part, "solid.loft", json!({"sections": [{"sketch": "Section0"}, {"sketch": "Section1"}, {"sketch": "Section2"}]}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 1, "blade");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["loft"]["capped"], true, "between its flat ends: {read}");
    assert!((read["loft"]["length"].as_f64().unwrap() - 100.0).abs() < 0.1, "{read}");
    assert_eq!(build_all(&mut s), ["scan.loft_it"], "{read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.02 * expected, "{v} against the blade's {expected}");
}

/// A plate 60 × 40 × 10 with a block 20 × 14 × 12 on its top: with only the plate built, Reconstruct
/// the Rest finds the block the plate leaves unexplained, reads it on its own and builds it, all
/// one undo step, and leaves nothing selected.
#[test]
fn reconstruct_the_rest_builds_what_the_body_leaves_unexplained() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 10, "center": [0, 0, 5]}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.box", json!({"length": 20, "width": 14, "height": 12, "center": [10, 0, 16], "operation": "join", "targets": [body]}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 2, "rest");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let plate = read["prisms"].as_array().unwrap().iter().position(|p| (p["depth"].as_f64().unwrap() - 10.0).abs() < 0.1).unwrap();
    run(&mut s, "scan.extrude_it", json!({"prism": plate}));
    let solid = format!("{scan} Reconstructed");
    let plate_only = volume(&s, &solid);
    assert!((plate_only - 24000.0).abs() < 0.01 * 24000.0, "{plate_only}");
    let rest = run(&mut s, "scan.reconstruct_rest", json!({}));
    assert!(rest["kept"].as_u64().unwrap() >= 1, "{rest}");
    let v = volume(&s, &solid);
    assert!((v - expected).abs() < 0.01 * expected, "{v} against the part's {expected}: {rest}");
    assert!(super::super::select::selected(&s).is_none_or(|k| k.count == 0), "nothing left selected");
    run(&mut s, "edit.undo", json!({}));
    assert!((volume(&s, &solid) - plate_only).abs() < 1e-6, "one undo takes the rest back");
}

/// A solid rebuilt from a scan's prism moves with the scan exactly once: its sketch is placed again
/// by the move, and the extrude built on it follows the sketch, not the Move as well.
#[test]
fn a_reconstructed_solid_moves_with_its_scan_once() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let (mut s, scan) = scanned(&part, &body, 2, "made-from-once");
    run(&mut s, "scan.find_all", json!({}));
    run(&mut s, "scan.reconstruct", json!({}));
    let solid = run(&mut s, "scan.extrude_it", json!({}))["solid"].as_str().unwrap().to_owned();
    let lowest = |s: &Session, name: &str| -> Vec3 {
        let state = s.model.state();
        let b = state.body(name).unwrap();
        let positions = match b.body.triangle_mesh() {
            Some((p, _)) => p.to_vec(),
            None => b.body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.01, angle: None }).unwrap().positions,
        };
        positions.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |lo, p| lo.min(*p))
    };
    let (solid0, scan0, v0) = (lowest(&s, &solid), lowest(&s, &scan), volume(&s, &solid));
    let moved = run(&mut s, "solid.move", json!({"bodies": [scan], "translate": [5, 0, 3]}));
    assert!(!moved.get("carried").is_some_and(|c| c.to_string().contains(&solid)), "the solid follows its sketch, not the Move: {moved}");
    assert!(moved["placed"].as_array().is_some_and(|p| !p.is_empty()), "{moved}");
    let shift = Vec3::new(5.0, 0.0, 3.0);
    assert!((lowest(&s, &scan) - scan0 - shift).len() < 1e-9);
    assert!((lowest(&s, &solid) - solid0 - shift).len() < 1e-6, "moved once: {:?} -> {:?}", solid0, lowest(&s, &solid));
    assert!((volume(&s, &solid) - v0).abs() < 1e-6 * v0);
}

/// A prism's profile read a few thousandths of a millimetre off the body it joins is put on it:
/// a straight corner onto the body's vertex, a rounded corner's arc moved whole with both its
/// ends, and an outline that would collapse is left as read. A drafted prism counts only what lies
/// in its sketch's plane.
#[test]
fn a_profile_a_hair_off_the_body_is_put_on_it() {
    use solidcore_record::{Plane, Vec2};
    use solidcraft_geom::prelude::*;
    use solidcraft_scan::profile::{Outline, Piece};
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 5, "corner": [0, 0, 0], "body_name": "Body"}));
    // Seen from above, sketched 3 mm over the box: its corners at (0,0), (20,0), (20,10), (0,10).
    let plane = Plane { origin: Vec3::new(0.0, 0.0, 8.0), x: Vec3::X, y: Vec3::Y };
    let off = Vec2::new(4e-3, -3e-3);
    let square = |at: [f64; 2]| Vec2::new(at[0], at[1]) + off;
    let mut outlines = vec![Outline {
        corners: vec![square([0.0, 0.0]), square([20.0, 0.0]), square([20.0, 10.0]), square([0.0, 10.0])],
        pieces: vec![Piece::Line; 4],
        faces: vec![None; 4],
    }];
    super::snap_to_body(&s, "Body", &plane, &mut outlines, 0.1, true);
    assert_eq!(outlines[0].corners, vec![Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(20.0, 10.0), Vec2::new(0.0, 10.0)]);
    // A half disc on the box's long side: the arc's ends both move by one offset, its centre too.
    let mut arc = vec![Outline {
        corners: vec![square([0.0, 0.0]), square([20.0, 0.0])],
        pieces: vec![Piece::Line, Piece::Arc { center: square([10.0, 0.0]), ccw: true }],
        faces: vec![None; 2],
    }];
    super::snap_to_body(&s, "Body", &plane, &mut arc, 0.1, true);
    assert_eq!(arc[0].corners, vec![Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0)]);
    assert!(matches!(arc[0].pieces[1], Piece::Arc { center, .. } if center.dist(Vec2::new(10.0, 0.0)) < 1e-12), "{:?}", arc[0].pieces);
    // Drafted: the box's vertices are 3 mm off the sketch's plane, so nothing moves.
    let mut drafted = vec![Outline {
        corners: vec![square([0.0, 0.0]), square([20.0, 0.0]), square([20.0, 10.0])],
        pieces: vec![Piece::Line; 3],
        faces: vec![None; 3],
    }];
    let read = drafted[0].corners.clone();
    super::snap_to_body(&s, "Body", &plane, &mut drafted, 0.1, false);
    assert_eq!(drafted[0].corners, read);
    // Two corners onto one vertex: the outline stays as read.
    let mut thin = vec![Outline {
        corners: vec![square([0.0, 0.0]), square([0.02, 0.0]), square([0.0, 5.0])],
        pieces: vec![Piece::Line; 3],
        faces: vec![None; 3],
    }];
    let read = thin[0].corners.clone();
    super::snap_to_body(&s, "Body", &plane, &mut thin, 0.1, true);
    assert_eq!(thin[0].corners, read);
}

/// Hole It starts its hole where the hole's axis enters the body built so far: a mouth read a
/// little above the body's face, or a little inside it, is moved onto the face.
#[test]
fn hole_it_starts_where_the_axis_enters_the_body() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [0, 0, 0], "body_name": "Body"}));
    let down = Vec3::new(0.0, 0.0, -1.0);
    for mouth_z in [10.3, 9.8] {
        let at = super::entry_on_body(&s, "Body", Vec3::new(10.0, 10.0, mouth_z), down, 5.0).expect("the axis enters the body");
        // On the face within the body's tolerance (where the kernel calls a point on it).
        assert!((at.z - 10.0).abs() < 1e-4 && at.x == 10.0 && at.y == 10.0, "{at:?}");
    }
    assert!(super::entry_on_body(&s, "Body", Vec3::new(40.0, 10.0, 10.0), down, 5.0).is_none(), "beside the body: nothing to drill");
}
