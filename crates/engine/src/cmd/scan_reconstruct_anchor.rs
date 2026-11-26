//! Anchoring: a prism Extrude It builds stands where its thickness parameters say, not at a world
//! number. Its sketch plane is put at an expression: the lower face it stands on (the datum: a
//! number, or the top or bottom of a prism built before it that the scan has it meet, as that
//! prism's own expression) plus half its thickness parameter (the extrude is symmetric about the
//! plane). Change a thickness and what stands above it moves with it; the lowest face of the part
//! stays where it is.
//!
//! Only a prism whose plane faces along a world axis, its walls square to it, is anchored: any
//! other stays at its numbers, as it was built before. The rebuilt model must come out the same
//! (the volume within rounding) or the plane goes back to its numbers.

use solidcore_record::Vec3;
use solidcraft_doc::{Expr, FeatureKind, PlaneRef};
use solidcraft_geom::prelude::*;

use crate::Session;

/// The world axis `n` runs along.
fn axis_of(n: Vec3) -> Option<usize> {
    let c = [n.x, n.y, n.z];
    (0..3).find(|&k| c[k].abs() > 1.0 - 1e-9)
}

/// The faces, along `axis`, of the prisms built before (but the extrude `but`) that stand on a
/// placed plane: where each is, and its expression.
fn anchors(s: &Session, axis: usize, but: u64) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for f in &s.doc.features {
        let FeatureKind::Extrude { sketch, extent, .. } = &f.kind else { continue };
        if f.id == but || !matches!(extent.direction, solidcraft_doc::Direction::Symmetric) {
            continue;
        }
        let Some(thick) = f.param_names.first() else { continue };
        let Some(FeatureKind::Sketch { plane: PlaneRef::Placed { origin, x_dir, y_dir }, .. }) = s.doc.feature(sketch.0).map(|k| &k.kind) else {
            continue;
        };
        if axis_of(x_dir.cross(*y_dir)) != Some(axis) {
            continue;
        }
        let middle = format!("({})", origin[axis]);
        for way in ["-", "+"] {
            let expr = format!("{middle} {way} ({thick}) / 2");
            if let Ok(at) = s.doc.eval(&expr, solidcraft_doc::expr::Kind::Length) {
                out.push((at, expr));
            }
        }
    }
    out
}

/// Puts the sketch of the prism just built (extruded symmetric through `depth`) at an expression of
/// its thickness parameter and of what it stands on. `tolerance` is how near another prism's face
/// counts as the face it stands on. Nothing changes when it does not apply or the model comes out
/// other than it was. Whether it was anchored.
pub(super) fn anchor_prism(s: &mut Session, sketch: u64, solid: &str, depth: f64, tolerance: f64) -> bool {
    let Some((id, Some(thick))) = s
        .doc
        .features
        .iter()
        .rev()
        .find(|f| matches!(&f.kind, FeatureKind::Extrude { sketch: k, .. } if k.0 == sketch))
        .map(|f| (f.id, f.param_names.first().cloned()))
    else {
        return false;
    };
    // The feature's own distance is a parameter by that name (a feature input, not a listed one).
    if s.doc.eval(&thick, solidcraft_doc::expr::Kind::Length).is_err() {
        return false;
    }
    let Some(FeatureKind::Extrude { extent, .. }) = s.doc.feature(id).map(|f| &f.kind) else { return false };
    if !matches!(extent.direction, solidcraft_doc::Direction::Symmetric) {
        return false;
    }
    let Some(FeatureKind::Sketch { plane: PlaneRef::Custom { plane: stored }, .. }) = s.doc.feature(sketch).map(|f| f.kind.clone()) else {
        return false;
    };
    let Some(axis) = axis_of(stored.x.cross(stored.y)) else { return false };
    let origin = [stored.origin.x, stored.origin.y, stored.origin.z];
    let (low, high) = (origin[axis] - depth / 2.0, origin[axis] + depth / 2.0);
    let near = anchors(s, axis, id);
    let nearest = |at: f64| near.iter().filter(|(x, _)| (x - at).abs() <= tolerance).min_by(|a, b| (a.0 - at).abs().total_cmp(&(b.0 - at).abs()));
    // Standing on a face of what is built (its lower face meets one), else hanging from one (its
    // upper face does), else its lower face is where it is.
    // The plane's place stays where it is built now, to the last digit: the face it stands on (or
    // hangs from) as that prism's expression, and the little between them the scan left as a number.
    let off = |by: f64| if by != 0.0 { format!(" + {by} mm") } else { String::new() };
    let along = match (nearest(low), nearest(high)) {
        (Some((at, face)), _) => format!("({face}){} + ({thick}) / 2", off(low - at)),
        (None, Some((at, face))) => format!("({face}){} - ({thick}) / 2", off(high - at)),
        (None, None) => format!("{low} mm + ({thick}) / 2"),
    };
    let placed = PlaneRef::Placed {
        origin: std::array::from_fn(|k| Expr::new(if k == axis { along.clone() } else { format!("{}", origin[k]) })),
        x_dir: stored.x,
        y_dir: stored.y,
    };
    let before = s.doc.clone();
    let volume = |s: &Session| s.model.state().body(solid).and_then(|b| solidcraft_kernel::measure(&b.body).ok()).map(|m| m.volume);
    let was = volume(s);
    if let Some(FeatureKind::Sketch { plane, .. }) = s.doc_mut().feature_mut(sketch).map(|f| &mut f.kind) {
        *plane = placed;
    }
    s.refresh();
    // The same model: the planes are re-expressed, not moved.
    let same = match (was, volume(s)) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * a.abs().max(1.0),
        _ => false,
    };
    if !same || s.model.state().sketch(sketch).is_none() {
        s.doc = before;
        s.refresh();
        return false;
    }
    true
}
