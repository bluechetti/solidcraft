use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// A 60 x 40 x 10 block with a plain hole through it, a blind hole 4 deep and a counterbored hole
/// through it: three holes numbered along X, the counterbore read from its wide end as one hole of
/// two diameters, and the table written as CSV.
#[test]
fn the_hole_table_reads_through_blind_and_counterbored_holes() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 60, "width": 40, "height": 10}));
    run(&mut s, "solid.hole", json!({"position": [10, 10, 10], "diameter": 6}));
    run(&mut s, "solid.hole", json!({"position": [30, 10, 10], "diameter": 5, "depth": 4}));
    run(&mut s, "solid.hole", json!({"position": [50, 10, 10], "diameter": 6, "type": "counterbore", "cb_diameter": 10, "cb_depth": 3}));
    let path = std::env::temp_dir().join(format!("solidcraft-hole-table-{}.csv", std::process::id()));
    let reply = run(&mut s, "inspect.hole_table", json!({"csv": path}));
    let holes = reply["holes"].as_array().unwrap();
    assert_eq!(holes.len(), 3, "{reply}");
    let num = |v: &serde_json::Value| v.as_f64().unwrap();

    let plain = &holes[0];
    assert_eq!((plain["hole"].as_u64(), plain["kind"].as_str()), (Some(1), Some("through")), "{plain}");
    assert!((num(&plain["steps"][0]["diameter"]) - 6.0).abs() < 1e-6 && (num(&plain["depth"]) - 10.0).abs() < 1e-6, "{plain}");
    assert!((num(&plain["at"][0]) - 10.0).abs() < 1e-6 && (num(&plain["at"][1]) - 10.0).abs() < 1e-6, "{plain}");

    let blind = &holes[1];
    assert_eq!(blind["kind"], "blind", "{blind}");
    assert!((num(&blind["at"][2]) - 10.0).abs() < 1e-6, "starts at the open top: {blind}");
    assert!(num(&blind["axis"][2]) < -0.99, "runs down into the block: {blind}");
    assert!((num(&blind["steps"][0]["diameter"]) - 5.0).abs() < 1e-6, "{blind}");
    assert!(num(&blind["depth"]) > 3.0 && num(&blind["depth"]) <= 4.0 + 1e-6, "the wall, without the drill point: {blind}");

    let counterbored = &holes[2];
    assert_eq!(counterbored["kind"], "through", "{counterbored}");
    let steps = counterbored["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2, "one hole, two diameters: {counterbored}");
    assert!(
        (num(&steps[0]["diameter"]) - 10.0).abs() < 1e-6 && (num(&steps[0]["depth"]) - 3.0).abs() < 1e-6,
        "the counterbore first: {counterbored}"
    );
    assert!((num(&steps[1]["diameter"]) - 6.0).abs() < 1e-6 && (num(&steps[1]["depth"]) - 7.0).abs() < 1e-6, "{counterbored}");
    assert!((num(&counterbored["at"][2]) - 10.0).abs() < 1e-6, "read from its wide end: {counterbored}");

    let csv = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(csv.lines().count(), 1 + 4, "a header and a line per diameter: {csv}");
    assert!(csv.lines().nth(1).is_some_and(|l| l.starts_with("1,\"Block\",through,6.0000,10.0000,1,")), "{csv}");

    let table = s.hole_table.clone().unwrap();
    assert!(table.current(&s) && table.holes.len() == 3);
    assert_eq!(run(&mut s, "inspect.hole_table", json!({"show": false}))["shown"], false);
    assert!(s.hole_table.is_none());
}

/// No holes, a missing body and a mesh body: an empty table, a refusal and a body left out.
#[test]
fn the_hole_table_refuses_and_skips_what_it_cannot_read() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"body_name": "Plain", "operation": "new", "length": 10, "width": 10, "height": 10}));
    assert_eq!(run(&mut s, "inspect.hole_table", json!({}))["holes"], json!([]));
    let err = s.execute("inspect.hole_table", &json!({"bodies": ["Nope"]})).unwrap_err().to_string();
    assert!(err.contains("no body `Nope`"), "{err}");
    let err = s.execute("inspect.hole_table", &json!({"bodies": "Plain"})).unwrap_err().to_string();
    assert!(err.contains("bodies"), "{err}");
}
