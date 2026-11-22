//! A bicubic net fitted to scan points in all three directions by least squares, held smooth by
//! the bending of its net, with each point's place on the square moved to the foot of its point
//! on the surface between rounds.
//!
//! A point fitted at the place a flattening or a plane gave it pulls the surface along itself as
//! well as across it: where the places are stretched or squeezed against the surface's own
//! parameters, the least squares answer bends off the part to meet them. Moving each place to
//! where its point's perpendicular meets the surface fitted so far (parameter correction, as in
//! Hoschek's "Intrinsic parametrization for approximation", 1988, and Weiss, Andor, Renner and
//! Várady's "Advanced surface fitting techniques", 2002) leaves only the distance across the
//! surface for the next fit to close. SurfaceCraft's patch fit does the same, four rounds of two
//! Gauss–Newton steps a point.
//!
//! The bending is the net's second differences along u and v and twice its twist, the discrete
//! form of the thin plate energy `∫ S_uu² + 2 S_uv² + S_vv²`: second differences alone charge a
//! saddle nothing, so where no point holds the net it would be free to twist.

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::surface::{cholesky_back, cholesky_factor, clamped_knots};
use crate::surface_edit::frame;

/// Rounds of fitting: the first at the places given, each later one after the places are
/// corrected on the surface the round before made.
pub(crate) const ROUNDS: usize = 4;

/// Gauss–Newton steps a place takes toward its foot each time it is corrected.
const STEPS: usize = 2;

/// What a point's place may do when it is corrected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hold {
    /// Go anywhere on the square.
    Free,
    /// Keep its u (it lies on the side u = 0 or u = 1) and slide along v.
    U,
    /// Keep its v and slide along u.
    V,
    /// Stay where it is (a corner).
    Both,
}

/// A net fitted to points, the points' places on it, and how far they are from it there.
#[derive(Clone, Debug)]
pub(crate) struct Fitted {
    pub surface: NurbsSurface,
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests check where the places went; the fitters keep only the surface"))]
    pub places: Vec<Vec2>,
    /// Root mean square and largest distance of the points from the surface at their places, which
    /// the last correction has taken to their feet (mm).
    pub rms: f64,
    pub max: f64,
}

/// A bicubic net `counts` across fitted to `samples` (each a place on the unit square and its
/// point), `rounds` times with the places corrected between, the bending weighed `weight` times the
/// data's own weight (the mean diagonal of the normal equations). `hold` says what each place may
/// do when corrected (default: anything). `None` when the equations have no answer.
pub(crate) fn fit(samples: &[(Vec2, Vec3)], hold: Option<&[Hold]>, counts: [usize; 2], weight: f64, rounds: usize) -> Option<Fitted> {
    let [nu, nv] = counts;
    if nu < 4 || nv < 4 || samples.is_empty() {
        return None;
    }
    let (ku, kv) = (clamped_knots(nu), clamped_knots(nv));
    let mut places: Vec<Vec2> = samples.iter().map(|(q, _)| Vec2::new(unit(q.x), unit(q.y))).collect();
    let points: Vec<Vec3> = samples.iter().map(|(_, p)| *p).collect();
    let mut surface = solve(&points, &places, &ku, &kv, [nu, nv], weight)?;
    for _ in 1..rounds.max(1) {
        correct(&surface, &points, &mut places, hold);
        surface = solve(&points, &places, &ku, &kv, [nu, nv], weight)?;
    }
    correct(&surface, &points, &mut places, hold);
    let net = crate::surface_edit::points(&surface);
    let (mut squares, mut max) = (0.0f64, 0.0f64);
    for (q, p) in places.iter().zip(&points) {
        if let Some((s, _, _)) = frame(&surface, &net, q.x, q.y) {
            let d = s.dist(*p);
            squares += d * d;
            max = max.max(d);
        }
    }
    let rms = (squares / points.len() as f64).sqrt();
    Some(Fitted { surface, places, rms, max })
}

/// The share of a region's area that may face across or away from its plane (within about 78° of
/// lying along it) before no plane sees it as a height field.
const ACROSS: f64 = 0.05;

/// Places on the unit square for `points` over their own plane: through their middle, square to
/// the way they spread least, its axes along their widest spread and across it, scaled to their
/// box. `facing` is the region's triangles' normals and areas. With `strict`, `None` when more
/// than a twentieth of the area faces across or away from the plane (a region that curves round,
/// which a plane folds onto itself); without, only when more than that faces away.
pub(crate) fn over_plane(points: &[Vec3], facing: &[(Vec3, f64)], strict: bool) -> Option<Vec<Vec2>> {
    if points.len() < 3 {
        return None;
    }
    let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in points {
        let d = *p - middle;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    let mut normal = axes[0].normalized()?;
    let mean = facing.iter().fold(Vec3::ZERO, |s, (f, w)| s + *f * *w);
    if normal.dot(mean) < 0.0 {
        normal = -normal;
    }
    let total: f64 = facing.iter().map(|(_, w)| w).sum();
    let limit = if strict { 0.2 } else { 0.0 };
    let across: f64 = facing.iter().filter(|(f, _)| f.dot(normal) < limit).map(|(_, w)| w).sum();
    if !(total > 0.0) || across > ACROSS * total {
        return None;
    }
    let e1 = axes[2].normalized()?;
    let e2 = normal.cross(e1);
    let flat: Vec<Vec2> = points.iter().map(|p| Vec2::new((*p - middle).dot(e1), (*p - middle).dot(e2))).collect();
    let (lo, hi) = flat.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    let size = Vec2::new(hi.x - lo.x, hi.y - lo.y);
    if !(size.x > 0.0 && size.y > 0.0) {
        return None;
    }
    Some(flat.iter().map(|q| Vec2::new((q.x - lo.x) / size.x, (q.y - lo.y) / size.y)).collect())
}

/// A parameter kept on the unit square, a value not a number at its middle.
fn unit(x: f64) -> f64 {
    if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.5 }
}

/// The least squares net for the points at `places`, held by the bending.
fn solve(points: &[Vec3], places: &[Vec2], ku: &[f64], kv: &[f64], [nu, nv]: [usize; 2], weight: f64) -> Option<NurbsSurface> {
    let k = nu * nv;
    let mut m = vec![0.0; k * k];
    let mut rhs = [vec![0.0; k], vec![0.0; k], vec![0.0; k]];
    for (q, p) in places.iter().zip(points) {
        let Some((cols, vals)) = row(ku, kv, nu, nv, q.x, q.y) else { continue };
        for a in 0..16 {
            for b in 0..16 {
                m[cols[a] * k + cols[b]] += vals[a] * vals[b];
            }
            rhs[0][cols[a]] += vals[a] * p.x;
            rhs[1][cols[a]] += vals[a] * p.y;
            rhs[2][cols[a]] += vals[a] * p.z;
        }
    }
    let scale = (0..k).map(|i| m[i * k + i]).sum::<f64>() / k as f64;
    let lambda = weight.max(0.0) * scale.max(1e-12);
    let at = |i: usize, j: usize| j * nu + i;
    let mut add = |terms: &[(usize, f64)], w: f64| {
        for &(a, wa) in terms {
            for &(b, wb) in terms {
                m[a * k + b] += lambda * w * wa * wb;
            }
        }
    };
    for j in 0..nv {
        for i in 1..nu - 1 {
            add(&[(at(i - 1, j), 1.0), (at(i, j), -2.0), (at(i + 1, j), 1.0)], 1.0);
        }
    }
    for i in 0..nu {
        for j in 1..nv - 1 {
            add(&[(at(i, j - 1), 1.0), (at(i, j), -2.0), (at(i, j + 1), 1.0)], 1.0);
        }
    }
    for j in 0..nv - 1 {
        for i in 0..nu - 1 {
            add(&[(at(i, j), 1.0), (at(i + 1, j), -1.0), (at(i, j + 1), -1.0), (at(i + 1, j + 1), 1.0)], 2.0);
        }
    }
    if !cholesky_factor(&mut m, k) {
        return None;
    }
    let [x, y, z] = rhs.map(|r| cholesky_back(&m, &r, k));
    let (x, y, z) = (x?, y?, z?);
    let net: Vec<Vec3> = (0..k).map(|i| Vec3::new(x[i], y[i], z[i])).collect();
    NurbsSurface::new([3, 3], [ku.to_vec(), kv.to_vec()], &net, &vec![1.0; k]).ok()
}

/// Each place moved toward the foot of its point on `surface` ([`STEPS`] Gauss–Newton steps on
/// the distance), as its [`Hold`] lets it, kept on the square.
fn correct(surface: &NurbsSurface, points: &[Vec3], places: &mut [Vec2], hold: Option<&[Hold]>) {
    let net = crate::surface_edit::points(surface);
    for (i, (q, p)) in places.iter_mut().zip(points).enumerate() {
        let h = hold.and_then(|h| h.get(i)).copied().unwrap_or(Hold::Free);
        if h == Hold::Both {
            continue;
        }
        let (mut u, mut v) = (q.x, q.y);
        for _ in 0..STEPS {
            let Some((s, su, sv)) = frame(surface, &net, u, v) else { break };
            let d = *p - s;
            let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let (ru, rv) = (d.dot(su), d.dot(sv));
            let (du, dv) = match h {
                Hold::U => (0.0, if c > 1e-18 { rv / c } else { 0.0 }),
                Hold::V => (if a > 1e-18 { ru / a } else { 0.0 }, 0.0),
                _ => {
                    let det = a * c - b * b;
                    if !(det.abs() > 1e-18) {
                        break;
                    }
                    ((c * ru - b * rv) / det, (a * rv - b * ru) / det)
                }
            };
            if !(du.is_finite() && dv.is_finite()) {
                break;
            }
            u = (u + du).clamp(0.0, 1.0);
            v = (v + dv).clamp(0.0, 1.0);
        }
        *q = Vec2::new(u, v);
    }
}

/// The 16 control points a place depends on and their weights, on a net `nu` × `nv` across.
pub(crate) fn row(ku: &[f64], kv: &[f64], nu: usize, nv: usize, s: f64, t: f64) -> Option<([usize; 16], [f64; 16])> {
    let (fu, bu) = solidcraft_geom::bspline::basis_derivatives(ku, 3, nu, s, 0)?;
    let (fv, bv) = solidcraft_geom::bspline::basis_derivatives(kv, 3, nv, t, 0)?;
    let (mut cols, mut vals) = ([0usize; 16], [0.0f64; 16]);
    for a in 0..4 {
        for b in 0..4 {
            cols[b * 4 + a] = (fv + b) * nu + fu + a;
            vals[b * 4 + a] = bu.get(a)?.first()? * bv.get(b)?.first()?;
        }
    }
    Some((cols, vals))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points on a quarter of a cylinder of radius 20 about Y, placed on the square by their
    /// straight-line x and y (as a plane would see them, squeezed toward the sides), not by their
    /// arc.
    fn quarter(n: usize) -> Vec<(Vec2, Vec3)> {
        let mut out = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let a = (i as f64 / n as f64 - 0.5) * std::f64::consts::FRAC_PI_2;
                let p = Vec3::new(20.0 * a.sin(), j as f64 * 30.0 / n as f64, 20.0 * a.cos());
                let x = 20.0 * a.sin() / (20.0 * std::f64::consts::FRAC_PI_4.sin());
                out.push((Vec2::new((x + 1.0) / 2.0, j as f64 / n as f64), p));
            }
        }
        out
    }

    fn off(s: &NurbsSurface) -> f64 {
        let mut worst = 0.0f64;
        for a in 0..=20 {
            for b in 0..=20 {
                let q = s.eval(f64::from(a) / 20.0, f64::from(b) / 20.0).unwrap();
                worst = worst.max(((q.x * q.x + q.z * q.z).sqrt() - 20.0).abs());
            }
        }
        worst
    }

    /// Started from places squeezed hard toward one side, the rounds of correction bring the
    /// surface a third nearer the points; every place stays on the square.
    #[test]
    fn correcting_the_places_brings_the_surface_nearer_the_points() {
        let samples: Vec<(Vec2, Vec3)> = quarter(40).into_iter().map(|(q, p)| (Vec2::new(q.x * q.x, q.y), p)).collect();
        let once = fit(&samples, None, [6, 6], 1e-3, 1).unwrap();
        let corrected = fit(&samples, None, [6, 6], 1e-3, ROUNDS).unwrap();
        assert!(
            corrected.rms < 0.75 * once.rms && corrected.max < 0.75 * once.max,
            "{} {} → {} {}",
            once.rms,
            once.max,
            corrected.rms,
            corrected.max
        );
        assert!(corrected.places.iter().all(|q| (0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y)));
        // Started where the points lie on it (by arc), one fit is already on the part.
        let even: Vec<(Vec2, Vec3)> = (0..=40)
            .flat_map(|j| (0..=40).map(move |i| (i, j)))
            .map(|(i, j)| {
                let a = (i as f64 / 40.0 - 0.5) * std::f64::consts::FRAC_PI_2;
                (Vec2::new(i as f64 / 40.0, j as f64 / 40.0), Vec3::new(20.0 * a.sin(), j as f64 * 0.75, 20.0 * a.cos()))
            })
            .collect();
        let good = fit(&even, None, [6, 6], 1e-4, ROUNDS).unwrap();
        assert!(good.rms < 0.01 && off(&good.surface) < 0.02, "{} {}", good.rms, off(&good.surface));
    }

    #[test]
    fn a_held_place_keeps_its_side() {
        let samples = quarter(20);
        let hold: Vec<Hold> = samples
            .iter()
            .map(|(q, _)| match (q.x == 0.0 || q.x == 1.0, q.y == 0.0 || q.y == 1.0) {
                (true, true) => Hold::Both,
                (true, false) => Hold::U,
                (false, true) => Hold::V,
                _ => Hold::Free,
            })
            .collect();
        let f = fit(&samples, Some(&hold), [6, 6], 1e-2, ROUNDS).unwrap();
        for ((q0, _), (q, h)) in samples.iter().zip(f.places.iter().zip(&hold)) {
            match h {
                Hold::Both => assert_eq!(q0, q),
                Hold::U => assert_eq!(q0.x, q.x),
                Hold::V => assert_eq!(q0.y, q.y),
                Hold::Free => {}
            }
        }
    }

    #[test]
    fn hostile_input_is_refused_or_kept_on_the_square() {
        assert!(fit(&[], None, [6, 6], 0.1, ROUNDS).is_none());
        assert!(fit(&quarter(10), None, [3, 6], 0.1, ROUNDS).is_none());
        let mut bad = quarter(10);
        bad[3].0 = Vec2::new(f64::NAN, f64::INFINITY);
        bad[5].0 = Vec2::new(-5.0, 9.0);
        let f = fit(&bad, None, [6, 6], 0.1, ROUNDS).unwrap();
        assert!(f.places.iter().all(|q| q.x.is_finite() && (0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y)));
        assert!(f.rms.is_finite());
    }
}
