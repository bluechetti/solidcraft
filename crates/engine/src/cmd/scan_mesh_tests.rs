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

fn mesh_of(s: &Session, body: &str) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    s.model.state().body(body).and_then(|b| b.body.triangle_mesh()).map(|(p, t)| (p.to_vec(), t.to_vec())).unwrap()
}

fn volume(positions: &[Vec3], triangles: &[[u32; 3]]) -> f64 {
    triangles.iter().map(|t| positions[t[0] as usize].dot(positions[t[1] as usize].cross(positions[t[2] as usize])) / 6.0).sum()
}

/// The block with its boss remeshed at a millimetre, sharp: written back into the scan, its volume
/// kept; one undo puts the scan back.
#[test]
fn a_scan_is_remeshed_in_place_keeping_its_shape() {
    let (positions, triangles) = part_mesh();
    let mut s = Session::default();
    let scan = insert(&mut s, &positions, &triangles, "block");
    let (before, before_triangles) = mesh_of(&s, &scan);
    let expected = volume(&before, &before_triangles);
    let reply = run(&mut s, "scan.remesh", json!({"cell": "1 mm", "sharp": true}));
    assert_eq!(reply["body"], scan);
    let (after, after_triangles) = mesh_of(&s, &scan);
    assert_eq!(after_triangles.len(), reply["triangles"].as_u64().unwrap() as usize);
    assert!(after_triangles.len() < before_triangles.len(), "{} from {}", after_triangles.len(), before_triangles.len());
    let v = volume(&after, &after_triangles);
    assert!((v - expected).abs() < 0.01 * expected, "{v} against {expected}");
    assert!(reply["scan_rms"].as_f64().unwrap() < 0.1 && reply["uncovered"].as_f64().unwrap() < 0.01, "{reply}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(mesh_of(&s, &scan).1.len(), before_triangles.len());
}

/// The block with its boss made again a parameter off its surface: grown by about its area times
/// the offset, shrunk by about as much when the parameter turns negative (the step keeps the
/// expression and follows it), the reply saying how far off it is and how near that its surface
/// lies; an offset of an area is refused.
#[test]
fn a_scan_is_offset_by_a_parameter() {
    let (positions, triangles) = part_mesh();
    let mut s = Session::default();
    let scan = insert(&mut s, &positions, &triangles, "offset");
    let (before, before_triangles) = mesh_of(&s, &scan);
    let v0 = volume(&before, &before_triangles);
    let area: f64 = before_triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| before[i as usize]);
            0.5 * (b - a).cross(c - a).len()
        })
        .sum();
    run(&mut s, "parameters.add", json!({"name": "gap", "expression": "1.5 mm"}));
    let reply = run(&mut s, "scan.remesh", json!({"cell": "0.75 mm", "offset": "gap"}));
    assert!((reply["offset"].as_f64().unwrap() - 1.5).abs() < 1e-9, "{reply}");
    assert!(reply["new_rms"].as_f64().unwrap() < 0.05 && reply["new_max"].as_f64().unwrap() < 0.3, "1.5 mm off the scan: {reply}");
    let (grown, grown_triangles) = mesh_of(&s, &scan);
    let v = volume(&grown, &grown_triangles);
    assert!(v > v0 + area * 1.5 && v < v0 + area * 1.5 * 1.15, "grown: {v} from {v0}, area {area}");
    let step = s.doc.features.iter().rev().find_map(|f| match &f.kind {
        solidcraft_doc::FeatureKind::ScanClean { step, .. } => Some(step.clone()),
        _ => None,
    });
    assert!(matches!(&step, Some(solidcraft_doc::scan_clean::CleanStep::Remesh { offset: Some(e), .. }) if e.as_str() == "gap"), "{step:?}");
    run(&mut s, "parameters.change", json!({"name": "gap", "expression": "-1.5 mm"}));
    let (shrunk, shrunk_triangles) = mesh_of(&s, &scan);
    let v = volume(&shrunk, &shrunk_triangles);
    assert!(v < v0 - area * 1.5 * 0.9 && v > v0 - area * 1.5 * 1.1, "shrunk: {v} from {v0}, area {area}");
    let refused = s.execute("scan.remesh", &json!({"offset": "1 mm", "point": [15, 10, 10], "radius": 6})).unwrap_err().to_string();
    assert!(refused.contains("not an area"), "{refused}");
}

/// Two passes of the block, each of part of it and laid where they belong, fused into the first.
#[test]
fn two_passes_are_fused_into_one_scan() {
    let (positions, triangles) = part_mesh();
    let half = |keep: &dyn Fn(Vec3) -> bool| triangles.iter().copied().filter(|t| t.iter().all(|&v| keep(positions[v as usize]))).collect::<Vec<_>>();
    let (left, right) = (half(&|p| p.x <= 36.0), half(&|p| p.x >= 24.0));
    let mut s = Session::default();
    let first = insert(&mut s, &positions, &left, "left");
    let second = insert(&mut s, &positions, &right, "right");
    let reply = run(&mut s, "scan.remesh", json!({"body": first, "with": [second], "cell": "1 mm", "sharp": true}));
    assert_eq!(reply["with"][0], second);
    let (after, after_triangles) = mesh_of(&s, &first);
    let lo = after.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let hi = after.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    assert!(lo < 0.5 && hi > 59.5, "the whole block in the one scan: {lo} to {hi}");
    assert!(reply["uncovered"].as_f64().unwrap() < 0.01, "{reply}");
    assert!(!after_triangles.is_empty());
}

/// A box 30 × 20 × 10 as a scan would mesh it: each face a grid of squares `n` across, two
/// triangles each, wound outward.
fn grid_box(n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let size = [30.0, 20.0, 10.0];
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let base = positions.len() as u32;
        for j in 0..=n {
            for i in 0..=n {
                let mut p = [0.0; 3];
                p[axis] = if high { size[axis] } else { 0.0 };
                p[u] = size[u] * f64::from(i) / f64::from(n);
                p[v] = size[v] * f64::from(j) / f64::from(n);
                positions.push(Vec3::new(p[0], p[1], p[2]));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let k = base + j * (n + 1) + i;
                let (a, b, c, d) = (k, k + 1, k + n + 2, k + n + 1);
                if high {
                    triangles.extend([[a, b, c], [a, c, d]]);
                } else {
                    triangles.extend([[a, c, b], [a, d, c]]);
                }
            }
        }
    }
    (positions, triangles)
}

/// The box scanned with noise along its faces (0.05 mm on 0.6 mm triangles): Denoise smooths it
/// at the strength it suggests, in one undo step; the box with no noise is refused.
#[test]
fn a_noisy_scan_is_denoised_and_a_clean_one_left_alone() {
    let (positions, triangles) = grid_box(50);
    let mut s = Session::default();
    let clean = insert(&mut s, &positions, &triangles, "clean");
    // A clean scan needs no smoothing: no failure, nothing changed, and the reply says why.
    let features = s.doc.features.len();
    let nothing = s.execute("scan.denoise", &json!({"body": clean})).unwrap();
    assert_eq!(
        (nothing["changed"].as_bool(), nothing["strength"].as_f64(), nothing["max_moved"].as_f64()),
        (Some(false), Some(0.0), Some(0.0)),
        "{nothing}"
    );
    assert!(nothing["note"].as_str().is_some_and(|n| n.contains("as smooth as it can be read")) && nothing.get("feature").is_none(), "{nothing}");
    assert_eq!(s.doc.features.len(), features, "no step is added");
    // Asking for no smoothing at all is a mistake in the call, and stays one.
    let refused = s.execute("scan.denoise", &json!({"body": clean, "strength": 0})).unwrap_err().to_string();
    assert!(refused.contains("smooths nothing"), "{refused}");
    // Each place moved alike (its copies on the box's edges too), along the face it lies on.
    let noisy: Vec<Vec3> = positions
        .iter()
        .map(|p| {
            let h = ((p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43_758.545).fract() - 0.5;
            let on = |v: f64, s: f64| v.abs() < 1e-9 || (v - s).abs() < 1e-9;
            let n = if on(p.x, 30.0) {
                Vec3::X
            } else if on(p.y, 20.0) {
                Vec3::Y
            } else {
                Vec3::Z
            };
            *p + n * (0.1 * h)
        })
        .collect();
    let scan = insert(&mut s, &noisy, &triangles, "noisy");
    let (before, _) = mesh_of(&s, &scan);
    let reply = run(&mut s, "scan.denoise", json!({"body": scan}));
    assert!(reply["strength"].as_f64().unwrap() > 0.0 && reply["strength"] == reply["suggested"], "{reply}");
    assert!(reply["roughness"].as_f64().unwrap() < 0.6 * reply["noise"].as_f64().unwrap(), "{reply}");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(mesh_of(&s, &scan).0, before);
}

/// Remesh round a point on the box's top: the triangles there made again as even triangles of
/// the cell asked for, every vertex outside the area where it was, and the step keeps its area; an
/// area and other scans fused in together are refused.
#[test]
fn remesh_works_on_an_area() {
    let (positions, triangles) = grid_box(30);
    let mut s = Session::default();
    let scan = insert(&mut s, &positions, &triangles, "box");
    let (before, before_tris) = mesh_of(&s, &scan);
    let reply = run(&mut s, "scan.remesh", json!({"body": scan, "cell": "3 mm", "point": [15, 10, 10], "radius": 6}));
    let (after, after_tris) = mesh_of(&s, &scan);
    assert!(after_tris.len() < before_tris.len(), "coarser round the point: {} from {}: {reply}", after_tris.len(), before_tris.len());
    let centre = Vec3::new(15.0, 10.0, 10.0);
    let kept: std::collections::HashSet<[u64; 3]> = after.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect();
    assert!(
        before.iter().filter(|p| p.dist(centre) > 8.0).all(|p| kept.contains(&[p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])),
        "every vertex away from the area where it was"
    );
    let step = s.doc.features.iter().rev().find_map(|f| match &f.kind {
        solidcraft_doc::FeatureKind::ScanClean { step, .. } => Some(step.clone()),
        _ => None,
    });
    assert!(matches!(step, Some(solidcraft_doc::scan_clean::CleanStep::Remesh { area: Some(_), .. })), "{step:?}");
    run(&mut s, "edit.undo", json!({}));
    let other = insert(&mut s, &positions, &triangles, "other");
    let refused = s.execute("scan.remesh", &json!({"body": scan, "with": [other], "point": [15, 10, 10], "radius": 6})).unwrap_err().to_string();
    assert!(refused.contains("not an area"), "{refused}");
}

/// Denoise and Reduce on part of the scan: round a point on the noisy box's top, only the scan
/// there moves or is reduced, and the step keeps its area; asked for what is selected with nothing
/// selected, they say so; a selection is never taken unasked.
#[test]
fn denoise_and_reduce_work_on_an_area() {
    let (positions, triangles) = grid_box(50);
    let noisy: Vec<Vec3> = positions
        .iter()
        .map(|p| {
            let h = ((p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43_758.545).fract() - 0.5;
            *p + Vec3::new(0.0, 0.0, if (p.z - 10.0).abs() < 1e-9 { 0.1 * h } else { 0.0 })
        })
        .collect();
    let mut s = Session::default();
    let scan = insert(&mut s, &noisy, &triangles, "noisy");
    let (before, _) = mesh_of(&s, &scan);
    let centre = Vec3::new(15.0, 10.0, 10.0);
    let reply = run(&mut s, "scan.denoise", json!({"body": scan, "strength": 3, "point": [15, 10, 10], "radius": 5}));
    let (after, _) = mesh_of(&s, &scan);
    for (a, b) in before.iter().zip(&after) {
        if a.dist(centre) > 6.0 {
            assert_eq!(a, b, "outside the area nothing moves");
        }
    }
    assert!(before.iter().zip(&after).filter(|(a, b)| a != b).count() > 50, "the area smoothed: {reply}");
    let kept = s.doc.features.iter().rev().find_map(|f| match &f.kind {
        solidcraft_doc::FeatureKind::ScanClean { step, .. } => Some(step.clone()),
        _ => None,
    });
    assert!(matches!(kept, Some(solidcraft_doc::scan_clean::CleanStep::Denoise { area: Some(_), .. })), "{kept:?}");
    run(&mut s, "edit.undo", json!({}));

    let count = |s: &Session| mesh_of(s, &scan).1.len();
    let whole = count(&s);
    let reply = run(&mut s, "scan.reduce", json!({"body": scan, "ratio": 0.2, "point": [15, 10, 10], "radius": 8}));
    let removed = whole - count(&s);
    assert!(removed > 0 && removed < whole / 10, "only the area reduced: {removed} of {whole}: {reply}");
    run(&mut s, "edit.undo", json!({}));

    let refused = s.execute("scan.denoise", &json!({"body": scan, "selected": true})).unwrap_err().to_string();
    assert!(refused.contains("nothing is selected"), "{refused}");
    run(&mut s, "scan.select", json!({"body": scan, "triangles": (0..200).collect::<Vec<u32>>()}));
    // Unasked, a selection does not narrow the step.
    let reply = run(&mut s, "scan.reduce", json!({"body": scan, "ratio": 0.5}));
    assert!(reply["triangles"].as_u64().unwrap() <= whole as u64 / 2 + 1, "{reply}");
}
