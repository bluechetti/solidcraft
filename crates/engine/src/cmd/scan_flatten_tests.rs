//! Flatten and Roll onto Scan on a sheet bent round a cylinder: it unrolls to its own size, and a
//! line drawn across the pattern rolls back onto the bent sheet.

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// A sheet 40 × 30 bent a quarter round a cylinder of radius 25 about Y, as an STL, inserted.
fn bent_scan(s: &mut Session) -> String {
    let (n, m) = (40u32, 30u32);
    let r = 25.0;
    let at = |i: u32, j: u32| {
        let a = f64::from(i) / f64::from(n) * std::f64::consts::FRAC_PI_2;
        Vec3::new(r * a.sin(), f64::from(j), r * (1.0 - a.cos()))
    };
    let mut stl = String::from("solid s\n");
    for j in 0..m {
        for i in 0..n {
            for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                stl.push_str("facet normal 0 0 0\nouter loop\n");
                for q in tri {
                    stl.push_str(&format!("vertex {} {} {}\n", q.x, q.y, q.z));
                }
                stl.push_str("endloop\nendfacet\n");
            }
        }
    }
    stl.push_str("endsolid s\n");
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-flatten-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bent.stl");
    std::fs::write(&path, stl).unwrap();
    run(s, "file.insert_mesh", json!({"path": path.to_string_lossy()}));
    s.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap()
}

#[test]
fn a_bent_sheet_flattens_and_a_line_on_the_pattern_rolls_back_onto_it() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let r = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2]}));
    // A quarter round of radius 25 is 39.27 long: the pattern is 39.27 × 30.
    assert!((r["flat_area"].as_f64().unwrap() - 39.27 * 30.0).abs() < 5.0, "{r}");
    assert!(r["strain_max"].as_f64().unwrap() < 0.02, "{r}");
    assert_eq!(r["flipped"], 0);
    assert_eq!(r["loops"], 1);
    assert!(r["lines"].as_u64().unwrap() >= 4, "the outline is drawn: {r}");
    let sketch = r["sketch"].as_u64().unwrap();
    // A line drawn across the middle of the pattern, along its length, in the pattern's sketch.
    let state = s.model.state();
    let solved = state.sketch(sketch).unwrap();
    let (lo, hi) =
        solved.sketch.curves.iter().enumerate().flat_map(|(i, _)| solved.sketch.segs(i)).fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), seg| {
            use solidcraft_geom::prelude::*;
            let (a, b) = (seg.point_at(0.0), seg.point_at(1.0));
            ([lo[0].min(a.x).min(b.x), lo[1].min(a.y).min(b.y)], [hi[0].max(a.x).max(b.x), hi[1].max(a.y).max(b.y)])
        });
    drop(state);
    let (y0, y1) = (lo[1] + 0.25 * (hi[1] - lo[1]), lo[1] + 0.75 * (hi[1] - lo[1]));
    let x = 0.5 * (lo[0] + hi[0]);
    run(&mut s, "sketch.edit", json!({"sketch": sketch}));
    run(&mut s, "sketch.line", json!({"points": [[x, y0], [x, y1]]}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "scan.roll_onto_scan", json!({"sketch": sketch}));
    assert_eq!(r["pieces"], 1, "{r}");
    assert_eq!(r["off_pattern"], 0, "{r}");
    // The rolled curve lies on the bent sheet.
    let state = s.model.state();
    let rolled = state.sketches.iter().find(|sk| sk.name == super::ROLLED_NAME).expect("Rolled Curves");
    let wire = rolled.sketch.wires.first().expect("a 3D curve");
    for q in &wire.pts {
        let off = (q.x.powi(2) + (q.z - 25.0).powi(2)).sqrt() - 25.0;
        assert!(off.abs() < 0.1, "on the cylinder: {q:?} ({off})");
    }
    // The sheet unrolls with no stretch, so the curve keeps the length the line had on the pattern.
    let length: f64 = wire.pts.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2) + (w[1].z - w[0].z).powi(2)).sqrt()).sum();
    assert!((length - (y1 - y0)).abs() < 0.02 * (y1 - y0), "{length} against {}", y1 - y0);
    drop(state);
    // Saved and opened again, the design still knows the pattern and what it was flattened from:
    // the roll needs nothing the app remembered.
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    let again = run(&mut reopened, "scan.roll_onto_scan", json!({"sketch": sketch}));
    assert_eq!((again["pieces"].as_u64(), again["off_pattern"].as_u64()), (Some(1), Some(0)), "{again}");
}

#[test]
fn a_roll_needs_a_pattern() {
    let mut s = Session::default();
    let e = s.execute("scan.roll_onto_scan", &json!({})).unwrap_err().to_string();
    assert!(e.contains("no flat pattern") || e.contains("no scan"), "{e}");
}

/// Two patterns flattened from one scan are laid apart, not one on the other, and a line drawn on
/// the second rolls back onto the scan where the second was flattened from.
#[test]
fn a_second_pattern_is_laid_clear_of_the_first_and_rolls_back_through_its_own_place() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let first = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2], "radius": 8, "name": "One"}));
    let second = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2], "radius": 8, "name": "Two"}));
    let boxes: Vec<[[f64; 2]; 2]> = s.doc.scan_flat_patterns.iter().map(|k| serde_json::from_value(k.params["box"].clone()).unwrap()).collect();
    let [a, b] = [boxes[0], boxes[1]];
    let apart = a[1][1] < b[0][1] || b[1][1] < a[0][1] || a[1][0] < b[0][0] || b[1][0] < a[0][0];
    assert!(apart, "the boxes do not overlap: {a:?} {b:?}");
    assert_ne!(first["shift"], second["shift"], "{first} {second}");
    // A line across the middle of the second pattern, drawn in its own sketch, lands on the sheet.
    let sketch = second["sketch"].as_u64().unwrap();
    let mid = [0.5 * (b[0][0] + b[1][0]), 0.5 * (b[0][1] + b[1][1])];
    run(&mut s, "sketch.edit", json!({"sketch": sketch}));
    run(&mut s, "sketch.line", json!({"points": [[mid[0] - 2.0, mid[1]], [mid[0] + 2.0, mid[1]]]}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "scan.roll_onto_scan", json!({"sketch": sketch, "pattern": sketch}));
    assert_eq!((r["pieces"].as_u64(), r["off_pattern"].as_u64()), (Some(1), Some(0)), "{r}");
}

/// A radius given as a length with its unit limits the region as a number of mm does; one that is
/// no length is refused rather than ignored.
#[test]
fn a_radius_with_its_unit_limits_the_region() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let bare = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2], "radius": 8}));
    run(&mut s, "edit.undo", json!({}));
    let unit = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2], "radius": "8 mm"}));
    assert_eq!(bare["triangles"], unit["triangles"], "{bare} {unit}");
    run(&mut s, "edit.undo", json!({}));
    let whole = run(&mut s, "scan.flatten", json!({"body": scan, "point": [10, 15, 2]}));
    assert!(whole["triangles"].as_u64() > unit["triangles"].as_u64(), "the radius limits it: {whole} {unit}");
    run(&mut s, "edit.undo", json!({}));
    let e = s.execute("scan.flatten", &json!({"body": scan, "point": [10, 15, 2], "radius": "wide"})).unwrap_err().to_string();
    assert!(e.contains("radius"), "{e}");
}

/// With part of the scan selected, Flatten unrolls that part, and remembers it by its triangles.
#[test]
fn what_is_selected_is_flattened() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let half: Vec<usize> = (0..1200).collect();
    run(&mut s, "scan.select", json!({"body": scan, "triangles": half}));
    let r = run(&mut s, "scan.flatten", json!({}));
    assert_eq!(r["triangles"], 1200, "{r}");
    let memo = &s.doc.scan_flat_patterns.last().unwrap().params;
    assert_eq!(memo["triangles"].as_array().map(Vec::len), Some(1200));
}

/// Grown from the click, the region is the part of the sheet facing within the angle of the way it
/// faces there (30° by default): about a third of a quarter round from one end, all of it at 90°.
/// A growth is not also bounded by a radius.
#[test]
fn a_region_grown_from_the_click_keeps_to_the_way_the_scan_faces_there() {
    let mut s = Session::default();
    let scan = bent_scan(&mut s);
    let grown = run(&mut s, "scan.flatten", json!({"body": scan, "point": [0.3, 15, 0], "grow": true}));
    let n = grown["triangles"].as_u64().unwrap();
    assert!((700..=900).contains(&n), "a third of 2400: {grown}");
    run(&mut s, "edit.undo", json!({}));
    let wide = run(&mut s, "scan.flatten", json!({"body": scan, "point": [0.3, 15, 0], "grow": 90}));
    assert_eq!(wide["triangles"], 2400, "{wide}");
    run(&mut s, "edit.undo", json!({}));
    let e = s.execute("scan.flatten", &json!({"body": scan, "point": [0.3, 15, 0], "grow": true, "radius": 5})).unwrap_err().to_string();
    assert!(e.contains("not both"), "{e}");
    let patches = run(&mut s, "scan.fit_patches", json!({"body": scan, "point": [0.3, 15, 0], "grow": true, "controls": 6}));
    assert!(patches["points"].as_u64().is_some_and(|p| p > 0), "{patches}");
}
