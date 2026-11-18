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

    /// The rigid move that takes the points `from` onto `to` (the same points, in order), if there
    /// is one: worked out from three of them well apart and checked on every point.
    pub fn between(from: &[Vec3], to: &[Vec3]) -> Option<Rigid> {
        if from.len() != to.len() || from.len() < 3 {
            return None;
        }
        let a = 0;
        let b = (0..from.len()).max_by(|&i, &j| from[i].dist(from[a]).total_cmp(&from[j].dist(from[a])))?;
        let c = (0..from.len()).max_by(|&i, &j| {
            let area = |k: usize| (from[b] - from[a]).cross(from[k] - from[a]).len();
            area(i).total_cmp(&area(j))
        })?;
        let frame = |net: &[Vec3]| -> Option<[Vec3; 3]> {
            let e1 = (net[b] - net[a]).normalized()?;
            let e2 = ((net[c] - net[a]) - e1 * (net[c] - net[a]).dot(e1)).normalized()?;
            Some([e1, e2, e1.cross(e2)])
        };
        let (f, g) = (frame(from)?, frame(to)?);
        // R = G Fᵀ: the frame of `from` turned onto the frame of `to`.
        let row = |i: usize| {
            let pick = |v: Vec3| [v.x, v.y, v.z][i];
            [0, 1, 2].map(|j| (0..3).map(|k| pick(g[k]) * [f[k].x, f[k].y, f[k].z][j]).sum::<f64>())
        };
        let turn = Rigid { rotation: [row(0), row(1), row(2)], translation: Vec3::ZERO };
        let moved = Rigid { translation: to[a] - turn.turned(from[a]), ..turn };
        let size = from.iter().map(|p| p.dist(from[a])).fold(1.0, f64::max);
        from.iter().zip(to).all(|(p, q)| moved.apply(*p).dist(*q) <= 1e-7 * size).then_some(moved)
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

/// Lays the scan along its own directions, square to its own faces: longest along X, thinnest
/// along Z, centred over the origin and resting on the grid. Its spread gives the first guess;
/// then each axis is turned to where the box round the scan is smallest, so a part of flat faces
/// lies square to them (a part 96 x 60 x 56 mm, sides near alike in size, was left turned 10° by
/// its spread alone, and its middle at the origin sank half of it through the grid). `None` for a
/// scan with no area.
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
    // The spread's axes, largest first, when they are told apart; else the scan's own.
    let first: [Vec3; 3] = match solidcraft_geom::inertia::principal(&spread, 0.0) {
        Some((values, axes)) if values[2] > 0.0 && (values[1] < 0.99 * values[2] || values[0] < 0.99 * values[1]) => {
            [axes[2].normalized()?, axes[1].normalized()?, axes[0].normalized()?]
        }
        _ => [Vec3::X, Vec3::Y, Vec3::Z],
    };
    // The scan's points, a sample, about its centre in those axes.
    let used: Vec<Vec3> = (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).flatten().collect();
    let sample = crate::thinned(&used, 20_000);
    let mut frame = first;
    let local = |frame: &[Vec3; 3], p: Vec3| {
        let d = p - centre;
        [d.dot(frame[0]), d.dot(frame[1]), d.dot(frame[2])]
    };
    // Each axis in turn, the frame turned about it to where the box across the other two is
    // smallest: whole degrees first, then a fiftieth of one; three rounds.
    for _ in 0..3 {
        for k in 0..3 {
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let points: Vec<(f64, f64)> = sample.iter().map(|p| local(&frame, *p)).map(|q| (q[a], q[b])).collect();
            let area = |angle: f64| {
                let (sn, cs) = angle.sin_cos();
                let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for (u, v) in &points {
                    let (x, y) = (cs * u + sn * v, -sn * u + cs * v);
                    lo = [lo[0].min(x), lo[1].min(y)];
                    hi = [hi[0].max(x), hi[1].max(y)];
                }
                (hi[0] - lo[0]) * (hi[1] - lo[1])
            };
            let best = |from: f64, to: f64, step: f64| {
                let n = ((to - from) / step).round() as i64;
                (0..=n).map(|i| from + step * i as f64).min_by(|x, y| area(*x).total_cmp(&area(*y))).unwrap_or(0.0)
            };
            let degree = std::f64::consts::PI / 180.0;
            let coarse = best(-45.0 * degree, 45.0 * degree, degree);
            let fine = best(coarse - degree, coarse + degree, degree / 50.0);
            if fine.abs() < 1e-12 {
                continue;
            }
            let (sn, cs) = fine.sin_cos();
            let (u, v) = (frame[a], frame[b]);
            frame[a] = (u * cs + v * sn).normalized()?;
            frame[b] = (v * cs - u * sn).normalized()?;
        }
    }
    // Longest along X, thinnest along Z, by the box's own sides.
    let extent = |axis: Vec3| {
        let (lo, hi) = sample.iter().map(|p| (*p - centre).dot(axis)).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        hi - lo
    };
    let mut order = frame;
    order.sort_by(|p, q| extent(*q).total_cmp(&extent(*p)));
    let mut x = order[0];
    let mut z = order[2];
    if x.x < 0.0 {
        x = -x;
    }
    if z.z < 0.0 {
        z = -z;
    }
    let y = z.cross(x).normalized()?;
    let x = y.cross(z).normalized()?;
    let turn = Rigid { rotation: [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]], translation: Vec3::ZERO };
    // Centred over the origin, resting on the grid.
    let placed: Vec<Vec3> = used.iter().map(|p| turn.turned(*p)).collect();
    let (lo, hi) = placed.iter().fold(
        (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    );
    Some(Rigid { translation: Vec3::new(-(lo.x + hi.x) / 2.0, -(lo.y + hi.y) / 2.0, -lo.z), ..turn })
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

        // Turned and moved anywhere, quick align lays it 60 along X, 20 along Y, 5 along Z, centred
        // over the origin, resting on the grid.
        let axis = Vec3::new(0.3, -0.5, 0.8).normalized().unwrap();
        let moved = turned(&positions, axis, 0.7, Vec3::new(100.0, -40.0, 15.0));
        let scan = Scan::new(&moved, &triangles);
        let align = quick_align(&scan).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| align.apply(*p)).collect();
        let lo = placed.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |m, p| m.min(*p));
        let hi = placed.iter().fold(Vec3::new(f64::MIN, f64::MIN, f64::MIN), |m, p| m.max(*p));
        assert!(close(hi - lo, Vec3::new(60.0, 20.0, 5.0), 1e-6), "{:?}", hi - lo);
        assert!(close(Vec3::new((hi.x + lo.x) / 2.0, (hi.y + lo.y) / 2.0, lo.z), Vec3::ZERO, 1e-6), "{lo:?} {hi:?}");
    }

    /// A block whose sides are near alike (60 x 40 x 36) turned anywhere: its spread alone left it
    /// turned; squared to its faces, it lies 60 x 40 x 36, on the grid.
    #[test]
    fn quick_align_squares_a_block_whose_sides_are_near_alike() {
        let (positions, triangles) = cuboid(Vec3::ZERO, Vec3::new(60.0, 40.0, 36.0));
        let axis = Vec3::new(0.3, 1.0, 0.2).normalized().unwrap();
        let moved = turned(&positions, axis, 0.61, Vec3::new(20.0, 10.0, 25.0));
        let align = quick_align(&Scan::new(&moved, &triangles)).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| align.apply(*p)).collect();
        let lo = placed.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |m, p| m.min(*p));
        let hi = placed.iter().fold(Vec3::new(f64::MIN, f64::MIN, f64::MIN), |m, p| m.max(*p));
        assert!(close(hi - lo, Vec3::new(60.0, 40.0, 36.0), 1e-3), "{:?}", hi - lo);
        assert!(lo.z.abs() < 1e-6);
    }

    /// A cube has no longest side, but it is still squared to its faces and set on the grid.
    #[test]
    fn a_cube_is_squared_to_its_faces() {
        let (positions, triangles) = cuboid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let moved = turned(&positions, Vec3::new(1.0, 1.0, 0.0).normalized().unwrap(), 0.4, Vec3::ZERO);
        let align = quick_align(&Scan::new(&moved, &triangles)).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| align.apply(*p)).collect();
        let lo = placed.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |m, p| m.min(*p));
        let hi = placed.iter().fold(Vec3::new(f64::MIN, f64::MIN, f64::MIN), |m, p| m.max(*p));
        assert!(close(hi - lo, Vec3::new(10.0, 10.0, 10.0), 0.01), "{:?}", hi - lo);
    }
}
