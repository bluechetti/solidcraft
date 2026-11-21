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
fn run_err(s: &mut Session, id: &str, p: Value) -> String {
    s.execute(id, &p).err().map(|e| e.to_string()).unwrap_or_default()
}

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
    scanned_where(s, body, levels, tag, |_| true)
}

/// [`scanned`], of the triangles `seen` keeps only: what a scanner saw of the part.
fn scanned_where(s: &Session, body: &str, levels: u32, tag: &str, seen: impl Fn([Vec3; 3]) -> bool) -> (Session, String) {
    let mesh = {
        let state = s.model.state();
        let b = state.body(body).unwrap_or_else(|| panic!("no {body}"));
        b.body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.005, angle: Some(4f64.to_radians()) }).unwrap()
    };
    let (positions, triangles) = subdivided(mesh.positions, mesh.triangles, levels);
    let triangles: Vec<[u32; 3]> = triangles.into_iter().filter(|t| seen(t.map(|i| positions[i as usize]))).collect();
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

/// The solid `body` of `s` as a scanner would give it: triangles about half a millimetre each
/// way, every point moved along the surface's normal by noise of standard deviation `sigma` (the
/// testkit's seeded degrader), inserted as the only body of a new design.
fn scanned_with_noise(s: &Session, body: &str, sigma: f64, tag: &str) -> (Session, String) {
    use solidcraft_testkit::scan_degrade::{Degrade, Scan, degraded};
    let mesh = {
        let state = s.model.state();
        let b = state.body(body).unwrap_or_else(|| panic!("no {body}"));
        b.body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.002, angle: None }).unwrap()
    };
    let clean = Scan { points: mesh.positions.iter().map(|p| [p.x, p.y, p.z]).collect(), triangles: mesh.triangles.clone() };
    let scan = degraded(&clean, &Degrade { seed: 20_261_118, edge: 0.5, sigma, ..Degrade::default() });
    let dir = std::env::temp_dir().join(format!("solidcraft-reconstruct-noise-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("part.stl");
    std::fs::write(&path, scan.stl()).unwrap();
    let mut out = Session::default();
    run(&mut out, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let _ = std::fs::remove_dir_all(&dir);
    let name = out.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (out, name)
}

fn volume(s: &Session, body: &str) -> f64 {
    s.model.state().body(body).and_then(|b| b.body.volume_at(0.01)).unwrap_or_else(|| panic!("no volume for {body}"))
}

/// A table: two posts 20 × 24 × 30, set apart so no wall of one lies in a wall of the other, under
/// a top 100 × 60 × 10. Reconstruct reads the posts as one
/// prism whose profile is two squares; extruded new, that made a body for each square, and the
/// steps after it joined only the first (the Artec cylinder head's gasket flange, in two lobes,
/// left a second body nothing joined). Extrude It joins the pieces into the one body it builds,
/// whatever prism it starts from: one Reconstructed body, both posts in it.
#[test]
fn a_prism_in_two_pieces_is_built_as_one_body() {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"corner": [5, 0, 0], "length": 20, "width": 24, "height": 30, "body_name": "Part"}));
    run(&mut part, "solid.box", json!({"corner": [75, 12, 0], "length": 20, "width": 24, "height": 30, "operation": "join", "targets": ["Part"]}));
    run(&mut part, "solid.box", json!({"corner": [0, -10, 30], "length": 100, "width": 60, "height": 10, "operation": "join", "targets": ["Part"]}));
    let (s, scan) = scanned_with_noise(&part, "Part", 0.02, "two-posts");
    let built = format!("{scan} Reconstructed");
    let mut s = s;
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let prisms = read["prisms"].as_array().unwrap().len();
    let mut split = 0;
    for k in 0..prisms {
        let mut each = Session::new(solidcraft_doc::Document::clone(&s.doc));
        each.refresh();
        let made = run(&mut each, "scan.extrude_it", json!({"prism": k}));
        let pieces = made["feature"]["created_bodies"].as_array().map_or(0, Vec::len);
        let reconstructed: Vec<String> = each.model.state().bodies.iter().map(|b| b.name.clone()).filter(|n| n.starts_with(&built)).collect();
        assert_eq!(reconstructed, std::slice::from_ref(&built), "prism {k}: one body, {pieces} pieces made");
        if pieces > 1 {
            split += 1;
            // Both posts, 2 × 20 × 24 × 30 = 28800, to the scan's noise.
            let v = volume(&each, &built);
            assert!((v - 28_800.0).abs() < 0.01 * 28_800.0, "prism {k}: {v}");
        }
    }
    assert!(split > 0, "the posts are read as a prism of two pieces: {read}");
}

/// The same two posts under a slab: Extrude It of the posts' prism alone, before the slab that joins
/// them, makes a body of two pieces (kept in one body on purpose, measured together), and its reply
/// says so in `note`, so a user or an agent knows the body is not one solid yet. Built with the
/// slab it is one piece and says nothing.
#[test]
fn a_prism_that_leaves_the_body_in_pieces_says_so() {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"corner": [5, 0, 0], "length": 20, "width": 24, "height": 30, "body_name": "Part"}));
    run(&mut part, "solid.box", json!({"corner": [75, 12, 0], "length": 20, "width": 24, "height": 30, "operation": "join", "targets": ["Part"]}));
    run(&mut part, "solid.box", json!({"corner": [0, -10, 30], "length": 100, "width": 60, "height": 10, "operation": "join", "targets": ["Part"]}));
    let (s, _) = scanned_with_noise(&part, "Part", 0.02, "posts-said");
    let mut s = s;
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let prisms = read["prisms"].as_array().unwrap().len();
    let mut said = 0;
    for k in 0..prisms {
        let mut each = Session::new(solidcraft_doc::Document::clone(&s.doc));
        each.refresh();
        let made = run(&mut each, "scan.extrude_it", json!({"prism": k}));
        let pieces = made["feature"]["created_bodies"].as_array().map_or(0, Vec::len);
        let note = made["note"].as_str().unwrap_or("");
        if pieces > 1 {
            assert!(note.contains("2 separate pieces"), "prism {k}: {made}");
            said += 1;
        } else {
            assert!(!note.contains("separate pieces"), "prism {k}: {made}");
        }
    }
    assert!(said > 0, "the posts are read as a prism of two pieces: {read}");
    // Every step in order: the slab joins the posts, one piece, nothing said at the end.
    let mut last = Value::Null;
    for step in read["proposals"].as_array().unwrap() {
        if let Ok(r) = s.execute(step["command"].as_str().unwrap(), &step["params"]) {
            last = r;
        }
    }
    assert!(!last["note"].as_str().unwrap_or("").contains("separate pieces"), "{last}");
}

/// A crankshaft's journals: three webs 160 × 160 × 20 joined by two round journals Ø 80, 40
/// long, each rounded into its webs R 6 (tangent, so no crease parts journal from web), and a
/// rod Ø 40 running on 560 mm, so the part is 700 long. Find All seeds a patch 4 % of the part's
/// size round each triangle (28 mm here): round a journal's middle that patch takes in the rounds
/// and the webs, no shape fits it, and every triangle in it was set aside: the Artec crankshaft's
/// journals, cylinders to 0.04 mm, were read as freeform (13 % of it in shapes). A seed no shape
/// fits is tried again at half the reach, down to an eighth, as a cylinder, kept where it is
/// about the patch's size and grows to the full patch's area: each journal is found, Ø 80.
#[test]
fn a_crankshafts_journals_rounded_into_their_webs_are_found_as_cylinders() {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"corner": [0, -80, -80], "length": 20, "width": 160, "height": 160, "body_name": "Part"}));
    for x in [60, 120] {
        run(
            &mut part,
            "solid.box",
            json!({"corner": [x, -80, -80], "length": 20, "width": 160, "height": 160, "operation": "join", "targets": ["Part"]}),
        );
    }
    for x in [20, 80] {
        run(
            &mut part,
            "solid.cylinder",
            json!({"base": [x, 0, 0], "axis": [1, 0, 0], "radius": 40, "height": 40, "operation": "join", "targets": ["Part"]}),
        );
    }
    run(
        &mut part,
        "solid.cylinder",
        json!({"base": [140, 0, 0], "axis": [1, 0, 0], "radius": 20, "height": 560, "operation": "join", "targets": ["Part"]}),
    );
    // the four circles where a journal meets a web
    let rims: Vec<[f64; 3]> = [20.0, 60.0, 80.0, 120.0].iter().map(|&x| [x, 0.0, 40.0]).collect();
    run(&mut part, "solid.fillet", json!({"edges": rims, "radius": 6}));
    let (mut s, _) = scanned_with_noise(&part, "Part", 0.02, "crank-journals");
    let found = run(&mut s, "scan.find_all", json!({}));
    let journals: Vec<&Value> = found["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sh| sh["kind"] == "cylinder" && sh["radius"].as_f64().is_some_and(|r| (r - 40.0).abs() < 0.2))
        .collect();
    assert!(
        journals.len() >= 2,
        "each journal a cylinder Ø 80: {} found; {}",
        journals.len(),
        json!(found["shapes"].as_array().unwrap().iter().map(|s| (s["kind"].clone(), s["radius"].clone(), s["area"].clone())).collect::<Vec<_>>())
    );
}

/// The scan benchmark's bracket (tests/scan-cases/bracket): an L of a base 60 × 40 × 10 and an
/// upright 10 × 40 × 30, a hole Ø 8 through each, scanned with noise 0.02. Reconstruct reads the
/// L as one prism and each plate, holes and all, as a prism of its own; the L holds both plates,
/// so Extrude It joining a plate adds nothing and builds nothing (it used to join it a hair off
/// the L's faces, the slivers' stray faces left a hole cut through the upright with a face loop
/// the kernel's check fails, and Reconstruct Rest's next cut was refused: 504 mm³ left on). Then
/// Reconstruct Rest cuts both holes, and the body is the part's own volume, within 0.1 %: each
/// hole takes 160 π ≈ 503 mm³, the before-upstream sample of this scan built 2.5 mm³ off it.
#[test]
fn a_brackets_plates_the_l_already_holds_add_nothing_and_both_holes_are_cut() {
    let mut part = Session::default();
    run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 10, "body_name": "Part"}));
    run(&mut part, "solid.box", json!({"corner": [0, 0, 10], "length": 10, "width": 40, "height": 30, "operation": "join", "targets": ["Part"]}));
    run(&mut part, "solid.cylinder", json!({"base": [40, 20, -1], "radius": 4, "height": 12, "operation": "cut", "targets": ["Part"]}));
    run(
        &mut part,
        "solid.cylinder",
        json!({"base": [-1, 20, 25], "radius": 4, "height": 12, "axis": [1, 0, 0], "operation": "cut", "targets": ["Part"]}),
    );
    let truth = 60.0 * 40.0 * 10.0 + 10.0 * 40.0 * 30.0 - 2.0 * std::f64::consts::PI * 16.0 * 10.0;
    let (mut s, scan) = scanned_with_noise(&part, "Part", 0.02, "bracket-held");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let mut held = 0;
    for step in read["proposals"].as_array().unwrap() {
        let built = run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
        assert_ne!(built["fit"]["verdict"], "worse", "{built}");
        if built["changed"] == false {
            assert!(built["note"].as_str().is_some_and(|n| n.contains("already holds")) && built["feature"].is_null(), "{built}");
            held += 1;
        }
    }
    assert!(held >= 1, "a plate the L holds is built again: {read}");
    let rest = run(&mut s, "scan.reconstruct_rest", json!({}));
    assert_eq!(rest["kept"], 2, "both holes cut: {rest}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - truth).abs() < 1e-3 * truth, "{v} mm³ against the part's {truth}");
}

/// The scan benchmark's flange (tests/scan-cases/flange): a disc Ø 60 × 8, a hub Ø 20 × 30 and
/// four bolt holes Ø 6 on a 44 circle, scanned with noise 0.1 and built as the benchmark builds
/// it (Survey's tidying, Reconstruct's steps, Reconstruct Rest). A bolt hole is 8 deep: read
/// through that noise its axis came out 0.1° to 0.6° off square (the benchmark's bar is 0.3°),
/// and Pattern It turned the copies about a ring axis read from the holes' places, itself off.
/// Cut It now takes the normal of the face a hole enters, and Pattern It the axis of the bore it
/// copies, when each is within the scan's tolerance over the hole's depth or the ring's radius,
/// so every Ø 6 bore is square to the disc, well within the 0.3° bar.
#[test]
fn a_flanges_bolt_holes_read_through_noise_are_cut_square_to_the_disc() {
    let mut part = Session::default();
    run(&mut part, "solid.cylinder", json!({"radius": 30, "height": 8, "body_name": "Part"}));
    run(&mut part, "solid.cylinder", json!({"radius": 10, "height": 30, "base": [0, 0, 8], "operation": "join", "targets": ["Part"]}));
    run(&mut part, "solid.cylinder", json!({"base": [22, 0, 0], "radius": 3, "height": 8, "operation": "cut", "targets": ["Part"], "name": "Bolt"}));
    run(&mut part, "solid.pattern.circular", json!({"features": ["Bolt"], "axis": "Z", "count": 4}));
    let (mut s, scan) = scanned_with_noise(&part, "Part", 0.1, "flange-square");
    let survey = run(&mut s, "scan.survey", json!({}));
    for step in survey["next"].as_array().into_iter().flatten() {
        let command = step["command"].as_str().unwrap();
        if ["scan.repair", "scan.remove_floating", "scan.denoise", "scan.fill_holes", "scan.tolerance"].contains(&command) {
            run(&mut s, command, step["params"].clone());
        }
    }
    run(&mut s, "scan.find_all", json!({}));
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let commands: Vec<&str> = read["proposals"].as_array().unwrap().iter().map(|p| p["command"].as_str().unwrap()).collect();
    assert!(commands.contains(&"scan.cut_it") && commands.contains(&"scan.pattern_it"), "one hole cut, then patterned: {read}");
    for step in read["proposals"].as_array().unwrap() {
        let built = run(&mut s, step["command"].as_str().unwrap(), step["params"].clone());
        if built["fit"]["verdict"] == "worse" {
            run(&mut s, "edit.undo", json!({}));
        }
    }
    run(&mut s, "scan.reconstruct_rest", json!({}));
    let state = s.model.state();
    let body = state.body(&format!("{scan} Reconstructed")).unwrap();
    let bores: Vec<f64> = body
        .body
        .record()
        .faces
        .iter()
        .filter_map(|f| match &f.surface {
            solidcore_record::brep::Surface::Cylinder { frame, radius } if (radius - 3.0).abs() < 0.1 => {
                let [x, y] = [frame.x_axis, frame.y_axis].map(|a| Vec3::new(a[0], a[1], a[2]));
                Some(x.cross(y).normalized().unwrap().cross(Vec3::Z).len().asin().to_degrees())
            }
            _ => None,
        })
        .collect();
    assert!(bores.len() >= 4, "the four bolt holes: {bores:?}");
    // Each bore takes the disc face's normal or the first bore's axis, so it is off square only
    // by the disc's own reading (0.008° in the benchmark); 0.05° is well under the copies turned
    // about the ring's read axis (0.17° to 0.24° on this scan, 0.35° to 0.49° on the benchmark's).
    assert!(bores.iter().all(|a| *a <= 0.05), "each bore within 0.05° of square: {bores:?}");
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
        tubes: Vec::new(),
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

/// A hexagon with a recessed web (as a gear is, its teeth aside, evenly round its axis): 40 across
/// its corners, 15 thick, bored R 4, a round recess 4 deep between the hub (R 8) and the rim (R 16)
/// on each face. The rim's inside reads as a hole, and the hexagon's ends and the hub's all lie
/// square to it, centred on its axis: none of those is its floor but the web, which closes the
/// recess's low end. Taken for floors, they left no face facing along the axis and no prism was read
/// (CADENA's gears, nothing proposed); it is a prism along its axis.
#[test]
fn a_hexagon_with_a_recessed_web_is_a_prism_along_its_axis() {
    let mut part = Session::default();
    run(&mut part, "sketch.create", json!({"plane": "XY", "name": "Hex"}));
    let corners: Vec<[f64; 2]> = (0..=6).map(|i| f64::from(i % 6) * std::f64::consts::PI / 3.0).map(|a| [20.0 * a.cos(), 20.0 * a.sin()]).collect();
    run(&mut part, "sketch.line", json!({"points": corners}));
    run(&mut part, "sketch.finish", json!({}));
    let made = run(&mut part, "solid.extrude", json!({"sketch": "Hex", "distance": 15, "operation": "new"}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    for (z, h) in [(11.0, 4.0), (0.0, 4.0)] {
        run(&mut part, "solid.cylinder", json!({"radius": 16, "height": h, "base": [0, 0, z], "operation": "cut", "targets": [body]}));
        run(&mut part, "solid.cylinder", json!({"radius": 8, "height": h, "base": [0, 0, z], "operation": "join", "targets": [body]}));
    }
    run(&mut part, "solid.cylinder", json!({"radius": 4, "height": 15, "operation": "cut", "targets": [body]}));
    let (mut s, _) = scanned(&part, &body, 3, "web");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let prisms = read["prisms"].as_array().unwrap();
    let along = |p: &Value| p["direction"].as_array().and_then(|d| d.get(2)).and_then(Value::as_f64).is_some_and(|z| z.abs() > 0.99);
    assert!(prisms.iter().any(along), "{read}");
    let first = read["proposals"].as_array().and_then(|p| p.first()).and_then(|p| p["command"].as_str());
    assert_eq!(first, Some("scan.extrude_it"), "{read}");
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

/// The same chamfered block scanned with 0.02 mm of noise: each of the four chamfers reads a few
/// microns off 2 (2.007, 2.000, 2.005, 2.004), and they are still one Chamfer of the distance as
/// designed, 2, all four edges built. Grouped to the micron they were four Chamfers of an edge
/// each, and the kernel refused two of them once the edges they end on were cut.
#[test]
fn a_noisy_scans_chamfers_that_read_alike_are_one_chamfer_as_designed() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.chamfer", json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "distance": 2, "tangent_chain": false}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.02, "chamfered");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let cuts = read["chamfers"].as_array().unwrap();
    assert_eq!(cuts.len(), 4, "{read}");
    assert!(cuts.iter().any(|c| c["distance"].as_f64().unwrap() != 2.0), "the noise shows in what is read: {read}");
    run(&mut s, "scan.extrude_it", read["proposals"][0]["params"].clone());
    let chamfered = run(&mut s, "scan.chamfer_it", json!({}));
    assert_eq!(chamfered["set_aside"], 0, "{chamfered}");
    assert_eq!(chamfered["chamfers"], json!([{"distance": 2.0, "edges": 4}]), "{chamfered}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.002 * expected, "{v} against {expected}");
}

/// The chamfered block under 0.1 mm of noise, which Find All reads on a smoothed copy: smoothing
/// rounds a chamfer 2.8 mm wide over at its edges, and on the copy it read as strips of cylinder
/// and cone. Each is the flat face the scan's own points lie on, so all four chamfers are read
/// and built as the one Chamfer.
#[test]
fn a_narrow_chamfer_under_noise_is_read_flat_as_the_scan_has_it() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.chamfer", json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "distance": 2, "tangent_chain": false}));
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.1, "chamfered-noisy");
    let found = run(&mut s, "scan.find_all", json!({}));
    let kinds: Vec<&str> = found["shapes"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert!(kinds.iter().all(|k| *k == "plane"), "a block of flat faces: {kinds:?}");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["chamfers"].as_array().map(Vec::len), Some(4), "{read}");
    run(&mut s, "scan.extrude_it", read["proposals"][0]["params"].clone());
    let chamfered = run(&mut s, "scan.chamfer_it", json!({}));
    assert_eq!(chamfered["set_aside"], 0, "{chamfered}");
    assert_eq!(chamfered["chamfers"], json!([{"distance": 2.0, "edges": 4}]), "{chamfered}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.002 * expected, "{v} against {expected}");
}

/// The same block and noise, tidied and given the tolerance Survey advises (Repair, then three
/// times the noise). Repair moves a few triangles, and on the smoothed copy parts of two chamfers
/// then read as strips of a cone that, fitted to the scan itself, is no cone at all (as steep as
/// a plane). Such a strip is not dropped: it is the plane its own points lie on, and all four
/// chamfers are read.
#[test]
fn a_chamfer_strip_no_curved_shape_fits_is_still_the_flat_face_it_lies_on() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 80, "width": 60, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.chamfer", json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "distance": 2, "tangent_chain": false}));
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.1, "chamfered-tidied");
    run(&mut s, "scan.repair", json!({"body": scan}));
    run(&mut s, "scan.tolerance", json!({"scan": scan, "tolerance": 0.3}));
    let found = run(&mut s, "scan.find_all", json!({}));
    assert!(found["in_shapes"].as_f64().is_some_and(|share| share > 0.995), "{}", found["in_shapes"]);
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["chamfers"].as_array().map(Vec::len), Some(4), "{read}");
}

/// A block 40 × 30 × 10 with its upright edges rounded R 5, scanned with 0.02 mm of noise, against
/// the block modelled with R 4, R 5 and R 6: Scan Deviation is least at R 5 (0.020 root mean
/// square, the noise, against 0.093 and 0.108) and passes that one only. A radius 1 mm out leaves the corner 0.41 mm off ((√2 − 1) × 1), four times the
/// tolerance.
#[test]
fn deviation_tells_the_fillet_radius_the_scan_has_from_one_a_millimetre_out() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let uprights = json!([[0, 0, 5], [40, 0, 5], [40, 30, 5], [0, 30, 5]]);
    run(&mut part, "solid.fillet", json!({"edges": uprights, "radius": 5}));
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.02, "fillet-radius");
    let mut read = Vec::new();
    for radius in [4, 5, 6] {
        let name = format!("R{radius}");
        run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10, "body_name": name, "operation": "new"}));
        run(&mut s, "solid.fillet", json!({"edges": uprights, "radius": radius, "body": name}));
        let d = run(&mut s, "scan.deviation", json!({"scan": scan, "body": name}));
        read.push((d["rms"].as_f64().unwrap(), d["model_max"].as_f64().unwrap(), d["passes"] == true, d["why_not"].clone()));
        run(&mut s, "scan.deviation", json!({"show": false}));
    }
    assert!(read[1].0 < 0.5 * read[0].0 && read[1].0 < 0.5 * read[2].0, "least at R 5: {read:?}");
    // At R 5 the model is nowhere far from the scan (under three tolerances: 0.2, where the scan's
    // 0.5 mm triangles cut the block's sharp edges); a millimetre out, the corner is.
    assert!(read[1].1 < 0.3 && read[0].1 > 0.35 && read[2].1 > 0.35, "the corner (√2 − 1) off: {read:?}");
    assert_eq!((read[0].2, read[1].2, read[2].2), (false, true, false), "{read:?}");
}

/// A plate 60 × 40 × 8 with a hole Ø 8, scanned with 0.1 mm of noise, and the plate built without
/// its hole: Reconstruct the Rest reads the hole off what the plate leaves unexplained and cuts
/// it. The hole's wall beside each face of the plate lies within the tolerance of that face and
/// is not left over, so the hole reads a little short at both ends; it is cut through all the
/// same, with no skin left over its mouth at either face.
#[test]
fn a_hole_read_off_what_the_body_leaves_is_cut_through_not_left_with_a_skin() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 8}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"base": [20, 20, -1], "radius": 4, "height": 10, "operation": "cut", "targets": [body]}));
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.1, "skin");
    let solid = format!("{scan} Reconstructed");
    run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 8, "body_name": solid, "operation": "new"}));
    run(&mut s, "scan.find_all", json!({}));
    let rest = run(&mut s, "scan.reconstruct_rest", json!({}));
    assert!(rest["kept"].as_u64().is_some_and(|k| k >= 1), "{rest}");
    let state = s.model.state();
    let built = &state.body(&solid).unwrap().body;
    for z in [0.05, 4.0, 7.95] {
        let at = Vec3::new(20.0, 20.0, z);
        assert!(matches!(built.classify_point(at), Ok(solidcraft_kernel::PointClass::Outside)), "the hole is open at z = {z}: {rest}");
    }
    assert!(matches!(built.classify_point(Vec3::new(40.0, 20.0, 4.0)), Ok(solidcraft_kernel::PointClass::Inside)), "the plate is there");
}

/// A plate 60 × 40 × 8 with two holes Ø 8, scanned with 0.1 mm of noise on half-millimetre
/// triangles: noise that tilts each triangle some twenty degrees. Read as it is, its faces break
/// into dozens of small planes, cones and tori that still hold over half of it; Find All reads it
/// again on a smoothed copy and fits the shapes found there to the scan itself: its six faces and
/// two holes, all of it, and Extrude It builds the plate with its holes back.
#[test]
fn a_plate_under_noise_that_tilts_its_triangles_reads_as_its_faces_and_holes() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 60, "width": 40, "height": 8}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    for x in [15, 45] {
        run(&mut part, "solid.cylinder", json!({"base": [x, 20, -1], "radius": 4, "height": 10, "operation": "cut", "targets": [body]}));
    }
    let expected = 19200.0 - 2.0 * std::f64::consts::PI * 16.0 * 8.0;
    assert!((volume(&part, &body) - expected).abs() < 1e-3 * expected);
    let (mut s, scan) = scanned_with_noise(&part, &body, 0.1, "plate");
    let found = run(&mut s, "scan.find_all", json!({}));
    let kinds: Vec<&str> = found["shapes"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect();
    assert_eq!(
        (kinds.iter().filter(|k| **k == "plane").count(), kinds.iter().filter(|k| **k == "cylinder").count(), kinds.len()),
        (6, 2, 8),
        "{kinds:?}"
    );
    assert!(found["in_shapes"].as_f64().unwrap() > 0.99, "{found}");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert_eq!(read["proposals"].as_array().map(Vec::len), Some(1), "{read}");
    let built = run(&mut s, "scan.extrude_it", read["proposals"][0]["params"].clone());
    // The outline's four sides, and each hole two half circles.
    assert_eq!((built["lines"].as_u64(), built["arcs"].as_u64()), (Some(4), Some(4)), "{built} {read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.005 * expected, "{v} against {expected}");
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

/// A shaft R 20 × 25 with a spigot, and three flat-bottomed holes Ø 6 drilled 13 deep into its
/// side, their bottoms 8 from the axis; scanned with 0.1 mm of noise. A hole's wall ends, on the
/// scan, at the farthest of its corners, which the noise at the bottom's rim carries past the
/// bottom (the hole read 13.22 deep). The bottom is read at its flat face: 8 from the axis within
/// 0.05, half the noise.
#[test]
fn a_blind_holes_bottom_is_at_its_flat_face_not_the_noisiest_corner_of_its_wall() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 20, "height": 25}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"radius": 12, "height": 15, "base": [0, 0, 25], "operation": "join", "targets": [body]}));
    run(
        &mut part,
        "solid.cylinder",
        json!({"base": [21, 0, 12.5], "radius": 3, "height": 13, "axis": [-1, 0, 0], "operation": "cut", "targets": [body], "name": "Hole"}),
    );
    run(&mut part, "solid.pattern.circular", json!({"features": ["Hole"], "axis": "Z", "count": 3}));
    let (mut s, _) = scanned_with_noise(&part, &body, 0.1, "blind-bottom");
    run(&mut s, "scan.find_all", json!({}));
    let read = run(&mut s, "scan.reconstruct", json!({}));
    let holes = read["holes"].as_array().unwrap();
    assert_eq!(holes.len(), 1, "one hole, repeated: {read}");
    let bottom: Vec<f64> = holes[0]["bottom"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let from_axis = bottom[0].hypot(bottom[1]);
    assert!((from_axis - 8.0).abs() < 0.05, "{from_axis}: {read}");
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
    assert!(names.iter().any(|n| n.starts_with("Extrude It 1 Rounds")), "the corners are filleted: {names:?}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.01 * expected, "{v} against the housing's {expected}");
}

/// A square bar's outline repeats four times round its middle, and Reconstruct says so of its
/// prism (`rotational`: the order, the middle, the diameters across its corners and its flats); a
/// bar 40 × 30 repeats only twice, which is no pattern, and says nothing.
#[test]
fn reconstruct_says_how_a_prisms_outline_repeats_round_its_middle() {
    let read_of = |length: f64, width: f64| {
        let mut part = Session::default();
        let made = run(&mut part, "solid.box", json!({"length": length, "width": width, "height": 12, "center": [7, -3, 6]}));
        let body = made["created_bodies"][0].as_str().unwrap().to_owned();
        let (mut s, _) = scanned(&part, &body, 3, "bar");
        run(&mut s, "scan.reconstruct", json!({}))
    };
    let square = read_of(30.0, 30.0);
    let prism = &square["prisms"][0];
    assert!((prism["depth"].as_f64().unwrap() - 12.0).abs() < 0.05, "upright: {square}");
    let repeats = &prism["rotational"];
    assert_eq!(repeats["order"], 4, "{square}");
    let centre: Vec<f64> = repeats["centre"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
    assert!((centre[0] - 7.0).abs() < 0.05 && (centre[1] + 3.0).abs() < 0.05 && (centre[2] - 6.0).abs() < 0.05, "{square}");
    let (tip, root) = (repeats["tip_diameter"].as_f64().unwrap(), repeats["root_diameter"].as_f64().unwrap());
    assert!((tip - 30.0 * std::f64::consts::SQRT_2).abs() < 0.1 && (root - 30.0).abs() < 0.1, "across corners and across flats: {square}");
    let oblong = read_of(40.0, 30.0);
    assert!((oblong["prisms"][0]["depth"].as_f64().unwrap() - 12.0).abs() < 0.05, "{oblong}");
    assert!(oblong["prisms"][0].get("rotational").is_none(), "{oblong}");
}

/// A nest for a turned part: a shaft Ø 20 × 40 with a step Ø 12 × 20 on it lies on its side in
/// a cradle. Its profile is revolved, every face offset by the clearance, and cut from a block
/// 10 mm beyond it all round whose top is at the axis (half the part held). No point of the scan
/// is in the nest's material and the least gap is the clearance, which follows its parameter.
#[test]
fn a_turned_part_is_nested_on_its_side_with_its_clearance() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 10, "height": 40}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.cylinder", json!({"base": [0, 0, 40], "radius": 6, "height": 20, "operation": "join", "targets": [body]}));
    let (mut s, scan) = scanned(&part, &body, 2, "nest-turned");
    run(&mut s, "parameters.add", json!({"name": "clearance", "expression": "0.3 mm"}));
    let built = run(&mut s, "scan.nest", json!({"clearance": "clearance", "turned": true}));
    let nest = built["solid"].as_str().unwrap().to_owned();
    assert_eq!(nest, format!("{scan} Nest"), "{built}");
    assert!(s.model.state().body(&format!("{scan} Nest Part")).is_none(), "the part is cut from the block, not left: {built}");
    let gap_at = |s: &mut Session| {
        let d = run(s, "scan.deviation", json!({"body": nest, "tolerance": "0.02 mm", "limit": 2}));
        assert_eq!(d["clearance"]["fits"], true, "{d}");
        d["clearance"]["gap"].as_f64().unwrap()
    };
    let gap = gap_at(&mut s);
    assert!((gap - 0.3).abs() < 0.03, "the gap is the clearance: {gap}");
    // The block: 60 + 2 × 10.3 along the axis, 2 × (10 + 10.3) across, 10 + 10.3 deep below the
    // axis; less half the part grown by 0.3 (Ø 20.6 × 40.3 and Ø 12.6 × 20.3, end to end).
    let pi = std::f64::consts::PI;
    let block = 80.6 * 40.6 * 20.3;
    let pocket = 0.5 * pi * (10.3f64.powi(2) * 40.3 + 6.3f64.powi(2) * 20.3);
    let v = volume(&s, &nest);
    assert!((v - (block - pocket)).abs() < 0.01 * (block - pocket), "{v} against {}", block - pocket);
    run(&mut s, "parameters.change", json!({"name": "clearance", "expression": "0.5 mm"}));
    let gap = gap_at(&mut s);
    assert!((gap - 0.5).abs() < 0.03, "{gap}");
}

/// A nest for the slotted disc (R 20 × 10, eight slots in its rim): a block 10 mm wider than the
/// disc all round, with a pocket half the disc deep that is the disc's outline grown by the
/// clearance, and that much deeper. No point of the scan is in the nest's material, and the least
/// gap between them is the clearance. `clearance` a parameter: changed from 0.3 to 0.5 the nest
/// rebuilds, with less material and the wider gap.
#[test]
fn a_nest_is_cut_from_a_prisms_outline_and_follows_its_clearance() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 20, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(
        &mut part,
        "solid.box",
        json!({"length": 12, "width": 8, "height": 10, "center": [20, 0, 5], "operation": "cut", "targets": [body], "name": "Slot"}),
    );
    run(&mut part, "solid.pattern.circular", json!({"features": ["Slot"], "axis": "Z", "count": 8}));
    let (mut s, scan) = scanned(&part, &body, 3, "nest");
    run(&mut s, "parameters.add", json!({"name": "clearance", "expression": "0.3 mm"}));
    let built = run(&mut s, "scan.nest", json!({"clearance": "clearance"}));
    let nest = built["solid"].as_str().unwrap().to_owned();
    assert_eq!(nest, format!("{scan} Nest"), "{built}");
    assert!((built["depth"].as_f64().unwrap() - 5.0).abs() < 0.05, "{built}");
    let gap_at = |s: &mut Session| {
        let d = run(s, "scan.deviation", json!({"body": nest, "tolerance": "0.02 mm", "limit": 2}));
        assert_eq!(d["clearance"]["fits"], true, "{d}");
        d["clearance"]["gap"].as_f64().unwrap()
    };
    let tight = volume(&s, &nest);
    let gap = gap_at(&mut s);
    assert!((gap - 0.3).abs() < 0.03, "the gap is the clearance: {gap}");
    // The scan lies on the outline read from it, so nothing to speak of is allowed for; and the
    // gap is no less than the clearance for what is.
    let allowed = built["allowance"].as_f64().unwrap();
    assert!((0.0..0.02).contains(&allowed) && gap > 0.3 + allowed - 0.01, "{built}, gap {gap}");
    // The block: 10 wider all round than the disc is across its tips beside the slots (a slot lies
    // on each axis, 8 wide, so that is 2 × √(20² − 4²)), 15 tall. Less the pocket: the disc's
    // outline grown by 0.3, 5.3 deep (its own section is a tenth of its volume, its outline about
    // 250 long with the slots).
    let across = 2.0 * (20.0f64 * 20.0 - 4.0 * 4.0).sqrt() + 20.0;
    let (block, section) = (across * across * 15.0, volume(&part, &body) / 10.0);
    let pocket = block - tight;
    assert!(pocket > section * 5.3 && pocket < (section + 250.0 * 0.3) * 5.3 * 1.05, "{pocket} for a section of {section}");
    run(&mut s, "parameters.change", json!({"name": "clearance", "expression": "0.5 mm"}));
    let loose = volume(&s, &nest);
    assert!(loose < tight - 20.0, "a wider pocket leaves less: {loose} against {tight}");
    let gap = gap_at(&mut s);
    assert!((gap - 0.5).abs() < 0.03, "{gap}");
    // A second nest of the same scan is refused in words, as is a clearance that is none.
    assert!(run_err(&mut s, "scan.nest", json!({})).contains("already"));
    assert!(run_err(&mut s, "scan.nest", json!({"clearance": 0})).contains("`clearance`"));
    assert!(run_err(&mut s, "scan.nest", json!({"allowance": -0.1})).contains("`allowance`"));
    // An allowance given is added to the clearance, round the part and under it: 0.3 and 0.2 cut
    // the pocket the clearance of 0.5 did (less what that one allowed for by itself).
    let (mut s, _) = scanned(&part, &body, 3, "nest-allowed");
    let built = run(&mut s, "scan.nest", json!({"allowance": 0.2}));
    assert_eq!(built["allowance"], 0.2, "{built}");
    let same = volume(&s, built["solid"].as_str().unwrap());
    assert!((same - loose).abs() < (section + 250.0 * 5.5) * (allowed + 0.001), "{same} against {loose}");
    let d = run(&mut s, "scan.deviation", json!({"body": built["solid"], "tolerance": "0.02 mm", "limit": 2}));
    assert!((d["clearance"]["gap"].as_f64().unwrap() - 0.5).abs() < 0.03, "{d}");
}

/// A plate 50 × 30 × 8 centred on the origin, from its scan: its outline is its own mirror image
/// both ways, so the sketch holds each corner to its image across a centre line and the profile
/// has two sizes, its length and its width, where four corners placed one by one had eight.
/// Changing the length moves both ends.
#[test]
fn a_symmetric_prism_is_sketched_with_one_size_for_both_sides() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.box", json!({"length": 50, "width": 30, "height": 8, "center": [0, 0, 4]}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let (mut s, _) = scanned(&part, &body, 3, "plate");
    let built = run(&mut s, "scan.extrude_it", json!({}));
    let solid = built["solid"].as_str().unwrap().to_owned();
    let listed = run(&mut s, "parameters.list", json!({"filter": "sketch"}));
    let sizes: Vec<(String, f64)> =
        listed["parameters"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap().to_owned(), p["value"].as_f64().unwrap())).collect();
    assert_eq!(sizes.len(), 2, "{listed}");
    let length = sizes.iter().find(|(_, v)| (v - 50.0).abs() < 0.05).unwrap_or_else(|| panic!("the length is a size: {listed}"));
    assert!(sizes.iter().any(|(_, v)| (v - 30.0).abs() < 0.05), "the width is a size: {listed}");
    assert!((volume(&s, &solid) - 50.0 * 30.0 * 8.0).abs() < 30.0, "{built}");
    run(&mut s, "parameters.change", json!({"name": length.0, "expression": "60 mm"}));
    let state = s.model.state();
    let bbox = solidcraft_kernel::measure(&state.body(&solid).unwrap().body).unwrap().bbox;
    assert!((bbox.min.x + 30.0).abs() < 0.05 && (bbox.max.x - 30.0).abs() < 0.05, "both ends moved: {bbox:?}");
    assert!((bbox.min.y + 15.0).abs() < 0.05 && (bbox.max.y - 15.0).abs() < 0.05, "{bbox:?}");
}

/// A disc R 20 × 10 with eight slots cut into its rim (8 wide, 6 deep) comes back from its scan
/// as the disc extruded, one slot cut through it and the cut patterned eight times: the count is
/// a number in the design. With `pattern` false it is one outline extruded; the same solid.
#[test]
fn a_prism_that_repeats_round_its_middle_is_one_gap_cut_and_patterned() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 20, "height": 10}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(
        &mut part,
        "solid.box",
        json!({"length": 12, "width": 8, "height": 10, "center": [20, 0, 5], "operation": "cut", "targets": [body], "name": "Slot"}),
    );
    run(&mut part, "solid.pattern.circular", json!({"features": ["Slot"], "axis": "Z", "count": 8}));
    let expected = volume(&part, &body);
    let (mut s, _) = scanned(&part, &body, 3, "slotted");
    let built = run(&mut s, "scan.extrude_it", json!({}));
    assert_eq!(built["count"], 8, "{built}");
    let solid = built["solid"].as_str().unwrap().to_owned();
    let got = volume(&s, &solid);
    assert!((got - expected).abs() < 0.005 * expected, "{got} against {expected}: {built}");
    let listed = run(&mut s, "parameters.list", json!({"filter": "feature"}));
    let count = listed["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["feature"] == "Extrude It 1 Pattern" && p["input"] == "Quantity")
        .unwrap_or_else(|| panic!("the pattern's count is a parameter: {listed}"));
    assert_eq!(count["value"].as_f64(), Some(8.0), "{listed}");
    // Asked not to: the whole outline in one sketch.
    let (mut s, _) = scanned(&part, &body, 3, "slotted-plain");
    let plain = run(&mut s, "scan.extrude_it", json!({"pattern": false}));
    assert!(plain.get("count").is_none_or(Value::is_null), "{plain}");
    let got = volume(&s, plain["solid"].as_str().unwrap());
    assert!((got - expected).abs() < 0.005 * expected, "{got} against {expected}: {plain}");
}

/// A spun cover scanned from outside only: of a tube R 30 × 20 with a bore R 12 and its top edge
/// chamfered 5, the scanner saw the outer wall, the chamfer and the top, not the bore or the
/// underside. Revolve It closes what it saw straight across into a solid and says it is one side
/// of a skin; given the skin's `wall`, it revolves what was seen as a surface and thickens it
/// inward: the outside stays where the scan has it, and the body is a shell, far lighter.
#[test]
fn one_side_of_a_skin_is_revolved_as_a_surface_and_thickened() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 30, "height": 20}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.chamfer", json!({"edges": [[30, 0, 20]], "distance": 5}));
    run(&mut part, "solid.hole", json!({"position": [0, 0, 20], "diameter": 24}));
    let outside = |[a, b, c]: [Vec3; 3]| {
        let m = (a + b + c) / 3.0;
        m.z > 1e-3 && m.x.hypot(m.y) > 12.5
    };
    let (mut drum, _) = scanned_where(&part, &body, 3, "skin-drum", outside);
    let closed = run(&mut drum, "scan.revolve_it", json!({}));
    assert!(closed["skin"].as_str().is_some_and(|t| t.contains("`wall`")), "{closed}");
    let solid = volume(&drum, closed["solid"].as_str().unwrap());
    let (mut s, _) = scanned_where(&part, &body, 3, "skin-wall", outside);
    let built = run(&mut s, "scan.revolve_it", json!({"wall": 2}));
    assert!(built["skin"].as_str().is_some_and(|t| t.contains("thickened 2 mm")), "{built}");
    let name = built["solid"].as_str().unwrap().to_owned();
    let shell = volume(&s, &name);
    assert!(shell < 0.5 * solid && shell > 0.1 * solid, "a shell of the solid: {shell} against {solid}");
    // Thickened inward: no wider or taller than the part.
    let state = s.model.state();
    let bbox = solidcraft_kernel::measure(&state.body(&name).unwrap().body).unwrap().bbox;
    assert!(bbox.max.x < 30.05 && bbox.max.z < 20.05 && bbox.min.x > -30.05, "{bbox:?}");
    assert!(run_err(&mut s, "scan.revolve_it", json!({"wall": -1})).contains("`wall`"));
}

/// A cup (a cylinder R 30 × 60, shelled 3 thick, open at the top) from its scan in two calls:
/// Section to Sketch about its axis draws the half of the cut on one side of it, closed along the
/// axis through the bottom's thickness and fully dimensioned, and a Revolve about the sketch's X
/// turns it back into the cup. Either side gives the same cup; `side` and `angle` go with `axis`,
/// which gives the plane itself.
#[test]
fn a_cups_half_profile_about_its_axis_revolves_back_into_the_cup() {
    let mut part = Session::default();
    let made = run(&mut part, "solid.cylinder", json!({"radius": 30, "height": 60}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut part, "solid.shell", json!({"faces": [[0, 0, 60]], "thickness": 3}));
    let expected = volume(&part, &body);
    let (mut s, _) = scanned(&part, &body, 1, "cup");
    let axis = json!({"origin": [0, 0, 0], "dir": [0, 0, 1]});
    for (side, name) in [("positive", "Cup"), ("negative", "Other Side")] {
        let half = run(&mut s, "scan.section_sketch", json!({"axis": axis, "side": side, "angle": 30}));
        assert_eq!((half["outlines"].as_u64(), half["closed_on_axis"].as_u64(), half["left_open"].as_u64()), (Some(1), Some(1), Some(0)), "{half}");
        assert_eq!((half["lines"].as_u64(), half["arcs"].as_u64(), half["dof"].as_u64()), (Some(6), Some(0), Some(0)), "{half}");
        run(&mut s, "solid.revolve", json!({"sketch": half["sketch"], "axis": "x", "operation": "new", "body_name": name}));
        let v = volume(&s, name);
        assert!((v - expected).abs() < 0.01 * expected, "{side}: {v} against the cup's {expected}");
    }
    let with_plane = s.execute("scan.section_sketch", &json!({"axis": axis, "plane": "XY"})).unwrap_err().to_string();
    assert!(with_plane.contains("leave `plane`"), "{with_plane}");
    let no_axis = s.execute("scan.section_sketch", &json!({"side": "negative"})).unwrap_err().to_string();
    assert!(no_axis.contains("go with `axis`"), "{no_axis}");
    let off = s.execute("scan.section_sketch", &json!({"axis": {"origin": [500, 0, 0], "dir": [0, 0, 1]}})).unwrap_err().to_string();
    assert!(off.contains("does not cut the scan"), "{off}");
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

/// A Ø 16 tube bent in two planes (along X, a quarter bend R 30 up, a quarter bend R 40 across,
/// along Y), as a manifold's runner is: no flat path, so no Pipe It; Reconstruct reads one tube
/// bent through space, and Tubes It sweeps it back along a 3D spline through its axis.
#[test]
fn a_tube_bent_in_two_planes_is_swept_back_with_tubes_it() {
    let mut part = Session::default();
    run(&mut part, "sketch.create", json!({"plane": "XY", "name": "Path"}));
    run(&mut part, "sketch.options", json!({"sketch_3d": true}));
    let mut path: Vec<Value> = Vec::new();
    for (command, params) in [
        ("sketch.line3d", json!({"points": [[0, 0, 0], [50, 0, 0]]})),
        ("sketch.arc3d", json!({"start": [50, 0, 0], "through": [71.21320343559643, 0, 8.786796564403573], "end": [80, 0, 30]})),
        ("sketch.arc3d", json!({"start": [80, 0, 30], "through": [80, 11.715728752538098, 58.2842712474619], "end": [80, 40, 70]})),
        ("sketch.line3d", json!({"points": [[80, 40, 70], [80, 100, 70]]})),
    ] {
        path.extend(run(&mut part, command, params)["curves"].as_array().unwrap().iter().cloned());
    }
    run(&mut part, "sketch.finish", json!({}));
    let made = run(&mut part, "solid.pipe", json!({"path_sketch": "Path", "path": path, "diameter": 16}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    let expected = volume(&part, &body);
    let (mut s, scan) = scanned(&part, &body, 1, "bent-tube");
    let read = run(&mut s, "scan.reconstruct", json!({}));
    assert!(read["pipe"].is_null(), "{read}");
    let tubes = read["tubes"].as_array().unwrap();
    assert_eq!(tubes.len(), 1, "{read}");
    assert!((tubes[0]["diameter"].as_f64().unwrap() - 16.0).abs() < 0.1, "{read}");
    assert_eq!(tubes[0]["bends"].as_u64(), Some(2), "{read}");
    assert_eq!(build_all(&mut s), ["scan.tubes_it"], "{read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.02 * expected, "{v} against the tube's {expected}");
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
    // Each section as a blade's is described: the chord 60 at one end and 40 at the other (read a
    // hundredth of the way in from each: 59.8 and 40.2), turned 30° between them (29.4° between
    // the slices), 24 % thick at mid chord, alike both sides of its chord, and no NACA section.
    let stations = read["loft"]["stations"].as_array().unwrap();
    assert_eq!(stations.len(), 5, "{read}");
    let number = |k: usize, key: &str| stations[k][key].as_f64().unwrap();
    let (first, last) = (number(0, "chord"), number(4, "chord"));
    let (wide, narrow) = (first.max(last), first.min(last));
    assert!((wide - 59.8).abs() < 0.3 && (narrow - 40.2).abs() < 0.3, "{read}");
    assert!((number(2, "chord") - 50.0).abs() < 0.3, "{read}");
    assert!(
        number(0, "twist").abs() < 1e-9 && (number(4, "twist").abs() - 29.4).abs() < 0.3 && (number(2, "twist").abs() - 14.7).abs() < 0.3,
        "{read}"
    );
    for (k, station) in stations.iter().enumerate().take(5) {
        assert!((number(k, "thickness_share") - 0.24).abs() < 0.005 && (number(k, "thickest_at") - 0.5).abs() < 0.05, "{read}");
        assert!(number(k, "camber_share") < 0.003 && station.get("naca").is_none(), "{read}");
    }
    assert_eq!(build_all(&mut s), ["scan.loft_it"], "{read}");
    let v = volume(&s, &format!("{scan} Reconstructed"));
    assert!((v - expected).abs() < 0.02 * expected, "{v} against the blade's {expected}");
    // The sections' splines are held where the scan has them: no size for each of their points.
    let listed = run(&mut s, "parameters.list", json!({"filter": "sketch"}));
    assert_eq!(listed["parameters"].as_array().map(Vec::len), Some(0), "{listed}");
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

/// A prism's profile running well past the scan is misread and refused before anything is built
/// (T-LESS 06: a stray corner 75 mm past a 92 mm part was built and replied `first`). The check:
/// a 92 × 54 × 57 box; a profile inside it, or past it by under 5 % of its diagonal, passes; one
/// corner 75 mm out is named with how far.
#[test]
fn a_profile_running_well_past_the_scan_is_refused() {
    use solidcraft_scan::profile::{Outline, Piece};
    let plane = solidcore_record::Plane { origin: Vec3::new(0.0, 0.0, 0.0), x: Vec3::X, y: Vec3::Y };
    let bounds = (Vec3::new(-46.0, -27.0, -28.5), Vec3::new(46.0, 27.0, 28.5));
    let square =
        |corners: Vec<solidcore_record::Vec2>| Outline { pieces: vec![Piece::Line; corners.len()], faces: vec![None; corners.len()], corners };
    let v = solidcore_record::Vec2::new;
    let inside = square(vec![v(-45.0, -26.0), v(45.0, -26.0), v(45.0, 26.0), v(-45.0, 26.0)]);
    assert!(super::past_scan(&plane, &[inside], bounds).is_none());
    let a_little = square(vec![v(-48.0, -26.0), v(45.0, -26.0), v(45.0, 26.0), v(-45.0, 26.0)]);
    assert!(super::past_scan(&plane, &[a_little], bounds).is_none(), "2 mm past a 132 mm diagonal");
    let stray = square(vec![v(-121.0, -26.0), v(45.0, -26.0), v(45.0, 26.0), v(-45.0, 26.0)]);
    let (at, by) = super::past_scan(&plane, &[stray], bounds).expect("refused");
    assert!((by - 75.0).abs() < 1e-9 && (at.x + 121.0).abs() < 1e-9, "{at:?} {by}");
}
