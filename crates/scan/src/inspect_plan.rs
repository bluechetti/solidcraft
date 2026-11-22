//! Measuring an inspection plan's checks on a scan: the scan points that lie on a face of the
//! model (within a band of its exact surface, and on the face itself), fitted the way the check
//! reads them. Least-squares (Gaussian) associated features, robust: points more than three
//! standard deviations off the first fit are dropped and the fit is made again.
//!
//! - A cylinder (a diameter, a hole's position): the points, seen along the nominal axis, fitted
//!   with a circle (Kåsa 1976, then a geometric Gauss–Newton refinement).
//! - A flat face (flatness, a distance): a plane through the points (the smallest eigenvector of
//!   their scatter); flatness is the spread of the points about it.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

/// What a fit found, and from how many points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// Centre in the plane square to the axis, as an offset from the nominal axis point (mm,
    /// along `u` and `v`), and the radius.
    pub centre: [f64; 2],
    pub radius: f64,
    /// Points used, and their root mean square distance off the circle.
    pub points: usize,
    pub rms: f64,
}

/// A nominal cylinder: its axis, radius and extent along the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    pub point: Vec3,
    pub axis: Vec3,
    pub radius: f64,
    pub start: f64,
    pub end: f64,
}

/// A plane fitted to points: a point on it, its unit normal (facing as `facing`), the points' spread
/// about it (largest above minus largest below), and how many points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FittedPlane {
    pub point: Vec3,
    pub normal: Vec3,
    pub spread: f64,
    pub points: usize,
}

/// Fewest points a fit is made from.
pub const LEAST_POINTS: usize = 12;

/// Two directions square to `axis` (unit), square to each other.
fn frame(axis: Vec3) -> (Vec3, Vec3) {
    let u = axis.any_perp().normalized().unwrap_or(Vec3::X);
    (u, axis.cross(u))
}

/// The scan `points` on the nominal cylinder: within `band` of its surface and within its extent
/// (its ends trimmed by a twentieth, where a chamfer or a round begins), seen along its axis.
fn on_cylinder(points: &[Vec3], c: &Cylinder, band: f64) -> Vec<[f64; 2]> {
    let (u, v) = frame(c.axis);
    let trim = 0.05 * (c.end - c.start);
    points
        .iter()
        .filter_map(|&p| {
            let d = p - c.point;
            let s = d.dot(c.axis);
            if !(s >= c.start + trim && s <= c.end - trim) {
                return None;
            }
            let q = [d.dot(u), d.dot(v)];
            ((q[0].hypot(q[1]) - c.radius).abs() <= band).then_some(q)
        })
        .collect()
}

/// A circle through 2D points: Kåsa's algebraic fit, then a few geometric (Gauss–Newton) steps.
fn circle(q: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    if q.len() < 3 {
        return None;
    }
    // Kåsa: x² + y² + D x + E y + F = 0, least squares in (D, E, F).
    let mut m = [[0.0f64; 3]; 3];
    let mut r = [0.0f64; 3];
    for p in q {
        let row = [p[0], p[1], 1.0];
        let z = -(p[0] * p[0] + p[1] * p[1]);
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            r[i] += row[i] * z;
        }
    }
    let [d, e, f] = solve3(m, r)?;
    let mut c = [-d / 2.0, -e / 2.0];
    let mut rad = (c[0] * c[0] + c[1] * c[1] - f).max(0.0).sqrt();
    for _ in 0..10 {
        // Residual rᵢ = |p − c| − R; Jacobian rows [−(p−c)/|p−c|, −1].
        let mut jtj = [[0.0f64; 3]; 3];
        let mut jtr = [0.0f64; 3];
        for p in q {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            let l = dx.hypot(dy);
            if l <= 1e-12 {
                continue;
            }
            let row = [-dx / l, -dy / l, -1.0];
            let res = l - rad;
            for i in 0..3 {
                for j in 0..3 {
                    jtj[i][j] += row[i] * row[j];
                }
                jtr[i] -= row[i] * res;
            }
        }
        let Some(step) = solve3(jtj, jtr) else { break };
        c = [c[0] + step[0], c[1] + step[1]];
        rad += step[2];
        if step.iter().map(|x| x.abs()).fold(0.0, f64::max) < 1e-12 {
            break;
        }
    }
    (rad.is_finite() && rad > 0.0).then_some((c, rad))
}

/// `m x = r` by Cramer's rule; `None` when `m` is (near) singular.
fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(m);
    let scale = m.iter().flatten().map(|x| x.abs()).fold(0.0, f64::max).max(1e-300);
    if !(d.abs() > 1e-12 * scale * scale * scale) {
        return None;
    }
    let mut out = [0.0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        let mut a = m;
        for i in 0..3 {
            a[i][k] = r[i];
        }
        *o = det(a) / d;
    }
    out.iter().all(|x| x.is_finite()).then_some(out)
}

/// The circle the scan holds on the nominal cylinder `c`: fitted, then again without the points
/// more than three standard deviations off it. `None` with fewer than [`LEAST_POINTS`] points.
pub fn fit_cylinder(points: &[Vec3], c: &Cylinder, band: f64) -> Option<Circle> {
    let q = on_cylinder(points, c, band);
    if q.len() < LEAST_POINTS {
        return None;
    }
    let (centre, radius) = circle(&q)?;
    let off = |p: &[f64; 2]| (p[0] - centre[0]).hypot(p[1] - centre[1]) - radius;
    let sigma = (q.iter().map(|p| off(p).powi(2)).sum::<f64>() / q.len() as f64).sqrt();
    let kept: Vec<[f64; 2]> = q.iter().copied().filter(|p| off(p).abs() <= 3.0 * sigma + 1e-12).collect();
    let (centre, radius) = if kept.len() >= LEAST_POINTS { circle(&kept).unwrap_or((centre, radius)) } else { (centre, radius) };
    let used = if kept.len() >= LEAST_POINTS { &kept } else { &q };
    let rms = (used.iter().map(|p| ((p[0] - centre[0]).hypot(p[1] - centre[1]) - radius).powi(2)).sum::<f64>() / used.len() as f64).sqrt();
    Some(Circle { centre, radius, points: used.len(), rms })
}

/// The plane through `points` (each weighted alike), facing as `facing`; `None` for fewer than
/// three points or points on a line.
fn plane(points: &[Vec3], facing: Vec3) -> Option<(Vec3, Vec3)> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let c = points.iter().fold(Vec3::ZERO, |a, p| a + *p) / n;
    let mut m = [[0.0f64; 3]; 3];
    for p in points {
        let d = *p - c;
        let d = [d.x, d.y, d.z];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += d[i] * d[j];
            }
        }
    }
    // The smallest eigenvector, by inverse iteration from the nominal normal.
    let mut x = facing.normalized()?;
    let shift = 1e-9 * (m[0][0] + m[1][1] + m[2][2]).max(1e-300);
    for (i, row) in m.iter_mut().enumerate() {
        row[i] += shift;
    }
    for _ in 0..40 {
        let y = solve3(m, [x.x, x.y, x.z])?;
        let next = Vec3::new(y[0], y[1], y[2]).normalized()?;
        let done = (next - x).len() < 1e-14 || (next + x).len() < 1e-14;
        x = next;
        if done {
            break;
        }
    }
    let x = if x.dot(facing) < 0.0 { -x } else { x };
    Some((c, x))
}

/// The plane the scan holds on a flat face: the points within `band` of the nominal plane (point
/// `at`, unit normal `normal`) that `on_face` keeps, fitted, then again without the points more
/// than three standard deviations off it; `spread` is then the flatness.
pub fn fit_plane(points: &[Vec3], at: Vec3, normal: Vec3, band: f64, on_face: impl Fn(Vec3) -> bool) -> Option<FittedPlane> {
    let near: Vec<Vec3> = points.iter().copied().filter(|p| (*p - at).dot(normal).abs() <= band && on_face(*p)).collect();
    if near.len() < LEAST_POINTS {
        return None;
    }
    let (c, n) = plane(&near, normal)?;
    let off = |p: &Vec3| (*p - c).dot(n);
    let sigma = (near.iter().map(|p| off(p).powi(2)).sum::<f64>() / near.len() as f64).sqrt();
    let kept: Vec<Vec3> = near.iter().copied().filter(|p| off(p).abs() <= 3.0 * sigma + 1e-12).collect();
    let used = if kept.len() >= LEAST_POINTS { kept } else { near };
    let (c, n) = plane(&used, normal).unwrap_or((c, n));
    let (lo, hi) = used.iter().map(|p| (*p - c).dot(n)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), d| (a.min(d), b.max(d)));
    Some(FittedPlane { point: c, normal: n, spread: hi - lo, points: used.len() })
}

/// Whether `p`, seen square to a flat face (unit `normal`), lies inside its outline: inside the
/// first loop (the outer one) and outside every other (its holes), and at least `margin` from all
/// of them (the edge left out, where the scan turns onto the next face). Loops are closed polygons
/// of points on the face.
pub fn inside_outline(p: Vec3, normal: Vec3, loops: &[Vec<Vec3>], margin: f64) -> bool {
    let (u, v) = frame(normal);
    let flat = |q: Vec3| (q.dot(u), q.dot(v));
    let (x, y) = flat(p);
    let inside = |poly: &[Vec3]| {
        let mut c = false;
        let n = poly.len();
        for i in 0..n {
            let (xi, yi) = flat(poly[i]);
            let (xj, yj) = flat(poly[(i + n - 1) % n]);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
        }
        c
    };
    let clear = |poly: &[Vec3]| {
        let n = poly.len();
        (0..n).all(|i| {
            let (ax, ay) = flat(poly[i]);
            let (bx, by) = flat(poly[(i + 1) % n]);
            let (dx, dy) = (bx - ax, by - ay);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (x - ax - t * dx).hypot(y - ay - t * dy) >= margin
        })
    };
    let loops: Vec<&Vec<Vec3>> = loops.iter().filter(|l| l.len() >= 3).collect();
    let Some((outer, holes)) = loops.split_first() else { return false };
    inside(outer) && holes.iter().all(|h| !inside(h)) && (margin <= 0.0 || loops.iter().all(|l| clear(l)))
}

/// How far the scan is off a face at `at` along its unit `normal`: the mean signed distance of the
/// scan points within `reach` of `at` and within `band` of the surface there, and how many.
pub fn offset_at(points: &[Vec3], at: Vec3, normal: Vec3, reach: f64, band: f64) -> Option<(f64, usize)> {
    let d: Vec<f64> = points
        .iter()
        .filter_map(|p| {
            let off = (*p - at).dot(normal);
            let side = (*p - at - normal * off).len();
            (side <= reach && off.abs() <= band).then_some(off)
        })
        .collect();
    (d.len() >= 3).then(|| (d.iter().sum::<f64>() / d.len() as f64, d.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points round a bore Ø10.1 (centre 0.03, -0.02 off the nominal axis) along z 0…20, with a
    /// little noise and a few strays.
    fn bore() -> Vec<Vec3> {
        let mut out = Vec::new();
        let mut k = 0u32;
        for i in 0..40 {
            for j in 0..72 {
                k = k.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (f64::from(k >> 16) / 65536.0 - 0.5) * 0.01;
                let th = std::f64::consts::TAU * f64::from(j) / 72.0;
                let r = 5.05 + noise + if (i * 72 + j) % 400 == 0 { 0.3 } else { 0.0 };
                out.push(Vec3::new(0.03 + r * th.cos(), -0.02 + r * th.sin(), 0.5 * f64::from(i)));
            }
        }
        out
    }

    #[test]
    fn a_bore_reads_its_diameter_and_where_its_axis_is() {
        let nominal = Cylinder { point: Vec3::ZERO, axis: Vec3::Z, radius: 5.0, start: 0.0, end: 20.0 };
        let c = fit_cylinder(&bore(), &nominal, 0.5).unwrap();
        assert!((2.0 * c.radius - 10.1).abs() < 0.005, "{c:?}");
        let (u, v) = frame(Vec3::Z);
        let offset = u * c.centre[0] + v * c.centre[1];
        assert!((offset - Vec3::new(0.03, -0.02, 0.0)).len() < 0.003, "{c:?}");
        assert!(c.rms < 0.01, "the strays dropped: {c:?}");
        assert!(fit_cylinder(&bore(), &Cylinder { radius: 9.0, ..nominal }, 0.5).is_none(), "nothing within the band");
    }

    #[test]
    fn a_face_reads_its_flatness_inside_its_outline_only() {
        // A 20 x 20 face on z = 0 bowed 0.04 in the middle, with a 4 x 4 hole, and points of
        // another face beside it on the same plane.
        let mut pts = Vec::new();
        for i in 0..=40 {
            for j in 0..=40 {
                let (x, y) = (0.5 * f64::from(i), 0.5 * f64::from(j));
                let bow = 0.04 * (1.0 - ((x - 10.0).powi(2) + (y - 10.0).powi(2)) / 200.0);
                pts.push(Vec3::new(x, y, bow));
                pts.push(Vec3::new(x + 30.0, y, 0.5));
            }
        }
        let square = |a: f64, b: f64, c: f64, d: f64| vec![Vec3::new(a, c, 0.0), Vec3::new(b, c, 0.0), Vec3::new(b, d, 0.0), Vec3::new(a, d, 0.0)];
        let loops = vec![square(0.0, 20.0, 0.0, 20.0), square(8.0, 12.0, 8.0, 12.0)];
        let fitted = fit_plane(&pts, Vec3::ZERO, Vec3::Z, 1.0, |p| inside_outline(p, Vec3::Z, &loops, 0.0)).unwrap();
        assert!(fitted.normal.z > 0.999, "{fitted:?}");
        assert!(fitted.spread > 0.02 && fitted.spread < 0.045, "the bow, not the other face: {fitted:?}");
        assert!(!inside_outline(Vec3::new(10.0, 10.0, 0.0), Vec3::Z, &loops, 0.0), "in the hole");
        assert!(!inside_outline(Vec3::new(12.2, 10.0, 0.0), Vec3::Z, &loops, 0.5), "by the hole's edge, within the margin");
        assert!(inside_outline(Vec3::new(2.0, 2.0, 0.0), Vec3::Z, &loops, 0.5));
    }

    #[test]
    fn a_point_reads_how_far_the_scan_is_off() {
        let pts: Vec<Vec3> = (0..100).map(|i| Vec3::new(0.1 * f64::from(i % 10), 0.1 * f64::from(i / 10), 0.07)).collect();
        let (off, n) = offset_at(&pts, Vec3::new(0.5, 0.5, 0.0), Vec3::Z, 0.3, 0.5).unwrap();
        assert!((off - 0.07).abs() < 1e-9 && n > 10);
        assert!(offset_at(&pts, Vec3::new(50.0, 0.0, 0.0), Vec3::Z, 0.3, 0.5).is_none());
    }
}
