use serde_json::json;

use crate::vr_joints::{Axis, driven, past_limit, wanted};

fn hinge() -> Axis {
    Axis {
        joint: 10,
        name: "Hinge".into(),
        index: 0,
        origin: [50.0, 60.0, 20.0],
        axis: [-1.0, 0.0, 0.0],
        angular: true,
        scale: 1.0,
        value: 0.0,
        limits: Some((0.0, 110f64.to_radians())),
    }
}

#[test]
fn a_drag_adds_its_motion_over_the_scale_and_stops_at_the_limits() {
    let h = hinge();
    assert!((wanted(&h, 0.5) - 0.5).abs() < 1e-12);
    assert_eq!(wanted(&h, -1.0), 0.0);
    assert!((wanted(&h, 10.0) - 110f64.to_radians()).abs() < 1e-12);
    let slider = Axis { angular: false, scale: 2.0, value: 5.0, limits: None, ..hinge() };
    assert_eq!(wanted(&slider, 10.0), 10.0);
    let stuck = Axis { scale: 0.0, ..slider.clone() };
    assert_eq!(wanted(&stuck, 10.0), 5.0);
    assert!(past_limit(&h, 10.0) && past_limit(&h, -0.1));
    assert!(!past_limit(&h, 1.0) && !past_limit(&slider, 1e9));
}

#[test]
fn the_reply_says_the_value_and_what_held_it() {
    let reply = json!({"values": [{"name": "angle", "value": 110.0, "unit": "deg"}], "held": [{"name": "angle", "why": "limit"}]});
    let d = driven(&reply, &hinge()).expect("driven");
    assert_eq!((d.value, d.unit.as_str(), d.held.as_deref()), (110.0, "deg", Some("limit")));
    let free = json!({"values": [{"name": "angle", "value": 30.0, "unit": "deg"}]});
    assert_eq!(driven(&free, &hinge()).and_then(|d| d.held), None);
    assert!(driven(&json!({}), &hinge()).is_none());
}

#[test]
fn clearances_overlaps_are_read_with_their_boxes() {
    use crate::vr_joints::clashes_of;
    let reply = json!({"overlaps": [
        {"a": "Body1", "b": "Body3", "bbox": {"max": [55.0, 35.0, 20.0], "min": [45.0, 25.0, 16.0]}, "volume_mm3": 314.16},
        {"a": "Body2", "b": "Body3", "bbox": {"max": [55.0, 35.0], "min": [45.0, 25.0, 20.0]}, "volume_mm3": 471.2}]});
    let c = clashes_of(&reply);
    assert_eq!(c.len(), 1, "a box without three coordinates is left out");
    assert_eq!((c[0].a.as_str(), c[0].b.as_str(), c[0].bbox, c[0].volume), ("Body1", "Body3", [[45.0, 25.0, 16.0], [55.0, 35.0, 20.0]], 314.16));
    assert!(clashes_of(&json!({})).is_empty());
}

#[test]
fn a_pin_in_a_hole_is_round_and_a_corner_overlap_is_not() {
    use crate::vr_joints::{Clash, round_axis};
    let clash = |bbox, volume| Clash { a: "A".into(), b: "B".into(), bbox, volume };
    // A 10 mm pin 4 mm deep: 314.16 mm³ in a 10 x 10 x 4 box, along Z.
    assert_eq!(round_axis(&clash([[45.0, 25.0, 16.0], [55.0, 35.0, 20.0]], 314.159)), Some(2));
    assert_eq!(round_axis(&clash([[0.0; 3], [10.0, 10.0, 4.0]], 400.0)), None, "a full box");
    assert_eq!(round_axis(&clash([[0.0; 3], [10.0, 20.0, 4.0]], 628.3)), None, "not square across");
    assert_eq!(round_axis(&clash([[0.0; 3], [0.0; 3]], 1.0)), None);
}
