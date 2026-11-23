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
