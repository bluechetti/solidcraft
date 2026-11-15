//! What was made from a scan stays with it. A scan command that makes something (a surface
//! fitted to the scan, a solid reconstructed from it, a piece split off it, a section sketch, a
//! curve drawn on it, a fitted plane) notes the scan on each feature it adds (`Feature::made_from`).
//! A Move of the scan moves those bodies by the same move, in the same feature, and places those
//! sketches and construction features again by it, in the same undo step: they stay exactly on
//! the scan, whichever way it is laid afterwards (Align, Stand on Base, the Move tool). What is
//! built on a sketch or plane moved so (an extrude of a section) follows it, and is not moved
//! again by the Move.

use std::collections::BTreeSet;

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_doc::construct::GeoRef;
use solidcraft_doc::expr::Kind;
use solidcraft_doc::{Document, FeatureKind, Mat, PlaneRef, apply_point, apply_vector, mat_inverse, mat_mul};
use solidcraft_sketch::WireSpace;

use crate::Session;

/// The scan a scan command `id` works on, read before it runs: its `scan`, else its `body`, when
/// that names a scan, else the scan it would take with neither (the selected or only one).
/// `None` for a command that is not a scan command, or that finds no one scan.
pub(crate) fn source(s: &Session, id: &str, p: &Value) -> Option<String> {
    if !id.starts_with("scan.") {
        return None;
    }
    let state = s.model.state();
    let is_scan = |name: &str| state.body(name).is_some_and(|b| b.body.is_mesh());
    for key in ["scan", "body"] {
        if let Some(name) = p.get(key).and_then(Value::as_str).filter(|n| !n.trim().is_empty()) {
            if is_scan(name) {
                return Some(name.to_owned());
            }
            if key == "body" {
                // A command whose `body` is a model (Fillet from Scan) reads its scan elsewhere.
                break;
            }
        }
    }
    super::mesh_body_name(s, None, id).ok()
}

/// Notes `scan` on every feature the command just added (those not in `before`), except a Move
/// of the scan itself and a clean-up step of it: they change the scan, not something made from it.
pub(crate) fn note(s: &mut Session, before: &Document, scan: &str) {
    let had: BTreeSet<u64> = before.features.iter().map(|f| f.id).collect();
    let added: Vec<u64> = s
        .doc
        .features
        .iter()
        .filter(|f| !had.contains(&f.id))
        .filter(|f| !matches!(f.kind, FeatureKind::Move { .. } | FeatureKind::ScanClean { .. }))
        .filter(|f| !f.body_names.iter().any(|n| n == scan))
        .filter(|f| !f.made_from.iter().any(|n| n == scan))
        .map(|f| f.id)
        .collect();
    if added.is_empty() {
        return;
    }
    let doc = s.doc_mut();
    for id in added {
        if let Some(f) = doc.feature_mut(id) {
            f.made_from.push(scan.to_owned());
        }
    }
}

/// What a Move of scans takes along: the bodies made from them, moved in the Move with the scans,
/// and the sketches and construction planes, axes and points made from them, placed again in
/// their own features by the same move (`place`). What is built on those (an extrude of a section,
/// a sketch on a fitted plane) is left to follow them, so nothing is moved twice.
#[derive(Debug, Default)]
pub(crate) struct Carried {
    /// Bodies the Move moves besides those named, in the design's order.
    pub bodies: Vec<String>,
    /// Sketch and construction features placed again by the move, in timeline order.
    pub placed: Vec<u64>,
}

/// What a Move of `bodies` takes along (none of `bodies` itself).
pub(crate) fn carried(s: &Session, bodies: &[String]) -> Carried {
    let state = s.model.state();
    let mut sources: Vec<String> = bodies.iter().filter(|b| state.body(b).is_some_and(|m| m.body.is_mesh())).cloned().collect();
    let mut taken: BTreeSet<String> = bodies.iter().cloned().collect();
    let mut found = Vec::new();
    let mut made: BTreeSet<u64> = BTreeSet::new();
    while let Some(scan) = sources.pop() {
        for f in s.doc.features.iter().filter(|f| !f.suppressed && made_from(f, &scan)) {
            made.insert(f.id);
            for name in bodies_of(s, f) {
                if taken.insert(name.clone()) {
                    found.push(name.clone());
                    sources.push(name);
                }
            }
        }
    }
    // In timeline order: a sketch or construction made from the scans is placed again unless it
    // stands on one already moving; whatever stands on a moving one follows it by itself.
    let mut placed: Vec<u64> = Vec::new();
    let mut moving: Vec<(u64, &str)> = Vec::new();
    let mut follow: BTreeSet<String> = BTreeSet::new();
    for f in s.doc.features.iter().filter(|f| !f.suppressed) {
        let sketches = f.kind.input_sketches();
        let stands_on_moving = moving.iter().any(|(id, name)| sketches.contains(id) || f.kind.names_feature(*id, name));
        if stands_on_moving {
            moving.push((f.id, f.name.as_str()));
            follow.extend(bodies_of(s, f));
        } else if made.contains(&f.id) && placeable(s, f) {
            moving.push((f.id, f.name.as_str()));
            placed.push(f.id);
        }
    }
    found.retain(|b| !follow.contains(b));
    // In the design's order, so the Move lists them as the browser does.
    let order: Vec<&str> = state.bodies.iter().map(|b| b.name.as_str()).collect();
    found.sort_by_key(|n| order.iter().position(|o| o == n).unwrap_or(usize::MAX));
    Carried { bodies: found, placed }
}

/// The bodies `f` names or built that the model has now.
fn bodies_of(s: &Session, f: &solidcraft_doc::Feature) -> Vec<String> {
    let state = s.model.state();
    let named = f.body_names.iter().map(String::as_str);
    let built = state.bodies.iter().filter(|b| b.feature == f.id).map(|b| b.name.as_str());
    let mut out: Vec<String> = Vec::new();
    for name in named.chain(built) {
        if state.body(name).is_some() && !out.iter().any(|o| o == name) {
            out.push(name.to_owned());
        }
    }
    out
}

/// Whether `f` can be placed again by a move of its own: a sketch on a plane given by numbers (an
/// origin plane, a plane placed in space, or one offset or turned from those), or a construction
/// held by nothing but numbers (one built on the scan's faces moves with the scan already, #10127).
fn placeable(s: &Session, f: &solidcraft_doc::Feature) -> bool {
    match &f.kind {
        FeatureKind::Sketch { plane, .. } => fixed(plane),
        kind if kind.is_construction() => {
            let state = s.model.state();
            let held_by_bodies = state.construct.iter().any(|c| c.feature == f.id && !c.bodies.is_empty());
            let numbers = |refs: Vec<&GeoRef>| refs.iter().all(|g| matches!(g, GeoRef::Point { .. }));
            let by_numbers = match kind {
                FeatureKind::ConstructionPlane { plane: PlaneRef::ThreePoints { a, b, c } } => numbers(vec![&**a, &**b, &**c]),
                FeatureKind::ConstructionPlane { plane } => fixed(plane),
                FeatureKind::ConstructionAxis { def } => numbers(def.refs()),
                FeatureKind::ConstructionPoint { def } => numbers(def.refs()),
                _ => false,
            };
            !held_by_bodies && by_numbers
        }
        _ => false,
    }
}

/// Whether `plane` is given by the origin planes and numbers alone.
fn fixed(plane: &PlaneRef) -> bool {
    match plane {
        PlaneRef::Origin { .. } | PlaneRef::Custom { .. } | PlaneRef::Placed { .. } => true,
        PlaneRef::Offset { base, .. } | PlaneRef::Flipped { base, .. } => fixed(base),
        PlaneRef::AtAngle { base, line: None, pivot: None, .. } => fixed(base),
        _ => false,
    }
}

/// The move a Move feature made, in the world: its turn and shift in its component's frame,
/// carried into the world. `None` when it is not a Move or its values cannot be read.
pub(crate) fn world_move(s: &Session, feature: u64) -> Option<Mat> {
    let f = s.doc.feature(feature)?;
    let FeatureKind::Move { translate, rotate_axis, origin, angle, copy: false, .. } = &f.kind else { return None };
    let length = |e: &solidcraft_doc::Expr| s.doc.eval(e, Kind::Length).ok();
    let t = Vec3::new(length(&translate[0])?, length(&translate[1])?, length(&translate[2])?);
    let turn = match angle {
        Some(a) => s.doc.eval(a, Kind::Angle).ok()?,
        None => 0.0,
    };
    let axis = rotate_axis.map(Vec3::from).unwrap_or(Vec3::Z);
    let about = match origin {
        Some(p) => Vec3::new(length(&p[0])?, length(&p[1])?, length(&p[2])?),
        None => Vec3::ZERO,
    };
    let in_frame = solidcraft_doc::rigid(t, about, axis, turn);
    let frame = s.doc.component_transform(f.component);
    Some(mat_mul(&mat_mul(&frame, &in_frame), &mat_inverse(&frame)?))
}

/// Places the features `placed` again by the world move `world`: a sketch's plane and its own 3D
/// curves, a construction's plane or points. One edit of the design, in the command's undo step.
pub(crate) fn place(s: &mut Session, placed: &[u64], world: &Mat) {
    let frames: Vec<(u64, Mat)> = placed.iter().filter_map(|id| s.doc.feature(*id).map(|f| (*id, s.doc.component_transform(f.component)))).collect();
    let doc = s.doc_mut();
    for (id, frame) in frames {
        let Some(inverse) = mat_inverse(&frame) else { continue };
        let delta = mat_mul(&mat_mul(&inverse, world), &frame);
        let Some(f) = doc.feature_mut(id) else { continue };
        match &mut f.kind {
            FeatureKind::Sketch { plane, sketch } => {
                super::super::clipboard::explicit_planes(plane);
                for w in sketch.wires.iter_mut().filter(|w| w.link.is_none()) {
                    let m = if w.space == WireSpace::Component { delta } else { *world };
                    w.map(|p| apply_point(&m, p), |v| apply_vector(&m, v));
                }
            }
            FeatureKind::ConstructionPlane { plane } => super::super::clipboard::explicit_planes(plane),
            _ => {}
        }
        f.kind.transform_geometry(&delta);
    }
}

/// Whether `f` was made from `scan`: noted so, or (a design from before features noted it) a
/// Free Form or Fit Patches surface whose recipe names the scan.
fn made_from(f: &solidcraft_doc::Feature, scan: &str) -> bool {
    f.made_from.iter().any(|n| n == scan)
        || matches!(&f.kind, FeatureKind::ScanSurface { recipe: Some(r), .. } if r.scan == scan && f.made_from.is_empty())
}
