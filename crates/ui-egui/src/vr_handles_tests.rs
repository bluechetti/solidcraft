use std::collections::HashMap;

use serde_json::json;

use crate::vr_handles::{Dim, mm_per, of_feature, scaled_user};

fn dims() -> HashMap<String, Dim> {
    let d = |unit: &str, value: f64, user: bool| Dim { unit: unit.into(), value, user };
    HashMap::from([
        ("base_l".into(), d("mm", 120.0, true)),
        ("base_w".into(), d("mm", 80.0, true)),
        ("post_d".into(), d("mm", 12.0, true)),
        ("post_h".into(), d("mm", 30.0, true)),
        ("d1".into(), d("mm", 120.0, false)),
        ("d2".into(), d("mm", 80.0, false)),
        ("d3".into(), d("mm", 10.0, false)),
        ("d4".into(), d("mm", 6.0, false)),
        ("d5".into(), d("mm", 30.0, false)),
        ("hd".into(), d("in", 0.25, false)),
    ])
}

#[test]
fn a_size_that_is_a_multiple_of_a_user_parameter_drives_it() {
    let users = ["post_d", "post_h", "w"];
    assert_eq!(scaled_user("post_h", &users), Some(("post_h".into(), 1.0)));
    assert_eq!(scaled_user(" (post_d) / 2 ", &users), Some(("post_d".into(), 0.5)));
    assert_eq!(scaled_user("2 * w", &users), Some(("w".into(), 2.0)));
    assert_eq!(scaled_user("w*0.5", &users), Some(("w".into(), 0.5)));
    for e in ["post_h + 1", "post_d / 0", "other", "12", "w * w", "(post_d) / x", ""] {
        assert_eq!(scaled_user(e, &users), None, "{e}");
    }
}

#[test]
fn a_box_has_three_handles_on_its_far_faces_driving_its_user_parameters() {
    let info = json!({"name": "Base", "type": "box", "error": null, "suppressed": false,
        "definition": {"body_names": ["Base"]},
        "inputs": [{"label": "Length", "param": "d1", "expression": "base_l"},
                   {"label": "Width", "param": "d2", "expression": "base_w"},
                   {"label": "Height", "param": "d3", "expression": "10"}]});
    let h = of_feature(&info, Some([[0.0, 0.0, 0.0], [120.0, 80.0, 10.0]]), &dims());
    assert_eq!(h.len(), 3);
    assert_eq!((h[0].param.as_str(), h[0].at, h[0].dir), ("base_l", [120.0, 40.0, 5.0], [1.0, 0.0, 0.0]));
    assert_eq!(h[0].label, "Base Length (base_l)");
    assert_eq!((h[2].param.as_str(), h[2].at, h[2].dir), ("d3", [60.0, 40.0, 10.0], [0.0, 0.0, 1.0]));
    assert_eq!(h[2].label, "Base Height");
    assert!(h.iter().all(|h| h.per_mm == 1.0 && h.body == "Base"));
}

#[test]
fn a_cylinders_radius_pulls_its_diameter_parameter_twice_as_fast() {
    let info = json!({"name": "Post1", "type": "cylinder", "error": null, "suppressed": false,
        "definition": {"body_names": ["Post1"], "axis": [0.0, 0.0, 1.0]},
        "inputs": [{"label": "Radius", "param": "d4", "expression": "(post_d) / 2"},
                   {"label": "Height", "param": "d5", "expression": "post_h"}]});
    let h = of_feature(&info, Some([[6.0, 6.0, 10.0], [18.0, 18.0, 40.0]]), &dims());
    let height = h.iter().find(|h| h.param == "post_h").expect("height");
    assert_eq!((height.at, height.dir, height.per_mm, height.value), ([12.0, 12.0, 40.0], [0.0, 0.0, 1.0], 1.0, 30.0));
    let radius = h.iter().find(|h| h.param == "post_d").expect("radius");
    assert_eq!((radius.at, radius.dir, radius.per_mm), ([18.0, 12.0, 25.0], [1.0, 0.0, 0.0], 2.0));
}

#[test]
fn a_hole_in_inches_pulls_its_diameter_from_the_rim() {
    let info = json!({"name": "Hole1", "type": "hole", "error": null, "suppressed": false,
        "definition": {"position": [10.0, 20.0, 5.0], "direction": [0.0, 0.0, -1.0]},
        "inputs": [{"label": "Diameter", "param": "hd", "expression": "0.25"}]});
    let h = of_feature(&info, None, &dims());
    assert_eq!(h.len(), 1);
    assert!((h[0].at[0] - (10.0 + 0.25 * 25.4 / 2.0)).abs() < 1e-9);
    assert!((h[0].per_mm - 2.0 / 25.4).abs() < 1e-12);
}

#[test]
fn a_suppressed_or_failing_feature_or_one_without_a_box_has_none() {
    let mut info = json!({"name": "Base", "type": "box", "error": null, "suppressed": true,
        "definition": {"body_names": ["Base"]},
        "inputs": [{"label": "Length", "param": "d1", "expression": "base_l"}]});
    let b = Some([[0.0; 3], [1.0; 3]]);
    assert!(of_feature(&info, b, &dims()).is_empty());
    info["suppressed"] = json!(false);
    info["error"] = json!("broken");
    assert!(of_feature(&info, b, &dims()).is_empty());
    info["error"] = json!(null);
    assert!(of_feature(&info, None, &dims()).is_empty());
    assert_eq!(of_feature(&info, b, &dims()).len(), 1);
}

#[test]
fn only_length_units_have_handles() {
    assert_eq!(mm_per("in"), Some(25.4));
    assert_eq!(mm_per("deg"), None);
    assert_eq!(mm_per(""), None);
}

#[test]
fn a_move_snaps_to_whole_millimetres_or_tenths() {
    use crate::vr_handles::snapped;
    assert_eq!(snapped([12.4, -0.6, 0.2], false), [12.0, -1.0, 0.0]);
    assert_eq!(snapped([12.44, -0.06, 0.26], true), [12.4, -0.1, 0.3]);
    assert_eq!(snapped([f64::NAN, f64::INFINITY, -0.04], true), [0.0, 0.0, 0.0]);
}

#[test]
fn a_body_moves_by_solid_move_and_a_placed_part_by_occurrence_move() {
    use crate::vr_handles::move_command;
    use crate::vr_pick::Part;
    use solidcraft_engine::Sel;
    let body = Part { sel: Sel::Body { name: "Base".into() }, name: "Base".into(), bodies: vec!["Base".into()] };
    let (c, p) = move_command(&body, [1.0, 0.0, -2.0]).expect("body");
    assert_eq!((c, p), ("solid.move", json!({"bodies": ["Base"], "translate": [1.0, 0.0, -2.0]})));
    let placed = Part { sel: Sel::Occurrence { path: vec![4, 9] }, name: "Post1:1".into(), bodies: vec!["Post1".into()] };
    let (c, p) = move_command(&placed, [0.0, 5.0, 0.0]).expect("occurrence");
    assert_eq!(c, "occurrence.move");
    assert_eq!(p, json!({"occurrence": 9, "path": [4, 9], "space": "world", "translate": [0.0, 5.0, 0.0]}));
    let empty = Part { sel: Sel::Occurrence { path: vec![] }, name: String::new(), bodies: vec![] };
    assert!(move_command(&empty, [0.0; 3]).is_none());
}
