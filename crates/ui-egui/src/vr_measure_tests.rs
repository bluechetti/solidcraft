use serde_json::json;

use crate::vr_measure::{ends, number, words};

fn tr(k: &str) -> String {
    solidcraft_ui_text::tr(k).to_string()
}

#[test]
fn numbers_keep_three_places_at_most_and_no_trailing_zeros() {
    assert_eq!(number(29.999999999999993), "30");
    assert_eq!(number(12.5), "12.5");
    assert_eq!(number(0.12345), "0.123");
    assert_eq!(number(-0.0001), "0");
    assert_eq!(number(7.105427357601002e-15), "0");
}

#[test]
fn two_faces_say_their_distance_their_centres_and_their_angle() {
    let v = json!({"distance_mm": 83.99999999999999, "center_distance": 96.0, "angle_deg": 0.0,
                   "from": [18.0, 12.0, 40.0], "to": [102.0, 12.0, 40.0]});
    assert_eq!(words(&v), format!("{} 84 mm  ·  {} 96 mm  ·  {} 0°", tr("vr.measure_distance"), tr("vr.measure_centres"), tr("vr.measure_angle")));
    assert_eq!(ends(&v), Some(([18.0, 12.0, 40.0], [102.0, 12.0, 40.0])));
}

#[test]
fn one_face_says_its_diameter_or_its_area() {
    assert_eq!(words(&json!({"diameter": 12.0, "items": [{"area_mm2": 1130.97}]})), "⌀ 12 mm");
    assert_eq!(words(&json!({"items": [{"area_mm2": 9486.90266447077, "planar": true}]})), format!("{} 9486.903 mm²", tr("vr.measure_area")));
    assert_eq!(ends(&json!({"diameter": 12.0})), None);
    assert_eq!(ends(&json!({"from": [0.0, 1.0], "to": [0.0, 0.0, 0.0]})), None);
    assert_eq!(words(&json!({})), "");
}
