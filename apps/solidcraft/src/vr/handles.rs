//! Dimension handles in the headset (docs/vr.md): an arrow on each selected part for each of its
//! sizes that has a place on the model ([`solidcraft_ui_egui::vr_handles`]). Point at an arrow's tip
//! and hold Select to pull it: the size follows the hand along the arrow, snapped to a whole unit,
//! and to a tenth of one while the same hand's grip is held (precision: the hand then moves the
//! size a tenth as far, with no jump when the grip goes down or up). Letting go sets it, one undo
//! step; the sign at the controller says the value all the while.

use solidcraft_ui_egui::vr_handles::Handle;
use solidcraft_vr::marks::{self, Vertex};
use solidcraft_vr::room::Transform;

/// How long an arrow is in the room (metres), whatever the model's scale.
pub const ARROW: f64 = 0.05;

/// How near a ray must pass an arrow's tip to point at it (metres).
pub const REACH: f64 = 0.03;

/// The most handles drawn, and the vertices they take (a shaft and the bar at its tip each), with a
/// moving part's box (twelve edges).
pub const MAX_HANDLES: usize = 24;
pub const VERTICES: usize = MAX_HANDLES * 2 * LINE + 12 * LINE + 3 * LINE + (MAX_BOXES + 1) * 12 * LINE;

/// The vertices one line takes ([`marks::line`]: two sides of a quad).
const LINE: usize = 12;

/// A handle being pulled: by which hand, which one (its label), and how far it has gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub hand: usize,
    pub handle: Handle,
    /// The arrow's way in the room (a unit vector) when the pull began.
    dir_room: [f64; 3],
    /// Where the hand was last frame (room metres).
    last: [f64; 3],
    /// How far the size has been pulled (model millimetres), precision already applied.
    pulled: f64,
}

impl Drag {
    pub fn new(hand: usize, handle: Handle, transform: &Transform, at: [f64; 3]) -> Option<Drag> {
        let dir_room = dir_room(transform, &handle)?;
        Some(Drag { hand, handle, dir_room, last: at, pulled: 0.0 })
    }

    /// The hand is at `at` now; `fine` while its grip is held. Only this frame's move counts at
    /// this frame's rate, so the grip going down or up never makes the size jump.
    pub fn follow(&mut self, at: [f64; 3], scale: f64, fine: bool) {
        let moved: f64 = (0..3).map(|k| (at[k] - self.last[k]) * self.dir_room[k]).sum();
        if moved.is_finite() && scale > 0.0 {
            self.pulled += moved / scale * if fine { 0.1 } else { 1.0 };
        }
        self.last = at;
    }

    /// The size now, in the parameter's unit, snapped to its step (a tenth of it when `fine`), and
    /// never below one step.
    pub fn value(&self, fine: bool) -> f64 {
        let step = solidcraft_ui_egui::vr_params::step_of(&self.handle.unit, fine).unwrap_or(1.0);
        let raw = self.handle.value + self.pulled * self.handle.per_mm;
        let snapped = ((raw / step).round() * step).max(step);
        (snapped * 1e6).round() / 1e6
    }

    /// What `parameters.change` is given for `value`.
    pub fn expression(&self, value: f64) -> Option<String> {
        solidcraft_ui_egui::vr_params::typed_expression(&format!("{value}"), &self.handle.unit)
    }
}

/// The arrow's way in the room, a unit vector.
fn dir_room(t: &Transform, h: &Handle) -> Option<[f64; 3]> {
    let a = t.apply(h.at);
    let b = t.apply([0, 1, 2].map(|k| h.at[k] + h.dir[k]));
    let d = [0, 1, 2].map(|k| b[k] - a[k]);
    let n = d.iter().map(|v| v * v).sum::<f64>().sqrt();
    (n > 1e-12 && n.is_finite()).then(|| d.map(|v| v / n))
}

/// Where a handle's arrow starts and ends in the room.
pub fn arrow(t: &Transform, h: &Handle) -> Option<([f64; 3], [f64; 3])> {
    let base = t.apply(h.at);
    let d = dir_room(t, h)?;
    Some((base, [0, 1, 2].map(|k| base[k] + d[k] * ARROW)))
}

/// Which of `tips` a ray from `origin` along unit `dir` points at: the nearest along the ray
/// that it passes within [`REACH`] of, in front of the hand.
pub fn pointed(origin: [f64; 3], dir: [f64; 3], tips: &[[f64; 3]]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, tip) in tips.iter().enumerate() {
        let v = [0, 1, 2].map(|k| tip[k] - origin[k]);
        let along: f64 = (0..3).map(|k| v[k] * dir[k]).sum();
        if along <= 0.0 || !along.is_finite() {
            continue;
        }
        let off: f64 = (0..3).map(|k| (v[k] - dir[k] * along).powi(2)).sum::<f64>().sqrt();
        if off <= REACH && best.is_none_or(|(_, a)| along < a) {
            best = Some((i, along));
        }
    }
    best.map(|(i, _)| i)
}

/// The arrows: a shaft and a bar across its tip, facing `head`, in `colour`, the one pointed at
/// in `lit` and the one pulled in `pulled`.
pub fn draw(out: &mut Vec<Vertex>, t: &Transform, handles: &[Handle], head: [f64; 3], lit: &[String], pulled: Option<&str>, colours: [[f32; 3]; 3]) {
    for h in handles.iter().take(MAX_HANDLES) {
        let Some((base, tip)) = arrow(t, h) else { continue };
        let normal = [0, 1, 2].map(|k| head[k] - base[k]);
        let colour = if pulled == Some(h.label.as_str()) {
            colours[2]
        } else if lit.contains(&h.label) {
            colours[1]
        } else {
            colours[0]
        };
        marks::line(out, base, tip, normal, 0.0018, colour);
        let d = [0, 1, 2].map(|k| tip[k] - base[k]);
        let across = [d[1] * normal[2] - d[2] * normal[1], d[2] * normal[0] - d[0] * normal[2], d[0] * normal[1] - d[1] * normal[0]];
        let n = across.iter().map(|v| v * v).sum::<f64>().sqrt();
        if n > 1e-12 {
            let a = across.map(|v| v / n * 0.008);
            marks::line(out, [0, 1, 2].map(|k| tip[k] - a[k]), [0, 1, 2].map(|k| tip[k] + a[k]), normal, 0.0024, colour);
        }
    }
}

/// How far (metres) the hand must go with Select held on a selected part before the part moves:
/// less is a click, which selects as ever.
pub const MOVE_START: f64 = 0.01;

/// A selected part being moved by the hand holding Select on it.
#[derive(Clone, Debug)]
pub struct PartMove {
    pub hand: usize,
    pub part: std::sync::Arc<solidcraft_ui_egui::vr_pick::Part>,
    /// The part's box where it was (model units), drawn moved.
    pub from: Option<[[f64; 3]; 2]>,
    last: [f64; 3],
    /// How far the hand has gone (metres, any way): past [`MOVE_START`] it is a move.
    travel: f64,
    /// How far the part has gone (model millimetres, world axes), precision applied.
    moved: [f64; 3],
}

impl PartMove {
    pub fn new(hand: usize, part: std::sync::Arc<solidcraft_ui_egui::vr_pick::Part>, from: Option<[[f64; 3]; 2]>, at: [f64; 3]) -> PartMove {
        PartMove { hand, part, from, last: at, travel: 0.0, moved: [0.0; 3] }
    }

    /// The hand is at `at`; `fine` while its grip is held (a tenth as far, no jump).
    pub fn follow(&mut self, at: [f64; 3], t: &Transform, fine: bool) {
        let d = [0, 1, 2].map(|k| at[k] - self.last[k]);
        if d.iter().all(|v| v.is_finite()) && t.scale > 0.0 {
            self.travel += d.iter().map(|v| v * v).sum::<f64>().sqrt();
            let m = t.direction_to_model(d);
            let rate = if fine { 0.1 } else { 1.0 } / t.scale;
            self.moved = [0, 1, 2].map(|k| self.moved[k] + m[k] * rate);
        }
        self.last = at;
    }

    /// Whether it is a move yet, not a click.
    pub fn moving(&self) -> bool {
        self.travel > MOVE_START
    }

    /// The move so far, snapped (a tenth with `fine`).
    pub fn by(&self, fine: bool) -> [f64; 3] {
        solidcraft_ui_egui::vr_handles::snapped(self.moved, fine)
    }

    /// The part's box moved by `by`, as twelve edges in the room.
    pub fn draw(&self, out: &mut Vec<Vertex>, t: &Transform, head: [f64; 3], by: [f64; 3], colour: [f32; 3]) {
        let Some([lo, hi]) = self.from else { return };
        let corner = |c: usize| t.apply([0, 1, 2].map(|k| if c >> k & 1 == 1 { hi[k] } else { lo[k] } + by[k]));
        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
            let (a, b) = (corner(a), corner(b));
            let normal = [0, 1, 2].map(|k| head[k] - (a[k] + b[k]) / 2.0);
            marks::line(out, a, b, normal, 0.0012, colour);
        }
    }
}

/// A measurement's line between its two nearest points `a` and `b` (model units), with a bar
/// across each end.
pub fn measure_line(out: &mut Vec<Vertex>, t: &Transform, head: [f64; 3], a: [f64; 3], b: [f64; 3], colour: [f32; 3]) {
    let (a, b) = (t.apply(a), t.apply(b));
    let normal = [0, 1, 2].map(|k| head[k] - (a[k] + b[k]) / 2.0);
    marks::line(out, a, b, normal, 0.0012, colour);
    let d = [0, 1, 2].map(|k| b[k] - a[k]);
    let across = [d[1] * normal[2] - d[2] * normal[1], d[2] * normal[0] - d[0] * normal[2], d[0] * normal[1] - d[1] * normal[0]];
    let n = across.iter().map(|v| v * v).sum::<f64>().sqrt();
    if n > 1e-12 {
        let c = across.map(|v| v / n * 0.006);
        for p in [a, b] {
            marks::line(out, [0, 1, 2].map(|k| p[k] - c[k]), [0, 1, 2].map(|k| p[k] + c[k]), normal, 0.0012, colour);
        }
    }
}

/// A placed part held with Select and moved as its joints allow ([`solidcraft_ui_egui::vr_joints`]).
/// Once the hand has gone [`MOVE_START`], the axis that the hand's move follows best is taken: a
/// slide follows the hand along it, a turn follows the angle the hand sweeps about it; a tenth as
/// far while the same hand's grip is held.
#[derive(Clone, Debug)]
pub struct JointDrag {
    pub hand: usize,
    pub part: std::sync::Arc<solidcraft_ui_egui::vr_pick::Part>,
    axes: Vec<solidcraft_ui_egui::vr_joints::Axis>,
    pub chosen: Option<solidcraft_ui_egui::vr_joints::Axis>,
    /// Where the hand began and was last frame (model units), and how far it has gone (metres).
    start: [f64; 3],
    last: [f64; 3],
    last_room: [f64; 3],
    travel: f64,
    /// The part's motion so far (mm or radians), precision applied.
    pub amount: f64,
    /// The undo history's length when the drag began: the drag is one step after it.
    pub base: usize,
    /// What held the joint last frame (a limit, contact), to buzz once when it starts holding.
    pub held: Option<String>,
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|k| a[k] - b[k])
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| a[k] * b[k]).sum()
}
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit3(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot3(v, v).sqrt();
    (n > 1e-12 && n.is_finite()).then(|| v.map(|x| x / n))
}

/// The angle of `p` about the line through `origin` along unit `axis` (radians).
fn angle_about(p: [f64; 3], origin: [f64; 3], axis: [f64; 3]) -> Option<f64> {
    let r = sub3(p, origin);
    let radial = sub3(r, axis.map(|a| a * dot3(r, axis)));
    let u = unit3(cross3(axis, if axis[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }))?;
    let v = cross3(axis, u);
    (dot3(radial, radial) > 1e-12).then(|| dot3(radial, v).atan2(dot3(radial, u)))
}

impl JointDrag {
    pub fn new(
        hand: usize,
        part: std::sync::Arc<solidcraft_ui_egui::vr_pick::Part>,
        axes: Vec<solidcraft_ui_egui::vr_joints::Axis>,
        at_room: [f64; 3],
        t: &Transform,
        base: usize,
    ) -> JointDrag {
        let at = t.to_model(at_room);
        JointDrag { hand, part, axes, chosen: None, start: at, last: at, last_room: at_room, travel: 0.0, amount: 0.0, base, held: None }
    }

    /// How well a move `by` (model units) from `from` follows `axis`: along it for a slide, across
    /// the radius for a turn (the turn's tangent there).
    fn response(axis: &solidcraft_ui_egui::vr_joints::Axis, from: [f64; 3], by: [f64; 3]) -> f64 {
        if axis.angular {
            let r = sub3(from, axis.origin);
            unit3(cross3(axis.axis, r)).map_or(0.0, |tangent| dot3(by, tangent).abs())
        } else {
            dot3(by, axis.axis).abs()
        }
    }

    /// The hand is at `at_room`; `fine` while its grip is held. Whether the motion changed.
    pub fn follow(&mut self, at_room: [f64; 3], t: &Transform, fine: bool) -> bool {
        let at = t.to_model(at_room);
        if !at.iter().all(|v| v.is_finite()) {
            return false;
        }
        self.travel += dot3(sub3(at_room, self.last_room), sub3(at_room, self.last_room)).sqrt();
        self.last_room = at_room;
        if self.chosen.is_none() {
            if self.travel <= MOVE_START {
                return false;
            }
            let by = sub3(at, self.start);
            self.chosen = self.axes.iter().max_by(|a, b| Self::response(a, self.start, by).total_cmp(&Self::response(b, self.start, by))).cloned();
            self.last = self.start;
        }
        let Some(axis) = &self.chosen else { return false };
        let rate = if fine { 0.1 } else { 1.0 };
        let step = if axis.angular {
            match (angle_about(self.last, axis.origin, axis.axis), angle_about(at, axis.origin, axis.axis)) {
                (Some(a), Some(b)) => {
                    let mut d = b - a;
                    while d > std::f64::consts::PI {
                        d -= std::f64::consts::TAU;
                    }
                    while d < -std::f64::consts::PI {
                        d += std::f64::consts::TAU;
                    }
                    d
                }
                _ => 0.0,
            }
        } else {
            dot3(sub3(at, self.last), axis.axis)
        };
        self.last = at;
        self.amount += step * rate;
        step != 0.0
    }
}

/// A box (model units) as twelve edges in the room: an overlap, a sweep's contact.
pub fn draw_box(out: &mut Vec<Vertex>, t: &Transform, head: [f64; 3], [lo, hi]: [[f64; 3]; 2], colour: [f32; 3]) {
    let corner = |c: usize| t.apply([0, 1, 2].map(|k| if c >> k & 1 == 1 { hi[k] } else { lo[k] }));
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        let (a, b) = (corner(a), corner(b));
        let normal = [0, 1, 2].map(|k| head[k] - (a[k] + b[k]) / 2.0);
        marks::line(out, a, b, normal, 0.0012, colour);
    }
}

/// The most clash boxes drawn (and the sweep's mark).
pub const MAX_BOXES: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(unit: &str, value: f64, per_mm: f64) -> Handle {
        Handle {
            label: "Post1 Height (post_h)".into(),
            param: "post_h".into(),
            unit: unit.into(),
            value,
            per_mm,
            at: [0.0, 0.0, 40.0],
            dir: [0.0, 0.0, 1.0],
            body: "Post1".into(),
        }
    }

    fn room(scale: f64) -> Transform {
        Transform { axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], scale, translation: [0.0; 3], pivot: [0.0; 3] }
    }

    #[test]
    fn a_pull_along_the_arrow_grows_the_size_by_the_models_millimetres() {
        // 0.004 m in the room per millimetre of model.
        let t = room(0.004);
        let mut d = Drag::new(1, handle("mm", 30.0, 1.0), &t, [0.0; 3]).expect("drag");
        d.follow([0.0, 0.0, 0.02], t.scale, false); // 2 cm up: 5 mm of model
        assert_eq!(d.value(false), 35.0);
        d.follow([0.03, 0.0, 0.02], t.scale, false); // across the arrow: nothing
        assert_eq!(d.value(false), 35.0);
        d.follow([0.03, 0.0, -0.06], t.scale, false); // 8 cm down: 20 mm less
        assert_eq!(d.value(false), 15.0);
    }

    #[test]
    fn the_grip_pulls_a_tenth_as_far_with_no_jump_and_a_finer_step() {
        let t = room(0.004);
        let mut d = Drag::new(0, handle("mm", 30.0, 1.0), &t, [0.0; 3]).expect("drag");
        d.follow([0.0, 0.0, 0.004], t.scale, false); // 1 mm
        let before = d.value(true);
        d.follow([0.0, 0.0, 0.004], t.scale, true); // grip down, hand still: no jump
        assert_eq!(d.value(true), before);
        d.follow([0.0, 0.0, 0.012], t.scale, true); // 2 mm of hand, a tenth of it: 0.2 mm
        assert_eq!(d.value(true), 31.2);
        d.follow([0.0, 0.0, 0.012], t.scale, false); // grip up: still no jump
        assert_eq!(d.value(false), 31.0);
    }

    #[test]
    fn a_diameter_pulled_at_its_rim_grows_twice_as_fast_and_never_below_a_step() {
        let t = room(0.004);
        let mut d = Drag::new(0, handle("mm", 12.0, 2.0), &t, [0.0; 3]).expect("drag");
        d.follow([0.0, 0.0, 0.008], t.scale, false); // 2 mm of model
        assert_eq!(d.value(false), 16.0);
        d.follow([0.0, 0.0, -1.0], t.scale, false); // far below nothing
        assert_eq!(d.value(false), 1.0);
        assert_eq!(d.expression(16.0).as_deref(), Some("16 mm"));
    }

    #[test]
    fn the_ray_points_at_the_nearest_tip_it_passes_close_to_in_front() {
        let tips = [[0.0, 0.0, -1.0], [0.01, 0.0, -0.5], [0.0, 0.0, 1.0], [0.1, 0.0, -0.3]];
        assert_eq!(pointed([0.0; 3], [0.0, 0.0, -1.0], &tips), Some(1));
        assert_eq!(pointed([0.0; 3], [0.0, 1.0, 0.0], &tips), None);
        assert_eq!(pointed([0.0; 3], [0.0, 0.0, -1.0], &[[f64::NAN; 3]]), None);
    }

    #[test]
    fn an_arrow_is_the_same_length_in_the_room_at_any_scale() {
        for scale in [0.001, 0.004, 0.02] {
            let (a, b) = arrow(&room(scale), &handle("mm", 30.0, 1.0)).expect("arrow");
            let len = (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f64>().sqrt();
            assert!((len - ARROW).abs() < 1e-12, "{scale}");
        }
    }

    fn part() -> std::sync::Arc<solidcraft_ui_egui::vr_pick::Part> {
        std::sync::Arc::new(solidcraft_ui_egui::vr_pick::Part {
            sel: solidcraft_engine::Sel::Body { name: "Post1".into() },
            name: "Post1".into(),
            bodies: vec!["Post1".into()],
        })
    }

    #[test]
    fn a_short_shake_is_a_click_and_a_longer_move_moves_the_part_in_model_millimetres() {
        let t = room(0.004);
        let mut m = PartMove::new(0, part(), Some([[0.0; 3], [10.0; 3]]), [0.0; 3]);
        m.follow([0.003, 0.0, 0.0], &t, false);
        assert!(!m.moving());
        m.follow([0.04, 0.0, 0.0], &t, false); // 4 cm in the room: 10 mm of model along X
        assert!(m.moving());
        assert_eq!(m.by(false), [10.0, 0.0, 0.0]);
        m.follow([0.04, 0.0, 0.004], &t, true); // grip: 1 mm of model a tenth as far
        assert_eq!(m.by(true), [10.0, 0.0, 0.1]);
    }

    #[test]
    fn the_moving_box_is_twelve_edges_moved_by_the_move() {
        let t = room(0.001);
        let m = PartMove::new(0, part(), Some([[0.0; 3], [10.0; 3]]), [0.0; 3]);
        let mut out = Vec::new();
        m.draw(&mut out, &t, [0.0, 0.0, 1.0], [5.0, 0.0, 0.0], [1.0; 3]);
        assert_eq!(out.len(), 12 * LINE);
        let none = PartMove::new(0, part(), None, [0.0; 3]);
        let mut out = Vec::new();
        none.draw(&mut out, &t, [0.0, 0.0, 1.0], [5.0, 0.0, 0.0], [1.0; 3]);
        assert!(out.is_empty());
    }

    fn joint_axis(angular: bool) -> solidcraft_ui_egui::vr_joints::Axis {
        solidcraft_ui_egui::vr_joints::Axis {
            joint: 10,
            name: "Hinge".into(),
            index: 0,
            origin: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            angular,
            scale: 1.0,
            value: 0.0,
            limits: None,
        }
    }

    #[test]
    fn a_hinge_follows_the_angle_the_hand_sweeps_about_it_and_a_tenth_with_the_grip() {
        let t = room(0.001); // a millimetre of model is a millimetre of room
        let mut j = JointDrag::new(0, part(), vec![joint_axis(true)], [0.1, 0.0, 0.0], &t, 3);
        assert!(!j.follow([0.1, 0.005, 0.0], &t, false), "under a centimetre: not yet a drag");
        let quarter = [0.0, 0.1, 0.0];
        j.follow(quarter, &t, false);
        assert!((j.amount - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "{}", j.amount);
        j.follow([-0.1, 0.0, 0.0], &t, true); // another quarter turn at a tenth
        assert!((j.amount - std::f64::consts::FRAC_PI_2 * 1.1).abs() < 1e-9, "{}", j.amount);
    }

    #[test]
    fn of_a_hinge_and_a_slide_the_hand_takes_the_one_it_moves_along() {
        let t = room(0.001);
        let both = vec![joint_axis(true), solidcraft_ui_egui::vr_joints::Axis { axis: [1.0, 0.0, 0.0], ..joint_axis(false) }];
        // From (0.1, 0, 0) the turn about Z goes along Y; moving along X is the slide.
        let mut j = JointDrag::new(0, part(), both.clone(), [0.1, 0.0, 0.0], &t, 0);
        j.follow([0.13, 0.0, 0.0], &t, false);
        assert_eq!(j.chosen.as_ref().map(|a| a.angular), Some(false));
        assert!((j.amount - 30.0).abs() < 1e-9, "{}", j.amount);
        let mut j = JointDrag::new(0, part(), both, [0.1, 0.0, 0.0], &t, 0);
        j.follow([0.1, 0.03, 0.0], &t, false);
        assert_eq!(j.chosen.as_ref().map(|a| a.angular), Some(true));
    }
}
