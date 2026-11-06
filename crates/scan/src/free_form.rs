//! A Free Form (SurfaceCraft's, after QuickSurface's Free Form and Snap Mode): a surface made by
//! hand on the scan, a cubic B-spline net over four corners clicked round its first face. Fitted
//! to the scan as it is made: the corners put on the scan, each edge the cubic nearest the scan
//! along the straight line between its corners, and the points inside those that bring the face
//! nearest the scan under the Coons patch the edges bound, so the surface lies on the scan and
//! not only its control points. Overall Shape leaves out the places far off the face as fitted,
//! so a face spans openings, dents and ribs; Stiffness keeps the net from bending to the scan.

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{NurbsSurface, basis_derivatives};
use solidcraft_geom::prelude::*;

use crate::surface::{cholesky_solve, clamped_knots};
use crate::surface_edit::basis;

/// The points of a new net each way, at least and at most.
pub const LEAST_POINTS: usize = 4;
pub const MOST_POINTS: usize = 12;

/// How a new face is fitted to the scan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fitting {
    /// The net's points each way.
    pub count: usize,
    /// 0 (the net bends as the scan asks) to 1 (it keeps nearly to the plane of its corners).
    pub stiffness: f64,
    /// Leave out the places far off the face as fitted (the face spans openings and ribs).
    pub overall: bool,
}

impl Default for Fitting {
    fn default() -> Self {
        Fitting { count: LEAST_POINTS, stiffness: 0.0, overall: false }
    }
}

/// A face fitted to the scan: its net (u fastest), and how many places it was fitted to, how
/// many of those met the scan, and how many were kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub surface: NurbsSurface,
    pub places: usize,
    pub met: usize,
    pub kept: usize,
    /// How far the inside's places kept are from the face, root mean square (mm; 0 with none).
    pub rms: f64,
}

/// Places along each edge, and each way across the inside, for each point of the net that way;
/// and how many times the places are found again on what was fitted and it is fitted again (more
/// for Overall Shape, whose places left out are told again each time).
const EDGE_PLACES: usize = 6;
const INSIDE_PLACES: usize = 3;
const ROUNDS: usize = 4;
const OVERALL_ROUNDS: usize = 8;

/// Overall Shape leaves out a place farther from the face than this many times the median
/// distance, never one nearer than `TRIM_FLOOR` of the face's size.
const LEFT_OUT: f64 = 3.0;
const TRIM_FLOOR: f64 = 1e-4;

/// The weight of the net's bending against the places, for each place: a little always, so a part
/// of the net with no place under it (an opening) follows the rest, and up to `MOST_BEND`.
const LEAST_BEND: f64 = 1e-6;
const MOST_BEND: f64 = 10.0;

fn bend_weight(fitting: &Fitting, places: usize, terms: usize) -> f64 {
    (LEAST_BEND + MOST_BEND * fitting.stiffness.clamp(0.0, 1.0).powi(2)) * places.max(1) as f64 / terms.max(1) as f64
}

/// Which of the places at `distances` are kept: all, or for Overall Shape those within `LEFT_OUT`
/// times the median.
fn kept(distances: &[f64], overall: bool, floor: f64) -> Vec<bool> {
    if !overall || distances.is_empty() {
        return vec![true; distances.len()];
    }
    let mut sorted = distances.to_vec();
    sorted.sort_by(f64::total_cmp);
    let limit = (sorted[sorted.len() / 2] * LEFT_OUT).max(floor);
    distances.iter().map(|d| *d <= limit).collect()
}

/// The net over four `corners` taken in order round a face, `count` × `count` points, u fastest:
/// the bilinear patch they make.
pub fn net_over(corners: [Vec3; 4], count: usize) -> Vec<Vec3> {
    let last = (count.max(2) - 1) as f64;
    let mut net = Vec::with_capacity(count * count);
    for j in 0..count {
        let t = j as f64 / last;
        for i in 0..count {
            let s = i as f64 / last;
            net.push(corners[0] * ((1.0 - s) * (1.0 - t)) + corners[1] * (s * (1.0 - t)) + corners[2] * (s * t) + corners[3] * ((1.0 - s) * t));
        }
    }
    net
}

/// A clamped, evenly knotted cubic surface on the net `points` (`count` × `count`, u fastest).
pub fn surface(points: &[Vec3], count: usize) -> Option<NurbsSurface> {
    net_surface(points, [count, count])
}

/// A clamped, evenly knotted cubic surface on the net `points` (`counts[0]` × `counts[1]`, u
/// fastest).
pub fn net_surface(points: &[Vec3], counts: [usize; 2]) -> Option<NurbsSurface> {
    if counts.iter().any(|n| *n < LEAST_POINTS) {
        return None;
    }
    NurbsSurface::new([3, 3], [clamped_knots(counts[0]), clamped_knots(counts[1])], points, &vec![1.0; points.len()]).ok()
}

/// The bending rows of a net: three points in a line each way, u fastest.
fn bending_rows([nu, nv]: [usize; 2]) -> Vec<[usize; 3]> {
    let mut rows = Vec::new();
    for j in 0..nv {
        for i in 1..nu.saturating_sub(1) {
            rows.push([j * nu + i - 1, j * nu + i, j * nu + i + 1]);
        }
    }
    for i in 0..nu {
        for j in 1..nv.saturating_sub(1) {
            rows.push([(j - 1) * nu + i, j * nu + i, (j + 1) * nu + i]);
        }
    }
    rows
}

/// The points of `net` that `slot` numbers (`size` of them; the rest held) fitted to the places
/// `targets`, at `params` on the face, by least squares with the net's bending; each place then
/// found again on the face and the points fitted again, as many rounds as `fitting` asks. Which
/// places were kept at the last, or `None` when the net makes no surface.
#[allow(clippy::too_many_arguments)]
fn settle(
    net: &mut [Vec3],
    counts: [usize; 2],
    targets: &[Vec3],
    params: &mut [(f64, f64)],
    slot: &dyn Fn(usize) -> Option<usize>,
    size: usize,
    fitting: &Fitting,
    floor: f64,
) -> Option<Vec<bool>> {
    let mut rows = bending_rows(counts);
    rows.retain(|row| row.iter().any(|k| slot(*k).is_some()));
    let mut keep = vec![true; targets.len()];
    let rounds = if fitting.overall { OVERALL_ROUNDS } else { ROUNDS };
    for round in 0..rounds {
        let current = net_surface(net, counts)?;
        let mut m = vec![0.0; size * size];
        let mut rhs = vec![Vec3::ZERO; size];
        for ((place, &(u, t)), _) in targets.iter().zip(params.iter()).zip(&keep).filter(|(_, k)| **k) {
            let Some(b) = basis(&current, u, t) else { continue };
            let rest = b.cols.iter().zip(&b.value).filter(|(k, _)| slot(**k).is_none()).fold(*place, |r, (k, w)| r - net[*k] * *w);
            for (&a, &wa) in b.cols.iter().zip(&b.value) {
                let Some(sa) = slot(a) else { continue };
                rhs[sa] += rest * wa;
                for (&bb, &wb) in b.cols.iter().zip(&b.value) {
                    if let Some(sb) = slot(bb) {
                        m[sa * size + sb] += wa * wb;
                    }
                }
            }
        }
        let used = keep.iter().filter(|k| **k).count();
        let held: Vec<Vec3> = net.to_vec();
        add_bending(&mut m, &mut rhs, size, &rows, slot, |k| held[k], bend_weight(fitting, used, rows.len()));
        let Some(moved) = solve(&m, &rhs, size) else { break };
        for (k, point) in net.iter_mut().enumerate() {
            if let Some(s) = slot(k) {
                *point = moved[s];
            }
        }
        if round + 1 == rounds {
            break;
        }
        // Each place found again on the face: two Newton steps to the nearest point.
        let current = net_surface(net, counts)?;
        for (place, (u, t)) in targets.iter().zip(params.iter_mut()) {
            for _ in 0..2 {
                let Some((point, su, st)) = crate::surface_edit::frame(&current, net, *u, *t) else { break };
                let delta = *place - point;
                let (a, b, d) = (su.len2(), su.dot(st), st.len2());
                let det = a * d - b * b;
                if !(det.is_finite() && det.abs() > 1e-18) {
                    break;
                }
                let (ru, rt) = (delta.dot(su), delta.dot(st));
                *u = (*u + (d * ru - b * rt) / det).clamp(0.0, 1.0);
                *t = (*t + (a * rt - b * ru) / det).clamp(0.0, 1.0);
            }
        }
        let distances: Vec<f64> = targets
            .iter()
            .zip(params.iter())
            .map(|(p, (u, t))| crate::surface_edit::frame(&current, net, *u, *t).map_or(f64::INFINITY, |(q, _, _)| p.dist(q)))
            .collect();
        keep = kept(&distances, fitting.overall, floor);
    }
    Some(keep)
}

/// How far the places kept are from the face at their parameters, root mean square (0 with none).
fn rms_off(made: &NurbsSurface, net: &[Vec3], targets: &[Vec3], params: &[(f64, f64)], keep: &[bool]) -> f64 {
    let off: Vec<f64> = targets
        .iter()
        .zip(params)
        .zip(keep)
        .filter(|(_, k)| **k)
        .filter_map(|((p, (u, t)), _)| crate::surface_edit::frame(made, net, *u, *t).map(|(q, _, _)| p.dist(q)))
        .collect();
    if off.is_empty() { 0.0 } else { (off.iter().map(|d| d * d).sum::<f64>() / off.len() as f64).sqrt() }
}

/// The net `net` (`counts`, u fastest) fitted whole to the scan as `fitting` says (its count is
/// not used): places spread evenly across the face the net makes, put on the scan by `snap`
/// (`None` for a place with no scan within reach, which is left out), and every point of the net
/// moved to bring the face nearest them. `None` when the net makes no surface.
pub fn fitted_net(mut net: Vec<Vec3>, counts: [usize; 2], fitting: &Fitting, snap: impl Fn(&[Vec3]) -> Vec<Option<Vec3>>) -> Option<Fitted> {
    let [nu, nv] = counts;
    if net.len() != nu * nv {
        return None;
    }
    let start = net_surface(&net, counts)?;
    let (across_u, across_v) = (INSIDE_PLACES * nu, INSIDE_PLACES * nv);
    let mut params = Vec::with_capacity(across_u * across_v);
    let mut at = Vec::with_capacity(across_u * across_v);
    for j in 0..across_v {
        let t = (j as f64 + 0.5) / across_v as f64;
        for i in 0..across_u {
            let u = (i as f64 + 0.5) / across_u as f64;
            let (p, _, _) = crate::surface_edit::frame(&start, &net, u, t)?;
            params.push((u, t));
            at.push(p);
        }
    }
    let found = snap(&at);
    let (places, met) = (found.len(), found.iter().flatten().count());
    let (targets, mut params): (Vec<Vec3>, Vec<(f64, f64)>) = found.iter().zip(params).filter_map(|(p, uv)| p.map(|p| (p, uv))).unzip();
    let (lo, hi) =
        net.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let floor = TRIM_FLOOR * hi.dist(lo);
    let keep = settle(&mut net, counts, &targets, &mut params, &|k| (k < nu * nv).then_some(k), nu * nv, fitting, floor)?;
    let made = net_surface(&net, counts)?;
    let rms = rms_off(&made, &net, &targets, &params, &keep);
    Some(Fitted { surface: made, places, met, kept: keep.iter().filter(|k| **k).count(), rms })
}

/// The cubic B-spline of `points` (clamped, even knots) at `t`, and its slope.
fn curve_at(points: &[Vec3], knots: &[f64], t: f64) -> Option<(Vec3, Vec3)> {
    let (first, b) = basis_derivatives(knots, 3, points.len(), t, 1)?;
    let (mut p, mut d) = (Vec3::ZERO, Vec3::ZERO);
    for (a, ders) in b.iter().enumerate() {
        let q = *points.get(first + a)?;
        p += q * *ders.first()?;
        d += q * *ders.get(1)?;
    }
    Some((p, d))
}

/// `m x = rhs` for three right-hand sides at once (`k` × `k`, symmetric positive definite).
fn solve(m: &[f64], rhs: &[Vec3], k: usize) -> Option<Vec<Vec3>> {
    let column = |c: usize| rhs.iter().map(|r| [r.x, r.y, r.z][c]).collect::<Vec<f64>>();
    let x = cholesky_solve(&mut m.to_vec(), &column(0), k)?;
    let y = cholesky_solve(&mut m.to_vec(), &column(1), k)?;
    let z = cholesky_solve(&mut m.to_vec(), &column(2), k)?;
    Some((0..k).map(|i| Vec3::new(x[i], y[i], z[i])).collect())
}

/// The normal equations' bending rows for the second differences of the points `rows` (three of
/// the net each, in order), `slot` numbering the points that move (`None` for one held, whose
/// part goes to the right-hand side with `held` its place).
fn add_bending(
    m: &mut [f64],
    rhs: &mut [Vec3],
    size: usize,
    rows: &[[usize; 3]],
    slot: impl Fn(usize) -> Option<usize>,
    held: impl Fn(usize) -> Vec3,
    weight: f64,
) {
    let c = [1.0, -2.0, 1.0];
    for row in rows {
        let fixed = row.iter().zip(c).filter(|(k, _)| slot(**k).is_none()).fold(Vec3::ZERO, |s, (k, ck)| s + held(*k) * ck);
        for (a, ca) in row.iter().zip(c) {
            let Some(sa) = slot(*a) else { continue };
            rhs[sa] -= fixed * (weight * ca);
            for (b, cb) in row.iter().zip(c) {
                if let Some(sb) = slot(*b) {
                    m[sa * size + sb] += weight * ca * cb;
                }
            }
        }
    }
}

/// The cubic of `fitting.count` points from `a` to `b` nearest the places met on the scan (`None`
/// for one with no scan within reach), its inner points by least squares, each place at the
/// parameter nearest it on the curve before (at first, spread evenly); and how many places it kept.
fn edge_fitted(a: Vec3, b: Vec3, places: &[Option<Vec3>], fitting: &Fitting, floor: f64) -> (Vec<Vec3>, usize) {
    let n = fitting.count;
    let knots = clamped_knots(n);
    let last = (places.len().max(2) - 1) as f64;
    let mut hits: Vec<(Vec3, f64)> = places.iter().enumerate().filter_map(|(k, p)| p.map(|p| (p, k as f64 / last))).collect();
    let mut curve: Vec<Vec3> = (0..n).map(|i| a + (b - a) * (i as f64 / (n - 1) as f64)).collect();
    let mut keep = vec![true; hits.len()];
    let rounds = if fitting.overall { OVERALL_ROUNDS } else { ROUNDS };
    let slot = |i: usize| (1..n - 1).contains(&i).then(|| i - 1);
    let size = n - 2;
    for round in 0..rounds {
        let mut m = vec![0.0; size * size];
        let mut rhs = vec![Vec3::ZERO; size];
        for (&(place, t), _) in hits.iter().zip(&keep).filter(|(_, k)| **k) {
            let Some((first, b)) = basis_derivatives(&knots, 3, n, t, 0) else { continue };
            let w = |i: usize| b.get(i).and_then(|d| d.first()).copied().unwrap_or(0.0);
            let rest = (0..b.len()).filter(|&i| slot(first + i).is_none()).fold(place, |r, i| r - curve[first + i] * w(i));
            for i in 0..b.len() {
                let Some(si) = slot(first + i) else { continue };
                rhs[si] += rest * w(i);
                for k in 0..b.len() {
                    if let Some(sk) = slot(first + k) {
                        m[si * size + sk] += w(i) * w(k);
                    }
                }
            }
        }
        let rows: Vec<[usize; 3]> = (1..n - 1).map(|i| [i - 1, i, i + 1]).collect();
        let used = keep.iter().filter(|k| **k).count();
        add_bending(&mut m, &mut rhs, size, &rows, slot, |k| curve[k], bend_weight(fitting, used, rows.len()));
        let Some(inner) = solve(&m, &rhs, size) else { break };
        curve[1..n - 1].copy_from_slice(&inner);
        if round + 1 == rounds {
            break;
        }
        for (place, t) in &mut hits {
            for _ in 0..2 {
                let Some((point, slope)) = curve_at(&curve, &knots, *t) else { break };
                let length = slope.len2();
                if !(length.is_finite() && length > 1e-18) {
                    break;
                }
                *t = (*t + (*place - point).dot(slope) / length).clamp(0.0, 1.0);
            }
        }
        let distances: Vec<f64> = hits.iter().map(|(p, t)| curve_at(&curve, &knots, *t).map_or(f64::INFINITY, |(q, _)| p.dist(q))).collect();
        keep = kept(&distances, fitting.overall, floor);
    }
    (curve, keep.iter().filter(|k| **k).count())
}

/// The net over `corners` fitted to the scan as `fitting` says, `snap` putting places on it
/// (`None` for a place with no scan within reach, which is left out): the corners put on the scan
/// (one with none within reach stays where it was clicked), each edge the curve nearest the scan
/// over the straight line between its corners, and the points inside those that bring the face
/// nearest the scan under the Coons patch the edges bound. `None` when the net cannot be made.
pub fn fitted(corners: [Vec3; 4], fitting: &Fitting, snap: impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>) -> Option<Fitted> {
    let n = fitting.count.clamp(LEAST_POINTS, MOST_POINTS);
    let fitting = Fitting { count: n, ..*fitting };
    let on = snap(&corners, None);
    // The places along the edges and across the inside go to the scan along the way the face
    // faces as a whole (QuickSurface's ray projection), not to the scan's nearest place: on a rough
    // scan the nearest place is often off to one side, and places drawn sideways leave the net
    // uneven, its edges zigzagging.
    let facing = (corners[2] - corners[0]).cross(corners[3] - corners[1]).normalized();
    let snap = |ps: &[Vec3]| snap(ps, facing);
    let c: [Vec3; 4] = std::array::from_fn(|k| on.get(k).copied().flatten().unwrap_or(corners[k]));
    let floor = TRIM_FLOOR * c[2].dist(c[0]).max(c[3].dist(c[1]));
    let edge_places = EDGE_PLACES * n;
    let (mut places, mut met, mut used) = (0, 0, 0);
    let mut edge = |a: Vec3, b: Vec3| {
        let on = snap(&(0..edge_places).map(|k| a + (b - a) * (k as f64 / (edge_places - 1) as f64)).collect::<Vec<_>>());
        let (curve, kept) = edge_fitted(a, b, &on, &fitting, floor);
        places += on.len();
        met += on.iter().flatten().count();
        used += kept;
        curve
    };
    let (below, right, above, left) = (edge(c[0], c[1]), edge(c[1], c[2]), edge(c[3], c[2]), edge(c[0], c[3]));
    let mut net = net_over(c, n);
    for i in 0..n {
        net[i] = below[i];
        net[(n - 1) * n + i] = above[i];
        net[i * n] = left[i];
        net[i * n + n - 1] = right[i];
    }
    let knots = clamped_knots(n);
    let along = |points: &[Vec3], t: f64| curve_at(points, &knots, t).map_or(Vec3::ZERO, |(p, _)| p);
    // The Coons patch the edges bound, at places across the inside, put on the scan.
    let inside = INSIDE_PLACES * n;
    let mut params = Vec::new();
    let mut coons = Vec::new();
    for j in 0..inside {
        let t = (j as f64 + 0.5) / inside as f64;
        for i in 0..inside {
            let u = (i as f64 + 0.5) / inside as f64;
            let ruled = along(&below, u) * (1.0 - t) + along(&above, u) * t + along(&left, t) * (1.0 - u) + along(&right, t) * u;
            let corner = c[0] * ((1.0 - u) * (1.0 - t)) + c[1] * (u * (1.0 - t)) + c[2] * (u * t) + c[3] * ((1.0 - u) * t);
            params.push((u, t));
            coons.push(ruled - corner);
        }
    }
    let found = snap(&coons);
    places += found.len();
    met += found.iter().flatten().count();
    let (targets, mut params): (Vec<Vec3>, Vec<(f64, f64)>) = found.iter().zip(params).filter_map(|(p, uv)| p.map(|p| (p, uv))).unzip();
    // The points inside the net, by least squares with the edges held.
    let size = (n - 2) * (n - 2);
    let slot = |k: usize| {
        let (i, j) = (k % n, k / n);
        ((1..n - 1).contains(&i) && (1..n - 1).contains(&j)).then(|| (j - 1) * (n - 2) + i - 1)
    };
    let keep = settle(&mut net, [n, n], &targets, &mut params, &slot, size, &fitting, floor)?;
    used += keep.iter().filter(|k| **k).count();
    let made = surface(&net, n)?;
    let rms = rms_off(&made, &net, &targets, &params, &keep);
    Some(Fitted { surface: made, places, met, kept: used, rms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Projector;
    use crate::surface_edit::{distance, points};
    use crate::{Scan, surface_edit};

    /// A grid mesh of `z = f(x, y)` over 60 × 60, `n` squares a side.
    fn field(n: u32, f: impl Fn(f64, f64) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
                positions.push(Vec3::new(x, y, f(x, y)));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        (positions, triangles)
    }

    fn dome(x: f64, y: f64) -> f64 {
        8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2))
    }

    /// `p` on the scan: along `along` where the line meets it, else its nearest place.
    fn snapped(on: &Projector, p: Vec3, along: Option<Vec3>) -> Option<Vec3> {
        along.and_then(|d| on.along(p, d)).map(|(q, _)| q).or_else(|| on.nearest(p).map(|(q, _)| q))
    }

    /// On a rough scan (a dome with bumps), the places go to the scan along the face's normal, so
    /// the net stays even: its edges do not zigzag as the bumps would draw them.
    #[test]
    fn on_a_rough_scan_the_net_stays_even() {
        let rough = |x: f64, y: f64| dome(x, y) + 0.6 * ((x * 1.7).sin() * (y * 1.3).cos());
        let (positions, triangles) = field(90, rough);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let corners = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let fit = fitted(corners, &Fitting { count: 6, ..Fitting::default() }, snap).unwrap();
        let check = crate::surface_check::check_whole(&fit.surface, 24).unwrap();
        assert!(check.is_clean() && check.least_angle > 70.0 && check.stretch < 1.6, "{check:?}");
        // The edge along y = 10 stays near it: no point of it pulled sideways.
        let net = points(&fit.surface);
        let off = (0..6).map(|i| (net[i].y - 10.0).abs()).fold(0.0, f64::max);
        assert!(off < 1.0, "the edge's points keep to y = 10: {off}");
    }

    /// Four corners clicked round a dome: the face fitted to the scan lies on it (not only its
    /// control points), where the net left flat over the corners does not.
    #[test]
    fn a_face_over_a_dome_lies_on_it() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let corners = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        let fit = fitted(corners, &Fitting { count: 6, ..Fitting::default() }, snap).unwrap();
        assert_eq!(fit.surface.count(), [6, 6]);
        assert!(fit.met == fit.places && fit.kept > 0);
        let under = surface_edit::covered(&scan, &fit.surface, 1.0, 5_000);
        let (rms, max) = distance(&fit.surface, &under);
        assert!(rms < 0.01 && max < 0.05, "on the dome: {rms} {max}");
        let flat = surface(&net_over(corners, 6), 6).unwrap();
        let (flat_rms, _) = distance(&flat, &surface_edit::covered(&scan, &flat, 10.0, 5_000));
        assert!(flat_rms > 0.5, "flat over the corners is not: {flat_rms}");
        // The corners went onto the dome.
        assert!(on.nearest(points(&fit.surface)[0]).is_some_and(|(_, d)| d < 1e-9));
    }

    /// Overall Shape spans a rib: the face stays near the panel where Follow the Scan bends over it.
    #[test]
    fn overall_shape_spans_a_rib() {
        let rib = |x: f64, y: f64| if (28.0..32.0).contains(&x) { 4.0 } else { 0.0 } + 0.0 * y;
        let (positions, triangles) = field(60, rib);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let corners = [Vec3::new(5.0, 5.0, 0.0), Vec3::new(55.0, 5.0, 0.0), Vec3::new(55.0, 55.0, 0.0), Vec3::new(5.0, 55.0, 0.0)];
        let follow = fitted(corners, &Fitting { count: 6, ..Fitting::default() }, snap).unwrap();
        let overall = fitted(corners, &Fitting { count: 6, overall: true, ..Fitting::default() }, snap).unwrap();
        let middle = |f: &Fitted| f.surface.eval(0.5, 0.5).unwrap().z;
        assert!(middle(&overall).abs() < middle(&follow).abs(), "{} {}", middle(&overall), middle(&follow));
        assert!(overall.kept < follow.kept, "the rib's places left out");
    }
}
