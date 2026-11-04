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
/// it away; a tube's wall is refused.
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
    let err = s.execute("scan.fit_surface", &json!({"point": [40, 20, 15], "radius": 30})).unwrap_err().to_string();
    assert!(err.contains("wraps round"), "{err}");
    let err = s.execute("scan.fit_surface", &json!({"point": [12, 12, 8], "controls": 2})).unwrap_err().to_string();
    assert!(err.contains("4 to 30"), "{err}");
}
