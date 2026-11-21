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

/// An STL file states no unit. Read with none given, the reply says it was taken as millimetres
/// and how big that made it; a file that comes out under 3 across is refused until the caller says
/// which unit, millimetres among them. A part 4 x 2 x 1 inches reads as a plausible 4 x 2 x 1 mm
/// part, so no size rule can refuse it: the reply's size is what tells.
#[test]
fn insert_mesh_says_the_unit_it_took_and_refuses_a_tiny_file_until_told() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-units-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, size: Vec3| {
        let path = dir.join(name);
        std::fs::write(&path, box_stl(size, Vec3::Z, 0.0, Vec3::ZERO)).unwrap();
        path.to_string_lossy().into_owned()
    };
    // A part 100 x 50 x 25 mm saved in millimetres, in inches and in metres.
    let (mm, inches, metres) = (
        write("mm.stl", Vec3::new(100.0, 50.0, 25.0)),
        write("in.stl", Vec3::new(100.0, 50.0, 25.0) / 25.4),
        write("m.stl", Vec3::new(0.1, 0.05, 0.025)),
    );
    let mut s = Session::default();
    let read = run(&mut s, "file.insert_mesh", json!({"path": mm}));
    assert_eq!(read["units_assumed"], "mm", "{read}");
    assert_eq!(read["size"], json!([100.0, 50.0, 25.0]), "{read}");
    assert!(read.get("units").is_none(), "{read}");
    // Inches read as millimetres: let in, 3.94 across, and said so.
    let read = run(&mut s, "file.insert_mesh", json!({"path": inches}));
    assert_eq!(read["units_assumed"], "mm", "{read}");
    assert!((read["size"][0].as_f64().unwrap() - 100.0 / 25.4).abs() < STL, "{read}");
    let read = run(&mut s, "file.insert_mesh", json!({"path": inches, "units": "in"}));
    assert!(read.get("units_assumed").is_none() && read["units"] == "in", "{read}");
    assert!((read["size"][0].as_f64().unwrap() - 100.0).abs() < 1e-3, "{read}");
    // Metres read as millimetres would be 0.1 across: refused, with what each unit would make it.
    let before = s.doc.features.len();
    let err = s.execute("file.insert_mesh", &json!({"path": metres})).unwrap_err().to_string();
    assert!(err.contains("give `units`") && err.contains("as m it is 100.0 mm") && err.contains("as in 2.5 mm"), "{err}");
    assert_eq!(s.doc.features.len(), before, "nothing was inserted");
    let read = run(&mut s, "file.insert_mesh", json!({"path": metres, "units": "m"}));
    assert!((read["size"][0].as_f64().unwrap() - 100.0).abs() < 1e-3, "{read}");
    // Told it is millimetres, a tiny part is a tiny part.
    let read = run(&mut s, "file.insert_mesh", json!({"path": metres, "units": "mm"}));
    assert!((read["size"][0].as_f64().unwrap() - 0.1).abs() < STL && read.get("units_assumed").is_none(), "{read}");
    let _ = std::fs::remove_dir_all(&dir);
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
    // Laid along its axes over the origin, on the grid: 80 along X, so its end at X = 40 is a face.
    let reply = run(&mut s, "scan.stand_on_base", json!({"point": [40.0, 0.0, 5.0]}));
    assert_eq!(reply["base"]["kind"], "grown", "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 80.0).abs() < STL, "standing on its end: {lo:?} {hi:?}");
}

/// Three points on the slab's corners at one end and the far corner of the other: the slab
/// stands on the plane through them, its 50 × 10 end face and beyond (the plane cuts across). Two
/// points are not enough; three in a line are refused.
#[test]
fn a_scan_stands_on_the_plane_through_points_picked_on_it() {
    let (mut s, body) = session_with_a_scan("points");
    run(&mut s, "scan.quick_align", json!({}));
    // The end at X = 40: three of its corners.
    let reply = run(&mut s, "scan.stand_on_base", json!({"points": [[40.0, -25.0, 0.0], [40.0, 25.0, 0.0], [40.0, 25.0, 10.0]]}));
    assert_eq!(reply["moved"], true, "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < 1e-3 && (hi.z - 80.0).abs() < 1e-3, "standing on its end, the part above: {lo:?} {hi:?}");
    assert!(s.execute("scan.stand_on_base", &json!({"points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]})).is_err(), "two points");
    let line = s.execute("scan.stand_on_base", &json!({"points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]}));
    assert!(line.is_err(), "three in a line");
}

/// Align by name runs the way chosen with the rest of the parameters; a way not known is refused
/// with the ones there are.
#[test]
fn align_runs_the_way_named() {
    let (mut s, body) = session_with_a_scan("align");
    let reply = run(&mut s, "scan.align", json!({"method": "own_axes", "body": body}));
    assert_eq!(reply["moved"], true, "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!((hi.x - lo.x - 80.0).abs() < 1e-3, "laid along X as Quick Align lays it: {lo:?} {hi:?}");
    let refused = s.execute("scan.align", &json!({"method": "sideways"})).unwrap_err().to_string();
    assert!(refused.contains("own_axes, to_scan, all, by_points, to_model"), "{refused}");
}

/// Quick align lays the slab along X, Y and Z about the origin; rest and centre then move only
/// what they say.
#[test]
fn quick_align_rest_and_centre_place_the_scan() {
    let (mut s, body) = session_with_a_scan("place");
    run(&mut s, "scan.quick_align", json!({}));
    let (lo, hi) = bounds(&s, &body);
    assert!(((hi - lo) - Vec3::new(80.0, 50.0, 10.0)).len() < STL, "{:?}", hi - lo);
    let middle = (lo + hi) / 2.0;
    assert!(middle.x.abs() < STL && middle.y.abs() < STL && lo.z.abs() < STL, "over the origin, on the grid");

    run(&mut s, "solid.move", json!({"bodies": [body], "translate": [0, 0, 12]}));
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

/// A scan inserted at the origin sits in a component lifted into place. Resting, centring and
/// standing it go by the world's grid and origin, where it shows, not by its component's own.
#[test]
fn a_scan_inserted_at_the_origin_rests_and_stands_on_the_world_grid() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-placed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("slab.stl");
    let axis = Vec3::new(0.6, -0.3, 0.74).normalized().unwrap();
    std::fs::write(&path, box_stl(Vec3::new(80.0, 50.0, 10.0), axis, 1.2, Vec3::new(40.0, -25.0, 60.0))).unwrap();
    let mut s = Session::default();
    let inserted = run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy(), "origin": true}));
    assert!(inserted["moved_to_origin"].is_array(), "lifted into place: {inserted}");
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    let shown = |s: &mut Session| {
        let m = run(s, "inspect.measure", json!({"bodies": [body]}));
        let b = &m["bodies"][0]["bbox"];
        let v = |k: &str| Vec3::new(b[k][0].as_f64().unwrap(), b[k][1].as_f64().unwrap(), b[k][2].as_f64().unwrap());
        (v("min"), v("max"))
    };
    run(&mut s, "solid.move", json!({"bodies": [body], "axis": [1, 0, 0], "angle": "90 deg"}));
    run(&mut s, "scan.rest_on_grid", json!({}));
    let (lo, _) = shown(&mut s);
    assert!(lo.z.abs() < 1e-3, "its lowest point on the grid: {lo:?}");
    run(&mut s, "scan.center", json!({}));
    let (lo, hi) = shown(&mut s);
    assert!(((lo + hi) / 2.0).x.abs() < 1e-3 && ((lo + hi) / 2.0).y.abs() < 1e-3 && lo.z.abs() < 1e-3, "over the origin: {lo:?} {hi:?}");
    run(&mut s, "scan.stand_on_base", json!({"base": 0}));
    let (lo, hi) = shown(&mut s);
    assert!(lo.z.abs() < 1e-3 && (hi.z - 10.0).abs() < 1e-3, "flat on the grid, a broad face down: {lo:?} {hi:?}");
    let again = run(&mut s, "scan.stand_on_base", json!({"base": 0}));
    assert_eq!(again["moved"], false, "already standing there: {again}");
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

/// A scan that missed the block's end beyond X = 45 is a mirror image front to back only (about
/// Y = 10), and that is the plane Find Symmetry finds on it. Its tower (20..40 across, above the
/// slab) with its back wall left out is a mirror image side to side only: given those triangles,
/// the plane found is the region's own, X = 30, and the reply says how many triangles it was
/// read on.
#[test]
fn find_symmetry_on_a_region_finds_that_regions_plane() {
    let (mut s, body, place) = session_with_part_of_the_block("symmetry-region", |t| (t[0] + t[1] + t[2]).x / 3.0 < 45.0);
    let (origin, x, y, z) =
        (place(Vec3::ZERO), place(Vec3::X) - place(Vec3::ZERO), place(Vec3::Y) - place(Vec3::ZERO), place(Vec3::Z) - place(Vec3::ZERO));
    let normal_of = |reply: &Value| {
        let v: Vec<f64> = reply["mirror"]["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        Vec3::new(v[0], v[1], v[2])
    };
    let whole = run(&mut s, "scan.symmetry", json!({"keep": false}));
    assert!(normal_of(&whole).dot(y).abs() > 1.0 - 1e-4, "front to back: {whole}");
    assert!(whole.get("region").is_none(), "{whole}");

    // The triangles whose middle, put back where the block was made, is above the slab and not
    // on the tower's back wall (Y = 20).
    let region: Vec<usize> = {
        let mesh = super::shown_mesh(&s, &body).unwrap();
        (0..mesh.triangles.len())
            .filter(|&t| {
                let [a, b, c] = mesh.triangles[t].map(|i| mesh.positions[i as usize]);
                let middle = (a + b + c) / 3.0 - origin;
                middle.dot(z) > 10.01 && middle.dot(y) < 19.99
            })
            .collect()
    };
    assert!(region.len() > 100, "{}", region.len());
    let reply = run(&mut s, "scan.symmetry", json!({"triangles": region, "keep": false}));
    assert_eq!(reply["mirror"]["symmetric"], true, "{reply}");
    assert_eq!(reply["region"].as_u64(), Some(region.len() as u64), "{reply}");
    let normal = normal_of(&reply);
    // Within 0.05° of the block's own X.
    assert!(normal.dot(x).abs() > 0.05f64.to_radians().cos(), "side to side: {reply}");
    let point: Vec<f64> = reply["mirror"]["point"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let off = (Vec3::new(point[0], point[1], point[2]) - (origin + x * 30.0)).dot(normal);
    assert!(off.abs() < 0.05, "through X = 30: {off} {reply}");
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

/// A scan that matches little of itself in the plane is completed anyway (it may be one side of a
/// mirror-image part), but the reply says what was added may not be the part: the Stanford bunny
/// was given 10,600 reflected triangles without a word. A scan missing only a sliver says nothing.
#[test]
fn complete_by_mirroring_warns_when_the_scan_is_no_mirror_image() {
    let (mut s, _, place) = session_with_part_of_the_block("mirror-one-side", |t| t.iter().any(|p| p.x < 45.0));
    let (point, normal) = (place(Vec3::new(30.0, 10.0, 10.0)), place(Vec3::X) - place(Vec3::ZERO));
    let plane = json!({"point": [point.x, point.y, point.z], "normal": [normal.x, normal.y, normal.z]});
    let reply = run(&mut s, "scan.complete_by_mirroring", json!({"plane": plane}));
    assert_eq!(reply["mirror"]["symmetric"], false, "{reply}");
    assert!(reply["warning"].as_str().is_some_and(|w| w.contains("matches its reflection") && w.contains("one side")), "{reply}");
    let (mut sliver, _, place) = session_with_part_of_the_block("mirror-sliver", |t| t.iter().any(|p| p.x < 58.0));
    let (point, normal) = (place(Vec3::new(30.0, 10.0, 10.0)), place(Vec3::X) - place(Vec3::ZERO));
    let plane = json!({"point": [point.x, point.y, point.z], "normal": [normal.x, normal.y, normal.z]});
    let reply = run(&mut sliver, "scan.complete_by_mirroring", json!({"plane": plane}));
    assert_eq!(reply["mirror"]["symmetric"], true, "{reply}");
    assert!(reply.get("warning").is_none(), "{reply}");
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

/// Onto XZ, as a vehicle is drawn, the block lies evenly either side of Y = 0 instead, its mirror
/// plane turned a quarter turn from where YZ lays it; anything else for `onto` is refused.
#[test]
fn square_to_symmetry_onto_xz_puts_the_mirror_plane_on_y_zero() {
    let (mut s, body, _) = session_with_a_symmetric_scan("square-xz");
    let reply = run(&mut s, "scan.square_to_symmetry", json!({"onto": "XZ"}));
    assert_eq!(reply["moved"], true, "{reply}");
    let (lo, hi) = bounds(&s, &body);
    assert!((lo.y + hi.y).abs() < STL * 10.0 && hi.y > 5.0, "evenly either side of Y = 0: {lo:?} {hi:?}");
    assert_eq!(run(&mut s, "scan.square_to_symmetry", json!({"onto": "xz"}))["moved"], false, "already square");
    let err = s.execute("scan.square_to_symmetry", &json!({"onto": "XY"})).unwrap_err().to_string();
    assert!(err.contains("YZ or XZ"), "{err}");
}

/// Refused unasked (as a car is, its underbody being no mirror image), Find
/// Symmetry gives the plane it found in the refusal, ready to pass back as `plane`, so an agent
/// needs no guide drawn on screen.
#[test]
fn a_refused_symmetry_gives_its_plane_to_pass_back() {
    // A slab with two blocks of different sizes off its corners: no plane mirrors it.
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(60.0, 20.0, 4.0), 8);
    triangles.extend(fine_box(Vec3::new(4.0, 0.0, 4.0), Vec3::new(24.0, 13.0, 34.0), 12));
    triangles.extend(fine_box(Vec3::new(38.0, 11.0, 4.0), Vec3::new(56.0, 20.0, 20.0), 12));
    let (mut s, _) = session_with_triangles("symmetry-pass-back", &triangles);
    let refused = s.execute("scan.symmetry", &json!({"keep": false})).unwrap_err().to_string();
    let at = refused.find("{\"point\"").unwrap_or_else(|| panic!("no plane in: {refused}"));
    let end = at + refused[at..].find('}').unwrap() + 1;
    let plane: Value = serde_json::from_str(&refused[at..end]).unwrap();
    let reply = run(&mut s, "scan.symmetry", json!({"plane": plane, "keep": false}));
    assert!(reply["mirror"]["coverage"].as_f64().unwrap() > 0.5, "{reply}");
}

/// A body 100 × 40 × 20 with a long box slung under one side of it: a mirror image about its
/// middle with both sides there, though the box has none. Find Symmetry says how much of the scan
/// that is; of a whole mirror image it says nothing more.
#[test]
fn find_symmetry_says_how_much_of_a_mirror_image_has_no_match() {
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(100.0, 40.0, 20.0), 40);
    triangles.extend(fine_box(Vec3::new(5.0, 2.0, -14.0), Vec3::new(70.0, 16.0, 0.0), 26));
    let (mut s, _) = session_with_triangles("symmetry-partly", &triangles);
    let reply = run(&mut s, "scan.symmetry", json!({"plane": {"point": [50, 20, 10], "normal": [0, 1, 0]}, "keep": false}));
    let mirror = &reply["mirror"];
    assert_eq!(mirror["symmetric"], true, "{reply}");
    let coverage = mirror["coverage"].as_f64().unwrap();
    assert!(coverage > 0.6 && coverage < 0.85, "{reply}");
    let said = mirror["partly"].as_str().unwrap_or_else(|| panic!("nothing said: {reply}"));
    assert!(said.starts_with(&format!("{:.0} % of the scan has no mirror image", 100.0 * (1.0 - coverage))), "{said}");
    // Completed by mirroring, the box would get an image the thing has not: warned of, and no
    // mirror image there.
    let done = run(&mut s, "scan.complete_by_mirroring", json!({"plane": {"point": [50, 20, 10], "normal": [0, 1, 0]}}));
    assert_eq!(done["mirror"]["symmetric"], false, "{done}");
    assert!(done["warning"].as_str().is_some_and(|w| w.contains("matches its reflection")) && done["mirror"].get("partly").is_none(), "{done}");
    let (mut s, _, _) = session_with_a_symmetric_scan("symmetry-whole");
    let reply = run(&mut s, "scan.symmetry", json!({"keep": false}));
    assert!(reply["mirror"]["symmetric"] == true && reply["mirror"].get("partly").is_none(), "{reply}");
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
    // A sketch on it runs as one on XY does: its X along the world's X, not any direction.
    let sketch = run(&mut s, "sketch.create", json!({"plane": plane["feature"]["name"].clone()}));
    run(&mut s, "sketch.finish", json!({}));
    s.refresh();
    let x = s.model.state().sketch(sketch["sketch"].as_u64().unwrap()).map(|k| k.plane.x).unwrap();
    assert!(x.x.abs() > 1.0 - 1e-6, "{x:?}");

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

/// Every way of choosing a region fits: what is selected on the scan (brushed), a kept selection,
/// the scan round a point within a radius; a point alone still grows the region, whatever is
/// selected. Fit Surface fits what is selected too, and trims to it.
#[test]
fn a_fit_takes_the_region_chosen_by_hand() {
    let (mut s, body) = session_with_a_plate_and_boss("fit-by-hand");
    run(&mut s, "scan.select", json!({"body": body, "brush": {"points": [[40, 20, 15]], "radius": 4}}));
    let brushed = run(&mut s, "scan.fit_cylinder", json!({"keep": false}));
    assert!((brushed["cylinder"]["diameter"].as_f64().unwrap() - 20.0).abs() < 0.05, "{brushed}");
    assert!(brushed["area"].as_f64().unwrap() < 0.5 * std::f64::consts::PI * 20.0 * 15.0, "only what was brushed: {brushed}");
    run(&mut s, "scan.keep_selection", json!({"name": "Wall"}));
    let kept = run(&mut s, "scan.fit_cylinder", json!({"saved": "Wall", "keep": false}));
    assert_eq!(kept["triangles"], brushed["triangles"], "{kept}");
    // A point given grows from it, whatever is selected.
    let grown = run(&mut s, "scan.fit_plane", json!({"point": [8, 8, 8], "keep": false}));
    assert!((grown["area"].as_f64().unwrap() - 2400.0).abs() < 1.0, "the whole top: {grown}");
    let round = run(&mut s, "scan.fit_plane", json!({"point": [8, 8, 8], "radius": 5, "keep": false}));
    assert!(round["plane"]["normal"][2].as_f64().unwrap().abs() > 1.0 - 1e-6, "{round}");
    assert!(round["area"].as_f64().unwrap() < 200.0, "only round the point: {round}");
    run(&mut s, "scan.select", json!({"body": body, "brush": {"points": [[12, 12, 8]], "radius": 10}}));
    let surface = run(&mut s, "scan.fit_surface", json!({"controls": 4, "trim": true}));
    assert!(surface["rms"].as_f64().unwrap() < 0.01 && surface.get("trimmed").is_some(), "{surface}");
}

/// A scan hidden with part of it still selected on is not the one a scan tool takes while another
/// scan shows: the shown one is, as the only scan would be.
#[test]
fn a_hidden_scan_gives_way_to_the_shown_one() {
    let (mut s, plate) = session_with_a_plate_and_boss("hidden-scan");
    // Writes the slab's file.
    session_with_a_scan("hidden-scan-slab");
    let slab_path = std::env::temp_dir().join(format!("solidcraft-scan-hidden-scan-slab-{}", std::process::id())).join("slab.stl");
    run(&mut s, "file.insert_mesh", json!({"path": slab_path.to_string_lossy()}));
    let slab = s.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).find(|n| *n != plate).unwrap();
    run(&mut s, "scan.select", json!({"body": plate, "brush": {"points": [[40, 20, 15]], "radius": 4}}));
    assert_eq!(super::mesh_body_name(&s, None, "scan.fill_holes").unwrap(), plate, "selected on, so taken");
    run(&mut s, "browser.visibility", json!({"items": [plate.as_str()], "visible": false}));
    assert_eq!(super::mesh_body_name(&s, None, "scan.fill_holes").unwrap(), slab);
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

/// A design rolled back is measured and exported as of the history marker: Scan Deviation's
/// `rolled_back` and the export's warnings say so (two agents in trials took a rolled-back model
/// for the built one), and say nothing once the marker is back at the end.
#[test]
fn a_rolled_back_design_is_said_when_measured_or_exported() {
    let (mut s, _) = session_with_a_plate_and_boss("rolled-back");
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 60, "width": 40, "height": 8}));
    run(&mut s, "solid.fillet", json!({"edges": [[30, 0, 8]], "radius": 1, "body": "Plate"}));
    let dir = std::env::temp_dir().join(format!("sc-rolled-back-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plate.stl");
    let at_end = run(&mut s, "file.export_stl", json!({"path": path, "bodies": ["Plate"]}));
    assert!(!at_end.to_string().contains("rolled back"), "{at_end}");
    assert!(run(&mut s, "scan.deviation", json!({"body": "Plate"}))["rolled_back"].is_null());
    let total = s.doc.features.len();
    run(&mut s, "timeline.rollback", json!({"position": total - 1}));
    let exported = run(&mut s, "file.export_stl", json!({"path": path, "bodies": ["Plate"]}));
    assert!(exported["warnings"].to_string().contains("rolled back: 1 feature"), "{exported}");
    let measured = run(&mut s, "scan.deviation", json!({"body": "Plate"}));
    assert!(measured["rolled_back"].as_str().is_some_and(|n| n.contains("1 feature")), "{measured}");
    run(&mut s, "timeline.rollback", json!({"position": "end"}));
    assert!(run(&mut s, "scan.deviation", json!({"body": "Plate"}))["rolled_back"].is_null());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scan of a block 60 × 40 × 20 passes against the block, both ways. A model with a boss the
/// scan does not have has every scan point on it or hidden inside it, so the scan's side alone
/// would pass it: the model's side, measured against the scan, fails it and says where. A model
/// half as tall is failed from the scan's side.
#[test]
fn scan_deviation_fails_a_model_with_material_the_scan_lacks_or_lacking_what_it_has() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-both-ways-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut stl = String::from("solid b\n");
    for t in fine_box(Vec3::ZERO, Vec3::new(60.0, 40.0, 20.0), 40) {
        stl += "facet normal 0 0 0\nouter loop\n";
        for p in t {
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    let path = dir.join("block.stl");
    std::fs::write(&path, stl + "endsolid b\n").unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let _ = std::fs::remove_dir_all(&dir);

    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 60, "width": 40, "height": 20}));
    let right = run(&mut s, "scan.deviation", json!({"tolerance": "0.1 mm"}));
    assert_eq!(right["passes"], true, "{right}");
    assert!(right.get("why_not").is_none(), "{right}");
    assert!((right["within_all"].as_f64().unwrap() - 1.0).abs() < 1e-9 && (right["model_within"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{right}");
    assert!(right["model_max"].as_f64().unwrap() < STL, "{right}");

    // A boss Ø 20 × 15 on top that the scan does not have. The scan's points under it (π 100 of
    // the scan's 8800 mm², 3.6 %) are too deep inside to measure and the rest lie on the model:
    // 96.4 % of all its points are within, which alone would pass. Of the model's 9742 mm², the
    // boss's top and all but the foot of its wall have no scan within the tolerance: 12.8 %.
    run(&mut s, "solid.cylinder", json!({"base": [30, 20, 20], "radius": 10, "height": 15, "operation": "join", "targets": ["Block"]}));
    let made_up = run(&mut s, "scan.deviation", json!({"tolerance": "0.1 mm"}));
    assert!(made_up["within_all"].as_f64().unwrap() > 0.95, "the scan's side alone would pass: {made_up}");
    let borne = made_up["model_within"].as_f64().unwrap();
    let expected = 1.0 - (std::f64::consts::PI * 100.0 + std::f64::consts::TAU * 10.0 * 14.9) / (8800.0 + std::f64::consts::TAU * 10.0 * 15.0);
    assert!((borne - expected).abs() < 0.01, "{borne} against {expected}: {made_up}");
    assert_eq!(made_up["passes"], false, "{made_up}");
    assert!(made_up["why_not"].as_str().unwrap().contains("has no scan near it"), "{made_up}");
    let at = made_up["model_farthest_at"].as_array().unwrap();
    assert!(at[2].as_f64().unwrap() > 22.0, "the farthest place is on the boss: {made_up}");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));

    // The block with a hole Ø 6 through it that the scan does not have: 1.3 % of the model's
    // surface and 0.6 % of the scan, so the shares within the tolerance pass both ways. The hole's
    // wall is up to 10 mm from the scan and the scan over its mouths up to 3 mm from the model.
    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 60, "width": 40, "height": 20}));
    run(&mut s, "solid.cylinder", json!({"base": [15, 20, -1], "radius": 3, "height": 22, "operation": "cut", "targets": ["Block"]}));
    let drilled = run(&mut s, "scan.deviation", json!({"tolerance": "0.1 mm"}));
    assert!(drilled["within_all"].as_f64().unwrap() > 0.95 && drilled["model_within"].as_f64().unwrap() > 0.95, "{drilled}");
    // Far off is three tolerances, 0.3. Of the model, the wall farther than that from an end:
    // 2 π 3 × 19.4 of 8800 − 2 π 9 + 2 π 3 × 20.
    assert!((drilled["far"].as_f64().unwrap() - 0.3).abs() < 1e-12, "{drilled}");
    let wall = std::f64::consts::TAU * 3.0;
    let expected = wall * 19.4 / (8800.0 - 2.0 * std::f64::consts::PI * 9.0 + wall * 20.0);
    assert!((drilled["model_far_off"].as_f64().unwrap() - expected).abs() < 0.002, "{expected}: {drilled}");
    assert_eq!(drilled["passes"], false, "{drilled}");
    assert!(drilled["why_not"].as_str().unwrap().contains("the model has something there the scan does not"), "{drilled}");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));

    // A block half as tall: the scan's top and the upper half of its sides are not on it.
    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 60, "width": 40, "height": 10}));
    let short = run(&mut s, "scan.deviation", json!({"tolerance": "0.1 mm"}));
    assert_eq!(short["passes"], false, "{short}");
    assert!(short["within_all"].as_f64().unwrap() < 0.7, "{short}");
    assert!(short["why_not"].as_str().unwrap().contains("missing what the scan has"), "{short}");
}

/// A plate's scan with a dent 1.5 deep over a round patch of radius 8, against the plate's solid:
/// Deviation gives the volume worn off (π R² D / 2 for the dent's bowl) and the one region it is,
/// below the model, deepest at its middle; nothing above. Neither the scan nor a closed pair of
/// them is needed for it.
#[test]
fn scan_deviation_measures_the_volume_and_the_region_of_a_dent() {
    let (radius, depth) = (8.0, 1.5);
    let triangles: Vec<[Vec3; 3]> = fine_box(Vec3::ZERO, Vec3::new(40.0, 40.0, 10.0), 80)
        .into_iter()
        .map(|t| {
            t.map(|p| {
                let r = (p.x - 20.0).hypot(p.y - 20.0) / radius;
                if (p.z - 10.0).abs() < 1e-9 && r < 1.0 { Vec3::new(p.x, p.y, p.z - depth * (1.0 - r * r)) } else { p }
            })
        })
        .collect();
    let (mut s, _) = session_with_triangles("dent", &triangles);
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 40, "width": 40, "height": 10, "center": [20, 20, 5]}));
    let reply = run(&mut s, "scan.deviation", json!({"tolerance": "0.05 mm", "limit": 2}));
    let expected = std::f64::consts::PI * radius * radius * depth / 2.0;
    let lost = reply["volume_below"].as_f64().unwrap();
    assert!((lost - expected).abs() < 0.03 * expected, "{lost} against {expected}: {reply}");
    assert!(reply["volume_above"].as_f64().unwrap() < 0.01 * expected && reply["area_above"].as_f64().unwrap() < 1.0, "{reply}");
    assert_eq!((reply["regions_found"].as_u64(), reply["regions"].as_array().map(Vec::len)), (Some(1), Some(1)), "{reply}");
    let dent = &reply["regions"][0];
    assert_eq!(dent["side"], "below", "{reply}");
    assert!((dent["deepest"].as_f64().unwrap() - depth).abs() < 1e-3 && (dent["volume"].as_f64().unwrap() - lost).abs() < 1e-9, "{reply}");
    let at: Vec<f64> = dent["at"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
    assert!((at[0] - 20.0).abs() < 1e-3 && (at[1] - 20.0).abs() < 1e-3 && (at[2] - 8.5).abs() < 1e-3, "{reply}");
}

/// A tool (a scanned block 28 × 18 × 8) in the pocket of its foam (a block 60 × 40 × 20 with a
/// pocket 30 × 20 × 10 in its top): 1 mm clear all round and of the floor, it fits, and the least
/// gap is that 1 mm, measured on the foam's exact faces. Sunk 3 mm lower it is 2 mm into the
/// floor, and does not fit. Sunk 14 mm lower, most of it is far inside the foam, further from any
/// face than Deviation measures: it still does not fit.
#[test]
fn scan_deviation_says_whether_a_scanned_thing_clears_a_solid() {
    let reply_at = |bottom: f64| {
        let tool = fine_box(Vec3::new(16.0, 11.0, bottom), Vec3::new(44.0, 29.0, bottom + 8.0), 24);
        let (mut s, _) = session_with_triangles("clearance", &tool);
        run(&mut s, "solid.box", json!({"body_name": "Foam", "operation": "new", "length": 60, "width": 40, "height": 20, "center": [30, 20, 10]}));
        run(&mut s, "solid.box", json!({"length": 30, "width": 20, "height": 10, "center": [30, 20, 15], "operation": "cut", "targets": ["Foam"]}));
        run(&mut s, "scan.deviation", json!({"body": "Foam", "tolerance": "0.1 mm", "limit": 2}))
    };
    let seated = reply_at(11.0);
    let clear = &seated["clearance"];
    assert_eq!(clear["fits"], true, "{seated}");
    assert!((clear["gap"].as_f64().unwrap() - 1.0).abs() < 1e-6 && clear["inside"].as_f64() == Some(0.0), "{seated}");
    assert!(clear.get("deepest").is_none(), "{seated}");
    let pressed = reply_at(8.0);
    let clear = &pressed["clearance"];
    assert_eq!(clear["fits"], false, "{pressed}");
    assert!((clear["deepest"].as_f64().unwrap() - 2.0).abs() < 1e-6 && clear["inside"].as_f64().unwrap() > 0.05, "{pressed}");
    assert!((clear["deepest_at"][2].as_f64().unwrap() - 8.0).abs() < 1e-6, "{pressed}");
    let buried = reply_at(-3.0);
    assert_eq!(buried["clearance"]["fits"], false, "{buried}");
    assert!(buried["clearance"]["deepest"].as_f64().unwrap() > 2.5, "{buried}");
}

/// A block 60 × 20 × 10 with a boss 20 × 30 on its middle, wider than the block: its outline seen
/// from above (Section to Sketch with `outline`) is the plus of all it covers, one closed outline
/// that a cut through the block alone would not draw, dimensioned, with no point of the scan
/// outside it by more than the tolerance.
#[test]
fn section_sketch_draws_a_scans_whole_outline_seen_along_the_plane() {
    let mut triangles = fine_box(Vec3::new(-30.0, -10.0, 0.0), Vec3::new(30.0, 10.0, 10.0), 24);
    triangles.extend(fine_box(Vec3::new(-10.0, -15.0, 10.0), Vec3::new(10.0, 15.0, 16.0), 16));
    let (mut s, _) = session_with_triangles("outline", &triangles);
    let cut = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "5 mm", "name": "Cut"}));
    assert_eq!((cut["outlines"].as_u64(), cut["lines"].as_u64()), (Some(1), Some(4)), "a cut through the block is its rectangle: {cut}");
    assert!(cut.get("outside").is_none(), "{cut}");
    run(&mut s, "edit.undo", json!({}));
    let outline = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "outline": true}));
    assert_eq!((outline["outlines"].as_u64(), outline["circles"].as_u64(), outline["dof"].as_u64()), (Some(1), Some(0), Some(0)), "{outline}");
    let curves = outline["lines"].as_u64().unwrap() + outline["arcs"].as_u64().unwrap();
    assert!((12..=20).contains(&curves), "the plus's twelve sides, a corner perhaps eased: {outline}");
    let (outside, tolerance) = (outline["outside"].as_f64().unwrap(), outline["tolerance"].as_f64().unwrap());
    assert!(outside <= tolerance, "{outline}");
    // Extruded a millimetre, it holds the plus's area: the block's and the boss's overhang.
    run(&mut s, "solid.extrude", json!({"sketch": outline["sketch"], "distance": 1, "operation": "new", "body_name": "Foot"}));
    let area = s.model.state().body("Foot").and_then(|b| b.body.volume_at(0.01)).unwrap();
    let plus = 60.0 * 20.0 + 2.0 * 20.0 * 5.0;
    assert!((area - plus).abs() < 0.03 * plus, "{area} against {plus}");
    let both = s.execute("scan.section_sketch", &json!({"outline": true, "axis": {"origin": [0, 0, 0], "dir": [0, 0, 1]}})).unwrap_err().to_string();
    assert!(both.contains("ask for one"), "{both}");
    // Asked of the solid, it says where a model's section is drawn, and that works.
    let solid = s.execute("scan.section_sketch", &json!({"body": "Foot", "plane": "XZ"})).unwrap_err().to_string();
    assert!(solid.contains("is a solid") && solid.contains("sketch.intersect"), "{solid}");
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "Model Cut"}));
    let crossed = run(&mut s, "sketch.intersect", json!({"refs": [{"body": "Foot"}]}));
    assert!(!crossed["created"]["curves"].as_array().is_none_or(Vec::is_empty), "{crossed}");
}

/// The model as an STL (as a part's CAD often comes, T-LESS's among them): the scan is measured
/// against the mesh as against the solid it was made from, and with no solid in the design the
/// other mesh is the model; a scan is not measured against itself.
#[test]
fn scan_deviation_measures_the_scan_against_a_model_mesh() {
    let (mut s, scan) = session_with_a_plate_and_boss("deviation-mesh");
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 60, "width": 40, "height": 8}));
    let against_solid = run(&mut s, "scan.deviation", json!({}));
    let path = std::env::temp_dir().join(format!("solidcraft-scan-deviation-mesh-{}", std::process::id())).join("model.stl");
    run(&mut s, "file.export", json!({"path": path, "bodies": ["Plate"]}));
    run(&mut s, "edit.undo", json!({}));
    let model = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut s, "select.clear", json!({}));
    let err = s.execute("scan.deviation", &json!({"scan": scan, "body": scan})).unwrap_err().to_string();
    assert!(err.contains("not itself"), "{err}");
    let reply = run(&mut s, "scan.deviation", json!({"scan": scan}));
    assert_eq!(reply["body"].as_str(), Some(model.as_str()), "the other mesh is the model: {reply}");
    for k in ["within", "max_above", "max_below", "rms"] {
        let (mesh, solid) = (reply[k].as_f64().unwrap(), against_solid[k].as_f64().unwrap());
        assert!((mesh - solid).abs() < 1e-3, "{k}: {mesh} against the mesh, {solid} against the solid");
    }
}

/// The map's readings: where its largest distances are, a limit read from the scan, colours in
/// steps, and places labelled with their distances (added by clicking, kept when measured again).
#[test]
fn scan_deviation_labels_its_extremes_and_pinned_places() {
    let (mut s, _) = session_with_a_plate_and_boss("deviation-pins");
    let err = s.execute("scan.deviation_pin", &json!({"at": [30, 20, 8]})).unwrap_err().to_string();
    assert!(err.contains("show Scan Deviation"), "no map, nothing to pin: {err}");
    run(&mut s, "solid.box", json!({"body_name": "Plate", "operation": "new", "length": 60, "width": 40, "height": 8}));
    let reply = run(&mut s, "scan.deviation", json!({}));
    // The largest above is the boss's lowest ring, 1.25 mm over the plate's top.
    let above = reply["max_above_at"].as_array().unwrap();
    assert!((above[2].as_f64().unwrap() - 9.25).abs() < STL, "{reply}");
    assert_eq!(reply["bands"], 0, "smooth by default: {reply}");
    assert_eq!(reply["limit"], json!(0.5), "the default limit as before: {reply}");

    // The limit read from the scan: above the tolerance and no further than the largest distance.
    let auto = run(&mut s, "scan.deviation", json!({"auto_limit": true}));
    let (limit, tolerance) = (auto["limit"].as_f64().unwrap(), auto["tolerance"].as_f64().unwrap());
    let largest = auto["max_above"].as_f64().unwrap().max(-auto["max_below"].as_f64().unwrap());
    assert!(limit > tolerance && limit <= largest.max(tolerance * 1.05) + 1e-6, "{auto}");
    let err = s.execute("scan.deviation", &json!({"auto_limit": true, "limit": 2})).unwrap_err().to_string();
    assert!(err.contains("not both"), "{err}");
    for bad_bands in [1, 13] {
        let err = s.execute("scan.deviation", &json!({"bands": bad_bands})).unwrap_err().to_string();
        assert!(err.contains("`bands`"), "{bad_bands}: {err}");
    }
    run(&mut s, "scan.deviation", json!({"bands": 4}));
    let map = s.scan_deviation.clone().unwrap();
    assert_eq!(map.bands, 4);
    let stepped = map.scale(0.3).unwrap();
    assert!((stepped * 4.0 - (stepped * 4.0).round()).abs() < 1e-9, "on a step: {stepped}");

    // A pin on the plate's top is within; one on the boss's lowest ring is above.
    let on_top = run(&mut s, "scan.deviation_pin", json!({"at": [5, 5, 8]}));
    assert_eq!(on_top["side"], "within", "{on_top}");
    let on_boss = run(&mut s, "scan.deviation_pin", json!({"at": [40, 20, 9.25]}));
    assert_eq!(on_boss["side"], "above", "{on_boss}");
    assert!((on_boss["added"]["value"].as_f64().unwrap() - 1.25).abs() < STL, "{on_boss}");
    assert_eq!(on_boss["pins"].as_array().unwrap().len(), 2);

    // Measured again with the same places (as the map does after an edit): the labels stay.
    let again = run(&mut s, "scan.deviation", json!({"bands": 4, "pins": [[5, 5, 8], [40, 20, 9.25]]}));
    assert_eq!(again["pins"].as_array().unwrap().len(), 2, "{again}");

    let err = s.execute("scan.deviation_pin", &json!({"at": [5, 5, 8], "clear": true})).unwrap_err().to_string();
    assert!(err.contains("one of"), "{err}");
    let err = s.execute("scan.deviation_pin", &json!({"remove": 7})).unwrap_err().to_string();
    assert!(err.contains("index"), "{err}");
    assert_eq!(run(&mut s, "scan.deviation_pin", json!({"remove": 0}))["pins"].as_array().unwrap().len(), 1);
    assert_eq!(run(&mut s, "scan.deviation_pin", json!({"clear": true}))["pins"], json!([]), "none left");
    assert!(s.scan_deviation.as_ref().is_some_and(|m| m.pins.is_empty()));
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
    run(&mut s, "select.set", json!({"items": [{"type": "face", "body": body, "point": [40, 0, 5]}]}));
    let picked = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!(picked["base"]["kind"], "grown", "{picked}");
    assert!(picked.get("base_index").is_none());
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 80.0).abs() < STL, "on its end: {lo:?} {hi:?}");
}

/// A part brought in standing is left standing. A slab lying on a broad face, off the grid: the
/// first press keeps that face down (its two broad faces tie, and it was turned over or laid on
/// its side, the base after the one it stood on) and only puts it on the grid over the origin;
/// the press after that goes on to the next base, as pressing again does.
#[test]
fn a_scan_already_standing_on_a_good_base_is_left_standing() {
    let (mut s, body) = session_with_triangles("standing", &fine_box(Vec3::new(5.0, 3.0, 2.0), Vec3::new(85.0, 53.0, 12.0), 4));
    let first = run(&mut s, "scan.stand_on_base", json!({}));
    assert!(first["base"]["normal"][2].as_f64().is_some_and(|z| z < -0.999), "the face that was down: {first}");
    let (lo, hi) = bounds(&s, &body);
    assert!(lo.z.abs() < STL && (hi.z - 10.0).abs() < STL, "flat on the grid: {lo:?} {hi:?}");
    assert!((lo.x + hi.x).abs() < 1e-3 && (lo.y + hi.y).abs() < 1e-3, "over the origin: {lo:?} {hi:?}");
    let (index, count) = (first["base_index"].as_u64().unwrap(), first["base_count"].as_u64().unwrap());
    // Standing where it should already, the same press changes nothing more...
    let mut copy = s.scratch();
    copy.scan_base_cycle = None;
    let again = run(&mut copy, "scan.stand_on_base", json!({}));
    assert_eq!((again["base_index"].as_u64(), again["moved"].as_bool()), (Some(index), Some(false)), "{again}");
    // ...and pressed again here, it goes on to the next base.
    let second = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!((second["base_index"].as_u64(), second["moved"].as_bool()), (Some((index + 1) % count), Some(true)), "{second}");
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
        solidcraft_doc::FeatureKind::Import {
            file: "fitted surface".into(),
            step: step.into(),
            components: Vec::new(),
            skip: Vec::new(),
            records: None,
        },
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

/// With a tolerance and no control points asked for, Fit Surface picks the fewest that come within
/// half of it; with `trim` it is cut to the round area's outline, so it is smaller than the
/// rectangle it spans untrimmed, and it is still one undo step.
#[test]
fn fit_surface_chooses_its_detail_and_trims_to_the_area() {
    let (mut s, _) = session_with_a_plate_and_boss("fit-surface-trim");
    let whole = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "tolerance": "0.1 mm"}));
    assert_eq!(whole["controls"], 6, "a flat area needs the fewest: {whole}");
    assert!(whole.get("trimmed").is_none(), "{whole}");
    let rectangle = area_of(&s, whole["surface"].as_str().unwrap());
    run(&mut s, "edit.undo", json!({}));
    let cut = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "tolerance": "0.1 mm", "trim": true}));
    assert!(cut.get("trimmed").is_some(), "{cut}");
    let trimmed = area_of(&s, cut["surface"].as_str().unwrap());
    // The coarse plate's triangles round the point, in the rectangle that spans them: well under it
    // (180 of 336 mm²), but not a sliver.
    assert!(trimmed < 0.8 * rectangle && trimmed > 0.3 * rectangle, "{trimmed} of {rectangle}");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().bodies.iter().all(|b| b.body.is_mesh()), "one undo step");
    // The whole face, with no radius: the plate's top as far as it goes, more than 8 mm round.
    let face = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "whole_face": true, "tolerance": "0.1 mm"}));
    assert!(face["points"].as_u64().unwrap() > whole["points"].as_u64().unwrap(), "{face} against {whole}");
    assert!(face["rms"].as_f64().unwrap() < 0.05, "still the flat top, not round the boss: {face}");
}

/// A surface fitted to an area of the plate's top, 8 mm round a point 12 mm in from two of its
/// sides, is extended for trimming: Extend Surface pushes its four edges out 5 mm, so it is larger
/// by a 5 mm margin all round (a rectangle w × h becomes (w + 10) × (h + 10), at least
/// 20 √area + 100 more), and it now passes the plate's side wall at X = 0, where Trim Surface cuts
/// it: less than the extended surface, still more than the fit.
#[test]
fn a_fitted_surface_is_extended_past_its_area_and_trimmed_by_the_face_beside_it() {
    let (mut s, _) = session_with_a_plate_and_boss("fit-surface-extend");
    let fit = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    let surface = fit["surface"].as_str().unwrap().to_owned();
    let fitted = area_of(&s, &surface);
    // Its four edges, each by its middle.
    let edges: Vec<[f64; 3]> = {
        let state = s.world_state().unwrap();
        state.body(&surface).unwrap().body.edges(0.01).unwrap().iter().map(|e| [e.mid.x, e.mid.y, e.mid.z]).collect()
    };
    assert_eq!(edges.len(), 4, "a rectangle: {edges:?}");
    run(&mut s, "surface.extend", json!({"body": surface, "edges": edges, "distance": "5 mm"}));
    let extended = area_of(&s, &surface);
    let margin = 20.0 * fitted.sqrt() + 100.0;
    assert!(extended >= fitted + 0.99 * margin, "{extended} from {fitted}: 5 mm more all round is {margin} more");
    run(&mut s, "surface.trim", json!({"body": surface, "plane": {"origin": [0, 0, 0], "normal": [1, 0, 0]}, "keep": [12, 12, 8]}));
    let trimmed = area_of(&s, &surface);
    assert!(trimmed < extended - 1.0 && trimmed > fitted, "{trimmed} of {extended}, the fit {fitted}");
}

/// Every scan command on meshes a file can hold and no scanner makes: one triangle, a triangle
/// with no area, the same triangle twice (once each way round), three triangles on one edge,
/// corners a light year apart, a part too small to see, a strip of needles, a closed tetrahedron,
/// a not-a-number corner, an infinite one, a file that says 1000 triangles and holds one, and an
/// empty file. Each command is called with nothing, and with a point; it does its work or refuses,
/// and none panics inside or takes more than 20 seconds.
#[test]
fn every_scan_command_survives_meshes_no_scanner_makes() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stl = |triangles: &[[[f32; 3]; 3]], said: Option<u32>| {
        let mut bytes = vec![0u8; 80];
        bytes.extend(said.unwrap_or(triangles.len() as u32).to_le_bytes());
        for t in triangles {
            bytes.extend([0u8; 12]);
            for corner in t {
                for v in corner {
                    bytes.extend(v.to_le_bytes());
                }
            }
            bytes.extend([0u8; 2]);
        }
        bytes
    };
    let one = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
    let far = 9.4e18f32;
    let tetra = |k: f32| {
        let (a, b, c, d) = ([0.0, 0.0, 0.0], [k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]);
        vec![[a, c, b], [a, b, d], [b, c, d], [a, d, c]]
    };
    let needles: Vec<[[f32; 3]; 3]> = (0..40).map(|i| [[i as f32, 0.0, 0.0], [i as f32 + 1.0, 0.0, 0.0], [i as f32 + 0.5, 1e-7, 0.0]]).collect();
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("one triangle", stl(&[one], None)),
        ("no area", stl(&[[[1.0, 1.0, 1.0]; 3]], None)),
        ("twice", stl(&[one, [one[0], one[2], one[1]]], None)),
        ("three on an edge", stl(&[one, [one[0], one[1], [0.0, 0.0, 10.0]], [one[0], one[1], [0.0, -10.0, -10.0]]], None)),
        ("far apart", stl(&[[[0.0, 0.0, 0.0], [far, 0.0, 0.0], [0.0, far, 0.0]]], None)),
        ("too small", stl(&tetra(1e-30), None)),
        ("needles", stl(&needles, None)),
        ("tetrahedron", stl(&tetra(10.0), None)),
        ("not a number", stl(&[[one[0], one[1], [f32::NAN, 1.0, 1.0]]], None)),
        ("infinite", stl(&[[one[0], one[1], [f32::INFINITY, 1.0, 1.0]]], None)),
        ("cut short", stl(&[one], Some(1000))),
        ("empty", stl(&[], None)),
    ];
    let ids: Vec<&str> = crate::command_specs().iter().map(|spec| spec.id).filter(|id| id.starts_with("scan.")).collect();
    assert!(ids.len() > 80, "the scan commands: {}", ids.len());
    let mut wrong = Vec::new();
    for (name, bytes) in &files {
        let path = dir.join("hostile.stl");
        std::fs::write(&path, bytes).unwrap();
        let mut start = Session::default();
        if let Err(crate::EngineError::Internal(_, said)) = start.execute("file.insert_mesh", &json!({"path": path.to_string_lossy(), "units": "mm"}))
        {
            wrong.push(format!("{name}: file.insert_mesh: {said}"));
        }
        let saved = start.doc.to_json();
        for id in &ids {
            for params in [json!({}), json!({"point": [1, 1, 0]})] {
                // Each call on the file as it came in, whatever the calls before did to it.
                let mut s = Session::new(solidcraft_doc::Document::from_json(&saved).unwrap());
                let began = std::time::Instant::now();
                if let Err(crate::EngineError::Internal(_, said)) = s.execute(id, &params) {
                    wrong.push(format!("{name}: {id} {params}: {said}"));
                }
                if began.elapsed().as_secs() >= 20 {
                    wrong.push(format!("{name}: {id} {params}: took {} s", began.elapsed().as_secs()));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wrong.is_empty(), "{} of them:\n{}", wrong.len(), wrong.join("\n"));
}

/// A Fit Surface trimmed to its area is still edited by Edit Surface: a control point moved moves
/// the surface, which stays trimmed to the area, and the edit is one undo step.
#[test]
fn edit_surface_edits_a_trimmed_fit_surface() {
    let (mut s, _) = session_with_a_plate_and_boss("edit-trimmed");
    let cut = run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6, "trim": true}));
    let name = cut["surface"].as_str().unwrap().to_owned();
    let trimmed = area_of(&s, &name);
    let start = super::surface::editable(&s, Some(&name), "test").expect("a trimmed Fit Surface is editable");
    let net = solidcraft_scan::surface_edit::points(&start.surface);
    let up = net[2 * 6 + 2] + Vec3::new(0.0, 0.0, 2.0);
    let raised = run(&mut s, "scan.edit_surface", json!({"surface": name, "moves": [{"point": [2, 2], "to": [up.x, up.y, up.z]}]}));
    assert!(raised["rms"].as_f64().unwrap() > 0.05, "moved off the plate: {raised}");
    let edited = super::surface::editable(&s, Some(&name), "test").unwrap();
    assert!(solidcraft_scan::surface_edit::points(&edited.surface)[2 * 6 + 2].dist(up) < 1e-9, "the point is where it was put");
    let after = area_of(&s, &name);
    assert!(after < 1.2 * trimmed, "still trimmed to the area: {after} against {trimmed}");
    run(&mut s, "edit.undo", json!({}));
    assert!((area_of(&s, &name) - trimmed).abs() < 1e-6, "one undo step");
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
    // Nothing to take off is no failure (a batch goes on): the scan and the timeline are as they
    // were, and the reply says so and why.
    let features = s.doc.features.len();
    let nothing = run(&mut s, "scan.remove_floating", json!({"below": 0.0}));
    assert_eq!((nothing["changed"].as_bool(), nothing["removed"].as_u64(), nothing["pieces"].as_u64()), (Some(false), Some(0), Some(2)), "{nothing}");
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("no piece floats")) && nothing.get("feature").is_none(), "{nothing}");
    assert_eq!(s.doc.features.len(), features, "no step is added");
}

/// A plate with a hole and a speck beside it: Clean Up takes the speck off and closes the hole,
/// each its own step; Repair has nothing to do and says so; one undo takes it all back. Fill
/// Holes given a point by the plate's rim caps the rim instead of the hole.
#[test]
fn clean_up_runs_the_tools_each_as_a_step() {
    let mut triangles = Vec::new();
    for y in 0..20 {
        for x in 0..20 {
            if (4..=5).contains(&x) && (4..=5).contains(&y) {
                continue;
            }
            let p = |i: i32, j: i32| Vec3::new(f64::from(x + i), f64::from(y + j), 0.0);
            triangles.push([p(0, 0), p(1, 0), p(1, 1)]);
            triangles.push([p(0, 0), p(1, 1), p(0, 1)]);
        }
    }
    let plate = triangles.clone();
    triangles.extend(fine_box(Vec3::new(40.0, 0.0, 0.0), Vec3::new(41.0, 1.0, 1.0), 1));
    let (mut s, body) = session_with_triangles("clean-up", &triangles);
    let features = s.doc.features.len();
    let reply = run(&mut s, "scan.clean_up", json!({}));
    let steps = reply["steps"].as_array().unwrap();
    let done: Vec<(&str, bool)> = steps.iter().map(|st| (st["command"].as_str().unwrap(), st["done"].as_bool().unwrap())).collect();
    assert_eq!(done, [("scan.repair", false), ("scan.remove_floating", true), ("scan.fill_holes", true)], "{reply}");
    assert_eq!(s.doc.features.len(), features + 2, "a step each");
    assert!(bounds(&s, &body).1.x <= 20.0 + STL, "the speck is gone");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.features.len(), features, "one undo step");
    assert!(s.execute("scan.clean_up", &json!({"repair": false, "floating": false, "holes": false})).is_err());

    // A point on the rim's corner: the rim is capped, the hole left.
    let (mut s, _) = session_with_triangles("cap-rim", &plate);
    let reply = run(&mut s, "scan.fill_holes", json!({"at": [[0.0, 0.0, 0.0]]}));
    assert_eq!(reply["filled"].as_u64(), Some(1), "{reply}");
    let survey = super::clean::holes_of(&s, None, "scan.fill_holes").unwrap().1;
    assert_eq!(survey.rings.len(), 1, "one ring is left");
    assert_eq!(survey.rings[0].points.len(), 8, "the hole, not the rim");
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
    // No hole within the limit is nothing to do, not a failure: the hole is counted as larger.
    let nothing = run(&mut s, "scan.fill_holes", json!({"max_edges": 4}));
    assert_eq!((nothing["changed"].as_bool(), nothing["filled"].as_u64(), nothing["larger"].as_u64()), (Some(false), Some(0), Some(1)), "{nothing}");
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("the smallest has 8")) && nothing.get("feature").is_none(), "{nothing}");
    assert_eq!(count(&s), 192, "the scan is as it was");

    let (mut closed, _) = session_with_triangles("fill-closed", &fine_box(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0), 4));
    let nothing = run(&mut closed, "scan.fill_holes", json!({}));
    assert_eq!((nothing["changed"].as_bool(), nothing["holes"].as_u64()), (Some(false), Some(0)), "{nothing}");
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("it is closed")), "{nothing}");
}

/// A finely cut box reduced to a tenth keeps its shape, one undo step; asking for more than it has
/// is nothing to do.
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
    // Asked for more triangles than it has: nothing to do, not a failure.
    let nothing = run(&mut s, "scan.reduce", json!({"triangles": 5000}));
    assert_eq!(
        (nothing["changed"].as_bool(), nothing["collapses"].as_u64(), nothing["triangles"].as_u64()),
        (Some(false), Some(0), Some(1728)),
        "{nothing}"
    );
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("already")) && nothing.get("feature").is_none(), "{nothing}");
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
    // Repaired, it needs no repair: no failure, nothing changed, no undo step of its own.
    let nothing = run(&mut s, "scan.repair", json!({}));
    assert_eq!((nothing["changed"].as_bool(), nothing["triangles"].as_u64()), (Some(false), Some(before as u64 - 1)), "{nothing}");
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("needs no repair")) && nothing.get("feature").is_none(), "{nothing}");
    assert!(reply.get("changed").is_none() && reply.get("note").is_none(), "a repair that was done says neither: {reply}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(count(&s), before, "one undo step");
}

/// A clean tool adds a Scan Clean step that keeps its settings, not its triangles: the Mesh Import
/// keeps the scan as it came in; suppressing the step brings the speck back; the step made again
/// with other settings takes nothing off and says so; the design saved and opened again cleans the
/// scan once more from the settings; and `in_place` writes the scan back as before, with no step.
#[test]
fn a_clean_step_keeps_settings_and_makes_the_scan_again() {
    use solidcraft_doc::FeatureKind;
    use solidcraft_doc::scan_clean::CleanStep;
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 12);
    let block = triangles.len();
    triangles.extend(fine_box(Vec3::new(50.0, 0.0, 5.0), Vec3::new(51.0, 1.0, 6.0), 1));
    let all = triangles.len();
    let (mut s, body) = session_with_triangles("clean-step", &triangles);
    let count = |s: &Session| s.model.state().body(&body).and_then(|b| b.body.triangle_mesh()).map_or(0, |(_, t)| t.len());
    let imported = |s: &Session| {
        s.doc.features.iter().find_map(|f| match &f.kind {
            FeatureKind::MeshImport { meshes, .. } => Some(meshes[0].triangles.len() / 3),
            _ => None,
        })
    };
    let reply = run(&mut s, "scan.remove_floating", json!({}));
    let id = reply["feature"].as_u64().unwrap();
    let step = s.doc.feature(id).unwrap();
    assert_eq!(
        step.kind,
        FeatureKind::ScanClean { body: body.clone().into(), step: CleanStep::RemoveFloating { below: 0.02, keep: Vec::new(), remove: Vec::new() } }
    );
    assert_eq!(step.name, "RemoveFloating1");
    assert_eq!((count(&s), imported(&s)), (block, Some(all)), "the scan cleaned, the import as it came in");

    run(&mut s, "timeline.suppress", json!({"feature": id, "suppressed": true}));
    assert_eq!(count(&s), all, "the step suppressed: the speck is back");
    run(&mut s, "timeline.suppress", json!({"feature": id, "suppressed": false}));
    assert_eq!(count(&s), block);

    // Made again with another setting: the step keeps it, in its place.
    run(&mut s, "timeline.redefine", json!({"feature": id, "command": "scan.remove_floating", "params": {"body": body, "below": 0.5}}));
    let at = s.doc.feature_index(id).unwrap();
    assert!(matches!(&s.doc.features[at].kind, FeatureKind::ScanClean { step: CleanStep::RemoveFloating { below, .. }, .. } if *below == 0.5));
    assert_eq!(count(&s), block);
    // The same step put in before it: the later one finds nothing to do, leaves the scan, says so.
    run(&mut s, "timeline.roll_to", json!({"position": at}));
    run(&mut s, "scan.remove_floating", json!({}));
    run(&mut s, "timeline.roll_to", json!({}));
    let later = s.doc.features.last().unwrap().id;
    let warning = s.model.result(later).and_then(|r| r.warning.clone()).unwrap_or_default();
    assert!(warning.contains("no piece floats") && s.model.result(later).is_some_and(|r| r.error.is_none()), "{warning}");
    assert_eq!(count(&s), block);

    let dir = std::env::temp_dir().join(format!("solidcraft-scan-clean-step-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cleaned.solidcraft");
    run(&mut s, "file.save", json!({"path": path.to_string_lossy()}));
    let mut opened = Session::default();
    run(&mut opened, "doc.open", json!({"path": path.to_string_lossy()}));
    assert_eq!((count(&opened), imported(&opened)), (block, Some(all)), "made again from the settings when the design opens");

    let (mut old, old_body) = session_with_triangles("clean-in-place", &triangles);
    let reply = run(&mut old, "scan.remove_floating", json!({"in_place": true}));
    assert!(matches!(old.doc.feature(reply["feature"].as_u64().unwrap()).map(|f| &f.kind), Some(FeatureKind::MeshImport { .. })), "{reply}");
    assert!(!old.doc.features.iter().any(|f| matches!(f.kind, FeatureKind::ScanClean { .. })));
    assert_eq!(old.model.state().body(&old_body).and_then(|b| b.body.triangle_mesh()).map(|(_, t)| t.len()), Some(block));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The distinct meshes of the scan `body` that the states after the timeline's features hold.
fn scan_copies(s: &Session, body: &str) -> usize {
    let mut seen: Vec<*const solidcraft_geom::Mesh> = Vec::new();
    for r in &s.model.results {
        if let Some(m) = r.state.body(body).and_then(|b| b.body.kept_mesh()) {
            let at = std::sync::Arc::as_ptr(&m);
            if !seen.contains(&at) {
                seen.push(at);
            }
        }
    }
    seen.len()
}

/// A run of three Scan Clean steps holds one working scan: the states after the import and the
/// steps hold two meshes of it in all (the scan as it came in and the latest result), however many
/// steps. Rolling back onto the first step, or editing the second, makes the scan again from the
/// import, as the steps left it; saved and opened again, the same two, and no step done again
/// until something reads the state after it.
#[test]
fn a_run_of_clean_steps_holds_the_scan_as_it_came_in_and_the_latest_result() {
    use solidcraft_doc::FeatureKind;
    let mut triangles = fine_box(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 12);
    let block = triangles.len();
    triangles.extend(fine_box(Vec3::new(50.0, 0.0, 5.0), Vec3::new(51.0, 1.0, 6.0), 1));
    let (mut s, body) = session_with_triangles("clean-run", &triangles);
    let count = |s: &Session| s.model.state().body(&body).and_then(|b| b.body.triangle_mesh()).map_or(0, |(_, t)| t.len());
    run(&mut s, "scan.remove_floating", json!({}));
    let half = run(&mut s, "scan.reduce", json!({"ratio": 0.5}))["triangles"].as_u64().unwrap() as usize;
    let quarter = run(&mut s, "scan.reduce", json!({"ratio": 0.5}))["triangles"].as_u64().unwrap() as usize;
    assert!(quarter < half && half < block, "{quarter} {half} {block}");
    assert_eq!(count(&s), quarter);
    assert_eq!(scan_copies(&s, &body), 2, "the scan as it came in and the latest result");

    // Rolled back onto the first step: made again from the import, the speck taken off.
    let steps: Vec<usize> =
        s.doc.features.iter().enumerate().filter(|(_, f)| matches!(f.kind, FeatureKind::ScanClean { .. })).map(|(i, _)| i).collect();
    run(&mut s, "timeline.roll_to", json!({"position": steps[0] + 1}));
    assert_eq!(count(&s), block, "the first step's scan");
    run(&mut s, "timeline.roll_to", json!({}));
    assert_eq!(count(&s), quarter);

    // The second step made again keeping a quarter: the third reads the scan as the second leaves it.
    let second = s.doc.features[steps[1]].id;
    run(&mut s, "timeline.redefine", json!({"feature": second, "command": "scan.reduce", "params": {"body": body, "ratio": 0.25}}));
    let now = count(&s);
    assert!(now < quarter, "{now} {quarter}");
    assert_eq!(scan_copies(&s, &body), 2);

    let dir = std::env::temp_dir().join(format!("solidcraft-scan-clean-run-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.solidcraft");
    // The model is kept with the design: the thinned states are left out, the last one kept.
    let saved = s.model.saved(&s.doc).expect("a model to keep");
    let kept: Vec<bool> = saved.records["features"].as_array().unwrap().iter().map(|f| f.get("state").is_some()).collect();
    assert_eq!(kept.iter().filter(|k| !**k).count(), 2, "the first two steps' states left out: {kept:?}");
    assert_eq!(kept.last(), Some(&true));
    run(&mut s, "file.save", json!({"path": path.to_string_lossy()}));
    let mut opened = Session::default();
    run(&mut opened, "doc.open", json!({"path": path.to_string_lossy()}));
    assert_eq!(count(&opened), now, "the last step's scan, from the file");
    assert_eq!(opened.model.last_recomputed, 0, "no step done again on opening, only to be thinned");
    assert_eq!(scan_copies(&opened, &body), 2, "opened, the same two");
    // The steps left out are done where something reads them: rolled back onto the first one.
    let steps_opened: Vec<usize> =
        opened.doc.features.iter().enumerate().filter(|(_, f)| matches!(f.kind, FeatureKind::ScanClean { .. })).map(|(i, _)| i).collect();
    run(&mut opened, "timeline.roll_to", json!({"position": steps_opened[0] + 1}));
    assert_eq!(count(&opened), block, "the first step's scan, made again");
    run(&mut opened, "timeline.roll_to", json!({"position": steps_opened[1] + 1}));
    let second_made = count(&opened);
    assert!(second_made < block && second_made > now, "{second_made} {now}");
    run(&mut opened, "timeline.roll_to", json!({}));
    assert_eq!(count(&opened), now);
    assert_eq!(scan_copies(&opened, &body), 2, "rolled back and forward, the same two");
    let _ = std::fs::remove_dir_all(&dir);
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
    // Finished dimensioned from its origin, as a sketch drawn by hand: nothing left free.
    assert_eq!(boss["dof"].as_u64(), Some(0), "{boss}");

    let plate = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "4 mm"}));
    assert_eq!((plate["lines"].as_u64(), plate["arcs"].as_u64(), plate["outlines"].as_u64()), (Some(4), Some(0), Some(1)), "{plate}");
    assert_eq!(plate["dof"].as_u64(), Some(0), "{plate}");
    // Asked not to, it is left as drawn.
    let loose = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "4 mm", "dimensions": false}));
    assert!(loose.get("dof").is_none(), "{loose}");
    let refused = s.execute("scan.section_sketch", &json!({"plane": "XY", "offset": "80 mm"})).unwrap_err().to_string();
    assert!(refused.contains("does not cut"), "{refused}");
    assert!(s.active_sketch.is_none(), "a refused cut leaves no sketch open");
}

/// A car panel's section: shallow arcs (radius 800 and 550 mm) a degree or two off tangent to a
/// 14 mm line between them. Held tangent, each touch point slides by the radius times that
/// angle, the line runs backwards and the solver refuses the fold, which left the whole
/// section unsolved and AutoConstrain refused it with dof 540 (the DrivAer car cut at x =
/// 1500). The joins it cannot hold are left as shared ends, so the drawn sketch solves.
#[test]
fn a_section_holds_tangent_only_the_joins_it_can_solve() {
    use super::section::{Outline, Piece};
    use solidcore_record::Vec2;
    let v = Vec2::new;
    let pieces = vec![
        Piece::Line { from: v(-853.706, 554.078), to: v(-854.3678, 556.2459) },
        Piece::Arc { center: v(-847.3821, 560.0361), radius: 7.9476, from: v(-854.3678, 556.2459), to: v(-854.4377, 563.6945), ccw: false },
        Piece::Arc { center: v(-815.9996, 554.2557), radius: 39.58, from: v(-854.4377, 563.6945), to: v(-852.5849, 569.3585), ccw: false },
        Piece::Arc { center: v(-108.535, 262.0971), radius: 804.9968, from: v(-852.5849, 569.3585), to: v(-835.0874, 608.7112), ccw: false },
        Piece::Line { from: v(-835.0874, 608.7112), to: v(-828.6539, 621.1606) },
        Piece::Arc { center: v(-347.2827, 359.1716), radius: 548.0478, from: v(-828.6539, 621.1606), to: v(-794.2433, 676.3194), ccw: false },
        Piece::Line { from: v(-794.2433, 676.3194), to: v(-781.7778, 694.0004) },
        Piece::Arc { center: v(-776.2255, 692.1857), radius: 5.8413, from: v(-781.7778, 694.0004), to: v(-777.9452, 697.7682), ccw: false },
        Piece::Line { from: v(-777.9452, 697.7682), to: v(-771.3709, 699.774) },
    ];
    let solve = |sk: &mut solidcraft_sketch::Sketch| {
        solidcraft_sketch::solve_with(sk, &solidcraft_sketch::Hold::default(), None, solidcraft_sketch::Analysis::Freedom, None)
    };
    // Every smooth join held tangent, as before: the solve folds and is refused.
    let mut all = solidcraft_sketch::Sketch::new();
    let mut ends: Vec<usize> = pieces.iter().map(|p| all.add_point(p.from(), None).unwrap()).collect();
    ends.push(all.add_point(pieces[8].to(), None).unwrap());
    let mut curves = Vec::new();
    for (k, piece) in pieces.iter().enumerate() {
        curves.push(match *piece {
            Piece::Arc { center, from, to, .. } => all.add_arc(center, to, from, [None, Some(ends[k + 1]), Some(ends[k])], None).unwrap(),
            Piece::Line { .. } => all.add_line_pts(ends[k], ends[k + 1], None).unwrap(),
        });
    }
    // The joins within Section to Sketch's 3° (the car's tangents k42 to k46).
    for (a, b) in [(2, 3), (3, 4), (4, 5), (5, 6), (7, 8)] {
        all.add_constraint(solidcraft_sketch::ConstraintKind::Tangent { a: curves[a], b: curves[b] }, None).unwrap();
    }
    let folded = solve(&mut all);
    assert!(!folded.folded.is_empty(), "{folded:?}");

    let mut sk = solidcraft_sketch::Sketch::new();
    let mut added = super::section::Added::default();
    super::section::draw(&mut sk, &[(Outline::Chain(pieces), false)], &mut added).unwrap();
    let rep = solve(&mut sk);
    assert!(rep.ok() && rep.folded.is_empty(), "{rep:?}");
    assert!(added.tangents > 0 && added.tangents < 5, "some joins stay tangent: {}", added.tangents);
    assert_eq!(added.tangents, sk.constraints.len());
}

/// The Sketch tab's Fit Curves to Mesh Section on a scan cuts it as Section to Sketch does: the
/// boss's 96-sided ring is one circle of its radius in the sketch being edited, not 96 lines.
#[test]
fn fit_curves_to_mesh_section_cuts_a_scan_as_section_to_sketch_does() {
    let (mut s, scan) = session_with_a_plate_and_boss("fit-section");
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": "15 mm"}));
    let fitted = run(&mut s, "sketch.fit_curves_to_section", json!({"body": scan}));
    assert_eq!(fitted["curves"].as_array().map(Vec::len), Some(1), "{fitted}");
    let id = super::super::sketch_store::active(&s).unwrap();
    let sk = super::super::sketch_store::read(&s.doc, id).unwrap();
    let Some(solidcraft_sketch::CurveKind::Circle { r, .. }) = sk.curves.first().map(|c| &c.kind) else { panic!("a circle") };
    assert!((r - 10.0).abs() < 0.05, "the boss's radius: {r}");
    assert!(s.active_sketch.is_some(), "the sketch stays open for the user");
}

/// A scan command computed on a copy and adopted (an agent's, off the window thread) leaves the
/// scan's view state as running it here would: the selection it made, the deviation map it
/// measured or took away; what it left alone stays as the session has it.
#[test]
fn a_scan_command_computed_on_a_copy_brings_back_what_it_did_to_the_view() {
    let (mut s, _) = session_with_a_plate_and_boss("adopt-view");
    let made = run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 8}));
    let model = made["created_bodies"][0].as_str().unwrap().to_owned();
    let adopt = |s: &mut Session, id: &str, p: Value| {
        let prepared = s.scratch().compute_commands(&[(id.into(), p)], None).unwrap();
        s.accept_commands(prepared).unwrap();
    };
    adopt(&mut s, "scan.select", json!({"brush": {"points": [[30, 20, 8]], "radius": 5}}));
    let picked = s.scan_selection.clone().expect("the selection made on the copy");
    assert!(picked.count > 0);
    adopt(&mut s, "scan.deviation", json!({"body": model}));
    assert!(s.scan_deviation.is_some(), "the map measured on the copy");
    assert!(s.scan_selection.as_ref().is_some_and(|k| std::sync::Arc::ptr_eq(k, &picked)), "the selection it left alone");
    adopt(&mut s, "scan.deviation", json!({"show": false}));
    assert!(s.scan_deviation.is_none(), "the map taken away on the copy");
}

/// Compare Bodies with a scan (an open mesh) among them says to measure it with Scan Deviation.
#[test]
fn compare_bodies_on_a_scan_points_to_scan_deviation() {
    let (mut s, scan) = session_with_a_plate_and_boss("compare-scan");
    let made = run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 8}));
    let model = made["created_bodies"][0].as_str().unwrap().to_owned();
    let compared = run(&mut s, "inspect.compare", json!({"a": model, "b": scan}));
    let warnings = compared["warnings"].to_string();
    assert!(warnings.contains("scan.deviation") && warnings.contains(&scan), "{compared}");
}

/// Left to the cut, the tolerance is three times its noise and never under 0.1 mm: the clean plate
/// is fitted at 0.1 mm, as before; a cut whose points wander 0.1 mm either way in turn is fitted at
/// about 0.7 mm, so its noise is averaged, not traced (a T-LESS part read 1000 times too large had
/// 358 pieces at a fixed 0.1 mm, against 61 at its own size).
#[test]
fn a_section_left_to_itself_is_fitted_at_three_times_its_noise() {
    use solidcore_record::Vec2;
    use solidcraft_scan::section::Run;
    let (mut s, _) = session_with_a_plate_and_boss("section-noise");
    let plate = run(&mut s, "scan.section_sketch", json!({"plane": "XY", "offset": "4 mm"}));
    assert_eq!(plate["tolerance"].as_f64(), Some(super::section::TOLERANCE), "{plate}");
    let line = |wander: f64| {
        let points: Vec<Vec2> = (0..400).map(|i| Vec2::new(0.25 * f64::from(i), if i % 2 == 0 { wander } else { -wander })).collect();
        vec![Run { triangles: vec![0; points.len()], points, closed: false }]
    };
    assert_eq!(super::section::noise_tolerance(&line(0.0)), super::section::TOLERANCE, "a clean cut");
    let noisy = super::section::noise_tolerance(&line(0.1));
    assert!((0.5..1.0).contains(&noisy), "{noisy}");
    // Never more than a hundredth of the cut's size (its box 99.75 × 10 mm).
    let wild = super::section::noise_tolerance(&line(5.0));
    assert!((wild - 0.01 * 99.75f64.hypot(10.0)).abs() < 1e-9, "{wild}");
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

/// New Free Form over a sharp edge (two flats meeting square, no round): no net of points one face
/// can have follows it, so the reply says how far the face bows off the scan and to make a face
/// each side; over the dome it lies on the scan and says nothing. Overall Shape is not warned.
#[test]
fn free_form_over_an_edge_too_sharp_for_one_face_says_so() {
    let mut profile = Vec::new();
    for k in 0..=60 {
        profile.push((-30.0 + 30.0 * f64::from(k) / 60.0, 0.0));
    }
    for k in 1..=60 {
        profile.push((0.0, -30.0 * f64::from(k) / 60.0));
    }
    let at = |j: u32, k: usize| Vec3::new(0.5 * f64::from(j), profile[k].0, profile[k].1);
    let mut triangles = Vec::new();
    for j in 0..120 {
        for k in 0..profile.len() - 1 {
            triangles.push([at(j, k), at(j, k + 1), at(j + 1, k + 1)]);
            triangles.push([at(j, k), at(j + 1, k + 1), at(j + 1, k)]);
        }
    }
    let (mut s, _) = session_with_triangles("free-form-sharp", &triangles);
    let corners = json!([[10, -20, 0], [50, -20, 0], [50, 0, -20], [10, 0, -20]]);
    let made = run(&mut s, "scan.free_form", json!({"corners": corners}));
    let warning = made["warning"].as_str().unwrap_or_default();
    assert!(warning.contains("off the scan") && warning.contains("each side"), "{made}");
    assert_eq!(made["controls"], json!([12, 12]), "every count was tried: {made}");
    let overall = run(&mut s, "scan.free_form", json!({"corners": corners, "fit": "overall"}));
    assert!(overall.get("warning").is_none(), "{overall}");

    let (mut s, _) = session_with_triangles("free-form-dome-quiet", &dome_triangles(40));
    let made = run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]]}));
    assert!(made.get("warning").is_none(), "{made}");
}

/// New Free Form from four curves: the sides follow the curves given in any order and direction,
/// the inside lies on the scan; curves that do not close, or corners given as well, are refused.
#[test]
fn free_form_from_four_boundary_curves_follows_them_and_lies_on_the_scan() {
    let (mut s, _) = session_with_triangles("free-form-curves", &dome_triangles(40));
    let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
    let line = |a: [f64; 2], b: [f64; 2]| -> Vec<[f64; 3]> {
        (0..=8)
            .map(|i| {
                let t = f64::from(i) / 8.0;
                let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
                [x, y, z(x, y)]
            })
            .collect()
    };
    let mut top = line([50.0, 50.0], [10.0, 50.0]);
    top.reverse();
    let curves = json!([top, line([10.0, 10.0], [50.0, 10.0]), line([50.0, 10.0], [50.0, 50.0]), line([10.0, 50.0], [10.0, 10.0])]);
    let made = run(&mut s, "scan.free_form", json!({"curves": curves, "count": 6}));
    assert_eq!(made["controls"], json!([6, 6]), "{made}");
    assert!(made["rms"].as_f64().unwrap() < 0.1, "{made}");
    let both =
        s.execute("scan.free_form", &json!({"curves": curves, "corners": [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]]})).unwrap_err().to_string();
    assert!(both.contains("not both"), "{both}");
    let open = s.execute("scan.free_form", &json!({"curves": [line([10.0, 10.0], [50.0, 10.0]), line([50.0, 10.0], [50.0, 50.0]), line([50.0, 50.0], [10.0, 50.0]), line([10.0, 10.0], [10.0, 40.0])]})).unwrap_err().to_string();
    assert!(!open.is_empty(), "an open loop is refused");
}

/// A flat Free Form: fitted to the scan it lies in the one plane through the panel (a dome's top
/// is a plane the dome only leans away from), where Follow lies on the dome; given a plane it lies
/// flat on it with no scan at all; a plane with no direction, a plane with another fit and a bad
/// fit name are refused.
#[test]
fn a_flat_free_form_lies_in_one_plane_fitted_to_the_scan_or_chosen() {
    let (mut s, _) = session_with_triangles("flat-free-form", &dome_triangles(40));
    let corners = json!([[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]]);
    let flat = run(&mut s, "scan.free_form", json!({"corners": corners, "count": 5, "fit": "flat"}));
    assert_eq!((flat["surface"].as_str(), flat["controls"].clone()), (Some("FreeForm1"), json!([5, 5])), "{flat}");
    assert!(flat["met"] == flat["places"] && flat["kept"].as_u64().unwrap() > 0, "{flat}");
    let net = solidcraft_scan::surface_edit::points(&super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().surface);
    let (point, normal, _) = solidcraft_scan::free_form::flat_plane(&net).unwrap();
    let off = net.iter().map(|p| (*p - point).dot(normal).abs()).fold(0.0, f64::max);
    assert!(off < 1e-9, "every point of the net in one plane: {off}");
    // The dome curves away from its top: flat is farther from the scan than Follow the Scan is.
    let follow = run(&mut s, "scan.free_form", json!({"corners": corners, "count": 5}));
    assert!(flat["rms"].as_f64().unwrap() > follow["rms"].as_f64().unwrap(), "{flat} {follow}");

    // A chosen plane: no scan is looked at, the corners are put onto it.
    let (mut empty, _) = session_with_triangles("flat-on-plane", &dome_triangles(4));
    let on = run(
        &mut empty,
        "scan.free_form",
        json!({"corners": [[0, 0, 1], [10, 0, 3], [10, 10, -2], [0, 10, 4]], "plane": {"point": [0, 0, 5], "normal": [0, 0, 2]}, "count": 4}),
    );
    assert!(on.get("body").is_none() && on["places"] == 0, "{on}");
    let name = on["surface"].as_str().unwrap().to_owned();
    let net = solidcraft_scan::surface_edit::points(&super::surface::editable(&empty, Some(&name), "test").unwrap().surface);
    assert!(net.iter().all(|p| (p.z - 5.0).abs() < 1e-9), "all on z = 5: {:?}", net.iter().map(|p| p.z).collect::<Vec<_>>());
    let named = run(&mut empty, "scan.free_form", json!({"corners": [[0, 0, 1], [10, 0, 1], [10, 10, 1], [0, 10, 1]], "plane": "XY", "fit": "flat"}));
    assert!(named["rms"] == 0.0, "{named}");

    for (params, want) in [
        (json!({"corners": corners, "plane": {"point": [0, 0, 0], "normal": [0, 0, 0]}}), "direction"),
        (json!({"corners": corners, "plane": "XY", "fit": "follow"}), "flat"),
        (json!({"corners": corners, "fit": "level"}), "flat"),
        (json!({"corners": corners, "plane": "NoSuchPlane"}), "no plane"),
        (json!({"corners": [[0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0]], "plane": "XY"}), "no face"),
    ] {
        let refused = s.execute("scan.free_form", &params).unwrap_err().to_string();
        assert!(refused.contains(want), "{params}: {refused}");
    }
}

/// A face added on an open edge of a Free Form: one surface body of two faces sharing the edge,
/// tangent across it and lying on the dome; points on it moved by `dots` take the surface through
/// them, the other faces' points held; the net is doubled as a whole; each one undo step.
#[test]
fn add_face_grows_a_free_form_into_a_network_edited_by_its_points() {
    let (mut s, _) = session_with_triangles("add-face", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 50, 0], [10, 50, 0]], "count": 5}));
    let added = run(&mut s, "scan.add_face", json!({"edge": [[30, 10, 0], [30, 50, 0]], "far": [[50, 10, 0], [50, 50, 0]], "rows": 5}));
    assert_eq!((added["surface"].as_str(), added["faces"].as_u64(), added["face"].as_u64()), (Some("FreeForm1"), Some(2), Some(1)), "{added}");
    assert!(added["rms"].as_f64().unwrap() < 0.05 && added["kink"].as_f64().unwrap() < 0.5, "on the dome and tangent: {added}");
    let net = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap();
    assert_eq!(net.faces.len(), 2, "one body of two faces, editable");
    assert_eq!(solidcraft_kernel::brep_record(&s.model.state().body("FreeForm1").unwrap().body).unwrap().faces.len(), 2);
    // Its edge is not open any more: a face there is refused.
    let refused =
        s.execute("scan.add_face", &json!({"edge": [[30, 10, 0], [30, 50, 0]], "far": [[20, 10, 0], [20, 50, 0]]})).unwrap_err().to_string();
    assert!(refused.contains("open edge"), "{refused}");

    // A point in the middle of the shared edge pulled up 2 mm: the surface runs through it.
    let joins = solidcraft_scan::network::joins(&net.faces).unwrap();
    let dots = solidcraft_scan::network::dots(&net.faces, &joins);
    let on_seam =
        (0..dots.len()).filter(|p| joins.on_seam(*p)).min_by(|a, b| (dots[*a].at.y - 30.0).abs().total_cmp(&(dots[*b].at.y - 30.0).abs())).unwrap();
    let was = dots[on_seam].at;
    let moved = run(&mut s, "scan.edit_surface", json!({"dots": [{"at": [was.x, was.y, was.z], "along": "2 mm"}]}));
    assert_eq!((moved["moved"].as_u64(), moved["faces"].as_u64()), (Some(1), Some(2)), "{moved}");
    let after = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap();
    let aj = solidcraft_scan::network::joins(&after.faces).unwrap();
    let now = solidcraft_scan::network::dots(&after.faces, &aj);
    let risen = now.iter().map(|d| d.at.dist(was)).filter(|d| *d > 1.0).fold(f64::INFINITY, f64::min);
    assert!((risen - 2.0).abs() < 0.1, "a point on the surface 2 mm along the normal from where it was: {risen}");
    assert_eq!(aj.seams.len(), 1, "still one seam");
    assert!(solidcraft_scan::network::worst_kink(&after.faces, &aj) < 1.0, "still tangent");
    // The control net of a network is not edited point by point.
    let refused = s.execute("scan.edit_surface", &json!({"moves": [{"point": [1, 1], "along": "1 mm"}]})).unwrap_err().to_string();
    assert!(refused.contains("2 faces"), "{refused}");
    let refined = run(&mut s, "scan.edit_net", json!({"op": "refine"}));
    assert_eq!(refined["controls"], json!([7, 7]), "{refined}");
    assert_eq!(
        super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.iter().map(|f| f.count()).collect::<Vec<_>>(),
        vec![[7, 7], [7, 7]]
    );
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 1, "one undo step each");
}

/// A Free Form closed round a scanned box face by face with Add Face: each face after the first
/// shares one, two or (the lid) four sides; where the box turns the sides are left creases; the
/// kernel sews the six faces into a solid of the box's volume.
#[test]
fn add_face_closes_a_free_form_round_a_box() {
    let (mut s, _) = session_with_triangles("add-face-box", &fine_box(Vec3::ZERO, Vec3::new(40.0, 40.0, 40.0), 40));
    run(&mut s, "scan.free_form", json!({"corners": [[0, 0, 0], [40, 0, 0], [40, 0, 40], [0, 0, 40]], "count": 5}));
    let steps = [
        ([[0, 0, 0], [40, 0, 0]], [[0, 40, 0], [40, 40, 0]]),
        ([[40, 0, 0], [40, 40, 0]], [[40, 0, 40], [40, 40, 40]]),
        ([[0, 40, 0], [40, 40, 0]], [[0, 40, 40], [40, 40, 40]]),
        ([[0, 0, 0], [0, 40, 0]], [[0, 0, 40], [0, 40, 40]]),
        ([[0, 0, 40], [40, 0, 40]], [[0, 40, 40], [40, 40, 40]]),
    ];
    let mut last = Value::Null;
    for (edge, far) in steps {
        last = run(&mut s, "scan.add_face", json!({"edge": edge, "far": far, "rows": 5}));
        assert!(last["rms"].as_f64().unwrap() < 0.05 && last["sharp"].as_u64().unwrap() >= 1, "{last}");
    }
    assert_eq!((last["faces"].as_u64(), last["irregular"].as_u64(), last["sharp"].as_u64()), (Some(6), Some(8), Some(4)), "{last}");
    let state = s.model.state();
    let body = state.body("FreeForm1").expect("the Free Form");
    let volume = body.body.volume_at(1e-3).unwrap_or(0.0);
    assert!((volume - 64_000.0).abs() < 100.0, "closed into a solid of the box's volume: {volume}");
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 6, "still editable as a Free Form");
}

/// A ridge scanned as a grid, `n` squares a side over 60 mm: z = 10 − 0.2·|x − 30|, its sides
/// meeting at 23°, within what Add Face holds tangent.
fn ridge_triangles(n: u32) -> Vec<[Vec3; 3]> {
    let at = |i: u32, j: u32| {
        let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
        Vec3::new(x, y, 10.0 - 0.2 * (x - 30.0).abs())
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

/// A Free Form grown over a ridge the smooth way rounds the ridge; `crease` on the shared edge
/// at the ridge makes the faces meet at an angle and follow it (the reply counts the crease, and
/// it reads back as one); `uncrease` makes them smooth again. Each is one undo step. Hostile
/// picks are refused in words.
#[test]
fn crease_makes_a_shared_edge_follow_a_ridge_and_uncrease_makes_it_smooth_again() {
    let (mut s, _) = session_with_triangles("crease", &ridge_triangles(60));
    // Corners on the scan: a corner below the ridge would snap to the slope beside it.
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 6], [30, 10, 10], [30, 50, 10], [10, 50, 6]], "count": 5}));
    let grown = run(&mut s, "scan.add_face", json!({"edge": [[30, 10, 10], [30, 50, 10]], "far": [[50, 10, 6], [50, 50, 6]], "rows": 5}));
    assert_eq!((grown["faces"].as_u64(), grown["sharp"].as_u64()), (Some(2), Some(0)), "held tangent across the ridge: {grown}");
    let rounded = grown["rms"].as_f64().unwrap();
    assert!(rounded > 0.05, "the ridge rounded over: {grown}");
    let creased = run(&mut s, "scan.edit_surface", json!({"crease": [{"at": [30, 30, 10]}]}));
    assert_eq!((creased["faces"].as_u64(), creased["creases"].as_u64()), (Some(2), Some(1)), "{creased}");
    assert!(creased["rms"].as_f64().unwrap() < rounded / 2.0, "follows the ridge now: {creased} against {rounded}");
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    let joins = solidcraft_scan::network::joins(&faces).unwrap();
    assert_eq!(solidcraft_scan::network::creased(&joins), vec![true], "kept by the angle the faces meet at");
    assert!(solidcraft_scan::network::seam_angle(&faces, &joins.seams[0]) > 15.0);
    // Already a crease: nothing changes but the points refitted; still one crease.
    let again = run(&mut s, "scan.edit_surface", json!({"crease": [{"edge": [[30, 10, 10], [30, 50, 10]]}]}));
    assert_eq!(again["creases"].as_u64(), Some(1), "{again}");
    let smooth = run(&mut s, "scan.edit_surface", json!({"uncrease": [{"edge": [[30, 50, 10], [30, 10, 10]]}]}));
    assert_eq!(smooth["creases"].as_u64(), Some(0), "{smooth}");
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    let joins = solidcraft_scan::network::joins(&faces).unwrap();
    assert!(solidcraft_scan::network::worst_kink(&faces, &joins) < 1.0, "{}", solidcraft_scan::network::worst_kink(&faces, &joins));
    run(&mut s, "edit.undo", json!({}));
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    assert_eq!(solidcraft_scan::network::creased(&solidcraft_scan::network::joins(&faces).unwrap()), vec![true], "one undo step");
    for (params, words) in [
        (json!({"crease": [{"at": [0, 0, 100]}]}), "from the nearest shared edge"),
        (json!({"crease": [{"at": [30, 30, 10], "edge": [[30, 10, 10], [30, 50, 10]]}]}), "one of them"),
        (json!({"crease": [{}]}), "one of them"),
        (json!({"crease": [{"at": [30, 30, 10]}], "dots": [{"at": [20, 30, 8], "along": "1 mm"}]}), "without points"),
        (json!({"crease": [{"at": [30, 30, 10]}], "uncrease": [{"at": [30, 30, 10]}]}), "already named"),
        (json!({"crease": []}), "1 to 256 edges"),
        (json!({"uncrease": [{"edge": [[0, 0, 0], [1, 1, 1]]}]}), "not the ends of a shared edge"),
        (json!({"crease": [{"at": ["x", 0, 0]}]}), "numbers"),
    ] {
        let refused = s.execute("scan.edit_surface", &params).unwrap_err().to_string();
        assert!(refused.contains(words), "{params}: {refused}");
    }
    run(&mut s, "scan.free_form", json!({"corners": [[10, 52, 0], [30, 52, 0], [30, 58, 0], [10, 58, 0]], "count": 4}));
    let one = s.execute("scan.edit_surface", &json!({"surface": "FreeForm2", "crease": [{"at": [20, 55, 8]}]})).unwrap_err().to_string();
    assert!(one.contains("is one face"), "{one}");
}

/// A point on a Free Form pushed up off the dome is brushed back down: the smoothing brush evens
/// the points under it out and keeps them on the scan; with `snap` off it fairs the surface
/// instead. One undo step. Hostile brushes are refused in words.
#[test]
fn the_smoothing_brush_evens_out_points_and_keeps_them_on_the_scan() {
    let (mut s, _) = session_with_triangles("brush", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 5}));
    let dome = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
    let height = |s: &Session| -> f64 {
        let faces = super::surface::editable(s, Some("FreeForm1"), "test").unwrap().faces;
        let joins = solidcraft_scan::network::joins(&faces).unwrap();
        let dots = solidcraft_scan::network::dots(&faces, &joins);
        let d = dots.iter().min_by(|a, b| a.at.dist(Vec3::new(30.0, 30.0, 8.0)).total_cmp(&b.at.dist(Vec3::new(30.0, 30.0, 8.0)))).unwrap();
        d.at.z - dome(d.at.x, d.at.y)
    };
    run(&mut s, "scan.edit_surface", json!({"dots": [{"at": [30, 30, 8], "along": "3 mm"}]}));
    let pushed = height(&s);
    assert!(pushed > 2.5, "pushed up: {pushed}");
    let brushed = run(&mut s, "scan.edit_surface", json!({"brush": {"points": [[30, 30, 11]], "radius": "25 mm", "passes": 8}}));
    assert!(brushed["moved"].as_u64().unwrap() >= 5 && brushed["rms"].as_f64().unwrap() < 0.2, "{brushed}");
    let down = height(&s);
    assert!(down < 1.0, "brushed back onto the dome: {down}");
    run(&mut s, "edit.undo", json!({}));
    assert!((height(&s) - pushed).abs() < 1e-6, "one undo step");
    let faired =
        run(&mut s, "scan.edit_surface", json!({"brush": {"points": [[30, 30, 11]], "radius": "25 mm", "passes": 8, "snap": false, "strength": 1}}));
    assert!(faired["moved"].as_u64().unwrap() >= 5, "{faired}");
    assert!(height(&s) < 2.0, "faired: {}", height(&s));
    for (brush, words) in [
        (json!({"points": [], "radius": "10 mm"}), "1 to 10000"),
        (json!({"points": [[30, 30, 8]]}), "radius"),
        (json!({"points": [[30, 30, 8]], "radius": "0 mm"}), "above 0"),
        (json!({"points": [[30, 30, 8]], "radius": "10 mm", "strength": 2}), "at most 1"),
        (json!({"points": [[30, 30, 8]], "radius": "10 mm", "passes": 0}), "1 to 100"),
        (json!({"points": [[500, 500, 500]], "radius": "10 mm"}), "no point of the surface"),
        (json!({"points": [["x", 0, 0]], "radius": "10 mm"}), "numbers"),
    ] {
        let refused = s.execute("scan.edit_surface", &json!({"brush": brush})).unwrap_err().to_string();
        assert!(refused.contains(words), "{brush}: {refused}");
    }
    let with_dots = s
        .execute("scan.edit_surface", &json!({"brush": {"points": [[30, 30, 8]], "radius": "10 mm"}, "dots": [{"at": [30, 30, 8], "along": "1 mm"}]}))
        .unwrap_err()
        .to_string();
    assert!(with_dots.contains("give it alone"), "{with_dots}");
}

/// A half of a Free Form modelled on one side of the dome's mirror plane is mirrored into one
/// Free Form joined on the plane; a dot moved with `symmetry` takes its twin to the mirror image;
/// mirrored again it is one already; what is not a half is refused in words.
#[test]
fn mirror_free_form_makes_a_half_whole_and_symmetric_edits_keep_it() {
    let (mut s, _) = session_with_triangles("mirror", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [29.6, 10, 0], [29.6, 50, 0], [10, 50, 0]], "count": 5}));
    run(&mut s, "scan.symmetry", json!({"plane": {"point": [30, 30, 8], "normal": [1, 0, 0]}}));
    let made = run(&mut s, "scan.mirror_free_form", json!({"plane": "Symmetry Plane"}));
    assert_eq!(
        (
            made["faces"].as_u64(),
            made["how"].as_str(),
            made["added"].as_u64(),
            made["onto"].as_u64(),
            made["pairs"].as_u64(),
            made["on_plane"].as_u64()
        ),
        (Some(2), Some("mirrored"), Some(1), Some(5), Some(20), Some(5)),
        "{made}"
    );
    assert!(made["kink"].as_f64().unwrap() < 0.5, "{made}");
    let dots_of = |s: &Session| {
        let faces = super::surface::editable(s, Some("FreeForm1"), "test").unwrap().faces;
        let joins = solidcraft_scan::network::joins(&faces).unwrap();
        solidcraft_scan::network::dots(&faces, &joins)
    };
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 2);
    let moved = run(&mut s, "scan.edit_surface", json!({"symmetry": "Symmetry Plane", "dots": [{"at": [20, 30, 8], "along": "2 mm"}]}));
    assert_eq!((moved["faces"].as_u64(), moved["moved"].as_u64()), (Some(2), Some(2)), "both sides: {moved}");
    let dots = dots_of(&s);
    let near = |x: f64| dots.iter().min_by(|a, b| a.at.dist(Vec3::new(x, 30.0, 8.0)).total_cmp(&b.at.dist(Vec3::new(x, 30.0, 8.0)))).unwrap().at;
    let (a, b) = (near(20.0), near(40.0));
    assert!(b.dist(Vec3::new(60.0 - a.x, a.y, a.z)) < 1e-6 && a.z > 9.0, "the twin is the mirror image: {a:?} {b:?}");
    let again = run(&mut s, "scan.mirror_free_form", json!({"plane": {"point": [30, 0, 0], "normal": [1, 0, 0]}}));
    assert_eq!((again["how"].as_str(), again["faces"].as_u64()), (Some("already"), Some(2)), "{again}");
    run(&mut s, "edit.undo", json!({}));
    assert!(dots_of(&s).iter().all(|d| d.at.z < 8.5), "the dots edit undone");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 1, "the mirroring undone");
    for (params, words) in [
        (json!({"plane": "XY"}), "mm off"),
        (json!({"plane": "Nowhere"}), "no plane"),
        (json!({"plane": {"point": [30, 0, 0], "normal": [0, 0, 0]}}), "no direction"),
        (json!({"plane": {"normal": [1, 0, 0]}}), "needs `point`"),
        (json!({}), "`plane` is needed"),
        (json!({"plane": "YZ", "gap": "-1 mm"}), "above 0"),
    ] {
        let refused = s.execute("scan.mirror_free_form", &params).unwrap_err().to_string();
        assert!(refused.contains(words), "{params}: {refused}");
    }
    let not_mirror = s
        .execute("scan.edit_surface", &json!({"symmetry": "Symmetry Plane", "dots": [{"at": [20, 30, 8], "along": "1 mm"}]}))
        .unwrap_err()
        .to_string();
    assert!(not_mirror.contains("not a mirror image"), "{not_mirror}");
    let alone = s.execute("scan.edit_surface", &json!({"symmetry": "Symmetry Plane"})).unwrap_err().to_string();
    assert!(alone.contains("goes with an edit"), "{alone}");
    run(&mut s, "scan.free_form", json!({"corners": [[22, 10, 0], [45, 10, 0], [45, 50, 0], [22, 50, 0]], "count": 4}));
    let straddles = s.execute("scan.mirror_free_form", &json!({"surface": "FreeForm2", "plane": "Symmetry Plane"})).unwrap_err().to_string();
    assert!(straddles.contains("both sides"), "{straddles}");
}

/// The design keeps a Free Form's symmetry plane: once mirrored, a dot moved, a face added and a
/// face deleted are each made on both sides without `symmetry`; Keep One Half takes the other
/// side away and drops the plane. Undo puts the plane back with the faces.
#[test]
fn the_design_keeps_a_free_forms_symmetry_plane_and_edits_keep_it() {
    let (mut s, _) = session_with_triangles("kept-symmetry", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[14, 14, 0], [29.6, 14, 0], [29.6, 46, 0], [14, 46, 0]], "count": 5}));
    run(&mut s, "scan.mirror_free_form", json!({"plane": {"point": [30, 0, 0], "normal": [1, 0, 0]}}));
    let kept = s.doc.free_forms.first().cloned().expect("the plane kept");
    let plane = kept.symmetry.expect("a symmetry plane");
    assert!((plane.point[0] - 30.0).abs() < 1e-9 && (plane.normal[0].abs() - 1.0).abs() < 1e-9, "{plane:?}");
    // A dot moved with no `symmetry`: its twin goes too.
    let moved = run(&mut s, "scan.edit_surface", json!({"dots": [{"at": [20, 30, 8], "along": "2 mm"}]}));
    assert_eq!(moved["moved"].as_u64(), Some(2), "both sides: {moved}");
    // A face added on the half's far side brings its twin.
    let added = run(&mut s, "scan.add_face", json!({"edge": [[14, 14, 0], [14, 46, 0]], "far": [[4, 14, 0], [4, 46, 0]], "rows": 4}));
    assert_eq!((added["faces"].as_u64(), added["twin"].as_u64()), (Some(4), Some(3)), "{added}");
    // `none` adds one face alone and lets the plane go.
    let lone =
        run(&mut s, "scan.add_face", json!({"edge": [[14, 46, 0], [29.6, 46, 0]], "far": [[14, 56, 0], [30, 56, 0]], "rows": 4, "symmetry": "none"}));
    assert_eq!((lone["faces"].as_u64(), lone.get("twin")), (Some(5), None), "{lone}");
    assert!(s.doc.free_forms.iter().all(|k| k.symmetry.is_none()), "the plane let go");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc.free_forms.first().is_some_and(|k| k.symmetry.is_some()), "undo brings the plane back");
    // Delete Face on the added face takes its twin too.
    let deleted = run(&mut s, "scan.edit_surface", json!({"delete_face": {"at": [9, 30, 6]}}));
    assert_eq!(deleted["faces"].as_u64(), Some(2), "{deleted}");
    // Keep One Half: the +x side goes, and the plane with it.
    let half = run(&mut s, "scan.edit_surface", json!({"keep_half": {"side": [10, 30, 0]}}));
    assert_eq!(half["faces"].as_u64(), None, "one face left: {half}");
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    assert_eq!(faces.len(), 1);
    assert!(solidcraft_scan::surface_edit::points(&faces[0]).iter().all(|p| p.x <= 30.0 + 1e-6));
    assert!(s.doc.free_forms.is_empty(), "nothing kept any more");
    for (params, words) in [
        (json!({"keep_half": {"side": [10, 30, 0]}}), "is one face"),
        (json!({"delete_face": {"at": [10, 30, 6]}}), "is one face"),
        (json!({"smooth": true, "dots": [{"at": [20, 30, 8], "along": "1 mm"}]}), "give it alone"),
        (json!({"smooth": {"strength": 3}}), "at most 1"),
        (json!({"smooth": {"passes": 0}}), "1 to 20"),
        (json!({"smooth": "hard"}), "true or"),
        (json!({"weld": {"at": [0, 0, 0]}}), "weld.onto"),
        (json!({"kept_edges": {"creases": []}}), "goes with `faces`"),
    ] {
        let refused = s.execute("scan.edit_surface", &params).unwrap_err().to_string();
        assert!(refused.contains(words), "{params}: {refused}");
    }
}

/// A crease on a shared edge where the faces still meet smoothly is kept by the design (by its
/// ends): edits after it keep it a crease, and OK of Edit Surface sending every face with its
/// `kept_edges` keeps them. `smooth` evens out a point pushed up, in one undo step.
#[test]
fn a_crease_set_by_hand_is_kept_by_the_design_and_smooth_evens_the_surface() {
    let (mut s, _) = session_with_triangles("kept-crease", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 50, 0], [10, 50, 0]], "count": 5}));
    run(&mut s, "scan.add_face", json!({"edge": [[30, 10, 0], [30, 50, 0]], "far": [[50, 10, 0], [50, 50, 0]], "rows": 5}));
    // On a smooth dome a crease refits the faces to the scan, which goes on smoothly there.
    let creased = run(&mut s, "scan.edit_surface", json!({"crease": [{"at": [30, 30, 8], "chain": true}]}));
    assert_eq!(creased["creases"].as_u64(), Some(1), "{creased}");
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    assert_eq!(solidcraft_scan::network::creased(&solidcraft_scan::network::joins(&faces).unwrap()), vec![false], "the angle alone reads it smooth");
    assert_eq!(s.doc.free_forms.first().map(|k| k.creases.len()), Some(1), "so the design keeps it");
    let moved = run(&mut s, "scan.edit_surface", json!({"dots": [{"at": [20, 30, 8], "along": "3 mm"}]}));
    assert_eq!(moved["creases"].as_u64(), Some(1), "still a crease after an edit elsewhere: {moved}");
    let smoothed = run(&mut s, "scan.edit_surface", json!({"smooth": {"passes": 4}}));
    assert_eq!(smoothed["creases"].as_u64(), Some(1), "{smoothed}");
    let peak = |s: &Session| {
        let faces = super::surface::editable(s, Some("FreeForm1"), "test").unwrap().faces;
        let joins = solidcraft_scan::network::joins(&faces).unwrap();
        let dots = solidcraft_scan::network::dots(&faces, &joins);
        let d = dots.iter().min_by(|a, b| a.at.dist(Vec3::new(20.0, 30.0, 8.0)).total_cmp(&b.at.dist(Vec3::new(20.0, 30.0, 8.0)))).unwrap();
        d.at.z - (8.0 - 0.004 * ((d.at.x - 30.0).powi(2) + (d.at.y - 30.0).powi(2)))
    };
    assert!(peak(&s) < 2.0, "smoothed down toward the dome: {}", peak(&s));
    run(&mut s, "edit.undo", json!({}));
    assert!(peak(&s) > 2.5, "one undo step: {}", peak(&s));
    // Every face sent back with no edge kept: the crease goes.
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    let net_of = |f: &solidcraft_geom::bspline::NurbsSurface| {
        let points: Vec<[f64; 3]> = solidcraft_scan::surface_edit::points(f).iter().map(|p| [p.x, p.y, p.z]).collect();
        json!({"count": f.count(), "knots": [f.knots()[0].clone(), f.knots()[1].clone()], "degree": f.degree(), "points": points})
    };
    let all: Vec<Value> = faces.iter().map(net_of).collect();
    let read = solidcraft_scan::network::creased(&solidcraft_scan::network::joins(&faces).unwrap()).iter().filter(|c| **c).count();
    let sent = run(&mut s, "scan.edit_surface", json!({"faces": all, "kept_edges": {}}));
    assert_eq!(sent["creases"].as_u64(), Some(read as u64), "as the faces' angle reads them: {sent}");
    assert!(s.doc.free_forms.is_empty(), "nothing held by hand any more");
}

/// A corner on a Free Form's edge dropped on another's: the corner is one point with it, and the
/// two open sides whose ends then meet are sewn into a shared edge (the four faces round the
/// middle a grid's corner). A corner too far from `at` is refused.
#[test]
fn weld_drops_a_corner_on_another_and_sews_the_sides_that_meet() {
    let (mut s, _) = session_with_triangles("weld", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 30, 0], [10, 30, 0]], "count": 5}));
    run(&mut s, "scan.add_face", json!({"edge": [[30, 10, 0], [30, 30, 0]], "far": [[50, 10, 0], [50, 30, 0]], "rows": 5}));
    run(&mut s, "scan.add_face", json!({"edge": [[10, 30, 0], [30, 30, 0]], "far": [[10, 50, 0], [30, 50, 0]], "rows": 5}));
    // On the second face's top edge, its far corner short of the third face's.
    let fourth = run(&mut s, "scan.add_face", json!({"edge": [[30, 30, 0], [50, 30, 0]], "far": [[34, 53, 0], [50, 50, 0]], "rows": 5}));
    assert_eq!(fourth["faces"].as_u64(), Some(4), "{fourth}");
    let seams = |s: &Session| {
        let faces = super::surface::editable(s, Some("FreeForm1"), "test").unwrap().faces;
        solidcraft_scan::network::joins(&faces).unwrap().seams.len()
    };
    assert_eq!(seams(&s), 3, "the fourth face's left side open beside the third's right");
    let refused =
        s.execute("scan.edit_surface", &json!({"weld": {"at": [90, 90, 0], "onto": [30, 50, 6]}, "reach": "6 mm"})).unwrap_err().to_string();
    assert!(refused.contains("weld.at"), "{refused}");
    let welded = run(&mut s, "scan.edit_surface", json!({"weld": {"at": [34, 53, 6], "onto": [30, 50, 6]}, "reach": "6 mm"}));
    assert_eq!((welded["sewn"].as_u64(), welded["faces"].as_u64()), (Some(1), Some(4)), "{welded}");
    assert_eq!(seams(&s), 4, "sewn");
    let faces = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces;
    let joins = solidcraft_scan::network::joins(&faces).unwrap();
    assert!(joins.corners.iter().any(|c| c.faces == 4 && !c.border), "the middle is a grid's corner now");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(seams(&s), 3, "one undo step");
}

/// The area of the body `name`, mm².
fn area_of(s: &Session, name: &str) -> f64 {
    solidcraft_kernel::measure(&s.world_state().unwrap().body(name).unwrap().body).unwrap().area
}

/// Fit with `make: face` keeps a surface trimmed to the region: a half tube's face turns through
/// half a turn and is as large as the half tube, a whole tube's goes all the way round, and a flat
/// plate's is its outline with the square hole in it left open. `both` keeps the axis too.
#[test]
fn a_fitted_shape_is_kept_as_a_face_trimmed_to_its_region() {
    let (mut s, _) = session_with_triangles("fit-face-half", &tube_triangles(std::f64::consts::PI));
    let half = run(&mut s, "scan.fit_cylinder", json!({"point": [0.0, 10.0, 20.0], "make": "face"}));
    assert!(half.get("feature").is_none(), "no construction axis with face alone: {half}");
    let face = &half["face"];
    assert!((face["sweep"].as_f64().unwrap() - 180.0).abs() < 8.0, "{half}");
    let surface = face["surface"].as_str().unwrap().to_owned();
    assert!(s.world_state().unwrap().body(&surface).unwrap().body.is_surface(), "{half}");
    let want = std::f64::consts::PI * 10.0 * 40.0;
    assert!((area_of(&s, &surface) - want).abs() < 0.03 * want, "{} against {want}", area_of(&s, &surface));

    let (mut s, _) = session_with_triangles("fit-face-whole", &tube_triangles(std::f64::consts::TAU));
    let whole = run(&mut s, "scan.fit_cylinder", json!({"point": [10.0, 0.0, 20.0], "make": "both"}));
    assert!(whole.get("feature").is_some(), "the axis is kept too: {whole}");
    assert_eq!(whole["face"]["sweep"].as_f64(), Some(360.0), "{whole}");
    let want = std::f64::consts::TAU * 10.0 * 40.0;
    let got = area_of(&s, whole["face"]["surface"].as_str().unwrap());
    assert!((got - want).abs() < 0.03 * want, "{got} against {want}");

    // A 40 mm plate of 5 mm cells with the middle 10 mm square left out.
    let mut plate = Vec::new();
    for i in 0..8 {
        for j in 0..8 {
            if (3..5).contains(&i) && (3..5).contains(&j) {
                continue;
            }
            let p = |a: i32, b: i32| Vec3::new(5.0 * f64::from(a), 5.0 * f64::from(b), 0.0);
            plate.push([p(i, j), p(i + 1, j), p(i + 1, j + 1)]);
            plate.push([p(i, j), p(i + 1, j + 1), p(i, j + 1)]);
        }
    }
    let (mut s, _) = session_with_triangles("fit-face-plate", &plate);
    let flat = run(&mut s, "scan.fit_plane", json!({"point": [3.0, 3.0, 0.0], "make": "face"}));
    let got = area_of(&s, flat["face"]["surface"].as_str().unwrap());
    assert!((got - 1500.0).abs() < 1.0, "the outline less the hole: {got} {flat}");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.world_state().unwrap().bodies.iter().all(|b| b.body.is_mesh()), "one undo step");
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
    assert_eq!((made["folds"].as_u64(), made["crossings"].as_u64()), (Some(0), Some(0)), "{made}");
    assert!(made.get("warning").is_none(), "{made}");
    // A quarter of the tube does not close round on itself: Merge says how far apart its sides are.
    let open = s.execute("scan.merge_surfaces", &json!({"surfaces": ["FreeForm1"]})).unwrap_err().to_string();
    assert!(open.contains("does not close round on itself") && open.contains("mm apart"), "{open}");
    let about_z = run(&mut s, "scan.wrap_surface", json!({"point": point, "reach": "60 mm", "axis": "z", "details": "low"}));
    assert_eq!(about_z["surface"].as_str(), Some("FreeForm2"), "{about_z}");
    assert!((about_z["radius"].as_f64().unwrap() - 10.0).abs() < 0.05 && about_z["controls"][0] == 4, "{about_z}");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body("FreeForm2").is_none() && s.model.state().body("FreeForm1").is_some(), "one undo step");
    let refused = s.execute("scan.wrap_surface", &json!({"point": point, "details": "huge"})).unwrap_err().to_string();
    assert!(refused.contains("low, medium or fine"), "{refused}");
}

/// Wrap Surface on a part chosen any way: grown from the point (as far as the tube is round), the
/// triangles given, or what is selected with no point; each wraps the tube at its radius. A reach
/// with a part chosen by hand is refused.
#[test]
fn wrap_surface_takes_the_part_chosen_any_way() {
    let (mut s, body) = session_with_triangles("wrap-any", &tube_triangles(std::f64::consts::PI));
    let point = json!([10.0 * 0.7f64.cos(), 10.0 * 0.7f64.sin(), 20.0]);
    let radius = |r: &Value| (r["radius"].as_f64().unwrap() - 10.0).abs() < 0.1;
    let grown = run(&mut s, "scan.wrap_surface", json!({"point": point, "grow": true, "details": "low"}));
    assert!(radius(&grown), "{grown}");
    let half: Vec<usize> = (0..960).collect();
    let given = run(&mut s, "scan.wrap_surface", json!({"body": body, "triangles": half, "details": "low"}));
    assert!(radius(&given), "{given}");
    run(&mut s, "scan.select", json!({"body": body, "brush": {"points": [point], "radius": 15}}));
    let selected = run(&mut s, "scan.wrap_surface", json!({"details": "low"}));
    assert!(radius(&selected), "{selected}");
    let both = s.execute("scan.wrap_surface", &json!({"point": point, "grow": true, "reach": "20 mm"})).unwrap_err().to_string();
    assert!(both.contains("not with"), "{both}");
}

/// A wrap that folds says so and says what to do: Stiffness, then more of it; one that does not
/// says nothing.
#[test]
fn a_folded_wrap_is_told_to_raise_stiffness() {
    assert!(super::free_form::wrap_warning([0, 0, 0], 0.0).is_none());
    let soft = super::free_form::wrap_warning([20, 0, 0], 0.0).unwrap();
    assert!(soft.contains("20 folds") && soft.contains("raise Stiffness a little") && soft.contains("cannot sew"), "{soft}");
    let stiff = super::free_form::wrap_warning([3, 0, 1], 0.2).unwrap();
    assert!(stiff.contains("raise Stiffness further") && stiff.contains("1 crossing"), "{stiff}");
    let crossing = super::free_form::wrap_warning([0, 0, 13], 0.0).unwrap();
    assert!(crossing.starts_with("the surface passes through itself (13 crossings)"), "{crossing}");
}

/// Two Free Forms clicked side by side on a dome, a little apart and of other sizes, are merged:
/// their sides made one, and the two one Free Form of two faces in the first's feature, edited
/// with Edit Surface from then on; a third merged into it is taken whole, with its faces; one
/// undo step puts each merge back.
#[test]
fn merge_surfaces_makes_free_forms_side_by_side_one_free_form() {
    let (mut s, _) = session_with_triangles("merge", &dome_triangles(40));
    run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [30, 10, 0], [30, 50, 0], [10, 50, 0]], "count": 4}));
    run(&mut s, "scan.free_form", json!({"corners": [[30.5, 10, 0], [50, 10, 0], [50, 50, 0], [30.5, 50, 0]], "count": 6}));
    let refused = s.execute("scan.merge_surfaces", &json!({"surfaces": ["FreeForm1", "FreeForm2"], "gap": "0.1 mm"})).unwrap_err().to_string();
    assert!(refused.contains("within the gap"), "{refused}");
    let merged = run(&mut s, "scan.merge_surfaces", json!({}));
    assert_eq!((merged["seams"].as_u64(), merged["body"].as_str(), merged["faces"].as_u64()), (Some(1), Some("FreeForm1"), Some(2)), "{merged}");
    assert!(merged["feature"]["feature"].is_u64(), "the Remove feature: {merged}");
    {
        let state = s.model.state();
        let body = state.body("FreeForm1").expect("the merged body");
        assert_eq!(solidcraft_kernel::brep_record(&body.body).unwrap().faces.len(), 2, "two faces in one body");
        assert!(state.body("FreeForm2").is_none(), "taken into the first");
    }
    let kept = super::surface::editable(&s, Some("FreeForm1"), "test").unwrap();
    assert!(kept.is_network() && kept.faces.len() == 2, "one Free Form of two faces, editable: {}", kept.faces.len());
    // Edited as a Free Form: a point on the seam moved keeps both faces through it.
    let moved = run(&mut s, "scan.edit_surface", json!({"surface": "FreeForm1", "dots": [{"at": [30.25, 30, 8], "along": "1 mm"}]}));
    assert_eq!(moved["faces"].as_u64(), Some(2), "{moved}");
    // A third Free Form merged into it: the Free Form is taken whole, three faces now.
    run(&mut s, "scan.free_form", json!({"corners": [[10, 50.5, 0], [30, 50.5, 0], [30, 58, 0], [10, 58, 0]], "count": 4}));
    let again = run(&mut s, "scan.merge_surfaces", json!({}));
    assert_eq!((again["seams"].as_u64(), again["body"].as_str(), again["faces"].as_u64()), (Some(1), Some("FreeForm1"), Some(3)), "{again}");
    // The third took the removed body's name, as a body made after a Delete does.
    let third = again["surfaces"][1].as_str().unwrap().to_owned();
    assert!(s.model.state().body(&third).is_none(), "taken into the first: {again}");
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 3, "three faces, still editable");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(&third).is_some(), "one undo step");
    assert_eq!(super::surface::editable(&s, Some("FreeForm1"), "test").unwrap().faces.len(), 2);
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body("FreeForm2").is_some(), "the first merge undone too");
    // Not made one: each surface is written back as it now meets the other, still its own body.
    let apart = run(&mut s, "scan.merge_surfaces", json!({"stitch": false}));
    assert!(apart.get("body").is_none() && apart.get("faces").is_none() && apart["seams"] == 1, "{apart}");
    assert!(s.model.state().body("FreeForm2").is_some(), "still a body of its own");
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

/// Four flat Free Forms round a square tube merged into one open tube: Close Surface lists the two
/// openings without changing anything, then closes both with patches into a solid of the tube's
/// volume in one undo step; run again it has nothing left to close.
#[test]
fn close_surface_lists_and_closes_the_openings_of_a_tube() {
    let (mut s, _) = session_with_triangles("close", &dome_triangles(10));
    for corners in [
        json!([[0, 0, 0], [20, 0, 0], [20, 0, 40], [0, 0, 40]]),
        json!([[20, 0, 0], [20, 20, 0], [20, 20, 40], [20, 0, 40]]),
        json!([[20, 20, 0], [0, 20, 0], [0, 20, 40], [20, 20, 40]]),
        json!([[0, 20, 0], [0, 0, 0], [0, 0, 40], [0, 20, 40]]),
    ] {
        run(&mut s, "scan.free_form", json!({"corners": corners, "fit": "off"}));
    }
    run(&mut s, "scan.merge_surfaces", json!({}));
    let faces = |s: &Session| s.model.state().body("FreeForm1").map(|b| solidcraft_kernel::brep_record(&b.body).unwrap().faces.len());
    let before = faces(&s);
    let looked = run(&mut s, "scan.close_surface", json!({"close": false}));
    assert_eq!((looked["caps"].as_u64(), looked["open"].as_u64(), looked["solid"].as_bool()), (Some(0), Some(2), Some(false)), "{looked}");
    assert!(looked.get("volume").is_none(), "an open shell has no volume: {looked}");
    let loops = looked["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 2, "{looked}");
    let mut heights: Vec<f64> = loops.iter().map(|l| l["at"][2].as_f64().unwrap()).collect();
    heights.sort_by(f64::total_cmp);
    assert!(heights[0].abs() < 1e-6 && (heights[1] - 40.0).abs() < 1e-6, "{heights:?}");
    assert!(loops.iter().all(|l| (l["size"].as_f64().unwrap() - 20.0 * 2f64.sqrt()).abs() < 0.5), "{looked}");
    assert_eq!(faces(&s), before, "only looking changes nothing");
    let closed = run(&mut s, "scan.close_surface", json!({}));
    assert_eq!((closed["caps"].as_u64(), closed["open"].as_u64(), closed["solid"].as_bool()), (Some(2), Some(0), Some(true)), "{closed}");
    let volume = closed["volume"].as_f64().unwrap();
    assert!((volume - 16000.0).abs() < 160.0, "{volume}");
    assert_eq!(faces(&s), Some(6), "four sides and two caps");
    let again = run(&mut s, "scan.close_surface", json!({}));
    assert_eq!((again["caps"].as_u64(), again["solid"].as_bool()), (Some(0), Some(true)), "{again}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(faces(&s), before, "the no-op left no step, so one undo puts the openings back");
}

/// Close Surface refuses what it cannot work on, in words, and changes nothing.
#[test]
fn close_surface_refuses_a_scan_and_a_bad_continuity() {
    let (mut s, scan) = session_with_triangles("close-refused", &dome_triangles(10));
    let e = s.execute("scan.close_surface", &json!({"surface": scan})).unwrap_err().to_string();
    assert!(e.contains("is a scan"), "{e}");
    run(&mut s, "scan.free_form", json!({"corners": [[0, 0, 0], [20, 0, 0], [20, 20, 0], [0, 20, 0]], "fit": "off"}));
    let e = s.execute("scan.close_surface", &json!({"continuity": "g2"})).unwrap_err().to_string();
    assert!(e.contains("g0 or g1"), "{e}");
}

/// A wall 400 × 300 with an outlet box standing 25 off it, as a grid of `n` squares a side.
fn wall_triangles(n: u32) -> Vec<[Vec3; 3]> {
    let z = |x: f64, y: f64| if (100.0..=160.0).contains(&x) && (100.0..=180.0).contains(&y) { 25.0 } else { 0.0 };
    let at = |i: u32, j: u32| {
        let (x, y) = (400.0 * f64::from(i) / f64::from(n), 300.0 * f64::from(j) / f64::from(n));
        Vec3::new(x, y, z(x, y))
    };
    let mut out = Vec::new();
    for j in 0..n {
        for i in 0..n {
            out.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            out.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    out
}

/// Flat Template over a wall with an outlet: the wall's plane is fitted without the outlet, the
/// outline is laid exactly on it, the outlet is cut out, the scan's edge is construction, and one
/// undo takes the sketch away.
#[test]
fn flat_template_cuts_the_protrusion_out_and_shows_the_scan_edge() {
    let (mut s, body) = session_with_triangles("flat-template", &wall_triangles(200));
    let outline = json!([[20, 20, 4], [280, 20, -3], [280, 280, 2], [20, 280, 0]]);
    let r = run(&mut s, "scan.flat_template", json!({"body": body, "outline": outline}));
    assert_eq!(r["fitted"], true, "{r}");
    assert_eq!(r["cutouts"].as_array().unwrap().len(), 1, "{r}");
    let c = &r["cutouts"][0];
    assert!((c["area"].as_f64().unwrap() - 60.0 * 80.0).abs() < 0.15 * 4800.0, "{r}");
    assert!((c["height"].as_f64().unwrap() - 25.0).abs() < 1e-6 && c["side"] == "out", "{r}");
    let normal = r["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    assert!(normal[2].abs() > 0.999, "the wall's plane, not tilted by the outlet: {normal:?}");
    assert_eq!(r["scan_edge_loops"], 1, "{r}");
    assert!((r["area"].as_f64().unwrap() - 260.0 * 260.0).abs() < 1e-3, "{r}");
    let sketch = r["sketch"].as_u64().unwrap();
    {
        let state = s.model.state();
        let solved = state.sketch(sketch).unwrap();
        let construction = solved.sketch.curves.iter().filter(|c| c.construction).count();
        let drawn = solved.sketch.curves.iter().filter(|c| !c.construction).count();
        assert!(construction >= 4, "the scan's edge is construction: {construction}");
        assert!(drawn >= 8, "the outline's four sides and the outlet's loop: {drawn}");
    }
    // The outline's corner is at the origin: nothing drawn below or left of it.
    assert_eq!(r["shift"].as_array().unwrap().len(), 2);

    // Only the side asked for: the outlet stands off the +z side, so `in` cuts nothing.
    let none = run(&mut s, "scan.flat_template", json!({"body": body, "outline": outline, "side": "in", "name": "In only"}));
    assert_eq!(none["cutouts"].as_array().unwrap().len(), 0, "{none}");
    // A chosen plane is used as given.
    let given = run(
        &mut s,
        "scan.flat_template",
        json!({"body": body, "outline": outline, "plane": {"point": [0, 0, 0], "normal": [0, 0, 1]}, "tolerance": "3 mm", "name": "Given"}),
    );
    assert_eq!(given["fitted"], false);
    assert_eq!(given["cutouts"].as_array().unwrap().len(), 1, "{given}");

    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().sketch(sketch).is_some(), "one undo took the last sketch only");

    for bad in [
        json!({"outline": [[0, 0, 0], [1, 0, 0]]}),
        json!({"outline": outline, "side": "up"}),
        json!({"outline": outline, "tolerance": -1}),
        json!({"outline": outline, "least_area": "x"}),
        json!({"outline": [[0, 0, 0], [10, 0, 0], [20, 0, 0]]}),
        json!({"outline": outline, "plane": {"point": [0, 0, 0], "normal": [0, 0, 0]}}),
    ] {
        assert!(s.execute("scan.flat_template", &bad).is_err(), "{bad}");
    }
}

/// Flat Template with `make_face`: a flat Free Form face over the template with the outlet a hole
/// in it, its area the outline's less the cut-out's; the sketch lies on the wall, and the
/// outline's sides are told.
#[test]
fn flat_template_can_make_a_free_form_face_with_the_cutout_as_a_hole() {
    let (mut s, body) = session_with_triangles("flat-template-face", &wall_triangles(200));
    let outline = json!([[20, 20, 0], [280, 20, 0], [280, 280, 0], [20, 280, 0]]);
    let r = run(&mut s, "scan.flat_template", json!({"body": body, "outline": outline, "make_face": true}));
    let face = r["face"].as_str().unwrap_or_else(|| panic!("a face: {r}")).to_owned();
    let want = 260.0 * 260.0 - r["cut_area"].as_f64().unwrap();
    let area = solidcraft_kernel::measure(&s.world_state().unwrap().body(&face).unwrap().body).unwrap().area;
    assert!((area - want).abs() < 0.02 * want, "outline less the outlet: {area} vs {want}");
    let sides: Vec<f64> = r["edge_lengths"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(sides.len() == 4 && sides.iter().all(|l| (l - 260.0).abs() < 1e-6), "{sides:?}");
    // The sketch is on the wall: a plane through the template's corner, not XY at the origin.
    let sketch = r["sketch"].as_u64().unwrap();
    assert!(s.model.state().sketch(sketch).unwrap().plane.origin.dist(Vec3::new(20.0, 20.0, 0.0)) < 1e-6);
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().body(&face).is_none(), "one undo takes the face and the sketch away");
}

/// Flat Template with `outline: "scan"`: nothing picked. A wall 600 × 400 with a hump standing 120
/// off the middle of its bottom edge (a wheel well) and the outlet box inside: the outline is the
/// whole wall with the hump notched out of it, the box is the one cut-out, and the face made has
/// the outline's area less the box.
#[test]
fn flat_template_finds_the_whole_outline_from_the_scan() {
    let z = |x: f64, y: f64| {
        if ((x - 300.0) / 100.0).powi(2) + (y / 80.0).powi(2) < 1.0 {
            120.0
        } else if (400.0..=460.0).contains(&x) && (200.0..=280.0).contains(&y) {
            25.0
        } else {
            0.0
        }
    };
    let n = 150;
    let at = |i: u32, j: u32| {
        let (x, y) = (600.0 * f64::from(i) / f64::from(n), 400.0 * f64::from(j) / f64::from(n));
        Vec3::new(x, y, z(x, y))
    };
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            tris.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            tris.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    let (mut s, body) = session_with_triangles("flat-template-scan", &tris);
    let r = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "make_face": true, "smooth": 0, "round": 0}));
    assert_eq!(r["fitted"], true, "{r}");
    assert_eq!(r["step"], json!(2.0), "{r}");
    assert_eq!(r["scan_edge_loops"], 0, "the scan's ragged edge is not drawn unless asked: {r}");
    let notch = std::f64::consts::PI * 100.0 * 80.0 / 2.0;
    let area = r["area"].as_f64().unwrap();
    assert!((area - (600.0 * 400.0 - notch)).abs() < 0.02 * 240_000.0, "the wall less the wheel well: {area}");
    let cuts = r["cutouts"].as_array().unwrap();
    assert_eq!(cuts.len(), 1, "{r}");
    assert!((cuts[0]["area"].as_f64().unwrap() - 60.0 * 80.0).abs() < 0.2 * 4800.0 && cuts[0]["side"] == "out", "{r}");
    assert_eq!(r["guessed"], 0, "the outline lies on the scan all round: {r}");
    let face = r["face"].as_str().unwrap_or_else(|| panic!("a face: {r}")).to_owned();
    let made = solidcraft_kernel::measure(&s.world_state().unwrap().body(&face).unwrap().body).unwrap().area;
    let want = area - r["cut_area"].as_f64().unwrap();
    assert!((made - want).abs() < 0.02 * want, "outline less the box: {made} vs {want}");

    // Smoothed (the default, 100 mm): the box, 60 wide, goes behind the template; the wheel well,
    // 200 wide, is still notched.
    let clean = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "name": "Clean"}));
    assert_eq!(clean["cutouts"].as_array().unwrap().len(), 0, "{clean}");
    let clean_area = clean["area"].as_f64().unwrap();
    assert!((clean_area - (600.0 * 400.0 - notch)).abs() < 0.03 * 240_000.0, "still notched: {clean_area}");

    for bad in [
        json!({"outline": "scan", "smooth": -1}),
        json!({"outline": "scan", "round": -1}),
        json!({"outline": "scan", "finger": -1}),
        json!({"outline": "scan", "cover": [[0, 0]]}),
        json!({"outline": "scan", "cover": "here"}),
        json!({"outline": "scan", "gap": -1}),
        json!({"outline": "scan", "step": 0}),
        json!({"outline": "outline"}),
        json!({"outline": "scan", "side": "up"}),
    ] {
        assert!(s.execute("scan.flat_template", &bad).is_err(), "{bad}");
    }
}

/// Flat Template's face, both ways: the Free Form trimmed to the template (the usual way) and the
/// planar patch of the template's region (made when the kernel cannot trim the Free Form) are the
/// same face, on a wall with 72 small cut-outs (8 mm bolts) and a notch.
#[test]
fn flat_template_face_trimmed_and_patched_agree() {
    let bolt = |x: f64, y: f64| {
        let (i, j) = ((x - 47.0) / 61.0, (y - 33.0) / 43.0);
        let (fi, fj) = (i - i.floor(), j - j.floor());
        i >= 0.0 && j >= 0.0 && i < 9.0 && j < 8.0 && fi * 61.0 < 8.0 && fj * 43.0 < 8.0
    };
    let notch = |x: f64, y: f64| (250.0..350.0).contains(&x) && y < 60.0;
    let n = 300;
    let at = |i: u32, j: u32| {
        let (x, y) = (600.0 * f64::from(i) / f64::from(n), 400.0 * f64::from(j) / f64::from(n));
        Vec3::new(x, y, if bolt(x, y) || notch(x, y) { 20.0 } else { 0.0 })
    };
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            tris.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            tris.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    let (mut s, body) = session_with_triangles("flat-template-bolts", &tris);
    let r = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "smooth": 0, "round": 0, "make_face": true}));
    assert!(r["cutouts"].as_array().unwrap().len() >= 60, "the bolts are cut out: {r}");
    assert_eq!(r["face_kind"], "free_form", "{r}");
    let area = |s: &Session, name: &str| solidcraft_kernel::measure(&s.world_state().unwrap().body(name).unwrap().body).unwrap().area;
    let trimmed = area(&s, r["face"].as_str().unwrap());
    // The patch of the same sketch's region.
    let sketch = r["sketch"].as_u64().unwrap();
    let solved = s.model.state().sketch(sketch).unwrap().sketch.clone();
    let ids: Vec<String> = solved.curves.iter().map(|c| c.id.clone()).collect();
    let outline = solved.curves.iter().position(|c| matches!(c.kind, solidcraft_sketch::CurveKind::Circle { .. })).unwrap_or(ids.len());
    let patched = super::template::planar_face(&mut s, &body, &ids, outline, sketch, "test").unwrap();
    let flat = area(&s, &patched);
    assert!((flat - trimmed).abs() < 1e-3 * trimmed, "patched {flat} vs trimmed {trimmed}");
    assert!(flat < r["area"].as_f64().unwrap() - 0.5 * r["cut_area"].as_f64().unwrap(), "the bolts are holes in it");
}

/// Make Flat lays a bumpy Free Form exactly on its fitted plane, one undo step.
#[test]
fn make_flat_puts_a_free_form_exactly_flat() {
    use solidcraft_scan::surface_edit::points;
    let mut s = Session::default();
    run(&mut s, "scan.free_form", json!({"corners": [[0, 0, 0], [100, 0, 3], [100, 100, 0], [0, 100, 2]], "fit": "off", "count": 6}));
    let name = s.model.state().bodies.iter().find(|b| !b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    let r = run(&mut s, "scan.make_flat", json!({}));
    assert_eq!(r["surface"], json!(name), "{r}");
    assert!(r["max"].as_f64().unwrap() > 0.5 && r["max"].as_f64().unwrap() < 3.0, "it was a little off the plane: {r}");
    let target = super::surface::editable(&s, Some(&name), "test").unwrap();
    let net = points(&target.surface);
    let normal: Vec<f64> = r["normal"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let point = Vec3::new(r["point"][0].as_f64().unwrap(), r["point"][1].as_f64().unwrap(), r["point"][2].as_f64().unwrap());
    let n = Vec3::new(normal[0], normal[1], normal[2]);
    assert!(net.iter().all(|q| (*q - point).dot(n).abs() < 1e-9), "every point on the plane");
    run(&mut s, "edit.undo", json!({}));
    let target = super::surface::editable(&s, Some(&name), "test").unwrap();
    assert!(points(&target.surface).iter().any(|q| (*q - point).dot(n).abs() > 0.1), "undone");
    assert!(s.execute("scan.make_flat", &json!({"plane": {"point": [0, 0, 0], "normal": [0, 0, 0]}})).is_err());
}

/// A straight panel from a wall's scan: a wheel well standing out of the bottom edge, two rows of
/// 8 mm holes 34 mm apart across the middle and two columns 30 mm apart up it, on sheets smaller
/// than the wall. The proposal has the wheel well as a notch, a seam between the rows and one
/// between the columns, and the holes; the panel made with the notch dropped is the whole
/// rectangle, with a circle for each hole kept.
/// The wall the straight panel tests use: 600 × 400 at 1 mm, a wheel well standing out of its
/// bottom edge, two rows of 8 mm holes 34 mm apart across it and two columns up it.
fn panel_wall(name: &str) -> (Session, String, Vec<(f64, f64)>) {
    let centres = panel_wall_holes();
    let (s, body) = session_with_triangles(name, &panel_wall_triangles(&centres, false));
    (s, body, centres)
}

/// The panel wall's holes.
fn panel_wall_holes() -> Vec<(f64, f64)> {
    let mut centres = Vec::new();
    for k in 0..6 {
        let x = 60.0 + 90.0 * f64::from(k);
        centres.push((x, 183.0));
        centres.push((x, 217.0));
    }
    for y in [100.0, 260.0, 300.0, 340.0] {
        centres.push((290.0, y));
        centres.push((320.0, y));
    }
    centres
}

/// The panel wall's scan with holes at `centres`, mirrored left to right when asked (the wall
/// opposite, seen from the room).
fn panel_wall_triangles(centres: &[(f64, f64)], mirror: bool) -> Vec<[Vec3; 3]> {
    let hole = |x: f64, y: f64| centres.iter().any(|(cx, cy)| (x - cx).powi(2) + (y - cy).powi(2) < 16.0);
    let z = |x: f64, y: f64| if ((x - 300.0) / 100.0).powi(2) + (y / 80.0).powi(2) < 1.0 { 120.0 } else { 0.0 };
    let (w, h) = (600u32, 400u32);
    let at = |i: u32, j: u32| {
        let (x, y) = (f64::from(i), f64::from(j));
        Vec3::new(if mirror { 600.0 - x } else { x }, y, z(x, y))
    };
    let mut tris = Vec::new();
    for j in 0..h {
        for i in 0..w {
            let (cx, cy) = (f64::from(i) + 0.5, f64::from(j) + 0.5);
            if hole(cx, cy) {
                continue;
            }
            let (p, q, r, t) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            // Mirrored, the triangles turn the other way round: kept facing the room.
            if mirror {
                tris.push([p, r, q]);
                tris.push([p, t, r]);
            } else {
                tris.push([p, q, r]);
                tris.push([p, r, t]);
            }
        }
    }
    tris
}

#[test]
fn flat_template_panel_proposes_seams_between_the_rows_of_holes() {
    let (mut s, body, centres) = panel_wall("panel-wall");
    // No gap and no fixing points first: exact areas and counts.
    let sheet = json!({"sheet": [400, 250], "clearance": 0, "spacing": 0});
    let r = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": sheet}));
    assert_eq!(r["holes_found"].as_u64(), Some(centres.len() as u64), "every hole found: {r}");
    let e = &r["edges"];
    assert_eq!(
        (e["left"].as_f64(), e["right"].as_f64(), e["bottom"].as_f64(), e["top"].as_f64()),
        (Some(0.0), Some(600.0), Some(0.0), Some(400.0)),
        "{r}"
    );
    let cuts = r["cutouts"].as_array().unwrap();
    assert_eq!(cuts.len(), 1, "the wheel well: {r}");
    assert!(cuts[0]["notch"] == true && cuts[0]["edges"] == json!(["bottom"]) && (cuts[0]["depth"].as_f64().unwrap() - 80.0).abs() < 6.0, "{r}");
    let seams = r["seams"].as_array().unwrap();
    let across: Vec<&Value> = seams.iter().filter(|s| s["upright"] == false).collect();
    let up: Vec<&Value> = seams.iter().filter(|s| s["upright"] == true).collect();
    assert_eq!((across.len(), up.len()), (1, 1), "{r}");
    assert!(across[0]["between_rows"] == true && (across[0]["from"][1].as_f64().unwrap() - 200.0).abs() < 1.0, "{r}");
    assert!(up[0]["between_rows"] == true && (up[0]["from"][0].as_f64().unwrap() - 305.0).abs() < 1.0, "{r}");
    assert_eq!(r["over"], json!(0.0), "pieces 305 and 295 wide, 200 high fit 400 × 250: {r}");
    let holes = r["holes"].as_array().unwrap();
    assert_eq!(holes.len(), centres.len(), "all near a seam: {r}");
    for hl in holes {
        let (x, y) = (hl["center"][0].as_f64().unwrap(), hl["center"][1].as_f64().unwrap());
        assert!(centres.iter().any(|(cx, cy)| (x - cx).hypot(y - cy) < 0.3), "a hole's centre is where it is: {hl}");
        assert!((hl["diameter"].as_f64().unwrap() - 8.0).abs() < 1.0, "{hl}");
    }

    // Made, with the wheel well covered and one hole dropped: the whole rectangle, the seams, and
    // a circle for each hole kept.
    let made = run(
        &mut s,
        "scan.flat_template",
        json!({"body": body, "outline": "scan", "panel": {"sheet": [400, 250], "clearance": 0, "spacing": 0, "drop_cutouts": [0], "drop_holes": [0]}}),
    );
    assert!((made["area"].as_f64().unwrap() - 600.0 * 400.0).abs() < 1e-6, "{made}");
    assert_eq!(made["circles"].as_u64(), Some(centres.len() as u64 - 1), "{made}");
    assert_eq!(made["panel"]["seams"], 2, "{made}");
    assert_eq!(made["lines"].as_u64(), Some(4 + 2), "the rectangle and the two seams: {made}");

    // As proposed: the wheel well (200 × 80, small) as a box, a 3 mm gap all round and at the
    // seams, fixing points on the flat wall where an edge has no hole near, and the pieces laid on
    // the sheets they are cut from.
    let r = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": {"sheet": [400, 250]}}));
    assert_eq!(r["cutouts"][0]["shape"], "box", "{r}");
    assert_eq!(r["clearance"], json!(3.0), "{r}");
    let fixings = r["fixings"].as_array().unwrap();
    assert!(!fixings.is_empty() && fixings.iter().all(|f| f["behind"].as_f64().unwrap().abs() < 0.5), "the wall is flat at the panel: {r}");
    let sheets = r["sheets"].as_array().unwrap();
    let placed: usize = sheets.iter().map(|s| s["pieces"].as_array().unwrap().len()).sum();
    assert_eq!(placed, r["pieces"].as_array().unwrap().len(), "every piece on a sheet: {r}");
    assert!(r["fit_moves"].as_array().unwrap().is_empty(), "they all fit already: {r}");
    let boxed = run(
        &mut s,
        "scan.panel_proposals",
        json!({"body": body, "panel": {"sheet": [400, 250], "shapes": [{"cutout": 0, "shape": "straight"}], "hole_sizes": [8.5]}}),
    );
    assert!(boxed["holes"].as_array().unwrap().iter().all(|h| h["size"] == json!(8.5)), "{boxed}");
    let made = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "name": "Gapped", "panel": {"sheet": [400, 250]}}));
    // The box round the well (200 × 80) grown 3 mm: 206 × 80 out of the panel 3 mm in all round.
    assert!((made["area"].as_f64().unwrap() - (594.0 * 394.0 - 206.0 * 80.0)).abs() < 1e-6, "{made}");
    assert_eq!(made["panel"]["seams"], 2, "{made}");
    assert!(made["lines"].as_u64().unwrap() >= 4 + 4, "each seam two cut lines: {made}");
    assert!(made["panel"]["fixings"].as_u64().unwrap() > 0 && made["panel"]["sheets"].as_u64().unwrap() >= 1, "{made}");

    for bad in [
        json!({"body": body, "panel": {"drop_holes": [9999]}}),
        json!({"body": body, "panel": {"sheet": [0, 0]}}),
        json!({"body": body, "panel": {"edges": {"left": 500, "right": 100}}}),
        json!({"body": body, "panel": {"move_seams": [{"seam": 7, "by": 5}]}}),
        json!({"body": body, "panel": {"move_seams": [{"seam": 0, "by": 1e300}]}}),
        json!({"body": body, "panel": {"clearance": -1}}),
        json!({"body": body, "panel": {"spacing": 10}}),
        json!({"body": body, "panel": {"shapes": [{"cutout": 0, "shape": "round"}]}}),
        json!({"body": body, "panel": {"shapes": [{"cutout": 99, "shape": "box"}]}}),
        json!({"body": body, "panel": {"drop_fixings": [99999]}}),
        json!({"body": body, "panel": {"hole_sizes": [-1]}}),
    ] {
        assert!(s.execute("scan.panel_proposals", &bad).is_err(), "{bad}");
    }
    let picked = json!({"body": body, "outline": [[0, 0, 0], [600, 0, 0], [600, 400, 0]], "panel": {}});
    assert!(s.execute("scan.flat_template", &picked).is_err(), "a panel needs the whole outline");
}

/// A wall whose frame holes run 1.5° off its outline (the scan's edge is a door or a trim, not the
/// structure): the panel is squared to the rows and columns of holes, so the seam between the rows
/// stays level; asked to square to the outline, it is not turned.
#[test]
fn panel_is_squared_to_the_rows_and_columns_of_holes() {
    let (c, sn) = (1.5f64.to_radians().cos(), 1.5f64.to_radians().sin());
    let centres: Vec<(f64, f64)> = panel_wall_holes()
        .into_iter()
        // On whole millimetres, so each hole is as round on the 1 mm grid as the plain wall's.
        .map(|(x, y)| ((300.0 + c * (x - 300.0) - sn * (y - 200.0)).round(), (200.0 + sn * (x - 300.0) + c * (y - 200.0)).round()))
        .collect();
    let (mut s, body) = session_with_triangles("panel-turned", &panel_wall_triangles(&centres, false));
    let r = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": {"sheet": [400, 250]}}));
    let turn = r["squared_to_holes"].as_f64().unwrap_or(0.0);
    assert!((turn.abs() - 1.5).abs() < 0.2, "turned to the holes: {r}");
    let seams = r["seams"].as_array().unwrap();
    let across = seams.iter().find(|s| s["upright"] == false).unwrap();
    assert!((across["from"][1].as_f64().unwrap() - across["to"][1].as_f64().unwrap()).abs() < 1e-6, "{r}");
    assert!(across["between_rows"] == true, "level between the rows once turned: {r}");
    let outline = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": {"sheet": [400, 250], "square_to": "outline"}}));
    assert!(outline["squared_to_holes"].is_null(), "{outline}");
    let plain = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": {"sheet": [400, 250], "square_to": "holes"}}));
    assert!((plain["squared_to_holes"].as_f64().unwrap() - turn).abs() < 1e-9, "{plain}");
    assert!(s.execute("scan.panel_proposals", &json!({"body": body, "panel": {"square_to": "door"}})).is_err());
}

/// A wall with a wheel well standing out and a bay 160 × 120 going 120 mm back behind it (smaller than
/// the default least area, so a smaller one is asked for): proposed as an opening for an insert
/// piece, exported as a piece of its own (the gap taken off) with its own CSV and pages; dropped,
/// none; asked of no area, an error.
#[test]
fn panel_openings_behind_the_wall_become_insert_pieces() {
    let bay = |x: f64, y: f64| (400.0..560.0).contains(&x) && (250.0..370.0).contains(&y);
    let mut tris = Vec::new();
    for j in 0..400u32 {
        for i in 0..600u32 {
            let (cx, cy) = (f64::from(i) + 0.5, f64::from(j) + 0.5);
            // The wheel well standing out into the room, as on a van's wall: which way the room is.
            let well = ((cx - 150.0) / 100.0).powi(2) + (cy / 80.0).powi(2) < 1.0;
            let z = if bay(cx, cy) {
                -120.0
            } else if well {
                120.0
            } else {
                0.0
            };
            let at = |x: u32, y: u32| Vec3::new(f64::from(x), f64::from(y), z);
            tris.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            tris.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    let (mut s, body) = session_with_triangles("panel-bay", &tris);
    let panel = json!({"sheet": [700, 500], "spacing": 0, "insert_least": 10000});
    let r = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": panel}));
    let inserts = r["inserts"].as_array().unwrap();
    assert_eq!(inserts.len(), 1, "{r}");
    let o = &inserts[0];
    let size = (o["size"][0].as_f64().unwrap(), o["size"][1].as_f64().unwrap());
    assert!((size.0 - 160.0).abs() <= 8.0 && (size.1 - 120.0).abs() <= 8.0, "{o}");
    assert!((o["depth"].as_f64().unwrap() - 120.0).abs() < 1.0, "{o}");
    assert!((o["piece_size"][0].as_f64().unwrap() - (size.0 - 6.0)).abs() < 0.5, "3 mm off all round: {o}");

    let x = run(&mut s, "scan.panel_export", json!({"body": body, "panel": panel}));
    assert_eq!(x["inserts"].as_array().map(Vec::len), Some(1), "{x}");
    assert_eq!(x["inserts_csv"].as_str().unwrap().lines().count(), 2, "{x}");
    assert!(x["pieces_dxf"].as_str().unwrap().contains("INSERT 1"), "the insert drawn as a piece");
    let pieces = x["pieces"].as_array().unwrap().len() as u64;
    assert_eq!(x["pdf_pages"].as_u64(), Some(1 + pieces + 2 + 1), "the panel, its pieces, the inserts and the insert, the sheets: {x}");

    let dropped = json!({"sheet": [700, 500], "spacing": 0, "insert_least": 10000, "drop_inserts": [0]});
    let x = run(&mut s, "scan.panel_export", json!({"body": body, "panel": dropped}));
    assert_eq!(x["inserts"].as_array().map(Vec::len), Some(0), "{x}");
    let made = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "panel": dropped}));
    assert_eq!(made["panel"]["inserts"], 0, "{made}");
    for bad in [
        json!({"body": body, "panel": {"insert_least": 10000, "drop_inserts": [5]}}),
        json!({"body": body, "panel": {"insert_least": -1}}),
        json!({"body": body, "panel": {"insert_least": f64::MAX}}),
    ] {
        assert!(s.execute("scan.panel_proposals", &bad).is_err(), "{bad}");
    }
    let none = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": {"sheet": [700, 500], "insert_least": 0}}));
    assert!(none["inserts"].as_array().unwrap().is_empty(), "{none}");
}

/// Export Panel: the pieces at their own 0, 0 (outline, holes and fixing points on their layers,
/// labelled), the blanks on sheets, a row in the holes list for each hole and fixing point, the
/// cut list and distances to check; written beside the path when given.
#[test]
fn panel_export_writes_the_pieces_the_sheets_and_the_lists() {
    let (mut s, body, _) = panel_wall("panel-export");
    let r = run(&mut s, "scan.panel_export", json!({"body": body, "panel": {"sheet": [400, 250]}}));
    let pieces = r["pieces"].as_array().unwrap();
    assert_eq!(pieces.len(), 4, "{r}");
    let points: u64 = pieces.iter().map(|p| p["holes"].as_u64().unwrap() + p["fixings"].as_u64().unwrap()).sum();
    let csv = r["holes_csv"].as_str().unwrap();
    assert_eq!(csv.lines().count() as u64, points + 1, "{csv}");
    assert!(pieces.iter().all(|p| p["sheet"].as_u64().unwrap() >= 1), "{r}");
    let dxf = r["pieces_dxf"].as_str().unwrap();
    for needle in ["PIECE 1", "PIECE 4", "HOLES", "FIXINGS", "DIMENSIONS", "CIRCLE"] {
        assert!(dxf.contains(needle), "{needle} in the pieces' DXF");
    }
    assert!(r["sheets_dxf"].as_str().unwrap().contains("SHEET 1"), "{r}");
    assert_eq!(r["cut_list_csv"].as_str().unwrap().lines().count(), 5, "{r}");
    let checks = r["checks"].as_array().unwrap();
    assert!(!checks.is_empty() && checks.iter().all(|c| c["distance"].as_f64().unwrap() > 100.0), "{r}");
    assert_eq!(s.doc.features.len(), 1, "nothing made: only the scan");

    let dir = std::env::temp_dir().join(format!("solidcraft-panel-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wall.dxf");
    let w = run(&mut s, "scan.panel_export", json!({"body": body, "path": path.to_string_lossy(), "panel": {"sheet": [400, 250]}}));
    let files = w["files"].as_array().unwrap();
    assert_eq!(files.len(), 7, "{w}");
    assert_eq!(w["pdf_pages"].as_u64(), Some(4 + 2), "the panel, a page for each piece, the sheets: {w}");
    // The full-size print: each piece about 300 × 195 over 2 × 2 Letter pages, with its map, after the cover.
    assert_eq!(w["full_size_pages"].as_u64(), Some(1 + 4 * (1 + 4)), "{w}");
    let full = String::from_utf8_lossy(&std::fs::read(dir.join("wall-full-size.pdf")).unwrap()).into_owned();
    // The sheet's PDF writer deflates each page, and labels are drawn as letter outlines (the
    // page writer's own test), so the file is read for its page objects only.
    let pages = |pdf: &str| pdf.matches("/Type /Page").count() - pdf.matches("/Type /Pages").count();
    assert_eq!(pages(&full), 21, "the full-size pages");
    let none = run(&mut s, "scan.panel_export", json!({"body": body, "panel": {"sheet": [400, 250]}, "paper": "none"}));
    assert_eq!(none["full_size_pages"].as_u64(), Some(0), "{none}");
    let a4 = run(&mut s, "scan.panel_export", json!({"body": body, "panel": {"sheet": [400, 250]}, "paper": "a4"}));
    assert!(a4["full_size_pages"].as_u64().is_some_and(|n| n > 1), "{a4}");
    assert!(s.execute("scan.panel_export", &json!({"body": body, "paper": "A0"})).is_err());
    let pdf = std::fs::read(dir.join("wall.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-1.") && pdf.trim_ascii_end().ends_with(b"%%EOF"), "a PDF");
    let pdf_text = String::from_utf8_lossy(&pdf);
    assert_eq!(pages(&pdf_text), 6, "six pages");
    assert!(files.iter().all(|f| std::path::Path::new(f.as_str().unwrap()).exists()), "{w}");
    assert!(w.get("pieces_dxf").is_none(), "{w}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Checking the holes against the wall opposite: its scan is the mirror image, two of the holes
/// missing. The holes line up mirrored; the two are not found there (the other scan covers the
/// place), the rest are in both; dropped as asked, they leave the holes list.
#[test]
fn panel_holes_are_checked_against_the_mirror_scan() {
    let mut centres = panel_wall_holes();
    // A few holes off the rows, so the pattern lines up one way only.
    centres.extend([(130.0, 50.0), (470.0, 330.0), (77.0, 290.0), (555.0, 120.0), (410.0, 60.0)]);
    let (mut s, body) = session_with_triangles("panel-confirm", &panel_wall_triangles(&centres, false));
    let missing = [centres[0], centres[5]];
    let theirs: Vec<(f64, f64)> = centres.iter().copied().filter(|c| !missing.contains(c)).collect();
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-panel-mirror-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut stl = String::from("solid m\n");
    for t in panel_wall_triangles(&theirs, true) {
        stl += "facet normal 0 0 0\nouter loop\n";
        for p in t {
            stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
        }
        stl += "endloop\nendfacet\n";
    }
    let path = dir.join("mirror.stl");
    std::fs::write(&path, stl + "endsolid m\n").unwrap();
    let other = run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}))["bodies"][0].as_str().unwrap().to_owned();
    let panel = json!({"sheet": [400, 250], "spacing": 0, "confirm_with": other});
    let r = run(&mut s, "scan.panel_proposals", json!({"body": body, "panel": panel}));
    let c = &r["confirm"];
    assert!(c["mirrored"] == true && c["matched"].as_u64().unwrap() >= 15 && c["rms"].as_f64().unwrap() < 0.5, "{c}");
    let holes = r["holes"].as_array().unwrap();
    for h in holes {
        let at = (h["center"][0].as_f64().unwrap(), h["center"][1].as_f64().unwrap());
        let gone = missing.iter().any(|m| (m.0 - at.0).hypot(m.1 - at.1) < 1.0);
        assert_eq!(h["seen"], if gone { "not_found" } else { "both" }, "{h}");
    }
    assert_eq!(c["not_found"], 2, "{c}");
    // Dropped: two rows fewer in the holes list.
    let all = run(&mut s, "scan.panel_export", json!({"body": body, "panel": panel}));
    let mut dropping = panel.clone();
    dropping["drop_unconfirmed"] = json!(true);
    let fewer = run(&mut s, "scan.panel_export", json!({"body": body, "panel": dropping}));
    let rows = |v: &Value| v["holes_csv"].as_str().unwrap().lines().count();
    assert_eq!(rows(&all), rows(&fewer) + 2, "{}", fewer["holes_csv"]);
    assert!(all["holes_csv"].as_str().unwrap().contains(",both"), "the seen column: {}", all["holes_csv"]);
    assert!(s.execute("scan.panel_proposals", &json!({"body": body, "panel": {"confirm_with": body}})).is_err(), "not itself");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A surface fitted to a scan stays on it: a Move of the scan, a Stand on Base and a move after a
/// rename take the surface along by exactly the same move, in the same feature; `alone` moves the
/// scan by itself; and the surface is noted as made from the scan in the design.
#[test]
fn a_surface_made_from_a_scan_moves_with_it() {
    let (mut s, scan) = session_with_a_plate_and_boss("made-from");
    run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    let made = s.doc.features.iter().find(|f| f.body_names.iter().any(|n| n == "Surface1")).map(|f| f.made_from.clone());
    assert_eq!(made, Some(vec![scan.clone()]), "noted as made from the scan");
    let net = |s: &Session| solidcraft_scan::surface_edit::points(&super::surface::editable(s, Some("Surface1"), "test").unwrap().surface);
    let lo = |s: &Session, body: &str| bounds(s, body).0;
    let (start, scan_start) = (net(&s), lo(&s, &scan));

    let moved = run(&mut s, "solid.move", json!({"bodies": [scan], "translate": [10, -4, 25]}));
    assert_eq!(moved["carried"], json!(["Surface1"]), "{moved}");
    let shift = Vec3::new(10.0, -4.0, 25.0);
    assert!((lo(&s, &scan) - scan_start - shift).len() < STL);
    for (a, b) in start.iter().zip(net(&s)) {
        assert!((b - *a - shift).len() < 1e-9, "the surface moved exactly as the scan: {a:?} -> {b:?}");
    }

    // Standing the scan on a base is a Move of it too: the surface comes along.
    let stood = run(&mut s, "scan.stand_on_base", json!({}));
    assert_eq!(stood["feature"]["carried"], json!(["Surface1"]), "{stood}");

    // `alone` leaves the surface where it is.
    let before = net(&s);
    let alone = run(&mut s, "solid.move", json!({"bodies": [scan], "translate": [0, 0, 5], "alone": true}));
    assert!(alone.get("carried").is_none(), "{alone}");
    assert!(before.iter().zip(net(&s)).all(|(a, b)| (b - *a).len() < 1e-9));

    // Renamed, the scan still takes its surface along.
    run(&mut s, "body.rename", json!({"body": scan, "name": "Part scan"}));
    let renamed = run(&mut s, "solid.move", json!({"bodies": ["Part scan"], "translate": [1, 0, 0]}));
    assert_eq!(renamed["carried"], json!(["Surface1"]), "{renamed}");
}

/// A plane fitted to a scan and a curve drawn on it go with the scan as the surface does: a turned
/// and shifted Move of the scan places the plane and the curve's points again by exactly the move
/// the scan's own points made, in the same undo step.
#[test]
fn planes_and_curves_made_from_a_scan_move_with_it() {
    let (mut s, scan) = session_with_a_plate_and_boss("made-from-sketch");
    let fitted = run(&mut s, "scan.fit_plane", json!({"point": [8, 8, 8]}));
    let plane_feature = fitted["feature"]["feature"].as_u64().unwrap();
    run(&mut s, "scan.curve_on_scan", json!({"points": [[5, 5, 8], [20, 5, 8], [20, 20, 8]]}));
    run(&mut s, "scan.fit_surface", json!({"point": [12, 12, 8], "radius": 8, "controls": 6}));
    let plane_at = |s: &Session| match s.model.state().construct.iter().find(|c| c.feature == plane_feature).map(|c| c.geom.clone()) {
        Some(solidcraft_doc::construct::ConstructGeom::Plane(p)) => p.origin,
        other => panic!("{other:?}"),
    };
    let curve = |s: &Session| -> Vec<Vec3> {
        s.doc
            .features
            .iter()
            .find_map(|f| match &f.kind {
                solidcraft_doc::FeatureKind::Sketch { sketch, .. } if !f.made_from.is_empty() => {
                    Some(sketch.wires.iter().flat_map(|w| w.pts.clone()).collect())
                }
                _ => None,
            })
            .expect("the curve's sketch, noted as made from the scan")
    };
    let first = |s: &Session| s.model.state().body(&scan).and_then(|b| b.body.triangle_mesh()).map(|(p, _)| p[0]).unwrap();
    let (plane0, curve0, scan0) = (plane_at(&s), curve(&s), first(&s));
    let undo_depth = s.undo.len();

    let moved = run(&mut s, "solid.move", json!({"bodies": [scan], "translate": [10, -4, 25], "axis": [0, 0, 1], "angle": "30 deg"}));
    assert_eq!(moved["carried"], json!(["Surface1"]), "{moved}");
    assert_eq!(moved["placed"].as_array().map(Vec::len), Some(2), "the plane and the curve's sketch: {moved}");
    assert_eq!(s.undo.len(), undo_depth + 1, "one undo step");
    let m = solidcraft_doc::rigid(Vec3::new(10.0, -4.0, 25.0), Vec3::ZERO, Vec3::Z, 30f64.to_radians());
    let to = |p: Vec3| solidcraft_doc::apply_point(&m, p);
    assert!((first(&s) - to(scan0)).len() < 1e-9, "the scan moved by the move");
    assert!((plane_at(&s) - to(plane0)).len() < 1e-9, "the plane moved as the scan: {:?} -> {:?}", plane0, plane_at(&s));
    for (a, b) in curve0.iter().zip(curve(&s)) {
        assert!((b - to(*a)).len() < 1e-9, "the curve moved as the scan: {a:?} -> {b:?}");
    }
    run(&mut s, "edit.undo", json!({}));
    assert!((plane_at(&s) - plane0).len() < 1e-12 && (first(&s) - scan0).len() < 1e-12, "one undo puts all back");
}

/// An ASCII STL of a flat floor 100 mm square on z = 0 with balls of the radii given sunk half into
/// it: a scan with reference markers on it (and, for a larger radius, a ball that is not one).
fn floor_with_balls_stl(balls: &[(Vec3, f64)]) -> String {
    let mut stl = String::from("solid floor\n");
    let mut facet = |a: Vec3, b: Vec3, c: Vec3| {
        stl += &format!(
            "facet normal 0 0 0\nouter loop\nvertex {} {} {}\nvertex {} {} {}\nvertex {} {} {}\nendloop\nendfacet\n",
            a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
        );
    };
    let n = 60;
    let at = |i: usize, j: usize| Vec3::new(100.0 * i as f64 / n as f64, 100.0 * j as f64 / n as f64, 0.0);
    for i in 0..n {
        for j in 0..n {
            facet(at(i, j), at(i + 1, j), at(i + 1, j + 1));
            facet(at(i, j), at(i + 1, j + 1), at(i, j + 1));
        }
    }
    for &(c, r) in balls {
        let (rings, segments) = (20, 40);
        let p = |i: usize, j: usize| {
            let (phi, th) = (std::f64::consts::PI * i as f64 / rings as f64, std::f64::consts::TAU * j as f64 / segments as f64);
            c + Vec3::new(phi.sin() * th.cos(), phi.sin() * th.sin(), phi.cos()) * r
        };
        for i in 0..rings {
            for j in 0..segments {
                facet(p(i, j), p(i + 1, j), p(i + 1, j + 1));
                facet(p(i, j), p(i + 1, j + 1), p(i, j + 1));
            }
        }
    }
    stl + "endsolid floor\n"
}

/// Remove Markers: a click on one marker finds the other two of its radius (not the larger ball),
/// the preview changes nothing, OK takes them off and closes the floor as one Scan Clean step,
/// and undo brings them back.
#[test]
fn remove_markers_finds_them_all_from_one_and_keeps_a_step() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-markers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("markers.stl");
    let markers = [Vec3::new(20.0, 20.0, 0.0), Vec3::new(70.0, 30.0, 0.0), Vec3::new(40.0, 75.0, 0.0)];
    let mut balls: Vec<(Vec3, f64)> = markers.iter().map(|&c| (c, 3.0)).collect();
    balls.push((Vec3::new(80.0, 80.0, 0.0), 7.0));
    std::fs::write(&path, floor_with_balls_stl(&balls)).unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    let triangles = |s: &Session| s.model.state().body(&scan).and_then(|b| b.body.triangle_mesh().map(|(_, t)| t.len())).unwrap();
    let before = triangles(&s);
    let features = s.doc.features.len();

    let preview = run(&mut s, "scan.remove_markers", json!({"body": scan, "at": [20, 20, 3], "preview": true}));
    assert!((preview["radius"].as_f64().unwrap() - 3.0).abs() < 0.05, "{preview}");
    assert_eq!(preview["centres"].as_array().unwrap().len(), 3, "the three markers, not the larger ball: {preview}");
    assert_eq!((triangles(&s), s.doc.features.len()), (before, features), "a preview changes nothing");

    let done = run(&mut s, "scan.remove_markers", json!({"body": scan, "at": [20, 20, 3]}));
    assert_eq!((done["filled"].as_u64(), done["failed"].as_u64()), (Some(3), Some(0)), "{done}");
    assert_eq!(s.doc.features.len(), features + 1, "one Scan Clean step");
    let state = s.model.state();
    let (positions, _) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).unwrap();
    for m in markers {
        assert!(positions.iter().all(|p| p.dist(m) > 3.0 * 1.15 - 1e-6 || p.z.abs() < 1e-6), "only the floor is left near {m:?}");
    }
    assert!(positions.iter().any(|p| p.dist(Vec3::new(80.0, 80.0, 7.0)) < 1e-6), "the larger ball stays");
    drop(state);

    run(&mut s, "edit.undo", json!({}));
    assert_eq!((triangles(&s), s.doc.features.len()), (before, features), "undone");

    let err = s.execute("scan.remove_markers", &json!({"body": scan, "centres": [[20, 20, 0]]})).unwrap_err().to_string();
    assert!(err.contains("radius"), "{err}");
    let err = s.execute("scan.remove_markers", &json!({"body": scan})).unwrap_err().to_string();
    assert!(err.contains("click a marker"), "{err}");
    let err = s.execute("scan.remove_markers", &json!({"body": scan, "at": [50, 50, 0]})).unwrap_err().to_string();
    assert!(err.contains("not a sphere"), "the floor: {err}");
    let given = run(&mut s, "scan.remove_markers", json!({"body": scan, "centres": [[70, 30, 0]], "radius": 3}));
    assert_eq!(given["filled"], 1, "a marker given by its centre: {given}");
}

/// Remove and Fill: a ball standing on a floor, taken off round a click, leaves the floor closed
/// flat where it stood, as one Scan Clean step that undo takes back; with no part chosen the
/// command says what it needs.
#[test]
fn remove_and_fill_takes_a_boss_off_and_closes_the_floor() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-remove-fill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("boss.stl");
    let boss = Vec3::new(50.0, 50.0, 0.0);
    // A floor 100 mm square with a cone-shaped boss 4 mm high and 6 mm round, of one piece with it.
    let n = 60;
    let at = |i: usize, j: usize| {
        let (x, y) = (100.0 * i as f64 / n as f64, 100.0 * j as f64 / n as f64);
        let r = Vec3::new(x, y, 0.0).dist(boss);
        Vec3::new(x, y, if r < 6.0 { 4.0 * (1.0 - r / 6.0) } else { 0.0 })
    };
    let mut stl = String::from("solid boss\n");
    for i in 0..n {
        for j in 0..n {
            for [a, b, c] in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                stl += &format!(
                    "facet normal 0 0 0\nouter loop\nvertex {} {} {}\nvertex {} {} {}\nvertex {} {} {}\nendloop\nendfacet\n",
                    a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
                );
            }
        }
    }
    std::fs::write(&path, stl + "endsolid boss\n").unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    let triangles = |s: &Session| s.model.state().body(&scan).and_then(|b| b.body.triangle_mesh().map(|(_, t)| t.len())).unwrap();
    let before = triangles(&s);
    let features = s.doc.features.len();

    let err = s.execute("scan.remove_and_fill", &json!({"body": scan})).unwrap_err().to_string();
    assert!(err.contains("choose the part"), "{err}");

    let done = run(&mut s, "scan.remove_and_fill", json!({"body": scan, "point": [50, 50, 3.5], "radius": 9}));
    assert!(done["removed_triangles"].as_u64().unwrap() > 0, "{done}");
    assert_eq!((done["filled"].as_u64(), done["failed"].as_u64(), done["open_to_edge"].as_u64()), (Some(1), Some(0), Some(0)), "{done}");
    assert_eq!(s.doc.features.len(), features + 1, "one Scan Clean step");
    let state = s.model.state();
    let (positions, _) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).unwrap();
    let highest = positions.iter().filter(|p| Vec3::new(p.x, p.y, 0.0).dist(boss) < 7.0).map(|p| p.z.abs()).fold(0.0, f64::max);
    assert!(highest < 1e-6, "the floor is closed flat where the boss stood: {highest}");
    drop(state);

    run(&mut s, "edit.undo", json!({}));
    assert_eq!((triangles(&s), s.doc.features.len()), (before, features), "undone");
}

/// Length on Scan: on a flat sheet the line along the surface is the straight one, with its
/// way given when asked; nothing in the design changes; a dot off the scan is refused.
#[test]
fn measure_line_reads_the_length_along_the_scan_and_changes_nothing() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-measure-line-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.stl");
    std::fs::write(&path, sheet_stl(40.0, 20, 0.0)).unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    let (revision, features) = (s.revision, s.doc.features.len());
    let line = run(&mut s, "scan.measure_line", json!({"body": scan, "points": [[5, 5, 0], [35, 5, 0], [35, 25, 0]], "path": true}));
    assert!((line["length"].as_f64().unwrap() - 50.0).abs() < 1e-9, "{line}");
    assert!(line["under_read"].as_f64().unwrap().abs() < 1e-9 && line["not_followed"] == 0, "{line}");
    assert!((line["ends"].as_f64().unwrap() - (30.0f64 * 30.0 + 20.0 * 20.0).sqrt()).abs() < 1e-9);
    assert_eq!(line["delta"], json!([30.0, 20.0, 0.0]));
    let stretches = line["stretches"].as_array().unwrap();
    assert_eq!(stretches.len(), 2);
    assert!(stretches.iter().all(|st| st["path"].as_array().is_some_and(|p| p.len() >= 2)), "{line}");
    assert!(line.get("round").is_none(), "a straight line is no arc");
    assert_eq!((s.revision, s.doc.features.len()), (revision, features), "nothing changes");
    let err = s.execute("scan.measure_line", &json!({"body": scan, "points": [[5, 5, 0], [5, 5, 30]]})).unwrap_err().to_string();
    assert!(err.contains("dot 2 is not on the scan"), "{err}");
    let err = s.execute("scan.measure_line", &json!({"body": scan, "points": [[5, 5, 0]]})).unwrap_err().to_string();
    assert!(err.contains("2 to 1000"), "{err}");
}

/// Unseen Surface: a scan as it came in has none; a hole closed by Fill Holes is a patch where
/// the hole was; moved afterwards, the patch moves with it; a plane fitted over the plate says
/// how much of it leans on the patch.
#[test]
fn unseen_surface_finds_a_filled_hole_and_a_fit_over_it_says_so() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-unseen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("holed.stl");
    // A plate 40 mm square of 1 mm cells, a hole 8 mm square in its middle.
    let n = 40;
    let at = |i: usize, j: usize| Vec3::new(i as f64, j as f64, 0.0);
    let mut stl = String::from("solid holed\n");
    for i in 0..n {
        for j in 0..n {
            if (16..24).contains(&i) && (16..24).contains(&j) {
                continue;
            }
            for [a, b, c] in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                stl += &format!(
                    "facet normal 0 0 1\nouter loop\nvertex {} {} {}\nvertex {} {} {}\nvertex {} {} {}\nendloop\nendfacet\n",
                    a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
                );
            }
        }
    }
    std::fs::write(&path, stl + "endsolid holed\n").unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    let fresh = run(&mut s, "scan.unseen", json!({"body": scan}));
    assert_eq!((fresh["source"].as_str(), fresh["area"].as_f64()), (Some("as_scanned"), Some(0.0)), "{fresh}");

    run(&mut s, "scan.fill_holes", json!({"body": scan}));
    let filled = run(&mut s, "scan.unseen", json!({"body": scan}));
    assert_eq!(filled["source"], "cleaned", "{filled}");
    let area = filled["area"].as_f64().unwrap();
    assert!(area > 10.0 && area < 64.0, "the middle of the 64 mm² hole: {filled}");
    let centre = &filled["patches"][0]["centre"];
    assert!((centre[0].as_f64().unwrap() - 20.0).abs() < 1.0 && (centre[1].as_f64().unwrap() - 20.0).abs() < 1.0, "{filled}");

    run(&mut s, "solid.move", json!({"bodies": [scan], "translate": [100, 0, 0]}));
    let moved = run(&mut s, "scan.unseen", json!({"body": scan}));
    assert_eq!(moved["source"], "cleaned", "the move carries the scan before the steps along: {moved}");
    assert!((moved["area"].as_f64().unwrap() - area).abs() < 1e-6, "{moved}");
    assert!((moved["patches"][0]["centre"][0].as_f64().unwrap() - 120.0).abs() < 1.0, "{moved}");

    let fit = run(&mut s, "scan.fit_plane", json!({"body": scan, "point": [105, 5, 0], "keep": false}));
    let share = fit["unseen"].as_f64().expect("the fit leans on the patch");
    assert!(share > 0.0 && share < 0.1, "{fit}");
    assert!(fit["warning"].as_str().is_some_and(|w| w.contains("never saw")), "{fit}");
}

/// Fit Together: two bores scanned a hair off one axis and one size, fitted with the relations
/// they nearly hold (none given), come out on one axis with one radius, each kept as a construction
/// axis, one undo step; a relation naming a shape that is not there is refused.
#[test]
fn fit_together_holds_two_bores_on_one_axis_with_one_radius() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-together-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bores.stl");
    let mut stl = String::from("solid bores\n");
    // A tube of radius `r` about the vertical line through (cx, 0), from z0 to z1.
    let mut tube = |cx: f64, r: f64, z0: f64, z1: f64| {
        let (around, along) = (48, 10);
        let at = |i: usize, j: usize| {
            let a = std::f64::consts::TAU * i as f64 / around as f64;
            Vec3::new(cx + r * a.cos(), r * a.sin(), z0 + (z1 - z0) * j as f64 / along as f64)
        };
        for i in 0..around {
            for j in 0..along {
                for [a, b, c] in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    stl += &format!(
                        "facet normal 0 0 0\nouter loop\nvertex {} {} {}\nvertex {} {} {}\nvertex {} {} {}\nendloop\nendfacet\n",
                        a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
                    );
                }
            }
        }
    };
    tube(0.0, 5.0, 0.0, 10.0);
    tube(0.05, 5.04, 30.0, 40.0);
    std::fs::write(&path, stl + "endsolid bores\n").unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    let features = s.doc.features.len();
    let shapes = json!([{"kind": "cylinder", "point": [5, 0, 5]}, {"kind": "cylinder", "point": [5.09, 0, 35]}]);
    let done = run(&mut s, "scan.fit_together", json!({"body": scan, "shapes": shapes}));
    assert_eq!(done["suggested"], true, "{done}");
    let kinds: Vec<&str> = done["relations"].as_array().unwrap().iter().filter_map(|r| r["kind"].as_str()).collect();
    assert!(kinds.contains(&"coaxial") && kinds.contains(&"equal_radius"), "{kinds:?}");
    assert!(done["relations"].as_array().unwrap().iter().all(|r| r["off"].as_f64().unwrap() < 1e-6), "{done}");
    let (r0, r1) = (done["shapes"][0]["cylinder"]["radius"].as_f64().unwrap(), done["shapes"][1]["cylinder"]["radius"].as_f64().unwrap());
    assert!((r0 - r1).abs() < 1e-6 && r0 > 4.99 && r0 < 5.04, "{r0} {r1}");
    assert_eq!(s.doc.features.len(), features + 2, "two construction axes");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.features.len(), features, "one undo step");
    // Mirrored in XY, two upright bores are on one axis (an axis up the Z axis reflects into
    // itself) with one radius: held so, the relation holds.
    let relations = json!([{"kind": "mirror", "a": 0, "b": 1, "about": "xy"}]);
    let mirrored = run(&mut s, "scan.fit_together", json!({"body": scan, "shapes": shapes, "keep": false, "relations": relations}));
    assert_eq!(mirrored["relations"][0]["about"], "xy", "{mirrored}");
    assert!(mirrored["relations"][0]["off"].as_f64().unwrap() < 1e-6, "{mirrored}");
    let unplaced = json!([{"kind": "mirror", "a": 0, "b": 1}]);
    let err = s.execute("scan.fit_together", &json!({"body": scan, "shapes": shapes, "relations": unplaced})).unwrap_err();
    assert!(err.to_string().contains("needs `about`"), "{err}");
    let err =
        s.execute("scan.fit_together", &json!({"body": scan, "shapes": shapes, "relations": [{"kind": "coaxial", "a": 0, "b": 5}]})).unwrap_err();
    assert!(err.to_string().contains("there are 2 shapes"), "{err}");
}

/// Inspection Report: one HTML file naming the scan, its Fill Holes step and the surface its
/// scanner never saw; no renderer is installed in tests, so it says it has no pictures; the
/// design is unchanged; a path that is not .html is refused.
#[test]
fn report_writes_one_html_file_of_the_scan_and_what_was_done_to_it() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.stl");
    std::fs::write(&path, sheet_stl(40.0, 20, 0.0)).unwrap();
    let mut s = Session::default();
    let scan = run(&mut s, "file.insert_mesh", json!({"path": path}))["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut s, "scan.remove_and_fill", json!({"body": scan, "point": [20, 20, 0], "radius": 5}));
    let (revision, features) = (s.revision, s.doc.features.len());
    let out = dir.join("report.html");
    let done = run(&mut s, "scan.report", json!({"body": scan, "path": out, "title": "Sheet <1>"}));
    let html = std::fs::read_to_string(&out).unwrap();
    assert_eq!(done["bytes"].as_u64().unwrap() as usize, html.len());
    assert!(html.starts_with("<!doctype html>") && html.contains("<title>Sheet &lt;1&gt;</title>"), "the title, escaped");
    for part in ["The scan", "What was done to it", "Remove and Fill", "Surface the scanner never saw", "Pictures"] {
        assert!(html.contains(part), "{part}");
    }
    assert!(done["no_pictures"].as_str().is_some() && html.contains("No pictures"), "{done}");
    assert_eq!((s.revision, s.doc.features.len()), (revision, features), "nothing changes");
    let err = s.execute("scan.report", &json!({"body": scan, "path": dir.join("report.txt")})).unwrap_err();
    assert!(err.to_string().contains(".html"), "{err}");
}

/// A sheet `size` mm square of `n` × `n` squares at height `z`, as an STL.
fn sheet_stl(size: f64, n: usize, z: f64) -> String {
    let at = |i: usize, j: usize| Vec3::new(size * i as f64 / n as f64, size * j as f64 / n as f64, z);
    let mut stl = String::from("solid sheet\n");
    for i in 0..n {
        for j in 0..n {
            for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                stl += "facet normal 0 0 0\nouter loop\n";
                for p in tri {
                    stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
                }
                stl += "endloop\nendfacet\n";
            }
        }
    }
    stl + "endsolid sheet\n"
}

/// A scan in a component whose occurrence was turned and moved after it was inserted: the scan
/// tools read it where it shows. A point picked on it as shown is on it, and what is fitted there
/// is where it shows (#scan-in-a-moved-component).
#[test]
fn a_scan_in_a_moved_component_is_worked_on_where_it_shows() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-moved-component-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.stl");
    std::fs::write(&path, sheet_stl(40.0, 20, 5.0)).unwrap();
    let mut s = Session::default();
    let made = run(&mut s, "component.create", json!({"name": "Scanned", "grounded": false}));
    let component = made["component"].clone();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    run(&mut s, "component.activate", json!({"component": 0}));
    let occurrence = s.doc.occurrences.iter().find(|o| Some(o.component) == component.as_u64()).map(|o| o.id).expect("its occurrence");
    // Stood on its edge (turned a quarter about X) and moved 100 along X.
    run(
        &mut s,
        "occurrence.move",
        json!({"occurrence": occurrence, "space": "world", "axis": [1, 0, 0], "angle": "90 deg", "translate": [100, 0, 0]}),
    );
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
    // The sheet now shows upright: x 100…140, z 0…40, at y = -5.
    let m = run(&mut s, "inspect.measure", json!({"bodies": [body]}));
    let lo = &m["bodies"][0]["bbox"]["min"];
    assert!((lo[0].as_f64().unwrap() - 100.0).abs() < 1e-6 && (lo[1].as_f64().unwrap() + 5.0).abs() < 1e-6, "shown upright: {m}");
    let fitted = run(&mut s, "scan.fit_plane", json!({"body": body, "point": [120, -5, 20], "keep": false}));
    let n = &fitted["plane"]["normal"];
    assert!(n[1].as_f64().unwrap().abs() > 1.0 - 1e-6, "the plane faces along Y, as the sheet shows: {fitted}");
    let at = &fitted["plane"]["point"];
    assert!((at[1].as_f64().unwrap() + 5.0).abs() < 1e-6 && at[0].as_f64().unwrap() > 99.0, "where it shows: {fitted}");
}

/// Scan Deviation with nothing selected: a model put away (hidden) is not the one meant when
/// another shows, and with a scan and its CAD model as a mesh (an STL) both shown, the noisy one
/// is the scan, the clean one the model.
#[test]
fn deviation_takes_the_shown_model_and_tells_a_scan_from_its_cad_mesh() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-deviation-defaults-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The CAD: an exact sheet at z = 5. The scan: the same sheet, its points 0.05 mm up or down.
    std::fs::write(dir.join("cad.stl"), sheet_stl(40.0, 20, 5.0)).unwrap();
    let noisy = sheet_stl(40.0, 40, 5.0)
        .lines()
        .map(|l| match l.strip_prefix("vertex ") {
            Some(rest) => {
                let v: Vec<f64> = rest.split_whitespace().map(|x| x.parse().unwrap()).collect();
                // The same jitter for the same point, so the sheet stays one piece.
                let k = ((v[0] * 12.9898 + v[1] * 78.233).sin() * 43758.5453).rem_euclid(1.0);
                format!("vertex {} {} {}", v[0], v[1], v[2] + 0.1 * (k - 0.5))
            }
            None => l.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("scan.stl"), noisy).unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": dir.join("cad.stl").to_string_lossy()}));
    run(&mut s, "file.insert_mesh", json!({"path": dir.join("scan.stl").to_string_lossy()}));
    run(&mut s, "select.clear", json!({}));
    let r = run(&mut s, "scan.deviation", json!({}));
    assert_eq!((r["scan"].as_str(), r["body"].as_str()), (Some("scan"), Some("cad")), "{r}");
    // A solid model, hidden, and another shown: the shown one.
    run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 5, "corner": [0, 0, 0], "body_name": "Old"}));
    run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 5, "corner": [0, 0, 0], "body_name": "New", "operation": "new"}));
    run(&mut s, "browser.visibility", json!({"items": ["Old", "cad"], "visible": false}));
    run(&mut s, "select.clear", json!({}));
    let r = run(&mut s, "scan.deviation", json!({"scan": "scan"}));
    assert_eq!(r["body"].as_str(), Some("New"), "{r}");
    // Every solid hidden, the CAD mesh shown again: the CAD mesh, not a hidden solid.
    run(&mut s, "browser.visibility", json!({"items": ["New"], "visible": false}));
    run(&mut s, "browser.visibility", json!({"items": ["cad"], "visible": true}));
    run(&mut s, "select.clear", json!({}));
    let r = run(&mut s, "scan.deviation", json!({}));
    assert_eq!((r["scan"].as_str(), r["body"].as_str()), (Some("scan"), Some("cad")), "{r}");
}

/// Flat Template's whole outline across a tear: a wall 400 × 300 with a slot 40 wide and 100 deep
/// torn into its bottom edge. The outline bridges the slot (narrower than the 50 mm gap), and the
/// stretch across its mouth, with no scan near it, is said to be guessed.
#[test]
fn flat_template_says_where_the_outline_was_guessed() {
    let n = 100;
    let at = |i: u32, j: u32| Vec3::new(400.0 * f64::from(i) / f64::from(n), 300.0 * f64::from(j) / f64::from(n), 0.0);
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let (x, y) = (400.0 * (f64::from(i) + 0.5) / f64::from(n), 300.0 * (f64::from(j) + 0.5) / f64::from(n));
            if (180.0..220.0).contains(&x) && y < 100.0 {
                continue;
            }
            tris.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            tris.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    let (mut s, body) = session_with_triangles("flat-template-guessed", &tris);
    let r = run(&mut s, "scan.flat_template", json!({"body": body, "outline": "scan", "smooth": 0, "round": 0}));
    assert_eq!(r["guessed"], 1, "the slot's mouth: {r}");
    let length = r["guessed_length"].as_f64().unwrap();
    assert!((10.0..45.0).contains(&length), "across the mouth, less the reach each side: {length}");
}

/// A sphere scanned, `radius` round the origin: a cube's sides, `n` × `n` each, pushed out onto it.
fn sphere_triangles(radius: f64, n: u32) -> Vec<[Vec3; 3]> {
    let on = |p: Vec3| p.normalized().map_or(Vec3::ZERO, |d| d * radius);
    let sides = [
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0)),
        (Vec3::new(-1.0, 1.0, -1.0), Vec3::new(0.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 0.0)),
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(0.0, 2.0, 0.0), Vec3::new(2.0, 0.0, 0.0)),
        (Vec3::new(-1.0, -1.0, 1.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 2.0, 0.0)),
        (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 2.0, 0.0)),
        (Vec3::new(1.0, -1.0, -1.0), Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, 2.0)),
    ];
    let mut out = Vec::new();
    for (o, du, dv) in sides {
        let at = |i: u32, j: u32| on(o + du * (f64::from(i) / f64::from(n)) + dv * (f64::from(j) / f64::from(n)));
        for i in 0..n {
            for j in 0..n {
                out.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                out.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }
    out
}

/// A Free Form cup of five faces round a sphere (its top over four great circles' arcs, four
/// faces added round it) thickened by Thicken, which SMLib alone refuses for several doubly
/// curved faces: a closed solid the thickness thick over five sixths of the sphere, that follows
/// the thickness parameter.
#[test]
fn thicken_makes_a_free_form_network_of_curved_faces_a_solid() {
    let radius = 40.0;
    let (mut s, _) = session_with_triangles("thicken-network", &sphere_triangles(radius, 30));
    let c = radius / 3.0f64.sqrt();
    let p = |x: f64, y: f64, z: f64| Vec3::new(x * c, y * c, z * c);
    let arc = |a: Vec3, b: Vec3| -> Vec<[f64; 3]> {
        (0..=24).filter_map(|k| (a + (b - a) * (f64::from(k) / 24.0)).normalized().map(|d| d * radius)).map(|q| [q.x, q.y, q.z]).collect()
    };
    let corners = [p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)];
    let curves: Vec<Vec<[f64; 3]>> = (0..4).map(|k| arc(corners[k], corners[(k + 1) % 4])).collect();
    run(&mut s, "scan.free_form", json!({"curves": curves, "count": 8}));
    let xyz = |q: Vec3| [q.x, q.y, q.z];
    for (edge, far) in [
        ([p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0)], [p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0)]),
        ([p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0)], [p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0)]),
        ([p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)], [p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0)]),
        ([p(-1.0, 1.0, 1.0), p(-1.0, -1.0, 1.0)], [p(-1.0, 1.0, -1.0), p(-1.0, -1.0, -1.0)]),
    ] {
        let added = run(
            &mut s,
            "scan.add_face",
            json!({"surface": "FreeForm1", "edge": [xyz(edge[0]), xyz(edge[1])], "far": [xyz(far[0]), xyz(far[1])], "rows": 8, "symmetry": "none"}),
        );
        assert!(added["rms"].as_f64().unwrap() < 0.1, "{added}");
    }
    run(&mut s, "parameters.add", json!({"name": "wall", "expression": "3 mm"}));
    run(&mut s, "surface.thicken", json!({"bodies": ["FreeForm1"], "thickness": "wall", "body_name": "Cup"}));
    // Five sixths of the shell between the sphere and the sphere grown (or shrunk) by the wall.
    let shell = |r0: f64, r1: f64| 5.0 / 6.0 * 4.0 / 3.0 * std::f64::consts::PI * (r1.powi(3) - r0.powi(3)).abs();
    let volume = |s: &Session| s.model.state().body("Cup").and_then(|b| b.body.volume_at(0.0)).unwrap_or(0.0);
    // Outward: a Free Form faces out of the scan it is made on, however its sides were given.
    let near = |v: f64, t: f64| (v - shell(radius, radius + t)).abs() < 0.02 * shell(radius, radius + t);
    let v3 = volume(&s);
    assert!(near(v3, 3.0), "a closed solid 3 mm thick: {v3}");
    run(&mut s, "parameters.change", json!({"name": "wall", "expression": "5 mm"}));
    let v5 = volume(&s);
    assert!(near(v5, 5.0), "it follows the wall: {v5}");
    // A boss extruded down to the cup's curved top joins it (the faces Thicken sews face out of
    // the solid, as SMLib's cut of the tool by the cup needs): a cylinder r 10 from z 60 down to
    // the sphere of radius 45.
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 60, "name": "Top"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "diameter": 20}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Top", "to": [0, 0, radius + 5.0], "operation": "join", "targets": ["Cup"]}));
    let boss = std::f64::consts::PI * 100.0 * 60.0 - 2.0 * std::f64::consts::PI / 3.0 * (45f64.powi(3) - (45f64.powi(2) - 100.0).powf(1.5));
    let v = volume(&s);
    assert!((v - v5 - boss).abs() < 0.02 * boss, "the boss up to the curved face: {} against {boss}", v - v5);
}

/// A Free Form faces out of the scan it is made on, whichever way round its corners are clicked or
/// its sides given: on the dome (which faces up) it faces up, so Thicken grows it upward.
#[test]
fn a_free_form_faces_out_of_the_scan_whichever_way_its_corners_go() {
    let (mut s, _) = session_with_triangles("free-form-facing", &dome_triangles(40));
    let round = [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]];
    for (k, corners) in [round, [round[0], round[3], round[2], round[1]]].into_iter().enumerate() {
        let made = run(&mut s, "scan.free_form", json!({"corners": corners, "count": 5}));
        let name = made["surface"].as_str().unwrap().to_owned();
        let face = super::surface::editable(&s, Some(&name), "test").unwrap().surface;
        let ([[u0, u1], [v0, v1]], at) = (face.domain(), 0.5);
        let (u, v) = (u0 + (u1 - u0) * at, v0 + (v1 - v0) * at);
        let d = |du: f64, dv: f64| face.eval(u + du, v + dv).unwrap();
        let normal = (d(1e-4, 0.0) - d(-1e-4, 0.0)).cross(d(0.0, 1e-4) - d(0.0, -1e-4));
        assert!(normal.z > 0.0, "order {k}: faces up, out of the dome: {normal:?}");
        let body = format!("Slab{k}");
        run(&mut s, "surface.thicken", json!({"bodies": [name], "thickness": 2, "body_name": body}));
        let top = s.model.state().body(&body).unwrap().body.exact_bounds().unwrap().max.z;
        assert!(top > 9.5, "order {k}: grown upward from the dome's top at 8: {top}");
    }
}

/// Corners that enclose no area make no face in any fit, and say so: a corner clicked twice, four
/// in a line, a crossed order. They used to reach the STEP writer (a misleading "STEP import: no
/// bodies") or, in a line, be kept as a face with no width. A corner shared by two ends of one
/// side (a triangle) still makes a face.
#[test]
fn free_form_refuses_corners_that_enclose_no_area() {
    let (mut s, _) = session_with_triangles("free-form-no-area", &dome_triangles(40));
    for corners in [
        json!([[10, 10, 0], [10, 10, 0], [10, 10, 0], [10, 10, 0]]),
        json!([[10, 10, 0], [50, 10, 0], [50, 50, 0], [50, 10, 0]]),
        json!([[10, 10, 0], [20, 10, 0], [30, 10, 0], [40, 10, 0]]),
        json!([[10, 10, 0], [50, 50, 0], [50, 10, 0], [10, 50, 0]]),
    ] {
        for fit in ["follow", "flat", "off", "overall"] {
            let refused = s.execute("scan.free_form", &json!({"corners": corners, "fit": fit})).unwrap_err().to_string();
            assert!(refused.contains("enclose no area") || (fit == "flat" && refused.contains("no flat face")), "{corners} {fit}: {refused}");
            assert!(!refused.contains("STEP"), "{corners} {fit}: {refused}");
        }
    }
    let triangle = run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [10, 10, 0], [50, 50, 0], [50, 10, 0]]}));
    assert_eq!(triangle["surface"], "FreeForm1", "{triangle}");
    assert_eq!(s.model.state().bodies.len(), 2, "the scan and the one face; the refusals kept nothing");
}

/// Wrap Scan on a closed scan of a sphere R 20 (a cube's sides pushed out, 24 × 24 each): one closed
/// solid of three patches per coarse triangle, every scan point within the tolerance, its volume
/// that of the sphere, 4/3 π 20³ = 33,510.3 mm³, within 0.5 % (the scan's own facets lie up to the
/// chord's sag inside the sphere: R(1 − cos(π/48)) ≈ 0.04 mm at this density, about 0.6 % of the
/// volume at most, and the patches are fitted to the facets). A cap below 300 is refused.
#[test]
fn wrap_scan_makes_a_closed_scan_one_solid_within_its_tolerance() {
    let radius = 20.0;
    let (mut s, scan) = session_with_triangles("wrap-scan", &sphere_triangles(radius, 24));
    let r = run(&mut s, "scan.wrap_scan", json!({"body": scan, "tolerance": 0.1}));
    assert_eq!(r["solid"], true, "{r}");
    let (patches, coarse) = (r["patches"].as_u64().unwrap(), r["coarse_triangles"].as_u64().unwrap());
    assert!((2 * coarse..=3 * coarse).contains(&patches), "three patches to a triangle, four to a pair: {r}");
    assert!(r["within"].as_f64().unwrap() >= 0.99, "{r}");
    let surface = r["surface"].as_str().unwrap().to_string();
    let m = run(&mut s, "inspect.measure", json!({"bodies": [surface]}));
    let volume = m["bodies"][0]["volume_mm3"].as_f64().unwrap();
    let sphere = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);
    assert!((volume - sphere).abs() <= 0.005 * sphere, "volume {volume} against {sphere}");
    let refused = s.execute("scan.wrap_scan", &json!({"body": scan, "max_patches": 10})).unwrap_err().to_string();
    assert!(refused.contains("max_patches"), "{refused}");
}

