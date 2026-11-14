use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A ball of radius 20 about the origin, its poles on Z, as triangles.
fn ball_triangles() -> Vec<[Vec3; 3]> {
    let (rings, sides) = (48u32, 96u32);
    let at = |i: u32, j: u32| {
        let (a, b) = (std::f64::consts::PI * f64::from(i) / f64::from(rings), std::f64::consts::TAU * f64::from(j % sides) / f64::from(sides));
        Vec3::new(20.0 * a.sin() * b.cos(), 20.0 * a.sin() * b.sin(), 20.0 * a.cos())
    };
    let mut out = Vec::new();
    for i in 0..rings {
        for j in 0..sides {
            if i > 0 {
                out.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
            }
            if i + 1 < rings {
                out.push([at(i, j + 1), at(i + 1, j), at(i + 1, j + 1)]);
            }
        }
    }
    out
}

fn session_with(tag: &str, triangles: &[[Vec3; 3]]) -> (Session, String) {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-select-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = vec![0u8; 80];
    bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
    for t in triangles {
        bytes.extend_from_slice(&[0u8; 12]);
        for p in t {
            for v in [p.x, p.y, p.z] {
                bytes.extend_from_slice(&(v as f32).to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0u8; 2]);
    }
    let path = dir.join("scan.stl");
    std::fs::write(&path, bytes).unwrap();
    let mut s = Session::default();
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let body = s.model.state().bodies.iter().find(|b| b.body.is_mesh()).unwrap().name.clone();
    (s, body)
}

/// On a ball: a brush on its top, a region (all of a smooth ball), grown and shrunk, inverted,
/// none; kept in the design, opened again after it was changed, and deleted, each keep and delete
/// one undo step and the selecting none.
#[test]
fn a_ball_is_selected_kept_and_opened_again() {
    let (mut s, body) = session_with("ball", &ball_triangles());
    let undo = s.undo.len();
    let brushed = run(&mut s, "scan.select", json!({"brush": {"points": [[0, 0, 20]], "radius": "6 mm"}}));
    let few = brushed["triangles"].as_u64().unwrap();
    assert!(few > 20 && brushed["changed"] == true && brushed["body"] == body, "{brushed}");
    let grown = run(&mut s, "scan.select", json!({"grow": 2}));
    assert!(grown["triangles"].as_u64().unwrap() > few, "{grown}");
    let shrunk = run(&mut s, "scan.select", json!({"shrink": 2}));
    assert!(shrunk["triangles"].as_u64().unwrap() >= few, "{shrunk}");
    let inverted = run(&mut s, "scan.select", json!({"invert": true}));
    assert_eq!(inverted["triangles"].as_u64().unwrap() + shrunk["triangles"].as_u64().unwrap(), inverted["of"].as_u64().unwrap());
    // The region facing within 30° of the bottom: the cap round it, (1 − cos 30°) / 2 of the ball.
    let cap = run(&mut s, "scan.select", json!({"region": {"point": [0, 0, -20]}}));
    let share = cap["area"].as_f64().unwrap() / (4.0 * std::f64::consts::PI * 400.0);
    assert!((share - (1.0 - 30f64.to_radians().cos()) / 2.0).abs() < 0.01, "the cap: {share}");
    assert_eq!(s.undo.len(), undo, "selecting is not an undo step");

    run(&mut s, "scan.select", json!({"brush": {"points": [[0, 0, 20]], "radius": "6 mm"}}));
    let kept = run(&mut s, "scan.keep_selection", json!({"name": "Top"}));
    assert_eq!((kept["triangles"].as_u64(), kept["replaced"].as_bool()), (Some(few), Some(false)), "{kept}");
    assert_eq!(s.undo.len(), undo + 1, "keeping is one undo step");
    run(&mut s, "scan.select", json!({"none": true}));
    assert!(super::selected(&s).is_none());
    let opened = run(&mut s, "scan.select", json!({"saved": "Top"}));
    assert_eq!(opened["triangles"].as_u64(), Some(few));
    let added = run(&mut s, "scan.select", json!({"saved": "Top", "mode": "remove"}));
    assert_eq!(added["triangles"].as_u64(), Some(0));
    let refused = s.execute("scan.keep_selection", &json!({})).unwrap_err().to_string();
    assert!(refused.contains("nothing is selected"), "{refused}");
    run(&mut s, "scan.select", json!({"all": true}));
    let again = run(&mut s, "scan.keep_selection", json!({}));
    assert_eq!(again["name"], "Selection 1", "{again}");
    run(&mut s, "scan.delete_selection", json!({"name": "Top"}));
    assert!(s.execute("scan.select", &json!({"saved": "Top"})).is_err(), "deleted");
    run(&mut s, "edit.undo", json!({}));
    assert!(s.doc.scan_selections.iter().any(|k| k.name == "Top"), "the delete undone");
    assert!(s.execute("scan.select", &json!({"brush": {"points": [[0, 0, 20]], "radius": "6 mm"}, "all": true})).is_err(), "one way at a time");
    assert!(s.execute("scan.select", &json!({"triangles": [99_999_999]})).is_err(), "no such triangle");
}

/// With two scans, what is selected on one is the region every scan tool takes: Flatten and Fit
/// Patches work on that scan with no `body`, a kept selection is a region for both (`saved`) after
/// the selection is gone, and a clean tool given no scan cleans the one selected on.
#[test]
fn a_selection_on_one_of_two_scans_is_the_region_every_tool_takes() {
    let (mut s, first) = session_with("two-scans", &ball_triangles());
    let path = std::env::temp_dir().join(format!("solidcraft-scan-select-two-scans-{}", std::process::id())).join("scan.stl");
    run(&mut s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    let second = s.model.state().bodies.iter().rev().find(|b| b.body.is_mesh()).unwrap().name.clone();
    assert_ne!(first, second);
    let top = run(&mut s, "scan.select", json!({"body": second, "brush": {"points": [[0, 0, 20]], "radius": "6 mm"}}));
    let few = top["triangles"].as_u64().unwrap();
    let flat = run(&mut s, "scan.flatten", json!({}));
    assert_eq!((flat["body"].as_str(), flat["triangles"].as_u64()), (Some(second.as_str()), Some(few)), "{flat}");
    run(&mut s, "scan.keep_selection", json!({"name": "Top"}));
    run(&mut s, "scan.select", json!({"none": true}));
    let again = run(&mut s, "scan.flatten", json!({"saved": "Top"}));
    assert_eq!((again["body"].as_str(), again["triangles"].as_u64()), (Some(second.as_str()), Some(few)), "{again}");
    let patches = run(&mut s, "scan.fit_patches", json!({"saved": "Top", "controls": 4}));
    assert_eq!(patches["body"].as_str(), Some(second.as_str()), "{patches}");
    assert!(s.execute("scan.flatten", &json!({"saved": "Bottom"})).is_err(), "no such kept selection");
    run(&mut s, "scan.select", json!({"saved": "Top"}));
    let reduced = run(&mut s, "scan.reduce", json!({"ratio": 0.5}));
    assert_eq!(reduced["body"].as_str(), Some(second.as_str()), "{reduced}");
}

/// A circle drawn round the top of the ball with Curve on Scan selects the cap inside it, and
/// Curve from Selection Edge gives back a closed curve round the cap, one undo step.
#[test]
fn inside_a_curve_and_the_curve_round_a_selection() {
    let (mut s, _) = session_with("inside", &ball_triangles());
    let (r, z) = (20.0 * 40f64.to_radians().sin(), 20.0 * 40f64.to_radians().cos());
    let round: Vec<[f64; 3]> = (0..8)
        .map(|k| {
            let a = std::f64::consts::TAU * f64::from(k) / 8.0;
            [r * a.cos(), r * a.sin(), z]
        })
        .collect();
    run(&mut s, "scan.curve_on_scan", json!({"points": round, "closed": true}));
    let inside = run(&mut s, "scan.select_inside_curve", json!({}));
    let cap = inside["triangles"].as_u64().unwrap() as f64;
    let of = inside["of"].as_u64().unwrap() as f64;
    // The cap 40° round the pole is (1 − cos 40°) / 2 of the ball's area.
    let share = inside["area"].as_f64().unwrap() / (4.0 * std::f64::consts::PI * 400.0);
    assert!((share - (1.0 - 40f64.to_radians().cos()) / 2.0).abs() < 0.01, "the cap: {share} of the ball, {cap} of {of} triangles");
    let k = super::selected(&s).unwrap();
    let flags = k.selected.clone();
    drop(k);
    let state = s.model.state();
    let (positions, triangles) = state.bodies.iter().find(|b| b.body.is_mesh()).unwrap().body.triangle_mesh().unwrap();
    for (t, tri) in triangles.iter().enumerate() {
        let middle = tri.iter().fold(Vec3::ZERO, |a, &v| a + positions[v as usize]) / 3.0;
        if middle.z < 0.0 {
            assert!(!flags[t], "nothing of the far side");
        }
    }
    drop(state);
    let undo = s.undo.len();
    let made = run(&mut s, "scan.curve_from_selection", json!({}));
    assert_eq!(made["curves"].as_array().map(Vec::len), Some(1), "{made}");
    // Within the scan's own tolerance: its typical triangle side, no coarser than a thousandth of its size.
    assert!(made["max"].as_f64().unwrap() <= 0.07, "{made}");
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    let refused = s.execute("scan.select_inside_curve", &json!({"curves": ["nope"]})).unwrap_err().to_string();
    assert!(refused.contains("nope"), "{refused}");
}

/// A ball scanned, and a solid built from it that lacks its top (cut off above z = 15) and has
/// a block on its side the scan does not: what the solid leaves unexplained is the top cap,
/// largest and `missing`, then the patch of the ball inside the block, `extra`; each selected by
/// its number. The whole ball as a solid explains the whole scan: nothing is selected.
#[test]
fn what_a_solid_leaves_unexplained_is_selected_piece_by_piece() {
    let (mut s, body) = session_with("unexplained", &ball_triangles());
    run(&mut s, "solid.sphere", json!({"radius": 20, "operation": "new", "body_name": "Built"}));
    run(&mut s, "solid.box", json!({"length": 50, "width": 50, "height": 20, "center": [0, 0, 25], "operation": "cut", "targets": ["Built"]}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "center": [20, 0, 0], "operation": "join", "targets": ["Built"]}));
    let undo = s.undo.len();
    let r = run(&mut s, "scan.select", json!({"body": body, "unexplained": "Built"}));
    let pieces = r["pieces"].as_array().unwrap();
    assert_eq!(pieces.len(), 2, "{r}");
    assert_eq!(pieces[0]["side"], "missing", "{r}");
    assert!(pieces[0]["point"][2].as_f64().unwrap() > 15.0, "the top cap: {r}");
    assert!(pieces[0]["deepest"].as_f64().unwrap() > 4.0, "the pole is 5 mm above the cut: {r}");
    assert_eq!(pieces[1]["side"], "extra", "{r}");
    assert!(pieces[1]["point"][0].as_f64().unwrap() > 15.0, "inside the block: {r}");
    assert_eq!(r["limit"], 0.5);
    assert_eq!(r["triangles"], pieces[0]["triangles"], "the largest piece is selected");
    let cap = (s.scan_selection.as_ref().unwrap().selected).clone();
    let tri = s.model.state().body(&body).unwrap().body.triangle_mesh().unwrap().0.to_vec();
    let mesh = s.model.state().body(&body).unwrap().body.triangle_mesh().unwrap().1.to_vec();
    for (t, on) in cap.iter().enumerate() {
        if *on {
            assert!(mesh[t].iter().all(|&v| tri[v as usize].z > 14.0), "on the cap");
        }
    }
    let second = run(&mut s, "scan.select", json!({"unexplained": {"body": "Built", "piece": 1}}));
    assert_eq!(second["triangles"], pieces[1]["triangles"], "{second}");
    let grown = run(&mut s, "scan.select", json!({"unexplained": {"body": "Built", "piece": 1, "grow": 2}}));
    assert!(grown["triangles"].as_u64().unwrap() > second["triangles"].as_u64().unwrap(), "{grown}");
    assert!(s.execute("scan.select", &json!({"unexplained": {"body": "Built", "piece": 2}})).is_err(), "only two pieces");
    assert!(s.execute("scan.select", &json!({"unexplained": body})).is_err(), "the scan is no solid");
    assert!(s.execute("scan.select", &json!({"unexplained": "Nothing"})).is_err());
    assert_eq!(s.undo.len(), undo, "selecting is no undo step");
    run(&mut s, "solid.sphere", json!({"radius": 20, "operation": "new", "body_name": "Whole"}));
    let whole = run(&mut s, "scan.select", json!({"body": body, "unexplained": "Whole"}));
    assert_eq!(whole["pieces"], json!([]), "{whole}");
    assert_eq!(whole["triangles"], 0);
}
