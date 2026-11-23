//! Measuring in the VR view (docs/vr.md): one face pointed at says its own size (a round face's
//! diameter, a flat face's area); a second says the exact distance between the two (and between
//! centres for round faces, and the angle), from `inspect.measure` on the exact faces at the
//! points the rays stopped at, as the desktop's Measure gives them.

use serde_json::{Value, json};

use crate::SolveApp;

/// A face picked: its body and the point on it (model units).
pub type Pick = (String, [f64; 3]);

/// `inspect.measure` on one picked face, or two.
pub fn measure(app: &mut SolveApp, first: &Pick, second: Option<&Pick>) -> Result<Value, String> {
    let item = |(body, point): &Pick| json!({"type": "face", "body": body, "point": point});
    let mut items = vec![item(first)];
    items.extend(second.map(item));
    app.session.execute("inspect.measure", &json!({"items": items})).map_err(|e| e.to_string())
}

/// A number as the sign shows it: three places at most, no trailing zeros.
pub fn number(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// What the sign says of a measurement.
pub fn words(v: &Value) -> String {
    let tr = |k: &str| solidcraft_ui_text::tr(k).to_string();
    let mut out = Vec::new();
    if let Some(d) = v["distance_mm"].as_f64() {
        out.push(format!("{} {} mm", tr("vr.measure_distance"), number(d)));
        if let Some(c) = v["center_distance"].as_f64() {
            out.push(format!("{} {} mm", tr("vr.measure_centres"), number(c)));
        }
        if let Some(a) = v["angle_deg"].as_f64() {
            out.push(format!("{} {}°", tr("vr.measure_angle"), number(a)));
        }
    } else if let Some(d) = v["diameter"].as_f64() {
        out.push(format!("⌀ {} mm", number(d)));
    } else if let Some(a) = v["items"][0]["area_mm2"].as_f64() {
        out.push(format!("{} {} mm²", tr("vr.measure_area"), number(a)));
    }
    out.join("  ·  ")
}

/// The two nearest points of a two-face measurement (model units), for the line between them.
pub fn ends(v: &Value) -> Option<([f64; 3], [f64; 3])> {
    let p = |k: &str| -> Option<[f64; 3]> {
        let a = v[k].as_array()?;
        let x: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
        (x.len() == 3 && x.iter().all(|v| v.is_finite())).then(|| [x[0], x[1], x[2]])
    };
    Some((p("from")?, p("to")?))
}
