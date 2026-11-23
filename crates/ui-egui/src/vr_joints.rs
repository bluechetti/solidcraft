//! Joints in the VR view (docs/vr.md): a placed part held with Select moves as its joints allow.
//! Its axes are the ones the desktop's Move shows ([`solidcraft_engine::doc::joints::motion_axes`]),
//! and each frame of the drag drives the joint (`joint.drive`, continuous) as the desktop's joint
//! handles do: the whole drag is one undo step, the limits clamp it, and what held a value back (a
//! limit, contact) is said, so the controller can buzz.

use serde_json::{Value, json};
use solidcraft_engine::doc::joints;

use crate::SolveApp;

/// One way a placed part can move: a joint's value, in plain numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub joint: u64,
    pub name: String,
    pub index: usize,
    /// A point on the axis and its way (model units, world), turning (`angular`) or sliding.
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    pub angular: bool,
    /// The part's motion (mm or radians) per unit of the joint's value (mm or radians).
    pub scale: f64,
    /// The value when the drag began, and its limits, in the joint's own units (mm or radians).
    pub value: f64,
    pub limits: Option<(f64, f64)>,
}

/// The placed part an occurrence path ends in.
pub fn occurrence_of(part: &crate::vr_pick::Part) -> Option<u64> {
    match &part.sel {
        solidcraft_engine::Sel::Occurrence { path } => path.last().copied(),
        _ => None,
    }
}

/// The ways `occurrence` can move by its joints, as the desktop's Move shows them.
pub fn axes(app: &SolveApp, occurrence: u64) -> Vec<Axis> {
    let doc = &app.session.doc;
    let values = joints::joint_values(&doc.assembly);
    joints::motion_axes(doc, &app.session.model.state(), occurrence)
        .into_iter()
        .filter_map(|a| {
            let joint = doc.assembly.joints.iter().find(|j| j.id == a.joint)?;
            let value = values.get(&a.joint).and_then(|v| v.get(a.index)).copied()?;
            Some(Axis {
                joint: a.joint,
                name: joint.name.clone(),
                index: a.index,
                origin: [a.origin.x, a.origin.y, a.origin.z],
                axis: [a.axis.x, a.axis.y, a.axis.z],
                angular: a.angular,
                scale: a.scale,
                value,
                limits: joint.limits.get(a.index).copied().flatten().map(|(lo, hi)| (lo.min(hi), lo.max(hi))),
            })
        })
        .collect()
}

/// What a drive did: the value it reached (degrees or mm), and what held it back, if anything.
#[derive(Clone, Debug, PartialEq)]
pub struct Driven {
    pub value: f64,
    pub unit: String,
    pub held: Option<String>,
}

/// The value a drag of `amount` (the part's motion: mm or radians) gives `axis`, clamped to its
/// limits, in the joint's own units.
pub fn wanted(axis: &Axis, amount: f64) -> f64 {
    let v = axis.value + if axis.scale.abs() > 1e-12 { amount / axis.scale } else { 0.0 };
    match axis.limits {
        Some((lo, hi)) => v.clamp(lo, hi),
        None => v,
    }
}

/// Whether a drag of `amount` asks for more than `axis`'s limits allow: [`wanted`] holds it there,
/// so the drive is never asked past them and cannot say so itself.
pub fn past_limit(axis: &Axis, amount: f64) -> bool {
    let v = axis.value + if axis.scale.abs() > 1e-12 { amount / axis.scale } else { 0.0 };
    axis.limits.is_some_and(|(lo, hi)| v < lo - 1e-12 || v > hi + 1e-12)
}

/// What the reply of `joint.drive` says of `axis`: its value now and what held it.
pub fn driven(reply: &Value, axis: &Axis) -> Option<Driven> {
    let v = reply["values"].get(axis.index)?;
    let held = reply["held"].as_array().and_then(|h| h.first()).and_then(|h| h["why"].as_str()).map(str::to_string);
    Some(Driven { value: v["value"].as_f64()?, unit: v["unit"].as_str().unwrap_or_default().to_string(), held })
}

/// Drive `axis` to `value` (its own units) as one frame of a drag whose undo history began at
/// `base` steps: the drag stays one undo step, its first.
pub fn drive(app: &mut SolveApp, axis: &Axis, value: f64, base: usize) -> Result<Option<Driven>, String> {
    let shown = if axis.angular { value.to_degrees() } else { value };
    let reply = app.run("joint.drive", json!({"joint": axis.joint, "index": axis.index, "value": shown, "quiet": true, "continuous": true}))?;
    if app.session.undo.len() > base + 1 {
        app.session.undo.truncate(base + 1);
    }
    Ok(driven(&reply, axis))
}

/// Two bodies that overlap: their names, the overlap's box (model units) and its volume (mm³).
#[derive(Clone, Debug, PartialEq)]
pub struct Clash {
    pub a: String,
    pub b: String,
    pub bbox: [[f64; 3]; 2],
    pub volume: f64,
}

/// The overlaps `inspect.clearance` finds, from its reply.
pub fn clashes_of(reply: &Value) -> Vec<Clash> {
    let p = |v: &Value| -> Option<[f64; 3]> {
        let x: Vec<f64> = v.as_array()?.iter().filter_map(Value::as_f64).collect();
        (x.len() == 3).then(|| [x[0], x[1], x[2]])
    };
    reply["overlaps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            Some(Clash {
                a: o["a"].as_str()?.to_string(),
                b: o["b"].as_str()?.to_string(),
                bbox: [p(&o["bbox"]["min"])?, p(&o["bbox"]["max"])?],
                volume: o["volume_mm3"].as_f64().unwrap_or(0.0),
            })
        })
        .collect()
}

/// Every overlap between the design's solid bodies now.
pub fn clashes(app: &mut SolveApp) -> Result<Vec<Clash>, String> {
    let reply = app.session.execute("inspect.clearance", &json!({})).map_err(|e| e.to_string())?;
    Ok(clashes_of(&reply))
}

/// What a joint's sweep found: how far it goes clear each way (the joint's units, degrees or mm)
/// and where it first touches, if it does (world point).
#[derive(Clone, Debug, PartialEq)]
pub struct Swept {
    pub reached: [f64; 2],
    pub touches: Option<(String, [f64; 3])>,
}

/// `axis`'s joint swept from its value to each end of its travel (its limits, else a turn or
/// 100 mm each way), stopping at the first contact; the design is left as it was.
pub fn sweep(app: &mut SolveApp, axis: &Axis) -> Result<Swept, String> {
    let to_user = |v: f64| if axis.angular { v.to_degrees() } else { v };
    let now = to_user(axis.value);
    let (lo, hi) = match axis.limits {
        Some((lo, hi)) => (to_user(lo), to_user(hi)),
        None if axis.angular => (now - 360.0, now + 360.0),
        None => (now - 100.0, now + 100.0),
    };
    let mut reached = [lo, hi];
    let mut touches = None;
    for (k, end) in [lo, hi].into_iter().enumerate() {
        if (end - now).abs() < 1e-9 {
            reached[k] = now;
            continue;
        }
        let reply = app
            .session
            .execute("joint.sweep", &json!({"joint": axis.joint, "index": axis.index, "from": now, "to": end}))
            .map_err(|e| e.to_string())?;
        if let Some(stop) = reply["stopped_at"].as_f64() {
            reached[k] = stop;
            if touches.is_none()
                && let Some(by) = reply["stopped_by"].as_object()
            {
                let at: Vec<f64> = by.get("at").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_f64).collect();
                let names = [by.get("a"), by.get("b")].into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(" – ");
                if at.len() == 3 {
                    touches = Some((names, [at[0], at[1], at[2]]));
                }
            }
        }
    }
    Ok(Swept { reached, touches })
}
