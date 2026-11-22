//! Edit Surface's guides: X, Y and Z through the middle of the handles chosen, in the origin
//! axes' colours, so where the choice lies against the design's axes is seen where it is. Each
//! guide is solid near the middle, where a drag on it (off the choice itself, right at the middle)
//! moves the choice along that axis only, and runs on faint to the edges of the view.

use super::*;

/// How far each guide's solid part reaches either side of the middle, in pixels.
const GUIDE_PX: f64 = 70.0;

/// How near a guide's solid part the pointer must be to take it, in pixels.
const GUIDE_GRAB: f32 = 6.0;

/// The share of a guide's solid part, either side of the middle, that a press does not take: the
/// points and lines chosen sit there, and stay clicked, double-clicked and dragged as ever.
const GUIDE_INNER: f32 = 0.3;

/// And at least this far either side of the middle, in pixels.
const INNER_PX: f32 = 16.0;

pub(super) const DIRS: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// Whether the guides are shown: points are chosen and no other tool has the pointer.
pub(super) fn shown(k: &EditSurface) -> bool {
    !k.picked.is_empty()
        && k.adding.is_none()
        && !k.deleting
        && k.creasing.is_none()
        && !k.brushing
        && !k.keeping_half
        && k.marquee.is_none()
        && k.drag.as_ref().is_none_or(|d| d.grow.is_empty())
}

/// Each guide's solid part on the screen: the axis and its two ends.
fn solid_parts(k: &EditSurface, proj: &crate::viewport::Proj) -> Vec<(usize, egui::Pos2, egui::Pos2)> {
    let Some(c) = select::centre(k) else { return Vec::new() };
    let Some(l) = crate::gizmo::mm_per_px(proj, c).map(|m| m * GUIDE_PX) else { return Vec::new() };
    DIRS.iter().enumerate().filter_map(|(i, d)| Some((i, proj.to_screen(c - *d * l)?, proj.to_screen(c + *d * l)?))).collect()
}

/// The guide whose solid part is under `at` on the screen, if the guides are shown.
pub(super) fn guide_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    if !shown(k) {
        return None;
    }
    solid_parts(k, proj)
        .into_iter()
        // A guide seen (nearly) end on is a stub over the choice: it cannot be dragged along.
        .filter(|(_, a, b)| f64::from(a.distance(*b)) > GUIDE_PX * 0.5)
        .map(|(i, a, b)| {
            let (mid, half) = (a + (b - a) * 0.5, (b - a) * 0.5);
            let inner = half * GUIDE_INNER.max(INNER_PX / half.length().max(1.0)).min(1.0);
            (i, distance_to_segment(at, a, mid - inner).min(distance_to_segment(at, mid + inner, b)))
        })
        .filter(|(_, d)| *d <= GUIDE_GRAB)
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(i, _)| i)
}

/// The guides over the picture; the one under the pointer, or being dragged along, bold.
pub(super) fn draw(k: &EditSurface, painter: &egui::Painter, proj: &crate::viewport::Proj, pointer: Option<egui::Pos2>) {
    if !shown(k) {
        return;
    }
    let Some(c) = select::centre(k) else { return };
    let t = solidcraft_theme::Tokens::get();
    let colours = t.axes();
    let along = k.drag.as_ref().and_then(|d| d.axis.map(|(i, _)| i));
    let hot = along.or_else(|| if k.drag.is_none() { pointer.and_then(|p| guide_at(k, proj, p)) } else { None });
    for (i, d) in DIRS.iter().enumerate() {
        let colour = colours[i];
        // Faint to the edges of the view, a little short of where the view's box cuts it.
        if let Some((t0, t1)) = crate::viewport::line_in_view(&proj.vp, c, *d) {
            let (t0, t1) = (t0 * 0.999, t1 * 0.999);
            if let (Some(a), Some(b)) = (proj.to_screen(c + *d * t0), proj.to_screen(c + *d * t1)) {
                painter.line_segment([a, b], egui::Stroke::new(1.0, colour.gamma_multiply(0.45)));
            }
        }
    }
    for (i, a, b) in solid_parts(k, proj) {
        let width = if hot == Some(i) { 4.0 } else { 2.0 };
        cased_segment(painter, [a, b], egui::Stroke::new(width, colours[i]));
    }
}
