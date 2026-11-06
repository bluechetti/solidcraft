use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// An ASCII STL of the box `size`, turned by `angle` radians about the unit `axis` through the
/// origin and then moved by `shift`, as a scan arrives lying any way at all.
fn box_stl(size: Vec3, axis: Vec3, angle: f64, shift: Vec3) -> String {
    let (s, c) = angle.sin_cos();
    let place = |p: Vec3| p * c + axis.cross(p) * s + axis * (axis.dot(p) * (1.0 - c)) + shift;
    let corner = |i: usize| {
        place(Vec3::new(if i & 1 == 0 { 0.0 } else { size.x }, if i & 2 == 0 { 0.0 } else { size.y }, if i & 4 == 0 { 0.0 } else { size.z }))
    };
    let faces = [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    let mut stl = String::from("solid scan\n");
    for f in faces {
        stl += "facet normal 0 0 0\nouter loop\n";
        for i in f {
            let p = corner(i);
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    stl + "endsolid scan\n"
}

/// A session with one scan in it: a slab 80 × 50 × 10 lying tilted and away from the origin.
fn session_with_a_scan(tag: &str) -> (Session, String) {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("slab.stl");
    let axis = Vec3::new(0.6, -0.3, 0.74).normalized().unwrap();
    std::fs::write(&path, box_stl(Vec3::new(80.0, 50.0, 10.0), axis, 1.2, Vec3::new(40.0, -25.0, 60.0))).unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (s, body)
}

/// STL keeps single-precision coordinates, so a scan read from one is exact to about 1e-5 mm.
const STL: f64 = 1e-4;

/// The box round the body's points.
fn bounds(s: &Session, body: &str) -> (Vec3, Vec3) {
    let state = s.model.state();
    let (positions, _) = state.body(body).and_then(|b| b.body.triangle_mesh()).unwrap();
    positions.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
}

/// Standing a tilted slab on its steadiest base lays it flat on the grid, one broad face down; one
/// undo puts it back as it came.
#[test]
fn a_scan_stands_on_its_steadiest_base_in_one_undo_step() {
    let (mut s, body) = session_with_a_scan("stand");
    let before = bounds(&s, &body);
    let found = run(&mut s, "scan.bases", json!({}));
    assert_eq!(found["body"], body);
    let first = &found["bases"][0];
    assert_eq!(first["kind"], "flat", "{found}");
    assert!((first["area"].as_f64().unwrap() - 4000.0).abs() < 1.0, "a broad face first: {first}");

    let reply = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!(reply["moved"], true, "{reply}");
    assert!(reply["feature"]["feature"].is_u64(), "kept as a Move feature: {reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 10.0).abs() < STL, "flat on the grid: {lo:?} {hi:?}");

    // Standing it on that base again, or resting it on the grid, does nothing (pressed with no
    // base named, it would go on to the next base).
    let again = run(&mut s, "scan.stand_on_base", json!({"body": body, "base": 0}));
    assert_eq!(again["moved"], false, "{again}");
    assert_eq!(run(&mut s, "scan.rest_on_grid", json!({}))["moved"], false);

    run(&mut s, "edit.undo", json!({}));
    let (lo, hi) = bounds(&s, &body);
    assert!((lo - before.0).len() < 1e-9 && (hi - before.1).len() < 1e-9, "one undo puts it back");
}

/// A point on the slab's narrow end grows that face and stands the slab on it, 50 × 10 down.
#[test]
fn a_scan_stands_on_the_face_picked_on_it() {
    let (mut s, body) = session_with_a_scan("pick");
    run(&mut s, "scan.quick_align", json!({}));
    // Laid along its axes about the origin: 80 along X, so its end at X = 40 is a face.
    let reply = run(&mut s, "scan.stand_on_base", json!({"point": [40.0, 0.0, 0.0]}));
    assert_eq!(reply["base"]["kind"], "grown", "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 80.0).abs() < STL, "standing on its end: {lo:?} {hi:?}");
}

/// Quick align lays the slab along X, Y and Z about the origin; rest and centre then move only
/// what they say.
#[test]
fn quick_align_rest_and_centre_place_the_scan() {
    let (mut s, body) = session_with_a_scan("place");
    run(&mut s, "scan.quick_align", json!({}));
    let (lo, hi) = bounds(&s, &body);
    assert!(((hi - lo) - Vec3::new(80.0, 50.0, 10.0)).len() < STL, "{:?}", hi - lo);
    assert!(((lo + hi) / 2.0).len() < STL, "about the origin");

    let rest = run(&mut s, "scan.rest_on_grid", json!({}));
    assert_eq!(rest["moved"], true);
    let (lo, _) = bounds(&s, &body);
    assert!(lo.z.abs() < STL);
    run(&mut s, "solid.move", json!({"bodies": [body], "translate": [30, -20, 0]}));
    run(&mut s, "scan.center", json!({}));
    let (lo, hi) = bounds(&s, &body);
    let middle = (lo + hi) / 2.0;
    assert!(middle.x.abs() < STL && middle.y.abs() < STL && lo.z.abs() < STL, "centred, its height kept: {middle:?}");
}

/// Without a scan, or with a solid named, the commands say what is wrong; the tools not built
/// yet refuse.
#[test]
fn scan_commands_refuse_what_is_not_a_scan() {
    let mut s = Session::default();
    let err = s.execute("scan.rest_on_grid", &json!({})).unwrap_err().to_string();
    assert!(err.contains("no scan"), "{err}");
    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 10, "width": 10, "height": 10}));
    let err = s.execute("scan.bases", &json!({"body": "Block"})).unwrap_err().to_string();
    assert!(err.contains("not a scan"), "{err}");
    let err = s.execute("scan.stand_on_base", &json!({"body": "Nothing"})).unwrap_err().to_string();
    assert!(err.contains("no body"), "{err}");
    assert!(s.execute("scan.fit_plane", &json!({})).is_err(), "planned tools refuse");

    let (mut s, _) = session_with_a_scan("refuse");
    let err = s.execute("scan.stand_on_base", &json!({"base": 99})).unwrap_err().to_string();
    assert!(err.contains("must be below"), "{err}");
}

/// The triangles of the box from `lo` to `hi`, each side cut into `n` × `n` squares.
fn fine_box(lo: Vec3, hi: Vec3, n: u32) -> Vec<[Vec3; 3]> {
    let mut out = Vec::new();
    let size = hi - lo;
    let (s, l) = ([size.x, size.y, size.z], [lo.x, lo.y, lo.z]);
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for high in [false, true] {
            let at = |i: u32, j: u32| {
                let mut c = l;
                if high {
                    c[axis] += s[axis];
                }
                c[u] += s[u] * f64::from(i) / f64::from(n);
                c[v] += s[v] * f64::from(j) / f64::from(n);
                Vec3::new(c[0], c[1], c[2])
            };
            for i in 0..n {
                for j in 0..n {
                    out.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    out.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
    }
    out
}

/// A session with an L-shaped block scanned at an angle: a slab 60 × 20 × 10 with a 20 × 20 × 15
/// block on the middle of it, a mirror image about X = 30 and about Y = 10 as made.
fn session_with_a_symmetric_scan(tag: &str) -> (Session, String, impl Fn(Vec3) -> Vec3) {
    session_with_part_of_the_block(tag, |_| true)
}

/// The L-shaped block of [`session_with_a_symmetric_scan`], with only the triangles (as made) that
/// `keep` keeps: a scan that missed some of it.
fn session_with_part_of_the_block(tag: &str, keep: impl Fn(&[Vec3; 3]) -> bool) -> (Session, String, impl Fn(Vec3) -> Vec3) {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let axis = Vec3::new(0.3, 0.2, 1.0).normalized().unwrap();
    let (s, c) = 0.6f64.sin_cos();
    let place = move |p: Vec3| p * c + axis.cross(p) * s + axis * (axis.dot(p) * (1.0 - c)) + Vec3::new(50.0, 10.0, 5.0);
    let mut stl = String::from("solid l\n");
    for t in fine_box(Vec3::ZERO, Vec3::new(60.0, 20.0, 10.0), 12)
        .into_iter()
        .chain(fine_box(Vec3::new(20.0, 0.0, 10.0), Vec3::new(40.0, 20.0, 25.0), 8))
        .filter(|t| keep(t))
    {
        stl += "facet normal 0 0 0\nouter loop\n";
        for p in t.map(place) {
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    let path = dir.join("l.stl");
    std::fs::write(&path, stl + "endsolid l\n").unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (s, body, place)
}

/// Find Symmetry finds one of the block's two mirror planes and keeps it as a construction plane;
/// a guide along X = 30 finds that one, kept as a second plane.
#[test]
fn find_symmetry_keeps_the_mirror_plane_as_a_construction_plane() {
    let (mut s, _, place) = session_with_a_symmetric_scan("symmetry");
    let reply = run(&mut s, "scan.symmetry", json!({}));
    assert_eq!(reply["mirror"]["symmetric"], true, "{reply}");
    assert!(reply["plane"]["feature"].is_u64(), "kept: {reply}");
    assert!(s.doc.features.iter().any(|f| f.name == "Symmetry Plane"), "a construction plane named for it");

    // A guide drawn from above along the plane X = 30 as the block now lies.
    let guide = json!({"from": [place(Vec3::new(31.0, -5.0, 30.0)).x, place(Vec3::new(31.0, -5.0, 30.0)).y, 60.0],
        "to": [place(Vec3::new(31.0, 25.0, 30.0)).x, place(Vec3::new(31.0, 25.0, 30.0)).y, 60.0]});
    let reply = run(&mut s, "scan.symmetry", json!({"guide": guide, "keep": false}));
    let normal = reply["mirror"]["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    let truth = place(Vec3::X) - place(Vec3::ZERO);
    assert!((Vec3::new(normal[0], normal[1], normal[2]).dot(truth)).abs() > 1.0 - 1e-6, "the X = 30 plane: {reply}");
    assert!(reply.get("plane").is_none(), "not kept");

    // Kept, the guide's plane is a second construction plane beside the first.
    let reply = run(&mut s, "scan.symmetry", json!({"guide": guide}));
    assert!(reply["plane"]["feature"].is_u64(), "kept: {reply}");
    assert!(s.doc.features.iter().any(|f| f.name == "Symmetry Plane 2"), "a second plane, named apart");
}

/// A scan that missed the block's end beyond X = 45 gets that end back from the other side, as an
/// amber mesh body of its own, in one undo step; the whole block is missing nothing.
#[test]
fn complete_by_mirroring_adds_what_the_scan_missed() {
    let (mut s, body, place) = session_with_part_of_the_block("mirror-fill", |t| t.iter().any(|p| p.x < 45.0));
    let (point, normal) = (place(Vec3::new(30.0, 10.0, 10.0)), place(Vec3::X) - place(Vec3::ZERO));
    let plane = json!({"point": [point.x, point.y, point.z], "normal": [normal.x, normal.y, normal.z]});
    let before = s.doc.features.len();
    let reply = run(&mut s, "scan.complete_by_mirroring", json!({"plane": plane}));
    assert_eq!(reply["body"], body.as_str());
    let fill = reply["fill"].as_str().unwrap().to_owned();
    assert_eq!(fill, format!("{body} Mirror Fill"));
    assert!(reply["triangles"].as_u64().unwrap() > 50 && reply["joined"].as_u64().unwrap() > 0, "{reply}");
    let state = s.model.state();
    let (positions, _) = state.body(&fill).and_then(|b| b.body.triangle_mesh()).expect("the fill is a mesh body");
    // Every point of the fill, as the block was made, is on the missing end.
    let back = |p: Vec3| (p - point).dot(normal) + 30.0;
    assert!(positions.iter().all(|p| back(*p) > 44.0 - STL), "only the missing end");
    assert!(positions.iter().any(|p| (back(*p) - 60.0).abs() < 1e-3), "out to the block's end");
    drop(state);
    assert_eq!(s.doc.features.len(), before + 1, "one feature");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(&fill).is_none(), "one undo takes it away");

    let (mut whole, _, _) = session_with_a_symmetric_scan("mirror-whole");
    let refused = whole.execute("scan.complete_by_mirroring", &json!({"plane": plane})).unwrap_err().to_string();
    assert!(refused.contains("nothing is missing"), "{refused}");
}

/// Squared to its mirror plane, the block lies evenly either side of X = 0; one undo puts it back.
#[test]
fn square_to_symmetry_puts_the_mirror_plane_on_yz() {
    let (mut s, body, _) = session_with_a_symmetric_scan("square");
    let before = bounds(&s, &body);
    let reply = run(&mut s, "scan.square_to_symmetry", json!({}));
    assert_eq!(reply["moved"], true, "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!((lo.x + hi.x).abs() < STL * 10.0 && hi.x > 5.0, "evenly either side of X = 0: {lo:?} {hi:?}");
    assert_eq!(run(&mut s, "scan.square_to_symmetry", json!({}))["moved"], false, "already square");
    run(&mut s, "edit.undo", json!({}));
    let (lo, hi) = bounds(&s, &body);
    assert!((lo - before.0).len() < 1e-9 && (hi - before.1).len() < 1e-9);
}

/// A guide with no length, or with an end missing, is refused before anything is searched.
#[test]
fn a_guide_needs_two_ends_apart() {
    let (mut s, _) = session_with_a_scan("guide");
    let err = s.execute("scan.symmetry", &json!({"guide": {"from": [0, 0, 0], "to": [0, 0, 0]}})).unwrap_err().to_string();
    assert!(err.contains("marks no plane"), "{err}");
    let err = s.execute("scan.symmetry", &json!({"guide": {"from": [0, 0, 0]}})).unwrap_err().to_string();
    assert!(err.contains("`guide.to` is required"), "{err}");
}

/// A session with a plate 60 × 40 × 8 scanned with a round boss Ø 20 × 15 standing on its middle.
fn session_with_a_plate_and_boss(tag: &str) -> (Session, String) {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(60.0, 40.0, 8.0), 20);
    let sides = 96;
    let ring = |k: u32, z: f64| {
        let a = std::f64::consts::TAU * f64::from(k % sides) / f64::from(sides);
        Vec3::new(30.0 + 10.0 * a.cos(), 20.0 + 10.0 * a.sin(), z)
    };
    for k in 0..sides {
        for j in 0..12 {
            let (z0, z1) = (8.0 + 15.0 * f64::from(j) / 12.0, 8.0 + 15.0 * f64::from(j + 1) / 12.0);
            triangles.push([ring(k, z0), ring(k + 1, z0), ring(k + 1, z1)]);
            triangles.push([ring(k, z0), ring(k + 1, z1), ring(k, z1)]);
        }
    }
    let mut stl = String::from("solid p\n");
    for t in triangles {
        stl += "facet normal 0 0 0\nouter loop\n";
        for p in t {
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    let path = dir.join("plate.stl");
    std::fs::write(&path, stl + "endsolid p\n").unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (s, body)
}

/// Fit Plane on the plate's top keeps a construction plane at Z = 8; Fit Cylinder on the boss keeps
/// an axis through its middle and reports Ø 20.
#[test]
fn fitted_shapes_are_kept_as_construction_features() {
    let (mut s, _) = session_with_a_plate_and_boss("fit");
    let plane = run(&mut s, "scan.fit_plane", json!({"point": [8, 8, 8]}));
    let normal = &plane["plane"]["normal"];
    assert!(normal[2].as_f64().unwrap().abs() > 1.0 - 1e-6, "{plane}");
    assert!((plane["plane"]["point"][2].as_f64().unwrap() - 8.0).abs() < STL, "{plane}");
    assert!((plane["area"].as_f64().unwrap() - 2400.0).abs() < 1.0, "the whole top: {plane}");
    assert!(plane["feature"]["feature"].is_u64(), "kept: {plane}");

    let boss = run(&mut s, "scan.fit_cylinder", json!({"point": [40, 20, 15]}));
    let c = &boss["cylinder"];
    assert!((c["diameter"].as_f64().unwrap() - 20.0).abs() < 0.02, "{boss}");
    assert!((c["point"][0].as_f64().unwrap() - 30.0).abs() < 1e-3 && (c["point"][1].as_f64().unwrap() - 20.0).abs() < 1e-3, "{boss}");
    assert!((c["length"].as_f64().unwrap() - 15.0).abs() < 0.01, "{boss}");
    assert!(boss["feature"]["feature"].is_u64(), "kept: {boss}");
    let kept = |id: &Value| s.doc.features.iter().any(|f| Some(f.id) == id.as_u64());
    assert!(kept(&plane["feature"]["feature"]) && kept(&boss["feature"]["feature"]), "both in the history");
}

/// Without a `point`, the fit grows from the point picked on the scan; a cylinder on a flat face
/// and a fit with nothing picked are refused.
#[test]
fn a_fit_uses_the_pick_and_refuses_a_wrong_shape() {
    let (mut s, body) = session_with_a_plate_and_boss("pick-fit");
    let err = s.execute("scan.fit_plane", &json!({})).unwrap_err().to_string();
    assert!(err.contains("pick a point on the scan"), "{err}");
    run(&mut s, "select.set", json!({"items": [{"type": "face", "body": body, "point": [8, 30, 8]}]}));
    let reply = run(&mut s, "scan.fit_plane", json!({"keep": false}));
    assert!(reply["plane"]["normal"][2].as_f64().unwrap().abs() > 1.0 - 1e-6, "{reply}");
    assert!(reply.get("feature").is_none());
    let err = s.execute("scan.fit_cylinder", &json!({"point": [8, 8, 8]})).unwrap_err().to_string();
    assert!(err.contains("not that shape"), "{err}");
}

/// The plate scan measured against a box built to the plate alone: the plate's points are on it,
/// the boss's are too far to count; the map stands until the design changes, and `show: false`
/// takes it away.
#[test]
fn scan_deviation_measures_the_scan_against_the_model() {
    let (mut s, _) = session_with_a_plate_and_boss("deviation");
    let err = s.execute("scan.deviation", &json!({})).unwrap_err().to_string();
    assert!(err.contains("no model"), "{err}");
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 60, "width": 40, "height": 8}));
    let reply = run(&mut s, "scan.deviation", json!({}));
    assert_eq!((reply["shown"].as_bool(), reply["body"].as_str()), (Some(true), Some("Plate")), "{reply}");
    // The plate's points lie on it; the boss's lowest ring, 1.25 mm up, is within reach and above
    // the model, which has no boss, and the rest of the boss is too far to count.
    assert!(reply["within"].as_f64().unwrap() > 0.95 && reply["max_below"].as_f64().unwrap().abs() < STL, "{reply}");
    assert!((reply["max_above"].as_f64().unwrap() - 1.25).abs() < STL, "{reply}");
    assert!(reply["unmatched"].as_u64().unwrap() > 0, "the boss is not in the model: {reply}");
    let map = s.scan_deviation.clone().unwrap();
    assert!(map.current(&s), "the map stands for the design as it is");

    // The model drawn a millimetre short: the plate's top is 1 mm above it.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 60, "width": 40, "height": 7}));
    assert!(!map.current(&s), "a change makes the old map stale");
    let reply = run(&mut s, "scan.deviation", json!({"tolerance": "0.05 mm", "limit": 2}));
    assert!(reply["mean"].as_f64().unwrap() > 0.5, "the top a millimetre above: {reply}");
    assert!(reply["within"].as_f64().unwrap() < 0.9, "{reply}");

    assert_eq!(run(&mut s, "scan.deviation", json!({"show": false}))["shown"], false);
    assert!(s.scan_deviation.is_none());
    let err = s.execute("scan.deviation", &json!({"tolerance": 1, "limit": 0.5})).unwrap_err().to_string();
    assert!(err.contains("more than"), "{err}");
}

/// Pressed again and again, Stand on Base goes through the bases one by one and round; a face
/// picked on the scan is stood on instead.
#[test]
fn stand_on_base_cycles_and_stands_on_a_picked_face() {
    let (mut s, body) = session_with_a_scan("cycle");
    let first = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!(first["base_index"], 0, "{first}");
    let count = first["base_count"].as_u64().unwrap();
    assert!(count >= 3, "{first}");
    let second = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!((second["base_index"].as_u64(), second["moved"].as_bool()), (Some(1), Some(true)), "{second}");
    for _ in 2..count {
        run(&mut s, "scan.stand_on_base", json!({}));
    }
    assert_eq!(run(&mut s, "scan.stand_on_base", json!({}))["base_index"], 0, "round again");

    // Lying flat (base 0, a broad face down), the slab's end is picked and stood on: 80 high.
    run(&mut s, "scan.quick_align", json!({}));
    run(&mut s, "select.set", json!({"items": [{"type": "face", "body": body, "point": [40, 0, 0]}]}));
    let picked = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!(picked["base"]["kind"], "grown", "{picked}");
    assert!(picked.get("base_index").is_none());
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 80.0).abs() < STL, "on its end: {lo:?} {hi:?}");
}

/// A face picked part way up the L block (the raised part's side at X = 40) lies flat on the grid
/// with its middle over the origin, though the rest of the block reaches 20 below it.
#[test]
fn a_face_picked_part_way_up_lies_on_the_grid_at_the_origin() {
    let (mut s, body, place) = session_with_a_symmetric_scan("stand-step");
    let at = place(Vec3::new(40.0, 10.0, 17.5));
    let reply = run(&mut s, "scan.stand_on_base", json!({"point": [at.x, at.y, at.z]}));
    assert_eq!((reply["base"]["kind"].as_str(), reply["moved"].as_bool()), (Some("grown"), Some(true)), "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!((lo.z + 20.0).abs() < STL && (hi.z - 40.0).abs() < STL, "the face on the grid, the block below and above: {lo:?} {hi:?}");
    // Picked again at the origin, it is the same face, its middle there, and nothing moves.
    let again = run(&mut s, "scan.stand_on_base", json!({"point": [0.0, 0.0, 0.0]}));
    let anchor: Vec<f64> = again["base"]["anchor"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(anchor.iter().all(|c| c.abs() < 1e-3) && (again["base"]["area"].as_f64().unwrap() - 300.0).abs() < 0.01, "{again}");
    assert_eq!(again["moved"], false, "standing it there again does nothing: {again}");
}

/// Two points picked along X = 30 guide Find Symmetry to that plane of the L block.
#[test]
fn two_picks_guide_find_symmetry() {
    let (mut s, body, place) = session_with_a_symmetric_scan("picked-guide");
    let on = |p: Vec3| {
        let q = place(p);
        json!({"type": "face", "body": body, "point": [q.x, q.y, q.z]})
    };
    run(&mut s, "select.set", json!({"items": [on(Vec3::new(31.0, 2.0, 25.0)), on(Vec3::new(31.0, 18.0, 25.0))]}));
    let reply = run(&mut s, "scan.symmetry", json!({"keep": false}));
    let n = reply["mirror"]["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    let truth = place(Vec3::X) - place(Vec3::ZERO);
    assert!(Vec3::new(n[0], n[1], n[2]).dot(truth).abs() > 1.0 - 1e-6, "the X = 30 plane: {reply}");
}

/// Fit Sphere on a scanned ball keeps its centre as a construction point and reports Ø 24.
#[test]
fn fit_sphere_keeps_the_centre() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-ball-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (rings, sides) = (24u32, 48u32);
    let at = |i: u32, k: u32| {
        let polar = std::f64::consts::PI * f64::from(i) / f64::from(rings);
        let a = std::f64::consts::TAU * f64::from(k % sides) / f64::from(sides);
        Vec3::new(5.0 + 12.0 * polar.sin() * a.cos(), -3.0 + 12.0 * polar.sin() * a.sin(), 20.0 + 12.0 * polar.cos())
    };
    let mut stl = String::from("solid b\n");
    for i in 0..rings {
        for k in 0..sides {
            for t in [[at(i, k), at(i, k + 1), at(i + 1, k + 1)], [at(i, k), at(i + 1, k + 1), at(i + 1, k)]] {
                stl += "facet normal 0 0 0\nouter loop\n";
                for p in t {
                    stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
                }
                stl += "endloop\nendfacet\n";
            }
        }
    }
    let path = dir.join("ball.stl");
    std::fs::write(&path, stl + "endsolid b\n").unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let reply = run(&mut s, "scan.fit_sphere", json!({"point": [17, -3, 20]}));
    let c = &reply["sphere"]["center"];
    assert!((reply["sphere"]["diameter"].as_f64().unwrap() - 24.0).abs() < 0.1, "{reply}");
    assert!(
        (c[0].as_f64().unwrap() - 5.0).abs() < 0.05 && (c[1].as_f64().unwrap() + 3.0).abs() < 0.05 && (c[2].as_f64().unwrap() - 20.0).abs() < 0.05,
        "{reply}"
    );
    assert!(reply["feature"]["feature"].is_u64(), "kept: {reply}");
}

/// A NURBS surface made into a sheet, written as STEP and kept as an Import feature comes back as
/// one open surface body through the design's own STEP reader (the route fitted surfaces take).
#[test]
fn a_nurbs_surface_round_trips_through_an_import_feature() {
    use solidcraft_geom::bspline::NurbsSurface;
    let n = 5;
    let knots: Vec<f64> = [0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0].to_vec();
    let points: Vec<Vec3> = (0..n * n)
        .map(|k| {
            let (i, j) = (k % n, k / n);
            let (x, y) = (f64::from(i as u32) * 10.0, f64::from(j as u32) * 10.0);
            Vec3::new(x, y, 0.02 * (x - 20.0) * (y - 20.0))
        })
        .collect();
    let surface = NurbsSurface::new([3, 3], [knots.clone(), knots], &points, &vec![1.0; n * n]).unwrap();
    let ts = solidcraft_geom::tspline::TSpline::from_nurbs(&surface).unwrap();
    let sheet = solidcraft_kernel::tspline_sheet(&ts).unwrap();
    let step = solidcraft_kernel::step_export(&[&sheet], "SolidCraft").unwrap();
    let mut s = Session::default();
    let reply = super::super::features::add_feature(
        &mut s,
        &json!({"body_name": "Fitted"}),
        solidcraft_doc::FeatureKind::Import { file: "fitted surface".into(), step: step.into(), components: Vec::new(), skip: Vec::new() },
    )
    .unwrap();
    let state = s.model.state();
    let bodies: Vec<_> = state.bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(bodies.len(), 1, "{reply} {bodies:?}");
    let body = &state.bodies[0];
    assert!(!body.body.is_mesh());
    eprintln!("{reply}");
}

/// Fit Surface on the plate's top keeps a surface body over it, close to the scan; one undo takes
/// it away; on the boss's wall it keeps to the side picked.
#[test]
fn fit_surface_keeps_a_surface_body() {
    let (mut s, _) = session_with_a_plate_and_boss("fit-surface");
    let reply = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    assert_eq!(reply["surface"], "Surface1", "{reply}");
    assert!(reply["rms"].as_f64().unwrap() < 0.01, "{reply}");
    let state = s.model.state();
    let surface = state.body("Surface1").expect("the surface body");
    assert!(!surface.body.is_mesh());
    drop(state);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body("Surface1").is_none(), "one undo step");
    // On the boss's wall the area keeps to the side picked rather than wrapping round it.
    let wall = run(&mut s, "scan.fit_surface", json!({"point": [40, 20, 15], "radius": 30}));
    assert!(wall["rms"].as_f64().unwrap() < 0.1, "{wall}");
    let err = s.execute("scan.fit_surface", &json!({"point": [12, 12, 8], "controls": 2})).unwrap_err().to_string();
    assert!(err.contains("4 to 30"), "{err}");
}

/// A control point of a fitted surface raised 2 mm moves the surface off the plate; refitting the
/// ring round it with that point held brings the rest nearer; refitting all but nothing held puts
/// it back on the scan. Each edit is one undo step and the surface keeps its name.
#[test]
fn edit_surface_moves_control_points_and_refits_to_the_scan() {
    let (mut s, _) = session_with_a_plate_and_boss("edit-surface");
    run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    let start = super::surface::editable(&s, None, "test").unwrap();
    assert_eq!(start.surface.count(), [6, 6]);
    let net = solidcraft_scan::surface_edit::points(&start.surface);
    let up = net[2 * 6 + 2] + Vec3::new(0.0, 0.0, 2.0);
    let raised = run(&mut s, "scan.edit_surface", json!({"moves": [{"point": [2, 2], "to": [up.x, up.y, up.z]}]}));
    assert_eq!((raised["surface"].as_str(), raised["moved"].as_u64(), raised["refitted"].as_u64()), (Some("Surface1"), Some(1), Some(0)), "{raised}");
    let off = raised["rms"].as_f64().unwrap();
    assert!(off > 0.05, "off the plate: {raised}");
    let edited = super::surface::editable(&s, Some("Surface1"), "test").unwrap();
    assert!(solidcraft_scan::surface_edit::points(&edited.surface)[2 * 6 + 2].dist(up) < 1e-9, "the point is where it was put");

    // The ring round it follows the scan, the point held where it was put.
    let around = run(&mut s, "scan.edit_surface", json!({"hold": [[2, 2]], "refit": "around"}));
    assert!(around["refitted"].as_u64().unwrap() > 8 && around["rms"].as_f64().unwrap() < off, "{around}");
    // Everything refitted: back on the plate.
    let all = run(&mut s, "scan.edit_surface", json!({"refit": "all"}));
    assert!(all["rms"].as_f64().unwrap() < 0.02, "{all}");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    let back = super::surface::editable(&s, None, "test").unwrap();
    assert!(solidcraft_scan::surface_edit::points(&back.surface)[2 * 6 + 2].dist(up) < 1e-9, "two undos: as raised");

    let refused = s.execute("scan.edit_surface", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("nothing to change"), "{refused}");
    let refused = s.execute("scan.edit_surface", &json!({"moves": [{"point": [6, 0], "to": [0, 0, 0]}]})).unwrap_err().to_string();
    assert!(refused.contains("outside the net"), "{refused}");
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    let refused = s.execute("scan.edit_surface", &json!({"surface": body, "refit": "all"})).unwrap_err().to_string();
    assert!(refused.contains("not a surface"), "{refused}");
}

/// A fitted surface turned and moved by a later Move is still edited where it now lies: a point
/// raised 2 mm along the surface's normal ends up 2 mm off as the design shows it, the Move still
/// applied after the edit; a reshaped surface is refused.
#[test]
fn edit_surface_works_where_a_later_move_put_the_surface() {
    let (mut s, _) = session_with_a_plate_and_boss("edit-moved-surface");
    run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    run(&mut s, "solid.move", json!({"bodies": ["Surface1"], "translate": [5, -3, 20], "axis": [0, 1, 0], "angle": "30 deg"}));
    let moved = super::surface::editable(&s, Some("Surface1"), "test").unwrap();
    assert!(moved.placed.is_some(), "known to be moved");
    let k = 2 * 6 + 3;
    let (before, normal) =
        (solidcraft_scan::surface_edit::points(&moved.surface)[k], solidcraft_scan::surface_edit::normal_at(&moved.surface, k).unwrap());
    let features = s.doc.features.len();
    run(&mut s, "scan.edit_surface", json!({"surface": "Surface1", "moves": [{"point": [3, 2], "along": "2 mm"}]}));
    assert_eq!(s.doc.features.len(), features, "the import edited in place, the Move kept");
    let after = super::surface::editable(&s, Some("Surface1"), "test").unwrap();
    let now = solidcraft_scan::surface_edit::points(&after.surface)[k];
    assert!(now.dist(before + normal * 2.0) < 1e-6, "2 mm along the normal where it lies: {before:?} → {now:?}");
    let refused = s.execute("scan.edit_surface", &json!({"moves": [{"point": [3, 2]}]})).unwrap_err().to_string();
    assert!(refused.contains("one of `to`"), "{refused}");
}

/// A point raised with a falloff carries the points round it up less the further they are and
/// none past the falloff; sliding a point along the flat surface keeps it in the plane and moves
/// it about as far as asked, carrying its neighbours by their share.
#[test]
fn edit_surface_moves_with_a_falloff_and_slides_points_along_the_surface() {
    let (mut s, _) = session_with_a_plate_and_boss("edit-falloff");
    run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    let start = super::surface::editable(&s, None, "test").unwrap();
    let net = solidcraft_scan::surface_edit::points(&start.surface);
    let step = net[2 * 6 + 3].dist(net[2 * 6 + 2]);
    let k = 2 * 6 + 2;
    let up = net[k] + Vec3::new(0.0, 0.0, 2.0);
    // A falloff of two steps: the next point along takes about half, two along none.
    let raised = run(&mut s, "scan.edit_surface", json!({"moves": [{"point": [2, 2], "to": [up.x, up.y, up.z]}], "falloff": 2.0 * step}));
    assert_eq!(raised["moved"].as_u64(), Some(1), "{raised}");
    assert!(raised["carried"].as_u64().unwrap() >= 8, "the ring round it: {raised}");
    let now = solidcraft_scan::surface_edit::points(&super::surface::editable(&s, None, "test").unwrap().surface);
    assert!(now[k].dist(up) < 1e-9);
    let next = now[k + 1].z - net[k + 1].z;
    assert!(next > 0.8 && next < 1.2, "half way, about: {next}");
    for (i, (a, b)) in now.iter().zip(&net).enumerate() {
        if b.dist(net[k]) >= 2.0 * step {
            assert!((*a - *b).len() < 1e-9, "point {i}, past the falloff: untouched");
        }
    }
    run(&mut s, "edit.undo", json!({}));

    // Slid 2 mm along u: the point stays on the plate's plane (its net is flat) and goes about 2 mm.
    let slid = run(&mut s, "scan.edit_surface", json!({"slide": {"points": [[2, 2]], "u": "2 mm"}}));
    assert_eq!((slid["moved"].as_u64(), slid["carried"].as_u64()), (Some(1), Some(0)), "{slid}");
    let now = solidcraft_scan::surface_edit::points(&super::surface::editable(&s, None, "test").unwrap().surface);
    let d = now[k] - net[k];
    assert!((d.len() - 2.0).abs() < 0.5 && d.z.abs() < 0.05, "{d:?}");
    run(&mut s, "edit.undo", json!({}));
    let with = run(&mut s, "scan.edit_surface", json!({"slide": {"points": [[2, 2]], "v": "-1.5 mm"}, "falloff": 2.0 * step}));
    assert!(with["carried"].as_u64().unwrap() > 0, "{with}");

    for (p, why) in [
        (json!({"slide": {"points": [[2, 2]]}}), "needs a length"),
        (json!({"slide": {"points": [], "u": 1}}), "slide.points"),
        (json!({"slide": {"points": [[9, 9]], "u": 1}}), "outside the net"),
        (json!({"slide": {"points": [[2, 2]], "u": 1}, "moves": [{"point": [1, 1], "along": 1}]}), "not both"),
        (json!({"moves": [{"point": [1, 1], "along": 1}], "falloff": -1}), "0 or more"),
        (json!({"moves": [{"point": [1, 1], "along": 1}], "falloff": "nope"}), "falloff"),
        (json!({"points": net.iter().map(|p| [p.x, p.y, p.z]).collect::<Vec<_>>(), "falloff": 3}), "without `falloff`"),
    ] {
        let refused = s.execute("scan.edit_surface", &p).unwrap_err().to_string();
        assert!(refused.contains(why), "{p}: {refused}");
    }
}

/// A curve clicked half way round the plate's boss is kept as a 3D spline in a new `Scan Curves`
/// 3D sketch, close to the scan; a closed one round it goes in the same sketch; each is one undo
/// step.
#[test]
fn curve_on_scan_keeps_a_3d_spline_in_scan_curves() {
    let (mut s, _) = session_with_a_plate_and_boss("curve-on-scan");
    // The scan's own tolerance: as fine as its triangles, no coarser than a thousandth of its size.
    let tolerance = super::curve::default_tolerance(&s, None).unwrap();
    let open = run(&mut s, "scan.curve_on_scan", json!({"points": [[40, 20, 15], [30, 30, 15], [20, 20, 15]]}));
    assert_eq!(open["closed"], false, "{open}");
    assert!(open["max"].as_f64().unwrap() <= tolerance && open["off_scan"].as_f64().unwrap() <= 2.0 * tolerance, "within {tolerance}: {open}");
    assert!(open["fit_points"].as_u64().unwrap() >= 3, "{open}");
    assert!((open["length"].as_f64().unwrap() - std::f64::consts::PI * 10.0).abs() < 0.5, "half way round: {open}");
    let sketch = s.doc.features.iter().find(|f| f.name == super::curve::SKETCH_NAME).map(|f| f.id).expect("Scan Curves made");
    assert!(s.active_sketch.is_none(), "the sketch is finished again");
    let wires = |s: &Session| super::super::sketch_store::read(&s.doc, sketch).map(|sk| sk.wires.len()).unwrap_or(0);
    assert_eq!(wires(&s), 1);

    let closed = run(&mut s, "scan.curve_on_scan", json!({"points": [[40, 20, 12], [25, 28.66, 12], [25, 11.34, 12]], "closed": true}));
    assert_eq!(closed["closed"], true, "{closed}");
    assert_eq!(closed["sketch"], json!(sketch), "the same sketch");
    assert_eq!(wires(&s), 2);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(wires(&s), 1, "one undo step");

    let refused = s.execute("scan.curve_on_scan", &json!({"points": [[40, 20, 15]]})).unwrap_err().to_string();
    assert!(refused.contains("at least two points"), "{refused}");
}

/// A session with one scan made of `triangles`.
fn session_with_triangles(tag: &str, triangles: &[[Vec3; 3]]) -> (Session, String) {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut stl = String::from("solid t\n");
    for t in triangles {
        stl += "facet normal 0 0 0\nouter loop\n";
        for p in t {
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    let path = dir.join("scan.stl");
    std::fs::write(&path, stl + "endsolid t\n").unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    (s, body)
}

/// A block with a speck beside it, lifted off the grid by Rest on Grid's opposite: the speck is
/// taken off, the block stays where the Move put it, and one undo brings the speck back.
#[test]
fn remove_floating_takes_the_speck_off_where_the_scan_lies() {
    let mut triangles = fine_box(Vec3::new(0.0, 0.0, 5.0), Vec3::new(30.0, 20.0, 15.0), 12);
    let block = triangles.len();
    triangles.extend(fine_box(Vec3::new(50.0, 0.0, 5.0), Vec3::new(51.0, 1.0, 6.0), 1));
    let (mut s, body) = session_with_triangles("floating", &triangles);
    run(&mut s, "scan.rest_on_grid", json!({}));
    let reply = run(&mut s, "scan.remove_floating", json!({}));
    assert_eq!((reply["pieces"].as_u64(), reply["removed"].as_u64(), reply["triangles"].as_u64()), (Some(2), Some(1), Some(block as u64)), "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 10.0).abs() < STL && hi.x < 30.0 + STL, "the block, resting on the grid: {lo:?} {hi:?}");
    run(&mut s, "edit.undo", json!({}));
    assert!((bounds(&s, &body).1.x - 51.0).abs() < STL, "one undo brings the speck back");
    let refused = s.execute("scan.remove_floating", &json!({"below": 0.0})).unwrap_err().to_string();
    assert!(refused.contains("no piece floats"), "{refused}");
}

/// A plate with a square hole: Fill Holes closes it flat (the plate's rim is left open), one undo
/// step; a closed box has no holes.
#[test]
fn fill_holes_closes_a_hole_in_a_plate() {
    let mut triangles = Vec::new();
    for y in 0..10 {
        for x in 0..10 {
            if (4..=5).contains(&x) && (4..=5).contains(&y) {
                continue;
            }
            let p = |i: i32, j: i32| Vec3::new(f64::from(x + i), f64::from(y + j), 0.0);
            triangles.push([p(0, 0), p(1, 0), p(1, 1)]);
            triangles.push([p(0, 0), p(1, 1), p(0, 1)]);
        }
    }
    let (mut s, body) = session_with_triangles("fill-holes", &triangles);
    let reply = run(&mut s, "scan.fill_holes", json!({}));
    assert_eq!((reply["holes"].as_u64(), reply["filled"].as_u64(), reply["failed"].as_u64()), (Some(1), Some(1), Some(0)), "{reply}");
    let count = |s: &Session| s.model.state().body(&body).and_then(|b| b.body.triangle_mesh()).map_or(0, |(_, t)| t.len());
    assert_eq!(count(&s), 192 + reply["added_triangles"].as_u64().unwrap() as usize);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(count(&s), 192, "one undo step");
    let refused = s.execute("scan.fill_holes", &json!({"max_edges": 4})).unwrap_err().to_string();
    assert!(refused.contains("the smallest has 8"), "{refused}");

    let (mut closed, _) = session_with_triangles("fill-closed", &fine_box(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0), 4));
    let refused = closed.execute("scan.fill_holes", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("it is closed"), "{refused}");
}

/// A finely cut box reduced to a tenth keeps its shape, one undo step; asking for more than it has
/// is refused.
#[test]
fn reduce_keeps_a_fine_box_its_shape() {
    let (mut s, body) = session_with_triangles("reduce", &fine_box(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 12));
    let reply = run(&mut s, "scan.reduce", json!({"ratio": 0.1}));
    assert_eq!(reply["before"], 1728, "{reply}");
    assert!(reply["triangles"].as_u64().unwrap() <= 173 && reply["stopped"] == "target", "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.dist(Vec3::ZERO) < STL && hi.dist(Vec3::new(30.0, 20.0, 10.0)) < STL, "the box kept: {lo:?} {hi:?}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.model.state().body(&body).and_then(|b| b.body.triangle_mesh()).map(|(_, t)| t.len()), Some(1728), "one undo step");
    let refused = s.execute("scan.reduce", &json!({"triangles": 5000})).unwrap_err().to_string();
    assert!(refused.contains("already"), "{refused}");
    let refused = s.execute("scan.reduce", &json!({"ratio": 0.5, "triangles": 10})).unwrap_err().to_string();
    assert!(refused.contains("not both"), "{refused}");
}

/// A box with a copied triangle: Repair takes the copy off, one undo step; then the box needs none.
#[test]
fn repair_takes_a_copied_triangle_off() {
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0), 4);
    let copy = triangles[5];
    triangles.push([copy[1], copy[2], copy[0]]);
    let (mut s, body) = session_with_triangles("repair", &triangles);
    let count = |s: &Session| s.model.state().body(&body).and_then(|b| b.body.triangle_mesh()).map_or(0, |(_, t)| t.len());
    let before = count(&s);
    let reply = run(&mut s, "scan.repair", json!({}));
    assert_eq!((reply["duplicates"].as_u64(), reply["triangles"].as_u64()), (Some(1), Some(before as u64 - 1)), "{reply}");
    let refused = s.execute("scan.repair", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("needs no repair"), "{refused}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(count(&s), before, "one undo step");
}

/// Section to Sketch on the plate and boss: cut through the boss it is one circle of the boss's
/// radius, in a new `Section` sketch; cut through the plate, the plate's four sides as four lines;
/// a plane above the part cuts nothing.
#[test]
fn section_sketch_fits_the_cut_with_lines_and_circles() {
    let (mut s, _) = session_with_a_plate_and_boss("section-sketch");
    let boss = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "15 mm"}));
    assert_eq!((boss["circles"].as_u64(), boss["lines"].as_u64(), boss["arcs"].as_u64()), (Some(1), Some(0), Some(0)), "{boss}");
    let id = boss["sketch"].as_u64().unwrap();
    assert!(s.active_sketch.is_none(), "the new sketch is finished");
    let sk = super::super::sketch_store::read(&s.doc, id).unwrap();
    let Some(solidcraft_sketch::CurveKind::Circle { r, .. }) = sk.curves.first().map(|c| &c.kind) else { panic!("a circle") };
    assert!((r - 10.0).abs() < 0.05, "the boss's radius: {r}");

    let plate = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "4 mm"}));
    assert_eq!((plate["lines"].as_u64(), plate["arcs"].as_u64(), plate["outlines"].as_u64()), (Some(4), Some(0), Some(1)), "{plate}");
    let refused = s.execute("scan.section_sketch", &json!({"plane": "XY", "offset": "80 mm"})).unwrap_err().to_string();
    assert!(refused.contains("does not cut"), "{refused}");
    assert!(s.active_sketch.is_none(), "a refused cut leaves no sketch open");
}

/// A ring (torus) about Z: its core a circle of radius 30, its tube of radius 5.
fn ring_triangles() -> Vec<[Vec3; 3]> {
    let (around, across) = (96u32, 32u32);
    let at = |i: u32, j: u32| {
        let (a, b) =
            (std::f64::consts::TAU * f64::from(i % around) / f64::from(around), std::f64::consts::TAU * f64::from(j % across) / f64::from(across));
        let r = 30.0 + 5.0 * b.cos();
        Vec3::new(r * a.cos(), r * a.sin(), 5.0 * b.sin())
    };
    let mut out = Vec::new();
    for i in 0..around {
        for j in 0..across {
            out.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            out.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    out
}

/// Sections Along Curve round a curve drawn over the top of a ring: eight sketches, each the
/// tube's circle (radius 5, round the core) on a plane through the ring's axis, in one undo
/// step; asked for every loop, the far side of the ring is cut too.
#[test]
fn sections_along_a_curve_over_a_ring_are_its_tube() {
    let (mut s, _) = session_with_triangles("sections-along", &ring_triangles());
    let over: Vec<[f64; 3]> = (0..6)
        .map(|k| {
            let a = std::f64::consts::TAU * f64::from(k) / 6.0;
            [30.0 * a.cos(), 30.0 * a.sin(), 5.0]
        })
        .collect();
    run(&mut s, "scan.curve_on_scan", json!({"points": over, "closed": true}));
    let before = s.doc.features.len();
    let made = run(&mut s, "scan.sections_along_curve", json!({}));
    let sketches: Vec<u64> = made["sketches"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    assert_eq!((sketches.len(), made["missed"].as_u64()), (8, Some(0)), "round the closed curve: {made}");
    assert!((made["length"].as_f64().unwrap() - std::f64::consts::TAU * 30.0).abs() < 3.0, "about the ring: {made}");
    for id in &sketches {
        let state = s.model.state();
        let ss = state.sketch(*id).unwrap();
        let plane = ss.plane;
        let normal = plane.x.cross(plane.y);
        assert!(normal.z.abs() < 0.02, "the plane holds the ring's axis: {normal:?}");
        let Some(solidcraft_sketch::CurveKind::Circle { r, c }) = ss.sketch.curves.first().map(|k| &k.kind) else {
            panic!("a circle: {:?}", ss.sketch.curves.first())
        };
        assert!((r - 5.0).abs() < 0.1, "the tube's radius: {r}");
        let q = ss.sketch.points[*c].pos;
        let centre = plane.origin + plane.x * q.x + plane.y * q.y;
        assert!(((centre.x.hypot(centre.y)) - 30.0).abs() < 0.2 && centre.z.abs() < 0.2, "round the core: {centre:?}");
    }
    assert!(s.doc.features.iter().any(|f| f.name == "Curve Section 8"));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.features.len(), before, "one undo step");
    let every = run(&mut s, "scan.sections_along_curve", json!({"every": true, "count": 2}));
    assert_eq!(every["outlines"].as_u64(), Some(4), "each plane cuts both sides of the ring: {every}");
    let refused = s.execute("scan.sections_along_curve", &json!({"count": 0})).unwrap_err().to_string();
    assert!(refused.contains("1 to 60"), "{refused}");
}

/// A plane given is the one asked for: kept however little of the scan matches it, the reply
/// saying how much; the best plane found unasked is still refused when it matches poorly.
#[test]
fn a_plane_given_is_kept_however_little_matches() {
    let (mut s, _, place) = session_with_a_symmetric_scan("symmetry-given");
    // A plane well off the block's middle (X = 20 as made): little of it matches.
    let (point, normal) = (place(Vec3::new(20.0, 10.0, 10.0)), place(Vec3::X) - place(Vec3::ZERO));
    let plane = json!({"point": [point.x, point.y, point.z], "normal": [normal.x, normal.y, normal.z]});
    let kept = run(&mut s, "scan.symmetry", json!({"plane": plane}));
    assert_eq!(kept["mirror"]["symmetric"], false, "{kept}");
    assert!(kept["plane"]["feature"].is_u64(), "kept anyway: {kept}");
    let squared = run(&mut s, "scan.square_to_symmetry", json!({"plane": plane}));
    assert_eq!(squared["moved"], true, "{squared}");
}

/// A scan of a dome, `z = 8 − 0.004 r²` over 60 × 60, `n` squares a side.
fn dome_triangles(n: u32) -> Vec<[Vec3; 3]> {
    let at = |i: u32, j: u32| {
        let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
        Vec3::new(x, y, 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2)))
    };
    let mut out = Vec::new();
    for i in 0..n {
        for j in 0..n {
            out.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            out.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    out
}

/// New Free Form over a dome: a surface of its own lying on the scan, the one Edit Surface opens
/// on; Edit Net changes its net, each one undo step; Edit Surface takes a whole net.
#[test]
fn free_form_makes_a_surface_on_the_scan_and_its_net_changes() {
    let (mut s, _) = session_with_triangles("free-form", &dome_triangles(40));
    let corners = json!([[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]]);
    let made = run(&mut s, "scan.free_form", json!({"corners": corners, "count": 5}));
    assert_eq!((made["surface"].as_str(), made["controls"].as_array().map(Vec::len)), (Some("FreeForm1"), Some(2)), "{made}");
    assert!(made["rms"].as_f64().unwrap() < 0.05 && made["met"] == made["places"], "{made}");
    let start = super::surface::editable(&s, None, "test").unwrap();
    assert_eq!((start.body.as_str(), start.surface.count()), ("FreeForm1", [5, 5]), "the one made last is edited");

    let refined = run(&mut s, "scan.edit_net", json!({"op": "refine"}));
    assert_eq!(refined["controls"], json!([7, 7]), "{refined}");
    let split = run(&mut s, "scan.edit_net", json!({"op": "split", "line": {"row": 2}}));
    assert_eq!((split["controls"].clone(), split["line"].clone()), (json!([7, 8]), json!({"row": 3})), "{split}");
    let creased = run(&mut s, "scan.edit_net", json!({"op": "crease", "line": {"column": 3}}));
    assert_eq!(creased["creases"], json!([{"column": 3}]), "{creased}");
    let out = run(&mut s, "scan.edit_net", json!({"op": "extend", "line": {"column": 6}, "distance": "5 mm"}));
    assert_eq!((out["controls"].clone(), out["line"].clone()), (json!([8, 8]), json!({"column": 7})), "{out}");
    let refused = s.execute("scan.edit_net", &json!({"op": "crease", "line": {"column": 1}})).unwrap_err().to_string();
    assert!(refused.contains("three lines"), "{refused}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(super::surface::editable(&s, None, "test").unwrap().surface.count(), [7, 8], "one undo step each");

    // A whole net sent back as it is.
    let now = super::surface::editable(&s, None, "test").unwrap().surface;
    let points: Vec<[f64; 3]> = solidcraft_scan::surface_edit::points(&now).iter().map(|p| [p.x, p.y, p.z + 1.0]).collect();
    let net = json!({"count": [7, 8], "knots": [now.knots()[0].clone(), now.knots()[1].clone()], "points": points});
    run(&mut s, "scan.edit_surface", json!({"net": net}));
    let raised = super::surface::editable(&s, None, "test").unwrap().surface;
    assert!((raised.eval(0.5, 0.5).unwrap().z - now.eval(0.5, 0.5).unwrap().z - 1.0).abs() < 1e-6, "raised 1 mm");
    let refused = s.execute("scan.free_form", &json!({"corners": [[0, 0, 0], [1, 0, 0]]})).unwrap_err().to_string();
    assert!(refused.contains("four points"), "{refused}");
}

/// `turn` radians round a tube of radius 10 along Z, 40 long, as triangles.
fn tube_triangles(turn: f64) -> Vec<[Vec3; 3]> {
    let (rings, segments) = (20u32, 48u32);
    let at = |r: u32, k: u32| {
        let a = turn * f64::from(k) / f64::from(segments);
        Vec3::new(10.0 * a.cos(), 10.0 * a.sin(), 40.0 * f64::from(r) / f64::from(rings))
    };
    let mut out = Vec::new();
    for r in 0..rings {
        for k in 0..segments {
            out.push([at(r, k), at(r, k + 1), at(r + 1, k + 1)]);
            out.push([at(r, k), at(r + 1, k + 1), at(r + 1, k)]);
        }
    }
    out
}

/// Wrap Surface lays a net on a tube round the point given, fitted to it, kept as a Free Form;
/// the world's Z is the same cylinder, X refuses; one undo step.
#[test]
fn wrap_surface_wraps_a_net_on_a_tube() {
    let (mut s, _) = session_with_triangles("wrap", &tube_triangles(std::f64::consts::FRAC_PI_2));
    let point = json!([10.0 * 0.7f64.cos(), 10.0 * 0.7f64.sin(), 20.0]);
    let made = run(&mut s, "scan.wrap_surface", json!({"point": point, "reach": "60 mm"}));
    assert_eq!((made["surface"].as_str(), made["controls"].clone()), (Some("FreeForm1"), json!([8, 20])), "{made}");
    assert!((made["radius"].as_f64().unwrap() - 10.0).abs() < 0.05 && made["rms"].as_f64().unwrap() < 0.05, "{made}");
    let about_z = run(&mut s, "scan.wrap_surface", json!({"point": point, "reach": "60 mm", "axis": "z", "details": "low"}));
    assert_eq!(about_z["surface"].as_str(), Some("FreeForm2"), "{about_z}");
    assert!((about_z["radius"].as_f64().unwrap() - 10.0).abs() < 0.05 && about_z["controls"][0] == 4, "{about_z}");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body("FreeForm2").is_none() && s.model.state().body("FreeForm1").is_some(), "one undo step");
    let refused = s.execute("scan.wrap_surface", &json!({"point": point, "details": "huge"})).unwrap_err().to_string();
    assert!(refused.contains("low, medium or fine"), "{refused}");
}

/// Two Free Forms clicked side by side on a dome, a little apart and of other sizes, are merged:
/// their sides made one and sewn into one body of two faces; one undo step puts them back.
#[test]
fn merge_surfaces_sews_free_forms_side_by_side() {
    let (mut s, _) = session_with_triangles("merge", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 50, 0], [10, 50, 0]], "count": 4}));
    run(&mut s, "scan.free_form", json!({"corners": [[30.5, 10, 0], [50, 10, 0], [50, 50, 0], [30.5, 50, 0]], "count": 6}));
    let refused = s.execute("scan.merge_surfaces", &json!({"surfaces": ["FreeForm1", "FreeForm2"], "gap": "0.1 mm"})).unwrap_err().to_string();
    assert!(refused.contains("within the gap"), "{refused}");
    let merged = run(&mut s, "scan.merge_surfaces", json!({}));
    assert_eq!((merged["seams"].as_u64(), merged["body"].as_str()), (Some(1), Some("FreeForm1")), "{merged}");
    {
        let state = s.model.state();
        let body = state.body("FreeForm1").expect("the sewn body");
        assert_eq!(solidcraft_kernel::brep_record(&body.body).unwrap().faces.len(), 2, "two faces in one body");
        assert!(state.body("FreeForm2").is_none(), "sewn into the first");
    }
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body("FreeForm2").is_some(), "one undo step");
}

/// Four flat Free Forms round a square tube are merged into one open tube; sliced across with a
/// cap it is closed at that end, and sliced again at the other it is a solid. One free form alone
/// has no loop to cap and is refused, but sliced open without one.
#[test]
fn slice_surface_caps_a_tube_into_a_solid() {
    let (mut s, _) = session_with_triangles("slice", &dome_triangles(10));
    for corners in [
        json!([[0, 0, 0], [20, 0, 0], [20, 0, 40], [0, 0, 40]]),
        json!([[20, 0, 0], [20, 20, 0], [20, 20, 40], [20, 0, 40]]),
        json!([[20, 20, 0], [0, 20, 0], [0, 20, 40], [20, 20, 40]]),
        json!([[0, 20, 0], [0, 0, 0], [0, 0, 40], [0, 20, 40]]),
    ] {
        run(&mut s, "scan.free_form", json!({"corners": corners, "fit": "off"}));
    }
    let lone = s.execute("scan.slice_surface", &json!({"surface": "FreeForm1", "plane": {"origin": [0, 0, 30], "normal": [0, 0, 1]}}));
    assert!(lone.as_ref().is_err_and(|e| e.to_string().contains("right round")), "{lone:?}");
    let merged = run(&mut s, "scan.merge_surfaces", json!({}));
    assert_eq!(merged["seams"], 4, "{merged}");
    let top = run(&mut s, "scan.slice_surface", json!({"plane": {"origin": [0, 0, 30], "normal": [0, 0, 1]}, "keep": "back"}));
    assert_eq!((top["body"].as_str(), top["caps"].as_u64(), top["solid"].as_bool()), (Some("FreeForm1"), Some(1), Some(false)), "{top}");
    let bottom = run(&mut s, "scan.slice_surface", json!({"surface": "FreeForm1", "plane": {"origin": [0, 0, 5], "normal": [0, 0, 1]}}));
    assert_eq!((bottom["caps"].as_u64(), bottom["solid"].as_bool()), (Some(1), Some(true)), "{bottom}");
    {
        let state = s.model.state();
        let body = state.body("FreeForm1").unwrap();
        assert_eq!(solidcraft_kernel::brep_record(&body.body).unwrap().faces.len(), 6, "four sides and two caps");
        let volume = body.body.volume_at(1e-3).unwrap_or(0.0);
        assert!((volume - 20.0 * 20.0 * 25.0).abs() < 1.0, "{volume}");
    }
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.model.state().body("FreeForm1").map(|b| solidcraft_kernel::brep_record(&b.body).unwrap().faces.len()), Some(5), "one undo step");
    let open = run(
        &mut s,
        "scan.slice_surface",
        json!({"surface": "FreeForm1", "plane": {"origin": [10, 10, 10], "normal": [1, 0, 0]}, "keep": [15, 10, 10], "cap": false}),
    );
    assert_eq!((open["caps"].as_u64(), open["solid"].as_bool()), (Some(0), Some(false)), "{open}");
}

/// A whole tube wrapped is open at a seam; Merge closes it, and the tube sliced at both ends with
/// caps is a solid of the tube's volume.
#[test]
fn a_whole_wrap_merged_and_sliced_at_both_ends_is_a_solid() {
    let (mut s, _) = session_with_triangles("wrap-whole", &tube_triangles(std::f64::consts::TAU));
    let made = run(&mut s, "scan.wrap_surface", json!({"point": [10, 0, 20], "reach": "60 mm"}));
    assert_eq!(made["surface"].as_str(), Some("FreeForm1"), "{made}");
    let merged = run(&mut s, "scan.merge_surfaces", json!({}));
    assert_eq!((merged["seams"].as_u64(), merged["body"].as_str()), (Some(1), Some("FreeForm1")), "{merged}");
    run(&mut s, "scan.slice_surface", json!({"plane": {"origin": [0, 0, 35], "normal": [0, 0, 1]}, "keep": "back"}));
    let closed = run(&mut s, "scan.slice_surface", json!({"plane": {"origin": [0, 0, 5], "normal": [0, 0, 1]}}));
    assert_eq!(closed["solid"].as_bool(), Some(true), "{closed}");
    let volume = s.model.state().body("FreeForm1").and_then(|b| b.body.volume_at(1e-3)).unwrap_or(0.0);
    let expected = std::f64::consts::PI * 100.0 * 30.0;
    assert!((volume - expected).abs() < 0.02 * expected, "{volume} against {expected}");
}
