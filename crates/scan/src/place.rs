//! Rigid moves that place a scan: stand it on a base, rest it on the grid, centre it over the
//! origin, lay it along its own axes. Each is a turn about the world origin followed by a shift,
//! which is how a Move feature moves a body, so the engine hands it over as one.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, surface_centre};

/// A rigid move, `p ↦ rotation · p + translation`, the rotation given row by row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rigid {
    pub rotation: [[f64; 3]; 3],
    pub translation: Vec3,
}

const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

impl Rigid {
    pub fn shift(translation: Vec3) -> Self {
        Rigid { rotation: IDENTITY, translation }
    }

    /// The turn by `angle` radians about the unit `axis` through the origin (right-handed).
    pub fn turn(axis: Vec3, angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        let Vec3 { x, y, z } = axis;
        Rigid {
            rotation: [
                [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
                [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
                [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
            ],
            translation: Vec3::ZERO,
        }
    }

    /// `v` turned, without the shift.
    pub fn turned(&self, v: Vec3) -> Vec3 {
        let [r0, r1, r2] = self.rotation;
        Vec3::new(r0[0] * v.x + r0[1] * v.y + r0[2] * v.z, r1[0] * v.x + r1[1] * v.y + r1[2] * v.z, r2[0] * v.x + r2[1] * v.y + r2[2] * v.z)
    }

    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.turned(p) + self.translation
    }

    /// The move that undoes this one.
    pub fn inverse(&self) -> Rigid {
        let r = self.rotation;
        let rotation = [[r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]]];
        let back = Rigid { rotation, translation: Vec3::ZERO };
        Rigid { rotation, translation: -back.turned(self.translation) }
    }

    /// The same turn about the point `pivot` instead of the origin: `pivot` stays where it is.
    fn about(self, pivot: Vec3) -> Self {
        Rigid { translation: pivot - self.turned(pivot), ..self }
    }

    /// The turn as a unit axis and an angle in radians (`0..=π`), about the origin; `None` when
    /// there is no turn (to 1e-12 radians).
    pub fn axis_angle(&self) -> Option<(Vec3, f64)> {
        let r = self.rotation;
        let trace = r[0][0] + r[1][1] + r[2][2];
        // The unit quaternion, from its largest component (Shepperd), so that a half turn is as
        // exact as a small one.
        let (w, x, y, z) = if trace > r[0][0].max(r[1][1]).max(r[2][2]) {
            let s = (1.0 + trace).max(0.0).sqrt() * 2.0;
            (s / 4.0, (r[2][1] - r[1][2]) / s, (r[0][2] - r[2][0]) / s, (r[1][0] - r[0][1]) / s)
        } else if r[0][0] >= r[1][1] && r[0][0] >= r[2][2] {
            let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).max(0.0).sqrt() * 2.0;
            ((r[2][1] - r[1][2]) / s, s / 4.0, (r[0][1] + r[1][0]) / s, (r[0][2] + r[2][0]) / s)
        } else if r[1][1] >= r[2][2] {
            let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).max(0.0).sqrt() * 2.0;
            ((r[0][2] - r[2][0]) / s, (r[0][1] + r[1][0]) / s, s / 4.0, (r[1][2] + r[2][1]) / s)
        } else {
            let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).max(0.0).sqrt() * 2.0;
            ((r[1][0] - r[0][1]) / s, (r[0][2] + r[2][0]) / s, (r[1][2] + r[2][1]) / s, s / 4.0)
        };
        let v = Vec3::new(x, y, z);
        let sin_half = v.len();
        let angle = 2.0 * sin_half.atan2(w);
        if !(angle.is_finite() && sin_half > 0.0) {
            return None;
        }
        // Keep the angle within half a turn: the same turn the other way about.
        let (axis, angle) = if angle > std::f64::consts::PI { (-(v / sin_half), std::f64::consts::TAU - angle) } else { (v / sin_half, angle) };
        (angle > 1e-12).then_some((axis, angle))
    }

    /// Whether the move shifts no point within `reach` of the origin by more than `distance`.
    pub fn is_negligible(&self, distance: f64, reach: f64) -> bool {
        let angle = self.axis_angle().map_or(0.0, |(_, a)| a);
        self.translation.len() + angle * reach <= distance
    }
}

/// The smallest turn that takes the unit `from` onto the unit `to` (half a turn about a square
/// direction when they are opposite).
pub(crate) fn turn_onto(from: Vec3, to: Vec3) -> Rigid {
    let cross = from.cross(to);
    let cos = from.dot(to).clamp(-1.0, 1.0);
    match cross.normalized() {
        Some(axis) if cross.len() > 1e-12 => Rigid::turn(axis, cross.len().atan2(cos)),
        _ if cos > 0.0 => Rigid::shift(Vec3::ZERO),
        _ => Rigid::turn(from.any_perp().normalized().unwrap_or(Vec3::X), std::f64::consts::PI),
    }
}

/// Stands the scan on a base: the smallest turn about `anchor` that makes the base's outward
/// `normal` face straight down, then up or down until `anchor` is on the grid (Z = 0). `None` for
/// a normal with no direction.
pub fn stand_on(normal: Vec3, anchor: Vec3) -> Option<Rigid> {
    let normal = normal.normalized()?;
    if !anchor.is_finite() {
        return None;
    }
    let mut stand = turn_onto(normal, -Vec3::Z).about(anchor);
    stand.translation.z -= stand.apply(anchor).z;
    Some(stand)
}

/// Stands the scan on a base as [`stand_on`] does, then sideways until the middle of the base is
/// over the origin: the face lies flat on the grid's plane at the origin, even when some of the
/// part reaches lower (a face picked part way up), and standing it there again does nothing.
pub fn stand(normal: Vec3, anchor: Vec3) -> Option<Rigid> {
    let mut stand = stand_on(normal, anchor)?;
    let middle = stand.apply(anchor);
    stand.translation.x -= middle.x;
    stand.translation.y -= middle.y;
    Some(stand)
}

/// Up or down until the scan's lowest point is on the grid (Z = 0).
pub fn rest_on_grid(scan: &Scan) -> Option<Rigid> {
    let (lo, _) = scan.bounds()?;
    Some(Rigid::shift(Vec3::new(0.0, 0.0, -lo.z)))
}

/// Sideways until the middle of the scan's box is over the origin; its height is kept.
pub fn center_on_grid(scan: &Scan) -> Option<Rigid> {
    let (lo, hi) = scan.bounds()?;
    let middle = (lo + hi) / 2.0;
    Some(Rigid::shift(Vec3::new(-middle.x, -middle.y, 0.0)))
}

/// Lays the scan along its own directions, longest along X, thinnest along Z, with the middle of
/// its surface at the origin. `None` for a scan with no area or one too round to have a longest
/// or a thinnest direction.
pub fn quick_align(scan: &Scan) -> Option<Rigid> {
    let centre = surface_centre(scan)?;
    let mut spread = [[0.0f64; 3]; 3];
    // The exact second moment of each triangle about the centre, ∫ d dᵀ dA = A/12 (Σ dᵢdᵢᵀ + s sᵀ)
    // with s = Σ dᵢ, so that the way a mesh is cut into triangles does not lean the axes.
    for t in 0..scan.triangle_count() {
        let Some(corners) = scan.corners(t) else { continue };
        let [a, b, c] = corners;
        let w = (b - a).cross(c - a).len() / 24.0;
        if !w.is_finite() {
            continue;
        }
        let d = corners.map(|p| p - centre);
        let s = d[0] + d[1] + d[2];
        for q in [d[0], d[1], d[2], s] {
            let q = [q.x, q.y, q.z];
            for (row, qi) in spread.iter_mut().zip(q) {
                for (cell, qj) in row.iter_mut().zip(q) {
                    *cell += w * qi * qj;
                }
            }
        }
    }
    let (values, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    // Smallest first: a scan whose spreads are all within 1 % has no direction to lay along.
    if !(values[2] > 0.0 && (values[1] < 0.99 * values[2] || values[0] < 0.99 * values[1])) {
        return None;
    }
    let mut x = axes[2].normalized()?;
    let mut z = axes[0].normalized()?;
    if x.x < 0.0 {
        x = -x;
    }
    if z.z < 0.0 {
        z = -z;
    }
    let y = z.cross(x).normalized()?;
    let x = y.cross(z).normalized()?;
    let turn = Rigid { rotation: [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]], translation: Vec3::ZERO };
    Some(Rigid { translation: -turn.turned(centre), ..turn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{cuboid, turned};

    fn close(a: Vec3, b: Vec3, tol: f64) -> bool {
        (a - b).len() <= tol
    }

    #[test]
    fn axis_angle_gives_back_the_turn_it_came_from() {
        for (axis, angle) in [(Vec3::Z, 0.3), (Vec3::new(1.0, 2.0, -0.5), 2.9), (Vec3::X, std::f64::consts::PI), (Vec3::new(0.0, 1.0, 1.0), 1.0)] {
            let axis = axis.normalized().unwrap();
            let r = Rigid::turn(axis, angle);
            let (a, t) = r.axis_angle().unwrap();
            let again = Rigid::turn(a, t);
            for v in [Vec3::X, Vec3::Y, Vec3::Z] {
                assert!(close(r.turned(v), again.turned(v), 1e-12), "{axis:?} {angle}");
            }
        }
        assert!(Rigid::shift(Vec3::new(1.0, 2.0, 3.0)).axis_angle().is_none());
    }

    #[test]
    fn standing_on_a_side_puts_that_side_face_down_on_the_grid() {
        let anchor = Vec3::new(10.0, 3.0, 7.0);
        for normal in [Vec3::X, -Vec3::Y, Vec3::Z, -Vec3::Z, Vec3::new(1.0, 1.0, 1.0)] {
            let stand = stand_on(normal, anchor).unwrap();
            let n = normal.normalized().unwrap();
            assert!(close(stand.turned(n), -Vec3::Z, 1e-12), "{normal:?} faces down");
            assert!(stand.apply(anchor).z.abs() < 1e-12, "{normal:?} on the grid");
            let a = stand.apply(anchor);
            assert!((a.x - anchor.x).abs() < 1e-9 && (a.y - anchor.y).abs() < 1e-9, "the smallest turn about the anchor");
        }
        assert!(stand_on(Vec3::ZERO, anchor).is_none());
    }

    #[test]
    fn rest_centre_and_quick_align_place_a_box() {
        let (positions, triangles) = cuboid(Vec3::new(5.0, 6.0, 7.0), Vec3::new(65.0, 26.0, 12.0));
        let scan = Scan::new(&positions, &triangles);
        assert_eq!(rest_on_grid(&scan).unwrap().translation, Vec3::new(0.0, 0.0, -7.0));
        assert_eq!(center_on_grid(&scan).unwrap().translation, Vec3::new(-35.0, -16.0, 0.0));

        // Turned and moved anywhere, quick align lays it 60 along X, 20 along Y, 5 along Z, about the origin.
        let axis = Vec3::new(0.3, -0.5, 0.8).normalized().unwrap();
        let moved = turned(&positions, axis, 0.7, Vec3::new(100.0, -40.0, 15.0));
        let scan = Scan::new(&moved, &triangles);
        let align = quick_align(&scan).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| align.apply(*p)).collect();
        let lo = placed.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |m, p| m.min(*p));
        let hi = placed.iter().fold(Vec3::new(f64::MIN, f64::MIN, f64::MIN), |m, p| m.max(*p));
        assert!(close(hi - lo, Vec3::new(60.0, 20.0, 5.0), 1e-9), "{:?}", hi - lo);
        assert!(close((hi + lo) / 2.0, Vec3::ZERO, 1e-9));
    }

    #[test]
    fn a_cube_is_too_round_to_lay_along() {
        let (positions, triangles) = cuboid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        assert!(quick_align(&Scan::new(&positions, &triangles)).is_none());
    }
}
