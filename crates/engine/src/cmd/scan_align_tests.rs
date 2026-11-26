//! Aligning on scans of a part modelled here: a block with a boss, meshed finely, each pass what
//! one side of it sees.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// The mesh with every triangle cut into four `levels` times, the cuts shared along each edge, so
/// its points are spread over its surface as a scan's are.
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

/// The part's surface, meshed finely: positions and triangles; and the session it is a body of.
fn part() -> (Session, String, Vec<Vec3>, Vec<[u32; 3]>) {
    let mut s = Session::default();
    let made = run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 20, "body_name": "Part"}));
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    run(&mut s, "solid.cylinder", json!({"radius": 6, "height": 12, "base": [12, 10, 20], "operation": "join", "targets": [body]}));
    run(&mut s, "solid.cylinder", json!({"radius": 3, "height": 25, "base": [45, 28, 0], "operation": "cut", "targets": [body]}));
    let mesh = {
        let state = s.model.state();
        state.body(&body).unwrap().body.mesh(&solidcraft_kernel::MeshTolerance { chord: 0.01, angle: Some(4f64.to_radians()) }).unwrap()
    };
    let (positions, triangles) = subdivided(mesh.positions, mesh.triangles, 3);
    (s, body, positions, triangles)
}

/// `positions` turned by `angle` about the unit `axis` through the origin, then moved.
fn turned(positions: &[Vec3], axis: Vec3, angle: f64, shift: Vec3) -> Vec<Vec3> {
    let (s, c) = angle.sin_cos();
    positions.iter().map(|p| *p * c + axis.cross(*p) * s + axis * (axis.dot(*p) * (1.0 - c)) + shift).collect()
}

/// The triangles a pass from `from` gets: all but those facing well away from it (the side it
/// cannot see), so passes have most of the part in common, as real passes round a part do.
fn seen(positions: &[Vec3], triangles: &[[u32; 3]], from: Vec3) -> Vec<[u32; 3]> {
    let from = from.normalized().unwrap_or(Vec3::Z);
    triangles
        .iter()
        .copied()
        .filter(|&[a, b, c]| {
            let (a, b, c) = (positions[a as usize], positions[b as usize], positions[c as usize]);
            (b - a).cross(c - a).normalized().is_some_and(|n| n.dot(from) > -0.9)
        })
        .collect()
}

/// The mesh inserted as a scan named `name`; its body's name.
fn insert(s: &mut Session, positions: &[Vec3], triangles: &[[u32; 3]], name: &str) -> String {
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
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-align-{name}-{}-{:?}", std::process::id(), std::thread::current().id()));
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

/// The farthest a scan's points are from where they belong (the part's own points, nearest).
fn worst_off(s: &Session, body: &str, truth: &[Vec3]) -> f64 {
    let grid: std::collections::HashMap<[i64; 3], Vec<Vec3>> = truth.iter().fold(Default::default(), |mut m, q| {
        m.entry([(q.x / 2.0).floor() as i64, (q.y / 2.0).floor() as i64, (q.z / 2.0).floor() as i64]).or_insert_with(Vec::new).push(*q);
        m
    });
    let nearest = |p: Vec3| {
        let k = [(p.x / 2.0).floor() as i64, (p.y / 2.0).floor() as i64, (p.z / 2.0).floor() as i64];
        let mut best = f64::INFINITY;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for q in grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]).into_iter().flatten() {
                        best = best.min(q.dist(p));
                    }
                }
            }
        }
        best
    };
    positions_of(s, body).iter().map(|p| nearest(*p)).fold(0.0, f64::max)
}

/// Three passes, the second and third turned and moved far off: Align All Scans lays both back
/// on the part, the first held; one undo takes both moves back.
#[test]
fn three_passes_are_laid_together_in_one_undo_step() {
    let (_, _, p, t) = part();
    let mut s = Session::default();
    let a = insert(&mut s, &p, &seen(&p, &t, Vec3::new(0.6, -0.5, 0.6)), "a");
    let axis = Vec3::new(-0.4, 0.2, 0.9).normalized().unwrap();
    let b = insert(&mut s, &turned(&p, axis, 2.1, Vec3::new(120.0, -40.0, 15.0)), &seen(&p, &t, Vec3::new(-0.6, -0.4, 0.7)), "b");
    let c = insert(&mut s, &turned(&p, axis, -1.3, Vec3::new(-90.0, 60.0, 5.0)), &seen(&p, &t, Vec3::new(0.1, 0.7, 0.7)), "c");
    let before_b = positions_of(&s, &b);
    let r = run(&mut s, "scan.align_all", json!({}));
    let passes = r["passes"].as_array().unwrap();
    assert_eq!(passes[0]["held"], true);
    assert!(passes.iter().skip(1).all(|q| q["moved"] == true && q["placed"] == true), "{r}");
    for body in [&b, &c] {
        let off = worst_off(&s, body, &p);
        assert!(off < 0.05, "{body} lands {off} mm off: {r}");
    }
    assert!(worst_off(&s, &a, &p) < 1e-4, "the first stays (as the file rounds it)");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(positions_of(&s, &b), before_b, "one undo puts every pass back");
}

/// Combine Scans in one step: the second and third passes laid on the first, then all three fused
/// into the first as one surface. The first stays where it is, the others stay as they were laid,
/// and one undo puts every pass back and the first's triangles with it.
#[test]
fn three_passes_are_combined_into_one_scan_in_one_undo_step() {
    let (_, _, p, t) = part();
    let mut s = Session::default();
    let a = insert(&mut s, &p, &seen(&p, &t, Vec3::new(0.6, -0.5, 0.6)), "a");
    let axis = Vec3::new(-0.4, 0.2, 0.9).normalized().unwrap();
    let b = insert(&mut s, &turned(&p, axis, 2.1, Vec3::new(120.0, -40.0, 15.0)), &seen(&p, &t, Vec3::new(-0.6, -0.4, 0.7)), "b");
    let c = insert(&mut s, &turned(&p, axis, -1.3, Vec3::new(-90.0, 60.0, 5.0)), &seen(&p, &t, Vec3::new(0.1, 0.7, 0.7)), "c");
    let (before_a, before_b) = (positions_of(&s, &a), positions_of(&s, &b));
    let r = run(&mut s, "scan.combine", json!({}));
    assert_eq!(r["body"], json!(a), "{r}");
    assert_eq!(r["with"], json!([b, c]), "{r}");
    assert_eq!(r["aligned"]["passes"][0]["held"], true, "{r}");
    assert!(r["fused"]["triangles"].as_u64().unwrap() > 0, "{r}");
    assert!(r["fused"]["new_rms"].as_f64().unwrap() < 0.2, "the fused surface keeps to the passes: {r}");
    for body in [&b, &c] {
        assert!(worst_off(&s, body, &p) < 0.05, "{body} was laid on the part: {r}");
        assert!(!s.body_visible(body), "{body}, fused in, is hidden: one scan shows");
    }
    assert!(s.body_visible(&a), "the scan they are fused into shows");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.body_visible(&b) && s.body_visible(&c), "one undo shows the passes again");
    assert_eq!(positions_of(&s, &b), before_b, "one undo puts every pass back");
    assert_eq!(positions_of(&s, &a), before_a, "and the first's triangles");
}

/// Combine Scans with `align` false skips the alignment, and says in words what it cannot do.
#[test]
fn combine_scans_can_skip_the_alignment_and_refuses_one_scan() {
    let (_, _, p, t) = part();
    let mut s = Session::default();
    insert(&mut s, &p, &seen(&p, &t, Vec3::new(0.6, -0.5, 0.6)), "a");
    let alone = s.execute("scan.combine", &json!({})).unwrap_err().to_string();
    assert!(alone.contains("two scans or more"), "{alone}");
    insert(&mut s, &p, &seen(&p, &t, Vec3::new(-0.6, -0.4, 0.7)), "b");
    let r = run(&mut s, "scan.combine", json!({"align": false}));
    assert!(r.get("aligned").is_none(), "{r}");
    assert!(r["fused"]["triangles"].as_u64().unwrap() > 0, "{r}");
}

/// Four points on a moved scan picked against their places on the part's own scan: the points'
/// move lays it within the picks' error, and the fit after lays it on.
#[test]
fn points_picked_in_pairs_lay_the_scan_and_the_fit_finishes_it() {
    let (_, _, p, t) = part();
    let mut s = Session::default();
    let reference = insert(&mut s, &p, &t, "reference");
    let axis = Vec3::new(0.3, 0.8, -0.2).normalized().unwrap();
    let moved = turned(&p, axis, 0.9, Vec3::new(30.0, 10.0, -20.0));
    let scan = insert(&mut s, &moved, &t, "moved");
    let corners = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(60.0, 0.0, 0.0), Vec3::new(60.0, 40.0, 20.0), Vec3::new(12.0, 10.0, 32.0)];
    let on_moved = turned(&corners, axis, 0.9, Vec3::new(30.0, 10.0, -20.0));
    // Picked a little off, as by hand.
    let onto: Vec<Vec3> = corners.iter().enumerate().map(|(i, q)| *q + Vec3::new(0.2 * (i as f64 - 1.5), 0.15, -0.1)).collect();
    let pts = |v: &[Vec3]| v.iter().map(|q| json!([q.x, q.y, q.z])).collect::<Vec<_>>();
    let r = run(&mut s, "scan.align_pairs", json!({"body": scan, "from": pts(&on_moved), "onto": pts(&onto), "to": reference}));
    assert!(r["rms"].as_f64().unwrap() > 0.05, "the picks were off: {r}");
    assert!(r["refined_rms"].as_f64().unwrap() < 0.01, "the fit lays it on: {r}");
    assert!(worst_off(&s, &scan, &p) < 0.05);
    let e = s.execute("scan.align_pairs", &json!({"body": scan, "from": pts(&on_moved[..2]), "onto": pts(&onto[..2])})).unwrap_err().to_string();
    assert!(e.contains("three pairs"), "{e}");
}

/// A scan of the part moved off is laid onto the part's own solid; the solid stays.
#[test]
fn a_scan_is_laid_onto_its_model() {
    let (mut s, body, p, t) = part();
    let axis = Vec3::new(0.1, -0.3, 0.95).normalized().unwrap();
    let scan = insert(&mut s, &turned(&p, axis, 0.6, Vec3::new(8.0, -5.0, 3.0)), &seen(&p, &t, Vec3::new(0.2, -0.3, 0.9)), "scan");
    let r = run(&mut s, "scan.align_to_model", json!({"body": scan}));
    assert_eq!(r["model"], body.as_str());
    assert!(r["moved"] == true && r["rms_after"].as_f64().unwrap() < 0.01, "{r}");
    assert!(worst_off(&s, &scan, &p) < 0.05);
    let e = s.execute("scan.align_to_model", &json!({"body": scan, "model": scan})).unwrap_err().to_string();
    assert!(e.contains("is the scan to move"), "{e}");
}

/// The model may be a mesh when it is the CAD saved as one (its corners on faces drawn exactly
/// flat): a customer's STL is laid onto as a solid is. A real scan (noise on every point) named as
/// the model is still refused, with Align to Scan named.
#[test]
fn a_scan_is_laid_onto_a_model_that_came_as_a_mesh() {
    let (_, _, p, t) = part();
    let mut s = Session::default();
    let cad = insert(&mut s, &p, &t, "cad");
    // The scan: the part seen from one side, turned and moved off, a little noise on each point.
    let axis = Vec3::new(0.1, -0.3, 0.95).normalized().unwrap();
    let noisy: Vec<Vec3> = turned(&p, axis, 0.6, Vec3::new(8.0, -5.0, 3.0))
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let wave = |k: f64| 0.004 * (k * i as f64).sin();
            *q + Vec3::new(wave(12.9898), wave(78.233), wave(37.719))
        })
        .collect();
    let scan = insert(&mut s, &noisy, &seen(&p, &t, Vec3::new(0.2, -0.3, 0.9)), "scan");
    let r = run(&mut s, "scan.align_to_model", json!({"body": scan, "model": cad}));
    assert_eq!(r["model"], cad.as_str());
    assert!(r["moved"] == true && r["rms_after"].as_f64().unwrap() < 0.02, "{r}");
    assert!(worst_off(&s, &scan, &p) < 0.1, "{}", worst_off(&s, &scan, &p));
    let e = s.execute("scan.align_to_model", &json!({"body": cad, "model": scan})).unwrap_err().to_string();
    assert!(e.contains("is a scan") && e.contains("Align to Scan"), "{e}");
}

/// A block 50 × 30 × 10 meshed finely, turned and moved off the origin, as a scan of a part that
/// was not set down square.
fn tilted_block(s: &mut Session) -> (String, impl Fn(Vec3) -> Vec3 + use<>) {
    let (positions, triangles) = solidcraft_scan_test_block();
    let (axis, angle, shift) = (Vec3::new(1.0, 2.0, 0.5).normalized().unwrap(), 0.6, Vec3::new(40.0, -25.0, 17.0));
    let moved = turned(&positions, axis, angle, shift);
    let body = insert(s, &moved, &triangles, "block");
    (body, move |p: Vec3| turned(&[p], axis, angle, shift)[0])
}

fn solidcraft_scan_test_block() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (lo, hi) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(50.0, 30.0, 10.0));
    let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    let triangles =
        vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
    subdivided((0..8).map(p).collect(), triangles, 4)
}

fn box_of(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold((Vec3::new(1e9, 1e9, 1e9), Vec3::new(-1e9, -1e9, -1e9)), |(lo, hi), p| {
        (Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)), Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)))
    })
}

#[test]
fn a_scan_set_down_crooked_is_laid_in_its_datum_frame_by_three_faces() {
    let mut s = Session::default();
    let (body, place) = tilted_block(&mut s);
    // Points on the middle of the bottom (z = 0), the end (x = 0) and the side (y = 0) faces.
    let at = |x, y, z| place(Vec3::new(x, y, z));
    let planes = json!([
        {"point": [at(25.0, 15.0, 0.0).x, at(25.0, 15.0, 0.0).y, at(25.0, 15.0, 0.0).z]},
        {"point": [at(0.0, 15.0, 5.0).x, at(0.0, 15.0, 5.0).y, at(0.0, 15.0, 5.0).z]},
        {"point": [at(25.0, 0.0, 5.0).x, at(25.0, 0.0, 5.0).y, at(25.0, 0.0, 5.0).z]}
    ]);
    let r = run(&mut s, "scan.align_datum", json!({"body": body, "planes": planes}));
    assert!(r["moved"] == true && r["third_inside"] == true && r["off_square"].as_f64().unwrap() < 0.5, "{r}");
    assert!(r["planes"].as_array().unwrap().iter().all(|p| p["fitted"] == true && p["triangles"].as_u64().unwrap() > 10), "{r}");
    let (lo, hi) = box_of(&positions_of(&s, &body));
    assert!(lo.dist(Vec3::ZERO) < 0.05 && hi.dist(Vec3::new(50.0, 30.0, 10.0)) < 0.05, "{lo:?} {hi:?}");
    // Laid already, it does not move again.
    let again = run(
        &mut s,
        "scan.align_datum",
        json!({"body": body, "planes": [
        {"point": [25, 15, 0], "normal": [0, 0, 1]}, {"point": [0, 15, 5], "normal": [1, 0, 0]}, {"point": [25, 0, 5], "normal": [0, 1, 0]}]}),
    );
    assert_eq!(again["moved"], false, "{again}");
}

#[test]
fn datum_planes_given_exactly_are_refused_when_they_make_no_corner() {
    let mut s = Session::default();
    let (body, _) = tilted_block(&mut s);
    let flat = |z: f64| json!({"point": [0, 0, z], "normal": [0, 0, 1]});
    let e = s
        .execute("scan.align_datum", &json!({"body": body, "planes": [flat(0.0), flat(5.0), {"point": [0, 0, 0], "normal": [1, 0, 0]}]}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("nearly parallel"), "{e}");
    for planes in [
        json!([]),
        json!([flat(0.0)]),
        json!("x"),
        json!([flat(0.0), flat(1.0), {"point": [0, 0, 0], "normal": [0, 0, 0]}]),
        json!([{"point": [0, 0]}, flat(1.0), flat(2.0)]),
    ] {
        assert!(s.execute("scan.align_datum", &json!({"body": body, "planes": planes})).is_err(), "{planes}");
    }
}
