//! Edit Surface's selection: any number of handles (points on the surface, or control points)
//! chosen at once, to drag, move, snap to the scan or fit together.
//!
//! - A click chooses one handle (a click on nothing chooses none); Ctrl-click toggles one; Shift-click adds one.
//! - A drag begun on nothing draws a box and chooses the handles inside it (Shift adds them, Ctrl
//!   toggles them, Alt takes them away).
//! - A click on a line of the grid between two handles chooses both; a double-click on one
//!   chooses the whole line it is part of, across the faces, to the edge (QuickSurface's).
//! - A double-click on a face (off its lines) chooses all its points.
//! - A drag of a chosen handle, or of a line, moves every chosen handle by the same amount.
//!
//! The handle `chosen` is the one last chosen: the one whose place is typed in the dialog.

use std::collections::BTreeSet;

use super::*;

pub(super) use crate::scan_how::How;

/// A box being drawn on the screen: its corners, and what was chosen before it began.
#[derive(Clone, Debug)]
pub(super) struct Marquee {
    pub from: egui::Pos2,
    pub to: egui::Pos2,
    before: BTreeSet<usize>,
    how: How,
}

impl Marquee {
    pub(super) fn rect(&self) -> egui::Rect {
        egui::Rect::from_two_pos(self.from, self.to)
    }
}

/// How many handles there are, as the handles are (dots or the control net).
pub(super) fn handle_count(k: &EditSurface) -> usize {
    if k.handles == HANDLES_DOTS { k.shown_dots.len() } else { k.shown_net.len() }
}

/// Where handle `i` is.
pub(super) fn handle_place(k: &EditSurface, i: usize) -> Option<Vec3> {
    if k.handles == HANDLES_DOTS { k.shown_dots.get(i).map(|d| d.at) } else { k.shown_net.get(i).copied() }
}

/// Nothing chosen.
pub(super) fn clear(k: &mut EditSurface) {
    k.picked.clear();
    k.chosen = None;
}

/// Handle `index` chosen as `how` says.
pub(super) fn pick(k: &mut EditSurface, index: usize, how: How) {
    pick_many(k, &[index], how);
}

/// Handles `ids` chosen as `how` says; the last of them becomes the one `chosen`.
pub(super) fn pick_many(k: &mut EditSurface, ids: &[usize], how: How) {
    let count = handle_count(k);
    let ids: Vec<usize> = ids.iter().copied().filter(|i| *i < count).collect();
    match how {
        How::Replace => {
            k.picked = ids.iter().copied().collect();
        }
        How::Add => k.picked.extend(ids.iter().copied()),
        How::Toggle => {
            for i in &ids {
                if !k.picked.remove(i) {
                    k.picked.insert(*i);
                }
            }
        }
        How::Remove => {
            for i in &ids {
                k.picked.remove(i);
            }
        }
    }
    settle(k, ids.last().copied());
}

/// `chosen` kept to the choice: the handle just chosen when it still is, else the highest one left.
fn settle(k: &mut EditSurface, last: Option<usize>) {
    k.chosen = match last.filter(|i| k.picked.contains(i)) {
        Some(i) => Some(i),
        None => k.chosen.filter(|i| k.picked.contains(i)).or_else(|| k.picked.iter().next_back().copied()),
    };
}

/// Every handle chosen.
pub(super) fn all(k: &mut EditSurface) {
    k.picked = (0..handle_count(k)).collect();
    settle(k, None);
}

/// The handles chosen and not chosen swapped.
pub(super) fn invert(k: &mut EditSurface) {
    k.picked = (0..handle_count(k)).filter(|i| !k.picked.contains(i)).collect();
    settle(k, None);
}

/// The handles next to `ids` along the faces' grids (one step across or along), in every face that has them.
fn next_to(k: &EditSurface, ids: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for &p in ids {
        for &(f, at) in k.joins.at.get(p).map_or(&[][..], Vec::as_slice) {
            let Some([nu, nv]) = k.faces.get(f).map(NurbsSurface::count) else { continue };
            let Some(of) = k.joins.of.get(f) else { continue };
            let (i, j) = (at % nu, at / nu);
            let steps = [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)];
            for (a, b) in steps {
                if a < nu
                    && b < nv
                    && let Some(&q) = of.get(b * nu + a)
                {
                    out.insert(q);
                }
            }
        }
    }
    out
}

/// The choice grown by one step along the grids.
pub(super) fn grow(k: &mut EditSurface) {
    if k.picked.is_empty() {
        return;
    }
    let next = next_to(k, &k.picked);
    k.picked.extend(next);
    settle(k, None);
}

/// The choice shrunk by one step: the handles with a neighbour not chosen are let go.
pub(super) fn shrink(k: &mut EditSurface) {
    let edge: Vec<usize> = k
        .picked
        .iter()
        .copied()
        .filter(|p| {
            let one: BTreeSet<usize> = std::iter::once(*p).collect();
            next_to(k, &one).iter().any(|q| !k.picked.contains(q))
        })
        .collect();
    for p in edge {
        k.picked.remove(&p);
    }
    settle(k, None);
}

/// The faces with a handle chosen.
pub(super) fn faces_touched(k: &EditSurface) -> Vec<usize> {
    (0..k.faces.len()).filter(|f| k.joins.of.get(*f).is_some_and(|ids| ids.iter().any(|p| k.picked.contains(p)))).collect()
}

/// Every handle of face `face` chosen as `how` says.
pub(super) fn pick_face(k: &mut EditSurface, face: usize, how: How) {
    let ids: BTreeSet<usize> = k.joins.of.get(face).map(|v| v.iter().copied().collect()).unwrap_or_default();
    let ids: Vec<usize> = ids.into_iter().collect();
    pick_many(k, &ids, how);
}

/// Every handle of the faces with one chosen, chosen.
pub(super) fn whole_faces(k: &mut EditSurface) {
    for f in faces_touched(k) {
        pick_face(k, f, How::Add);
    }
}

/// The handles chosen, lowest first.
pub(super) fn chosen_ids(k: &EditSurface) -> Vec<usize> {
    k.picked.iter().copied().filter(|i| *i < handle_count(k)).collect()
}

/// The handles to work on: those chosen, else all.
pub(super) fn scope(k: &EditSurface) -> Vec<usize> {
    let ids = chosen_ids(k);
    if ids.is_empty() { (0..handle_count(k)).collect() } else { ids }
}

/// The middle of the handles chosen.
pub(super) fn centre(k: &EditSurface) -> Option<Vec3> {
    let places: Vec<Vec3> = chosen_ids(k).into_iter().filter_map(|i| handle_place(k, i)).collect();
    if places.is_empty() {
        return None;
    }
    let sum = places.iter().fold(Vec3::ZERO, |a, p| a + *p);
    Some(sum * (1.0 / places.len() as f64))
}

/// How near a line of the grid the pointer must be to take it, in pixels.
const LINE_GRAB: f32 = 6.0;

/// One line of the grids: a step between two handles next to each other in a face's grid, the
/// face, the two places in its net and the two handles. A side two faces share is one line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GridLine {
    pub face: usize,
    pub at: [usize; 2],
    pub ends: [usize; 2],
}

/// Every line of the faces' grids, a shared one once.
pub(super) fn grid_lines(k: &EditSurface) -> Vec<GridLine> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (face, (s, of)) in k.shown.iter().zip(&k.joins.of).enumerate() {
        let [nu, nv] = s.count();
        for j in 0..nv {
            for i in 0..nu {
                let here = j * nu + i;
                for next in [(i + 1 < nu).then_some(here + 1), (j + 1 < nv).then_some(here + nu)].into_iter().flatten() {
                    let (Some(&a), Some(&b)) = (of.get(here), of.get(next)) else { continue };
                    if a != b && seen.insert((a.min(b), a.max(b))) {
                        out.push(GridLine { face, at: [here, next], ends: [a, b] });
                    }
                }
            }
        }
    }
    out
}

/// The line of the grid under `at` on the screen (within [`LINE_GRAB`]), and the place on it
/// under the pointer.
pub(super) fn line_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<(GridLine, Vec3)> {
    let screen: Vec<Option<egui::Pos2>> = (0..handle_count(k)).map(|i| handle_place(k, i).and_then(|p| proj.to_screen(p))).collect();
    grid_lines(k)
        .into_iter()
        .filter_map(|l| {
            let (a, b) = (screen.get(l.ends[0]).copied().flatten()?, screen.get(l.ends[1]).copied().flatten()?);
            let ab = b - a;
            let t = if ab.length_sq() > 0.0 { ((at - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            Some((l, t, (a + ab * t).distance(at)))
        })
        .filter(|(_, _, d)| *d <= LINE_GRAB)
        .min_by(|x, y| x.2.total_cmp(&y.2))
        .and_then(|(l, t, _)| {
            let (p, q) = (handle_place(k, l.ends[0])?, handle_place(k, l.ends[1])?);
            Some((l, p + (q - p) * f64::from(t)))
        })
}

/// The whole line of the grid through the step from handle `a` to handle `b`, as QuickSurface's
/// double-click takes it: on along the grid both ways, across the sides the faces share, to the
/// network's edge or a corner that is not a grid's (where the way straight on is not one line).
/// At each point the line goes on along the one step that shares no cell with the step it came
/// by; a loop round the surface ends where it began.
pub(super) fn whole_line(k: &EditSurface, a: usize, b: usize) -> Vec<usize> {
    use std::collections::HashMap;
    let key = |x: usize, y: usize| (x.min(y), x.max(y));
    let mut cells: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    let mut next_to: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    let mut cell = 0;
    for (s, of) in k.shown.iter().zip(&k.joins.of) {
        let [nu, nv] = s.count();
        for j in 0..nv.saturating_sub(1) {
            for i in 0..nu.saturating_sub(1) {
                let corners = [j * nu + i, j * nu + i + 1, (j + 1) * nu + i + 1, (j + 1) * nu + i].map(|c| of.get(c).copied());
                let [Some(c0), Some(c1), Some(c2), Some(c3)] = corners else { continue };
                let c = [c0, c1, c2, c3];
                for e in 0..4 {
                    let (x, y) = (c[e], c[(e + 1) % 4]);
                    if x == y {
                        continue;
                    }
                    cells.entry(key(x, y)).or_default().push(cell);
                    next_to.entry(x).or_default().insert(y);
                    next_to.entry(y).or_default().insert(x);
                }
                cell += 1;
            }
        }
    }
    let mut seen: BTreeSet<usize> = [a, b].into_iter().collect();
    let mut walk = |from: usize, at: usize| {
        let mut out = Vec::new();
        let (mut p, mut c) = (from, at);
        loop {
            let came = cells.get(&key(p, c)).map_or(&[][..], Vec::as_slice);
            let on: Vec<usize> = next_to
                .get(&c)
                .into_iter()
                .flatten()
                .copied()
                .filter(|n| *n != p && cells.get(&key(c, *n)).is_none_or(|q| q.iter().all(|x| !came.contains(x))))
                .collect();
            let [n] = on.as_slice() else { break };
            if !seen.insert(*n) {
                break;
            }
            out.push(*n);
            (p, c) = (c, *n);
        }
        out
    };
    let ahead = walk(a, b);
    let mut line = walk(b, a);
    line.reverse();
    line.extend([a, b]);
    line.extend(ahead);
    line
}

/// A box begun at `at`.
pub(super) fn begin_box(k: &mut EditSurface, at: egui::Pos2, how: How) {
    let before = if how == How::Replace { BTreeSet::new() } else { k.picked.clone() };
    k.marquee = Some(Box::new(Marquee { from: at, to: at, before, how }));
    if how == How::Replace {
        clear(k);
    }
}

/// The box dragged to `to`: the handles inside it chosen, with what was chosen before it as `how` says.
pub(super) fn drag_box(k: &mut EditSurface, proj: &crate::viewport::Proj, to: egui::Pos2) {
    let Some(m) = k.marquee.as_mut() else { return };
    m.to = to;
    let (rect, before, how) = (m.rect(), m.before.clone(), m.how);
    let inside: Vec<usize> =
        (0..handle_count(k)).filter(|i| handle_place(k, *i).and_then(|p| proj.to_screen(p)).is_some_and(|s| rect.contains(s))).collect();
    k.picked = before;
    let how = if how == How::Replace { How::Add } else { how };
    pick_many(k, &inside, how);
}

/// The box let go: what it chose stays chosen.
pub(super) fn end_box(k: &mut EditSurface) {
    k.marquee = None;
}

/// The points that go with a drag of handle `index` begun on it: all those chosen when it is one
/// of them, else it alone (and then it is the choice).
pub(super) fn dragged_with(k: &mut EditSurface, index: usize) -> Vec<usize> {
    if k.picked.contains(&index) {
        k.chosen = Some(index);
        chosen_ids(k)
    } else {
        pick(k, index, How::Replace);
        vec![index]
    }
}

/// The dialog's rows for the choice: how many handles are chosen and the tools on it.
pub(super) fn rows(ui: &mut egui::Ui, k: &mut EditSurface) {
    let count = chosen_ids(k).len();
    let line = solidcraft_ui_text::format("ui.scan_edit_selected_value", &[("count", &count.to_string()), ("all", &handle_count(k).to_string())]);
    form_row(ui, tr("ui.scan_edit_selected"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Cell));
    let some = count > 0;
    let mut tools: [(&'static str, fn(&mut EditSurface), bool); 6] = [
        ("ui.scan_edit_select_all", all, true),
        ("ui.scan_edit_select_none", clear, some),
        ("ui.scan_edit_select_invert", invert, true),
        ("ui.scan_edit_select_grow", grow, some),
        ("ui.scan_edit_select_shrink", shrink, some),
        ("ui.scan_edit_select_faces", whole_faces, some),
    ];
    for pair in tools.chunks_mut(2) {
        ui_kit::row(ui, "", |ui| {
            ui.horizontal(|ui| {
                for (id, action, enabled) in pair.iter() {
                    if ui_kit::button(ui, tr(id), if *enabled { ui_kit::Button::Small } else { ui_kit::Button::Enabled(false) }).clicked() && *enabled
                    {
                        action(k);
                    }
                }
            })
            .response
        });
    }
}

/// How far from the choice a handle is, mm: from the nearest handle chosen.
fn reach_of(places: &[(usize, Vec3)], p: Vec3) -> Option<(f64, usize)> {
    places.iter().enumerate().map(|(n, (_, q))| (q.dist(p), n)).min_by(|a, b| a.0.total_cmp(&b.0))
}

/// `moves` with the handles near them moved too, less the further they are (a soft selection).
/// `start` is where every handle was when the drag began and `from` where each one dragged was;
/// `held` marks handles that stay where they are (those moved by hand).
pub(super) fn soft_moves(start: &[Vec3], from: &[(usize, Vec3)], moves: &[(usize, Vec3)], radius: f64, held: &[bool]) -> Vec<(usize, Vec3)> {
    let mut out = moves.to_vec();
    if !(radius > 0.0) || !radius.is_finite() || from.is_empty() {
        return out;
    }
    let delta_of = |n: usize| -> Option<Vec3> {
        let (i, from) = from.get(n)?;
        moves.iter().find(|m| m.0 == *i).map(|m| m.1 - *from)
    };
    for (j, p) in start.iter().enumerate() {
        if moves.iter().any(|m| m.0 == j) || held.get(j).copied().unwrap_or(false) {
            continue;
        }
        let Some((d, n)) = reach_of(from, *p) else { continue };
        let w = edit::falloff_weight(d, radius);
        if w <= 1e-3 {
            continue;
        }
        if let Some(delta) = delta_of(n) {
            out.push((j, *p + delta * w));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64) -> Vec3 {
        Vec3::new(x, 0.0, 0.0)
    }

    #[test]
    fn keys_choose_how() {
        let none = egui::Modifiers::NONE;
        assert_eq!(How::of(none), How::Replace);
        assert_eq!(How::of(egui::Modifiers { shift: true, ..none }), How::Add);
        assert_eq!(How::of(egui::Modifiers { ctrl: true, ..none }), How::Toggle);
        assert_eq!(How::of(egui::Modifiers { alt: true, ..none }), How::Remove);
        // Ctrl and Shift together are the brush's keys: a plain choice.
        assert_eq!(How::of(egui::Modifiers { ctrl: true, shift: true, ..none }), How::Replace);
    }

    #[test]
    fn a_soft_selection_takes_the_points_near_less() {
        let start = vec![at(0.0), at(1.0), at(5.0), at(20.0)];
        let moves = vec![(0, Vec3::new(0.0, 0.0, 2.0))];
        let out = soft_moves(&start, &[(0, at(0.0))], &moves, 10.0, &[false; 4]);
        let z = |i: usize| out.iter().find(|m| m.0 == i).map_or(0.0, |m| m.1.z);
        assert!((z(0) - 2.0).abs() < 1e-12, "the dragged point goes all the way");
        assert!(z(1) > z(2) && z(2) > 0.0, "nearer points go further: {} {}", z(1), z(2));
        assert_eq!(z(3), 0.0, "beyond the radius nothing moves");
    }

    #[test]
    fn a_soft_selection_leaves_held_points_and_does_nothing_without_a_radius() {
        let start = vec![at(0.0), at(1.0)];
        let moves = vec![(0, Vec3::new(0.0, 0.0, 2.0))];
        let held = soft_moves(&start, &[(0, at(0.0))], &moves, 10.0, &[false, true]);
        assert_eq!(held.len(), 1);
        assert_eq!(soft_moves(&start, &[(0, at(0.0))], &moves, 0.0, &[false; 2]).len(), 1);
        assert_eq!(soft_moves(&start, &[(0, at(0.0))], &moves, f64::NAN, &[false; 2]).len(), 1);
    }
}
