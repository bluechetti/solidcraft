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

/// `s` turned over: its u the other way (each row's points reversed, its u knots mirrored), the
/// same surface facing the other way. `None` when it cannot be made.
pub fn turned_over(s: &NurbsSurface) -> Option<NurbsSurface> {
    let [nu, nv] = s.count();
    let ctrl = s.homogeneous();
    let flipped: Vec<_> = (0..nv).flat_map(|j| (0..nu).rev().map(move |i| ctrl[j * nu + i])).collect();
    let [ku, kv] = s.knots().clone();
    let (first, last) = (*ku.first()?, *ku.last()?);
    let ku: Vec<f64> = ku.iter().rev().map(|k| first + last - k).collect();
    NurbsSurface::from_homogeneous(s.degree(), [ku, kv], flipped).ok()
}

/// How many places across `s` (a grid of `PLACES` each way, inside it) face the way the scan
/// faces under them, less how many face against it; `normal` gives the scan's unit normal nearest
/// a point (`None` where there is no scan).
fn facing_score(s: &NurbsSurface, normal: &impl Fn(Vec3) -> Option<Vec3>) -> i64 {
    const PLACES: usize = 5;
    let net = crate::surface_edit::points(s);
    let [[u0, u1], [v0, v1]] = s.domain();
    let mut score = 0i64;
    for j in 0..PLACES {
        for i in 0..PLACES {
            let (u, v) = (u0 + (u1 - u0) * (i as f64 + 0.5) / PLACES as f64, v0 + (v1 - v0) * (j as f64 + 0.5) / PLACES as f64);
            let Some((p, su, sv)) = crate::surface_edit::frame(s, &net, u, v) else { continue };
            let (Some(n), Some(m)) = (su.cross(sv).normalized(), normal(p)) else { continue };
            score += if n.dot(m) >= 0.0 { 1 } else { -1 };
        }
    }
    score
}

/// `s` facing as the scan under it faces (out of a scanned part, toward where it was scanned
/// from): turned over when more of it faces against the scan than with it. A face made on a scan
/// then thickens and offsets outward with a positive length, whichever way round its corners or
/// sides were given.
pub fn facing_the_scan(s: NurbsSurface, normal: impl Fn(Vec3) -> Option<Vec3>) -> NurbsSurface {
    if facing_score(&s, &normal) < 0 {
        return turned_over(&s).unwrap_or(s);
    }
    s
}

/// More points each way must bring the face at least this much nearer the scan at worst to be
/// worth them: short of that the face is down to the scan's own noise or a spike in it.
const WORTH_MORE: f64 = 0.75;

/// Places each way across a face its worst distance off the scan is looked at, for each point of
/// its net that way: four times as many as the net has points, edges and corners included, so they
/// fall between the places it was fitted to.
const CHECK_PLACES: usize = 4;

/// How far `surface` lies off the scan at worst: looked at from places spread evenly across it,
/// its edges and corners too, each to the scan's nearest place (`snap` with no way to look along;
/// a place with no scan within reach is left out, and so is one whose nearest scan lies off to its
/// side rather than across from it: over a hole in the scan, a window's or a gap's, the nearest
/// scan is the hole's rim, and the face is not off the scan there by that). Not the places it
/// was fitted to: a face that bows off a narrow round between those still meets them, and its
/// distance there says nothing. 0 with no place on the scan.
pub fn worst_off(surface: &NurbsSurface, snap: impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>) -> f64 {
    let [nu, nv] = surface.count();
    let (mu, mv) = (CHECK_PLACES * nu.max(1), CHECK_PLACES * nv.max(1));
    let grid: Vec<Option<Vec3>> = (0..=mv)
        .flat_map(|j| (0..=mu).map(move |i| (i, j)))
        .map(|(i, j)| surface.eval(i as f64 / mu as f64, j as f64 / mv as f64).ok().filter(|q| q.is_finite()))
        .collect();
    let at = |i: usize, j: usize| grid.get(j * (mu + 1) + i).copied().flatten();
    // The face's normal at a place, from the places either side of it each way.
    let normal = |i: usize, j: usize| {
        let along_u = at((i + 1).min(mu), j)? - at(i.saturating_sub(1), j)?;
        let along_v = at(i, (j + 1).min(mv))? - at(i, j.saturating_sub(1))?;
        along_u.cross(along_v).normalized()
    };
    let places: Vec<(usize, usize, Vec3)> =
        (0..=mv).flat_map(|j| (0..=mu).map(move |i| (i, j))).filter_map(|(i, j)| Some((i, j, at(i, j)?))).collect();
    let on: Vec<Vec3> = places.iter().map(|p| p.2).collect();
    let across = ACROSS.cos();
    snap(&on, None)
        .iter()
        .zip(&places)
        .filter_map(|(s, &(i, j, q))| {
            let off = (*s)? - q;
            let d = off.len();
            // A place where the face has no normal (a degenerate corner) is taken as it is.
            let facing = normal(i, j).is_none_or(|n| n.dot(off).abs() >= across * d);
            (facing && d.is_finite()).then_some(d)
        })
        .fold(0.0, f64::max)
}

/// The nearest scan lies across from a place on a face when it is within this angle (radians) of
/// the face's normal there; farther round, it lies to the side (a hole's rim).
const ACROSS: f64 = std::f64::consts::FRAC_PI_3;

/// Follow the Scan as it should: the face over `corners` with the fewest points each way, from
/// `fitting.count` up to [`MOST_POINTS`] two at a time, that lies within `target` (mm) of the
/// scan everywhere ([`worst_off`]), so a face over a rounded edge or a strong curve follows it
/// instead of bowing off it as a 4 × 4 net does. Each count up to the most is tried while none is
/// within `target` (a count can do worse than the one before it and the next much better); one
/// is taken over fewer points only when it brings the face a quarter nearer at worst, so the
/// scan's noise does not buy points. Overall Shape, a stiffened net and a target that is not a
/// length keep `fitting.count`. `None` when the net cannot be made. With the face, how far off it
/// lies at worst.
pub fn fitted_closely(
    corners: [Vec3; 4],
    fitting: &Fitting,
    target: f64,
    snap: impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>,
) -> Option<(Fitted, f64)> {
    let mut best = fitted(corners, fitting, &snap)?;
    let mut off = worst_off(&best.surface, &snap);
    if fitting.overall || fitting.stiffness > 0.0 || !(target.is_finite() && target > 0.0) {
        return Some((best, off));
    }
    let mut count = fitting.count.clamp(LEAST_POINTS, MOST_POINTS);
    while off > target && count < MOST_POINTS {
        count = (count + 2).min(MOST_POINTS);
        let Some(next) = fitted(corners, &Fitting { count, ..*fitting }, &snap) else { continue };
        let next_off = worst_off(&next.surface, &snap);
        if next_off < WORTH_MORE * off {
            (best, off) = (next, next_off);
        }
    }
    Some((best, off))
}

/// How many times a flat face's plane is fitted again, leaving out the places far off the last.
const FLAT_ROUNDS: usize = 4;

/// The plane through `points`: through their middle, square to the way they spread least. `None`
/// for fewer than three, or points in a line (a plane through them could turn about it).
fn plane_through(points: &[Vec3]) -> Option<(Vec3, Vec3)> {
    if points.len() < 3 || !points.iter().all(|p| p.is_finite()) {
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
    let (moments, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    // The middle moment must be there: with it as small as the least, the points are in a line.
    if !(moments[1] > 1e-12 * moments[2].abs().max(f64::MIN_POSITIVE)) {
        return None;
    }
    Some((middle, axes[0].normalized()?))
}

/// The plane `places` lie nearest, fitted again with the places far off it left out (as Overall
/// Shape does, so a rib or a dent does not tilt it): a point on it, its unit normal, and how many
/// places were kept at the last. `None` for places that do not spread over an area.
pub fn flat_plane(places: &[Vec3]) -> Option<(Vec3, Vec3, usize)> {
    let mut plane = plane_through(places)?;
    let (lo, hi) = places
        .iter()
        .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let floor = TRIM_FLOOR * (hi - lo).len();
    let mut kept_count = places.len();
    for _ in 0..FLAT_ROUNDS {
        let distances: Vec<f64> = places.iter().map(|p| (*p - plane.0).dot(plane.1).abs()).collect();
        let keep = kept(&distances, true, floor);
        let used: Vec<Vec3> = places.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
        let Some(next) = plane_through(&used) else { break };
        plane = next;
        kept_count = used.len();
    }
    Some((plane.0, plane.1, kept_count))
}

/// The face over `corners` laid flat on the plane through `point` square to `normal`: each corner
/// put onto the plane (the foot of the line square to it), the net the bilinear patch they make.
/// `None` for a plane with no direction, or corners that make no face on it.
pub fn flat_over(corners: [Vec3; 4], count: usize, point: Vec3, normal: Vec3) -> Option<NurbsSurface> {
    let normal = normal.normalized()?;
    if !(point.is_finite() && corners.iter().all(|c| c.is_finite())) {
        return None;
    }
    let count = count.clamp(LEAST_POINTS, MOST_POINTS);
    let onto = corners.map(|c| c - normal * (c - point).dot(normal));
    // Corners that enclose no area (all in one place, or in a line) make no face.
    let (d0, d1) = (onto[2] - onto[0], onto[3] - onto[1]);
    let size = d0.len2().max(d1.len2());
    if !(size > 0.0 && d0.cross(d1).len() > 1e-9 * size) {
        return None;
    }
    surface(&net_over(onto, count), count)
}

/// A flat face over `corners`: the plane fitted to the scan under them (`snap` putting places on
/// the scan, `None` for a place with none within reach, which is left out; those far off the plane
/// are left out too), the corners laid on it. `places` are those looked for, `met` those found on
/// the scan, `kept` those the plane was fitted to, and `rms` how far they are from it. `None` when
/// the scan under the corners makes no plane.
pub fn flattened(corners: [Vec3; 4], fitting: &Fitting, snap: impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>) -> Option<Fitted> {
    let n = fitting.count.clamp(LEAST_POINTS, MOST_POINTS);
    let facing = (corners[2] - corners[0]).cross(corners[3] - corners[1]).normalized();
    let across = INSIDE_PLACES * n;
    let mut at = Vec::with_capacity(across * across);
    for j in 0..across {
        let t = (j as f64 + 0.5) / across as f64;
        for i in 0..across {
            let s = (i as f64 + 0.5) / across as f64;
            at.push(corners[0] * ((1.0 - s) * (1.0 - t)) + corners[1] * (s * (1.0 - t)) + corners[2] * (s * t) + corners[3] * ((1.0 - s) * t));
        }
    }
    let found = snap(&at, facing);
    let places = found.len();
    let on: Vec<Vec3> = found.into_iter().flatten().collect();
    let (point, normal, kept_count) = flat_plane(&on)?;
    let surface = flat_over(corners, n, point, normal)?;
    let off: Vec<f64> = on.iter().map(|p| (*p - point).dot(normal)).collect();
    let rms = if off.is_empty() { 0.0 } else { (off.iter().map(|d| d * d).sum::<f64>() / off.len() as f64).sqrt() };
    Some(Fitted { surface, places, met: on.len(), kept: kept_count, rms })
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

    /// A face fitted with its corners clicked either way round faces as the dome does (up, out of
    /// it) once turned to the scan; turned over it is the same surface, facing the other way.
    #[test]
    fn a_face_is_turned_to_face_as_the_scan_does() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let round = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        let facing_up = |s: &NurbsSurface| {
            let (_, su, sv) = crate::surface_edit::frame(s, &points(s), 0.5, 0.5).unwrap();
            su.cross(sv).z > 0.0
        };
        let mut ways = Vec::new();
        for corners in [round, [round[0], round[3], round[2], round[1]]] {
            let fit = fitted(corners, &Fitting { count: 5, ..Fitting::default() }, snap).unwrap().surface;
            ways.push(facing_up(&fit));
            let over = turned_over(&fit).unwrap();
            assert_ne!(facing_up(&over), facing_up(&fit), "turned over, it faces the other way");
            let (a, b) = (fit.eval(0.3, 0.7).unwrap(), over.eval(0.7, 0.7).unwrap());
            assert!(a.dist(b) < 1e-9, "the same surface: {a:?} {b:?}");
            assert!(facing_up(&facing_the_scan(fit, |p| on.normal(p))), "out of the dome");
        }
        assert_ne!(ways[0], ways[1], "the two orders fit faces facing opposite ways before they are turned");
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

    /// The corners clicked on a panel with a rib across it: a flat face lies in one plane, the
    /// panel's, with the rib's places left out, where Follow the Scan bends over the rib; and the
    /// plane is square to a tilted panel's own direction.
    #[test]
    fn a_flat_face_lies_in_the_panels_plane_and_leaves_a_rib_out() {
        let rib = |x: f64, y: f64| if (28.0..32.0).contains(&x) { 4.0 } else { 0.0 } + 0.0 * y;
        let (positions, triangles) = field(60, rib);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let corners = [Vec3::new(5.0, 5.0, 0.0), Vec3::new(55.0, 5.0, 0.0), Vec3::new(55.0, 55.0, 0.0), Vec3::new(5.0, 55.0, 0.0)];
        let flat = flattened(corners, &Fitting { count: 6, ..Fitting::default() }, snap).unwrap();
        assert_eq!(flat.surface.count(), [6, 6]);
        assert!(flat.met == flat.places && flat.kept < flat.met, "the rib's places left out: {} of {}", flat.kept, flat.met);
        let net = points(&flat.surface);
        let (point, normal, _) = flat_plane(&net).unwrap();
        let off = net.iter().map(|p| (*p - point).dot(normal).abs()).fold(0.0, f64::max);
        assert!(off < 1e-9, "every point of the net in one plane: {off}");
        assert!(
            net.iter().all(|p| p.z.abs() < 0.2),
            "in the panel's plane, not lifted by the rib: {:?}",
            net.iter().map(|p| p.z).fold(0.0, f64::max)
        );
        let follow = fitted(corners, &Fitting { count: 6, ..Fitting::default() }, snap).unwrap();
        assert!(
            follow.surface.eval(0.5, 0.5).unwrap().z.abs() > flat.surface.eval(0.5, 0.5).unwrap().z.abs(),
            "Follow bends over the rib, flat does not"
        );

        let tilt = |x: f64, y: f64| 3.0 + 0.2 * x + 0.0 * y;
        let (positions, triangles) = field(60, tilt);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let flat = flattened(corners, &Fitting::default(), snap).unwrap();
        let (_, normal, _) = flat_plane(&points(&flat.surface)).unwrap();
        let want = Vec3::new(-0.2, 0.0, 1.0).normalized().unwrap();
        assert!(normal.dot(want).abs() > 1.0 - 1e-9, "square to the tilted panel: {normal:?}");
        assert!(flat.rms < 1e-9, "on the panel: {}", flat.rms);
    }

    /// A chosen plane: the corners are put onto it; a plane with no direction, or places in a line,
    /// make no face and no panic.
    #[test]
    fn a_face_on_a_chosen_plane_and_hostile_input() {
        let corners = [Vec3::new(0.0, 0.0, 2.0), Vec3::new(10.0, 0.0, -1.0), Vec3::new(10.0, 10.0, 3.0), Vec3::new(0.0, 10.0, 1.0)];
        let face = flat_over(corners, 4, Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, 3.0)).unwrap();
        assert!(points(&face).iter().all(|p| (p.z - 5.0).abs() < 1e-12), "all on z = 5");
        assert!(flat_over(corners, 4, Vec3::ZERO, Vec3::ZERO).is_none(), "no direction");
        assert!(flat_over(corners, 4, Vec3::new(f64::NAN, 0.0, 0.0), Vec3::Z).is_none(), "not a place");
        let one_place = [Vec3::new(1.0, 2.0, 3.0); 4];
        assert!(flat_over(one_place, 4, Vec3::ZERO, Vec3::Z).is_none(), "four corners in one place enclose no area");
        let in_line = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(15.0, 0.0, 0.0)];
        assert!(flat_over(in_line, 4, Vec3::ZERO, Vec3::Z).is_none(), "corners in a line enclose no area");
        let above = [Vec3::new(0.0, 0.0, 9.0), Vec3::new(10.0, 0.0, 9.0), Vec3::new(10.0, 10.0, 9.0), Vec3::new(0.0, 10.0, 9.0)];
        assert!(flat_over(above, 4, Vec3::ZERO, Vec3::X).is_none(), "a plane square to the corners' own plane folds them into a line");
        let line: Vec<Vec3> = (0..20).map(|i| Vec3::new(f64::from(i), 0.0, 0.0)).collect();
        assert!(flat_plane(&line).is_none(), "places in a line make no plane");
        assert!(flat_plane(&[]).is_none() && flat_plane(&line[..2]).is_none());
        assert!(flat_plane(&[Vec3::new(f64::INFINITY, 0.0, 0.0), Vec3::X, Vec3::Y, Vec3::Z]).is_none());
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

    /// Corners either side of a part's rounded edge (a 3 mm fillet between two flats): the 4 × 4
    /// net bows off the scan by over a millimetre; Follow the Scan adds points until the face
    /// lies on it, judged by how far the face itself lies off the scan, not by the places it was
    /// fitted to. On the plain dome four points already do, so none are added; Overall Shape and
    /// a stiffened net keep the count asked.
    #[test]
    fn a_hole_in_the_scan_is_not_a_bow() {
        // A flat scan with a round hole 12 mm across its middle (a window, a gap): the face over
        // it lies on the scan everywhere, though the hole's rim is 12 mm from its middle.
        let (positions, triangles) = field(60, |_, _| 0.0);
        let middle = |t: &[u32; 3]| t.iter().fold(Vec3::ZERO, |a, &v| a + positions[v as usize]) / 3.0;
        let triangles: Vec<[u32; 3]> = triangles.iter().copied().filter(|t| middle(t).dist(Vec3::new(30.0, 30.0, 0.0)) > 12.0).collect();
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let corners = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        let four = fitted(corners, &Fitting::default(), snap).unwrap();
        assert!(worst_off(&four.surface, snap) < 0.01, "{}", worst_off(&four.surface, snap));
        let (kept, off) = fitted_closely(corners, &Fitting::default(), 0.05, snap).unwrap();
        assert_eq!(kept.surface.count(), [4, 4], "no points spent on the hole");
        assert!(off < 0.01, "{off}");
    }

    #[test]
    fn a_face_over_a_rounded_edge_follows_it_instead_of_bowing() {
        let f = 3.0;
        let mut profile = Vec::new();
        for k in 0..=60 {
            profile.push((-30.0 + (30.0 - f) * f64::from(k) / 60.0, 0.0));
        }
        for k in 1..=30 {
            let a = std::f64::consts::FRAC_PI_2 * f64::from(k) / 30.0;
            profile.push((-f + f * a.sin(), -f + f * a.cos()));
        }
        for k in 1..=60 {
            profile.push((0.0, -f - (30.0 - f) * f64::from(k) / 60.0));
        }
        let (m, rows) = (profile.len() as u32, 121u32);
        let positions: Vec<Vec3> = (0..rows).flat_map(|j| profile.iter().map(move |&(y, z)| Vec3::new(0.5 * f64::from(j), y, z))).collect();
        let mut triangles = Vec::new();
        for j in 0..rows - 1 {
            for i in 0..m - 1 {
                let a = j * m + i;
                triangles.push([a, a + 1, a + 1 + m]);
                triangles.push([a, a + 1 + m, a + m]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let worst = |fit: &Fitted| {
            (0..=40)
                .flat_map(|j| (0..=40).map(move |i| (f64::from(i) / 40.0, f64::from(j) / 40.0)))
                .filter_map(|(u, v)| fit.surface.eval(u, v).ok().and_then(|q| on.nearest(q)).map(|(_, d)| d))
                .fold(0.0, f64::max)
        };
        let corners = [Vec3::new(10.0, -20.0, 0.0), Vec3::new(50.0, -20.0, 0.0), Vec3::new(50.0, 0.0, -20.0), Vec3::new(10.0, 0.0, -20.0)];
        let four = fitted(corners, &Fitting::default(), snap).unwrap();
        assert!(worst(&four) > 1.0, "four points bow off the round: {}", worst(&four));
        let (close, off) = fitted_closely(corners, &Fitting::default(), 0.05, snap).unwrap();
        assert!(close.surface.count()[0] > 4 && worst(&close) < 0.1, "{:?} {}", close.surface.count(), worst(&close));
        // The distance at the places a face was fitted to says less than how far it bows between
        // them: at 8 points the root mean square there is well under 0.1 mm while the face lies
        // more than 0.1 mm off the round. Asked for 0.1 mm, it takes the points that bring the
        // face itself within it.
        let eight = fitted(corners, &Fitting { count: 8, ..Fitting::default() }, snap).unwrap();
        assert!(eight.rms < 0.1 && worst(&eight) > 0.1, "{} {}", eight.rms, worst(&eight));
        let (within, said) = fitted_closely(corners, &Fitting::default(), 0.1, snap).unwrap();
        assert!(within.surface.count()[0] > 8 && worst(&within) <= 0.1 && said <= 0.1, "{:?} {} {said}", within.surface.count(), worst(&within));
        // What it says it lies off at worst is what it does, within the finer look's difference.
        assert!((off - worst(&close)).abs() < 0.25 * worst(&close).max(0.01), "{off} {}", worst(&close));
        for kept in [Fitting { overall: true, ..Fitting::default() }, Fitting { stiffness: 0.3, ..Fitting::default() }] {
            assert_eq!(fitted_closely(corners, &kept, 0.05, snap).unwrap().0.surface.count(), [4, 4], "{kept:?}");
        }
        assert_eq!(fitted_closely(corners, &Fitting::default(), f64::NAN, snap).unwrap().0.surface.count(), [4, 4]);

        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let snap = |ps: &[Vec3], along: Option<Vec3>| ps.iter().map(|p| snapped(&on, *p, along)).collect::<Vec<_>>();
        let corners = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        assert_eq!(fitted_closely(corners, &Fitting::default(), 0.05, snap).unwrap().0.surface.count(), [4, 4], "the dome needs no more");
    }
}
