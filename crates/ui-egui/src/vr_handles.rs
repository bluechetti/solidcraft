//! Dimension handles for the VR view (docs/vr.md): an arrow on the model for each size that has an
//! obvious place there, pulled with the controller to change it. A box's length, width and height
//! (the middle of its far faces), a cylinder's height (its far end) and radius (its side), a hole's
//! diameter (its rim), and the distance of a sketch extruded into a body of its own (its far end).
//!
//! A handle drives a user parameter when the size is that parameter, or a multiple of it
//! (`post_h`, `(post_d) / 2`): the design stays driven by it, and everything else using it follows.
//! Otherwise it drives the feature's own input. What a handle is and where it stands is worked out
//! from `timeline.inspect` and the bodies' boxes, once per revision of the design.

use serde_json::{Value, json};

use crate::SolveApp;

/// One handle: what it changes and how, and where it stands on the model (model units, mm).
#[derive(Clone, Debug, PartialEq)]
pub struct Handle {
    /// What the sign says: the feature, the size and the parameter it drives (`Post1 Height (post_h)`).
    pub label: String,
    /// The parameter `parameters.change` sets.
    pub param: String,
    /// The parameter's unit, its value in it, and how many of it one millimetre of pull adds.
    pub unit: String,
    pub value: f64,
    pub per_mm: f64,
    /// Where the arrow starts, and the way it points (a unit vector): pulling that way grows it.
    pub at: [f64; 3],
    pub dir: [f64; 3],
    /// The body it is on: handles show for the parts selected.
    pub body: String,
}

/// The handles between frames, worked out again when the design changes.
#[derive(Default)]
pub struct VrHandles {
    listed: Option<(u64, u64, Vec<Handle>)>,
}

impl VrHandles {
    /// The design's handles, at its current revision.
    pub fn get(&mut self, app: &mut SolveApp) -> &[Handle] {
        let key = (app.session.uid, app.session.revision);
        if self.listed.as_ref().is_none_or(|(u, r, _)| (*u, *r) != key) {
            self.listed = Some((key.0, key.1, handles(app)));
        }
        self.listed.as_ref().map_or(&[], |(_, _, h)| h.as_slice())
    }
}

/// Millimetres in one of `unit` (a length unit), none for any other.
pub fn mm_per(unit: &str) -> Option<f64> {
    Some(match unit {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "um" => 0.001,
        "in" => 25.4,
        "ft" => 304.8,
        _ => return None,
    })
}

/// When `expression` is a multiple of one user parameter (`post_h`, `(post_d) / 2`, `2 * w`,
/// `w*0.5`): its name and the multiple. Anything else is none.
pub fn scaled_user(expression: &str, users: &[&str]) -> Option<(String, f64)> {
    let e: String = expression.chars().filter(|c| !c.is_whitespace()).collect();
    let bare = |s: &str| -> Option<String> {
        let s = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(s);
        users.iter().find(|u| **u == s).map(|u| (*u).to_string())
    };
    let number = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite() && *v != 0.0);
    if let Some(name) = bare(&e) {
        return Some((name, 1.0));
    }
    if let Some((a, b)) = e.rsplit_once('/') {
        return Some((bare(a)?, 1.0 / number(b)?));
    }
    if let Some((a, b)) = e.split_once('*') {
        return match (bare(a), number(b), number(a), bare(b)) {
            (Some(name), Some(k), _, _) | (_, _, Some(k), Some(name)) => Some((name, k)),
            _ => None,
        };
    }
    None
}

/// A parameter as the handles need it: its unit, its value in it, and whether it is the user's.
#[derive(Clone, Debug, PartialEq)]
pub struct Dim {
    pub unit: String,
    pub value: f64,
    pub user: bool,
}

/// The handles of one feature, from what `timeline.inspect` says of it (`info`), its body's box
/// (`bounds`, model units) and the design's parameters (`dims`, by name).
pub fn of_feature(info: &Value, bounds: Option<[[f64; 3]; 2]>, dims: &std::collections::HashMap<String, Dim>) -> Vec<Handle> {
    let mut out = Vec::new();
    if info["suppressed"].as_bool() == Some(true) || !info["error"].is_null() {
        return out;
    }
    let name = info["name"].as_str().unwrap_or_default();
    let d = &info["definition"];
    let body = d["body_names"].get(0).and_then(Value::as_str).unwrap_or_default().to_string();
    let users: Vec<&str> = dims.iter().filter(|(_, v)| v.user).map(|(k, _)| k.as_str()).collect();
    let input = |label: &str| info["inputs"].as_array().into_iter().flatten().find(|i| i["label"] == label);
    let mut handle = |label: &str, at: [f64; 3], dir: [f64; 3], gain: f64| {
        let Some(i) = input(label) else { return };
        let own = i["param"].as_str().unwrap_or_default();
        let expression = i["expression"].as_str().unwrap_or_default();
        let (param, multiple) = scaled_user(expression, &users).unwrap_or_else(|| (own.to_string(), 1.0));
        let Some(dim) = dims.get(&param) else { return };
        let Some(mm) = mm_per(&dim.unit) else { return };
        let label = if param == own { format!("{name} {label}") } else { format!("{name} {label} ({param})") };
        out.push(Handle { label, param, unit: dim.unit.clone(), value: dim.value, per_mm: gain / multiple / mm, at, dir, body: body.clone() });
    };
    let axis = |k: usize, sign: f64| -> [f64; 3] { std::array::from_fn(|j| if j == k { sign } else { 0.0 }) };
    let mid = bounds.map(|[lo, hi]| -> [f64; 3] { std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0) });
    let f3 = |v: &Value| -> Option<[f64; 3]> {
        let a = v.as_array()?;
        let x: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
        (x.len() == 3 && x.iter().all(|v| v.is_finite())).then(|| [x[0], x[1], x[2]])
    };
    match (info["type"].as_str(), bounds, mid) {
        (Some("box"), Some([_, hi]), Some(mid)) => {
            for (k, label) in ["Length", "Width", "Height"].into_iter().enumerate() {
                let mut at = mid;
                at[k] = hi[k];
                handle(label, at, axis(k, 1.0), 1.0);
            }
        }
        (Some("cylinder"), Some([lo, hi]), Some(mid)) => {
            let a = f3(&d["axis"]).unwrap_or([0.0, 0.0, 1.0]);
            let k = (0..3).max_by(|x, y| a[*x].abs().total_cmp(&a[*y].abs())).unwrap_or(2);
            let sign = if a[k] >= 0.0 { 1.0 } else { -1.0 };
            let mut top = mid;
            top[k] = if sign > 0.0 { hi[k] } else { lo[k] };
            handle("Height", top, axis(k, sign), 1.0);
            let side = (k + 1) % 3;
            let mut at = mid;
            at[side] = hi[side];
            handle("Radius", at, axis(side, 1.0), 1.0);
        }
        (Some("hole"), _, _) => {
            let dir = f3(&d["direction"]).unwrap_or([0.0, 0.0, -1.0]);
            let k = (0..3).max_by(|x, y| dir[*x].abs().total_cmp(&dir[*y].abs())).unwrap_or(2);
            let side = (k + 1) % 3;
            let radius =
                input("Diameter").and_then(|i| i["param"].as_str()).and_then(|p| dims.get(p)).and_then(|d| Some(d.value * mm_per(&d.unit)? / 2.0));
            if let (Some(pos), Some(r)) = (f3(&d["position"]), radius) {
                let mut at = pos;
                at[side] += r;
                handle("Diameter", at, axis(side, 1.0), 2.0);
            }
        }
        (Some("extrude"), Some([lo, hi]), Some(mid)) if d["operation"] == "new_body" => {
            let k = match d["plane_name"].as_str() {
                Some("YZ") => Some(0),
                Some("XZ") => Some(1),
                Some("XY") => Some(2),
                _ => None,
            };
            let sign = match d["extent"]["direction"].as_str() {
                Some("positive") => Some(1.0),
                Some("negative") => Some(-1.0),
                _ => None,
            };
            if let (Some(k), Some(sign), Some("total_distance")) = (k, sign, d["extent"]["convention"].as_str()) {
                let mut at = mid;
                at[k] = if sign > 0.0 { hi[k] } else { lo[k] };
                handle("Distance", at, axis(k, sign), 1.0);
            }
        }
        _ => {}
    }
    out
}

/// Every handle of the design as it is now.
fn handles(app: &mut SolveApp) -> Vec<Handle> {
    let dims: std::collections::HashMap<String, Dim> = crate::params_dialog::rows(app)
        .into_iter()
        .filter_map(|r| Some((r.name.clone(), Dim { unit: r.unit.clone(), value: r.value?, user: r.source == "user" })))
        .collect();
    let st = crate::open_meshing::shown_world(app);
    let boxes: std::collections::HashMap<String, [[f64; 3]; 2]> = crate::viewport::body_bounds(app, &st)
        .iter()
        .filter(|(_, b)| !b.is_empty())
        .map(|(name, b)| (name.clone(), [[b.min.x, b.min.y, b.min.z], [b.max.x, b.max.y, b.max.z]]))
        .collect();
    let names: Vec<String> = app.session.doc.features.iter().map(|f| f.name.clone()).collect();
    let mut out = Vec::new();
    for name in names {
        let Ok(mut info) = app.session.execute("timeline.inspect", &json!({"feature": name})) else { continue };
        // An extrude's sketch plane, when the sketch is on an origin plane.
        if info["type"] == "extrude"
            && let Some(sketch) = info["definition"]["sketch"].as_u64()
            && let Ok(s) = app.session.execute("timeline.inspect", &json!({"feature": sketch}))
            && s["definition"]["plane"]["type"] == "origin"
        {
            info["definition"]["plane_name"] = s["definition"]["plane"]["name"].clone();
        }
        let body = info["definition"]["body_names"].get(0).and_then(Value::as_str).unwrap_or_default();
        out.extend(of_feature(&info, boxes.get(body).copied(), &dims));
    }
    out
}
