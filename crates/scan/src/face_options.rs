//! Per-face refinement suggestions for a Free Form: where the scan lies far from a face, the fixes
//! that could help (more points one way or both, or the face cut in two), each tried on the whole
//! network and measured, so the interface can say what a button would do before it is pressed
//! ("Refine U: rms 0.31 -> 0.12 mm").
//!
//! Faces that share a side keep equal point counts along it, so a fix reaches the faces across the
//! face's sides along the direction it changes, and on from them: a strip of faces. Only that
//! strip is fitted again, with every other face and the network's open edge held, tangent across
//! smooth seams ([`network::moved`]).

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{Dir, NurbsSurface};
use solidcraft_geom::prelude::*;

use crate::net;
use crate::network::{self, Goal, Joins, MOST_FACES, Moves};
use crate::surface_check;
use crate::surface_edit::{frame, points};

/// The most scan points a suggestion is measured on; more are thinned evenly.
const MOST_POINTS: usize = 5000;

/// A suggestion must lower the rms by at least this share to be worth showing.
const WORTH: f64 = 0.001;

/// How a fit weighs evenness of motion, tried in turn (the best result that adds no folds wins).
const EVENNESS: [f64; 3] = [0.1, 1.0, 10.0];

/// Rounds of snapping to the scan and solving, for each evenness.
const ROUNDS: usize = 3;

/// How far the scan lies from one face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceDeviation {
    pub face: usize,
    /// Root mean square distance of the scan points nearest this face, mm.
    pub rms: f64,
    /// The largest such distance, mm.
    pub max: f64,
    /// How many scan points are nearest this face.
    pub count: usize,
}

/// What can be done to a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Knots between all of u's (the shape unchanged, twice the points along u).
    RefineU,
    RefineV,
    RefineBoth,
    /// The face cut in two across u, in the middle.
    SplitU,
    SplitV,
}

/// A fix tried on the network.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub face: usize,
    pub fix: Fix,
    /// The whole network after the fix, the faces it touched fitted again. A split puts the second
    /// half of each face it cut at the end.
    pub faces: Vec<NurbsSurface>,
    /// The faces that changed: the face and the strip the fix reached through shared sides.
    pub touched: Vec<usize>,
    /// The scan's distance from the touched faces after the fix, mm.
    pub rms: f64,
    pub max: f64,
    /// The same scan points' distance from the touched faces before it.
    pub rms_before: f64,
    pub max_before: f64,
}

/// The network read once for all fixes.
struct Base {
    joins: Joins,
    /// The scan points (thinned) and their places on the nearest face.
    goals: Vec<Goal>,
    nets: Vec<Vec<Vec3>>,
    reach: f64,
}

fn finite_points(scan: &[Vec3]) -> Vec<Vec3> {
    scan.iter().copied().filter(|p| p.is_finite()).collect()
}

/// Every `k`-th point, so that at most `most` are left.
fn thinned_to(scan: Vec<Vec3>, most: usize) -> Vec<Vec3> {
    if scan.len() <= most || most == 0 {
        return scan;
    }
    let step = scan.len().div_ceil(most);
    scan.into_iter().step_by(step).collect()
}

fn good_reach(reach: f64) -> bool {
    reach.is_finite() && reach > 0.0
}

/// The distance of a scan place from its face.
fn gap(faces: &[NurbsSurface], nets: &[Vec<Vec3>], g: &Goal) -> Option<f64> {
    let face = faces.get(g.face)?;
    let net = nets.get(g.face)?;
    frame(face, net, g.u, g.v).map(|(s, _, _)| s.dist(g.to)).filter(|d| d.is_finite())
}

/// Root mean square, largest and number of the gaps.
fn summary(gaps: impl Iterator<Item = f64>) -> (f64, f64, usize) {
    let (mut squares, mut max, mut n) = (0.0, 0.0f64, 0usize);
    for d in gaps {
        squares += d * d;
        max = max.max(d);
        n += 1;
    }
    if n == 0 { (0.0, 0.0, 0) } else { ((squares / n as f64).sqrt(), max, n) }
}

fn base(faces: &[NurbsSurface], scan: &[Vec3], reach: f64, limit: usize) -> Option<Base> {
    if faces.is_empty() || !good_reach(reach) {
        return None;
    }
    let pts = thinned_to(finite_points(scan), limit);
    if pts.is_empty() {
        return None;
    }
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    let goals = network::feet(faces, &pts, reach, &vec![true; faces.len()]);
    Some(Base { joins: Joins::default(), goals, nets, reach })
}

fn deviations_of(faces: &[NurbsSurface], b: &Base) -> Vec<FaceDeviation> {
    (0..faces.len())
        .map(|face| {
            let (rms, max, count) = summary(b.goals.iter().filter(|g| g.face == face).filter_map(|g| gap(faces, &b.nets, g)));
            FaceDeviation { face, rms, max, count }
        })
        .collect()
}

/// How far the scan lies from each face: the points whose nearest face foot (inside the face,
/// within `reach`) is on it. Empty with no faces, no usable points or a `reach` that is no
/// positive number.
pub fn deviations(faces: &[NurbsSurface], points: &[Vec3], reach: f64) -> Vec<FaceDeviation> {
    match base(faces, points, reach, usize::MAX) {
        Some(b) => deviations_of(faces, &b),
        None => Vec::new(),
    }
}

/// The fixes tried on each of the `worst` faces with the highest rms (all when there are fewer),
/// each with the deviation it would leave; those that fail, break the network or do not lower the
/// rms are left out. Per face the best (lowest rms) comes first.
pub fn suggest(faces: &[NurbsSurface], points: &[Vec3], reach: f64, worst: usize) -> Vec<Suggestion> {
    let Some(mut b) = base(faces, points, reach, MOST_POINTS) else { return Vec::new() };
    let Ok(joins) = network::joins(faces) else { return Vec::new() };
    b.joins = joins;
    let mut devs: Vec<FaceDeviation> = deviations_of(faces, &b).into_iter().filter(|d| d.count > 0 && d.rms > 0.0).collect();
    devs.sort_by(|x, y| y.rms.total_cmp(&x.rms));
    let mut out = Vec::new();
    for d in devs.into_iter().take(worst) {
        let mut mine: Vec<Suggestion> = [Fix::RefineU, Fix::RefineV, Fix::RefineBoth, Fix::SplitU, Fix::SplitV]
            .into_iter()
            .filter_map(|fix| try_fix(faces, &b, d.face, fix))
            .filter(|s| s.rms.is_finite() && s.rms < s.rms_before * (1.0 - WORTH))
            .collect();
        mine.sort_by(|x, y| x.rms.total_cmp(&y.rms));
        out.extend(mine);
    }
    out
}

/// One fix on `face`, tried and measured (even when it does not help). `None` when it does not
/// apply (a limit reached, the strip needs a face cut both ways), the network would not stay one,
/// or the fit fails.
pub fn apply(faces: &[NurbsSurface], points: &[Vec3], reach: f64, face: usize, fix: Fix) -> Option<Suggestion> {
    let mut b = base(faces, points, reach, MOST_POINTS)?;
    b.joins = network::joins(faces).ok()?;
    try_fix(faces, &b, face, fix)
}

/// The face and direction pairs a fix reaches: through every shared side that runs the way the
/// fix changes, to the neighbour's side, and on from there.
fn strip(joins: &Joins, count: usize, face: usize, dirs: &[usize]) -> Vec<[bool; 2]> {
    let mut marked = vec![[false; 2]; count];
    let mut stack: Vec<(usize, usize)> = dirs.iter().map(|d| (face, *d)).collect();
    while let Some((f, d)) = stack.pop() {
        let Some(slot) = marked.get_mut(f).and_then(|m| m.get_mut(d)) else { continue };
        if *slot {
            continue;
        }
        *slot = true;
        for s in &joins.seams {
            if s.a == f && s.a_side.along() == d {
                stack.push((s.b, s.b_side.along()));
            }
            if s.b == f && s.b_side.along() == d {
                stack.push((s.a, s.a_side.along()));
            }
        }
    }
    marked
}

/// The surface cut in two at the middle of direction `d` (0 u, 1 v): knots put in until the
/// middle is a triple knot, where the curve passes through a control point, and the nets parted
/// there. Both halves are the surface exactly.
fn halves(s: &NurbsSurface, d: usize) -> Option<(NurbsSurface, NurbsSurface)> {
    let dir = if d == 0 { Dir::S } else { Dir::T };
    let [lo, hi] = *s.domain().get(d)?;
    let m = 0.5 * (lo + hi);
    let tol = 1e-9 * (hi - lo);
    let present = s.knots().get(d)?.iter().filter(|k| (**k - m).abs() <= tol).count();
    if present > 3 || !(hi > lo) {
        return None;
    }
    let mut cur = s.clone();
    for _ in present..3 {
        cur = cur.insert_knot(dir, m).ok()?;
    }
    let knots = cur.knots().get(d)?;
    let a = knots.iter().position(|k| *k > m - tol)?;
    let n = *cur.count().get(d)?;
    if a < 4 || a + 3 > n || knots.get(a + 3).is_none_or(|k| *k <= m + tol) {
        return None;
    }
    let [nu, nv] = cur.count();
    let ctrl = cur.homogeneous();
    let part = |from: usize, to: usize| -> Option<Vec<_>> {
        let mut out = Vec::new();
        if d == 0 {
            for j in 0..nv {
                for i in from..to {
                    out.push(*ctrl.get(j * nu + i)?);
                }
            }
        } else {
            for j in from..to {
                for i in 0..nu {
                    out.push(*ctrl.get(j * nu + i)?);
                }
            }
        }
        Some(out)
    };
    let mut left_knots: Vec<f64> = knots.get(..a)?.to_vec();
    left_knots.extend([m; 4]);
    let mut right_knots = vec![m; 4];
    right_knots.extend_from_slice(knots.get(a + 3..)?);
    let with = |k: Vec<f64>, c: Vec<_>| {
        let mut both = cur.knots().clone();
        *both.get_mut(d)? = k;
        NurbsSurface::from_homogeneous(cur.degree(), both, c).ok()
    };
    Some((with(left_knots, part(0, a)?)?, with(right_knots, part(a - 1, n)?)?))
}

/// The network with the fix done and the faces it touched: the unfitted result.
fn grown(faces: &[NurbsSurface], joins: &Joins, face: usize, fix: Fix) -> Option<(Vec<NurbsSurface>, Vec<usize>)> {
    if face >= faces.len() {
        return None;
    }
    let dirs: &[usize] = match fix {
        Fix::RefineU | Fix::SplitU => &[0],
        Fix::RefineV | Fix::SplitV => &[1],
        Fix::RefineBoth => &[0, 1],
    };
    let marked = strip(joins, faces.len(), face, dirs);
    let mut out = faces.to_vec();
    let mut touched = Vec::new();
    let splitting = matches!(fix, Fix::SplitU | Fix::SplitV);
    if splitting && faces.len() + marked.iter().filter(|m| m[0] || m[1]).count() > MOST_FACES {
        return None;
    }
    for (i, m) in marked.iter().enumerate() {
        if !(m[0] || m[1]) {
            continue;
        }
        let original = faces.get(i)?;
        if splitting {
            if m[0] && m[1] {
                return None;
            }
            let (left, right) = halves(original, if m[0] { 0 } else { 1 })?;
            *out.get_mut(i)? = left;
            out.push(right);
            touched.push(i);
            touched.push(out.len() - 1);
        } else {
            *out.get_mut(i)? = net::refine(original, m[0], m[1]).ok()?;
            touched.push(i);
        }
    }
    Some((out, touched))
}

fn trouble(faces: &[NurbsSurface], among: &[bool]) -> usize {
    faces.iter().zip(among).filter(|(_, a)| **a).map(|(f, _)| surface_check::trouble(f).iter().sum::<usize>()).sum()
}

/// The scan places nearest the `among` faces and their gaps: root mean square, largest, number.
fn measured(faces: &[NurbsSurface], scan: &[Vec3], reach: f64, among: &[bool]) -> (f64, f64, usize) {
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    let goals = network::feet(faces, scan, reach, among);
    summary(goals.iter().filter_map(|g| gap(faces, &nets, g)))
}

fn try_fix(faces: &[NurbsSurface], b: &Base, face: usize, fix: Fix) -> Option<Suggestion> {
    let (start, touched) = grown(faces, &b.joins, face, fix)?;
    let joins = network::joins(&start).ok()?;
    let mut among = vec![false; start.len()];
    for &t in &touched {
        *among.get_mut(t)? = true;
    }
    // The scan under the faces that change, and how far it lies from them now.
    let before_faces: Vec<bool> = (0..faces.len()).map(|i| among.get(i).copied().unwrap_or(false)).collect();
    let mine: Vec<&Goal> = b.goals.iter().filter(|g| before_faces.get(g.face).copied().unwrap_or(false)).collect();
    let (rms_before, max_before, _) = summary(mine.iter().filter_map(|g| gap(faces, &b.nets, g)));
    let scan: Vec<Vec3> = mine.iter().map(|g| g.to).collect();
    if scan.is_empty() {
        return None;
    }
    // Free: points of the touched faces alone, off the network's open edge and irregular corners.
    let mut free = vec![false; joins.len()];
    for &t in &touched {
        for &p in joins.of.get(t)? {
            *free.get_mut(p)? = true;
        }
    }
    for (p, at) in joins.at.iter().enumerate() {
        if at.iter().any(|(f, _)| !among.get(*f).copied().unwrap_or(false)) {
            *free.get_mut(p)? = false;
        }
    }
    for &(f, side) in &joins.open {
        for p in joins.side(&start, f, side) {
            *free.get_mut(p)? = false;
        }
    }
    for c in joins.corners.iter().filter(|c| c.irregular()) {
        *free.get_mut(c.point)? = false;
    }
    if !free.iter().any(|f| *f) {
        return None;
    }
    let trouble_before = trouble(&start, &among);
    let mut best: Option<(f64, f64, Vec<NurbsSurface>)> = None;
    for even in EVENNESS {
        let mut cur = start.clone();
        for _ in 0..ROUNDS {
            let goals = network::feet(&cur, &scan, b.reach, &among);
            let moves = Moves { free: &free, goals: &goals, tangent: 40.0, even, bend: 1e-4, also_smooth: &[] };
            match network::moved(&cur, &joins, &moves) {
                Some(next) => cur = next,
                None => break,
            }
        }
        let (rms, max, n) = measured(&cur, &scan, b.reach, &among);
        if n * 2 < scan.len() || !rms.is_finite() || trouble(&cur, &among) > trouble_before {
            continue;
        }
        if best.as_ref().is_none_or(|(r, _, _)| rms < *r) {
            best = Some((rms, max, cur));
        }
    }
    let (rms, max, result) = best?;
    network::joins(&result).ok()?;
    Some(Suggestion { face, fix, faces: result, touched, rms, max, rms_before, max_before })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A network of `nx` by `ny` cubic faces (4 by 4 points each) on the plane z = 0, one unit
    /// each, sharing whole sides.
    fn flat_network(nx: usize, ny: usize) -> Vec<NurbsSurface> {
        let mut faces = Vec::new();
        for b in 0..ny {
            for a in 0..nx {
                let mut pts = Vec::new();
                for j in 0..4 {
                    for i in 0..4 {
                        pts.push(Vec3::new(a as f64 + i as f64 / 3.0, b as f64 + j as f64 / 3.0, 0.0));
                    }
                }
                let knots = crate::surface::clamped_knots(4);
                faces.push(NurbsSurface::new([3, 3], [knots.clone(), knots], &pts, &[1.0; 16]).unwrap());
            }
        }
        faces
    }

    /// Scan points of z = h(x, y) over `[0, w] x [0, h]`, kept off the edges.
    fn scan(w: f64, h: f64, f: impl Fn(f64, f64) -> f64) -> Vec<Vec3> {
        let n = 40;
        let mut out = Vec::new();
        for j in 1..n {
            for i in 1..n {
                let (x, y) = (w * i as f64 / n as f64 + 0.0013, h * j as f64 / n as f64 + 0.0007);
                out.push(Vec3::new(x, y, f(x, y)));
            }
        }
        out
    }

    fn ripple(x: f64, y: f64) -> f64 {
        0.3 * (3.0 * std::f64::consts::PI * x).sin() * (std::f64::consts::PI * y).sin()
    }

    fn ripple2(x: f64, y: f64) -> f64 {
        0.3 * (1.5 * std::f64::consts::PI * x).sin() * (0.5 * std::f64::consts::PI * y).sin()
    }

    #[test]
    fn deviations_name_the_worst_face() {
        let faces = flat_network(2, 1);
        let points = scan(2.0, 1.0, |x, _| if x < 1.0 { 0.0 } else { 0.0 + 0.2 * (x - 1.0) * (2.0 - x) * 4.0 });
        let d = deviations(&faces, &points, 1.0);
        assert_eq!(d.len(), 2);
        assert!(d[1].rms > d[0].rms, "{d:?}");
        assert!(d[0].count > 0 && d[1].count > 0);
    }

    #[test]
    fn one_face_refines_and_lowers_the_deviation() {
        let faces = flat_network(1, 1);
        let points = scan(1.0, 1.0, ripple);
        let all = suggest(&faces, &points, 1.0, 5);
        assert!(!all.is_empty());
        let s = apply(&faces, &points, 1.0, 0, Fix::RefineBoth).unwrap();
        assert!(s.rms < s.rms_before, "{} -> {}", s.rms_before, s.rms);
        assert_eq!(s.touched, vec![0]);
        assert!(network::joins(&s.faces).is_ok());
        assert!(all.iter().all(|s| s.rms < s.rms_before && s.face == 0));
        assert!(all.windows(2).all(|w| w[0].rms <= w[1].rms));
    }

    #[test]
    fn one_face_split_makes_two_faces() {
        let faces = flat_network(1, 1);
        let points = scan(1.0, 1.0, ripple);
        let s = apply(&faces, &points, 1.0, 0, Fix::SplitU).unwrap();
        assert_eq!(s.faces.len(), 2);
        assert_eq!(s.touched, vec![0, 1]);
        let joins = network::joins(&s.faces).unwrap();
        assert_eq!(joins.seams.len(), 1);
        assert!(s.rms < s.rms_before, "{} -> {}", s.rms_before, s.rms);
    }

    #[test]
    fn halves_are_the_surface() {
        let faces = flat_network(1, 1);
        let bump = {
            let mut pts = points(&faces[0]);
            pts[5] += Vec3::new(0.0, 0.0, 0.4);
            pts[10] += Vec3::new(0.0, 0.0, -0.2);
            crate::surface_edit::with_points(&faces[0], &pts).unwrap()
        };
        for d in 0..2 {
            let (l, r) = halves(&bump, d).unwrap();
            let [[u0, u1], [v0, v1]] = bump.domain();
            for k in 0..=10 {
                let t = k as f64 / 10.0;
                let want_l = bump.eval(u0 + (u1 - u0) * 0.5 * t, v0 + (v1 - v0) * t).unwrap();
                let want_r = bump.eval(u0 + (u1 - u0) * (0.5 + 0.5 * t), v0 + (v1 - v0) * t).unwrap();
                if d == 0 {
                    let [[a0, a1], [b0, b1]] = l.domain();
                    let got_l = l.eval(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t).unwrap();
                    let [[a0, a1], [b0, b1]] = r.domain();
                    let got_r = r.eval(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t).unwrap();
                    assert!(got_l.dist(want_l) < 1e-9 && got_r.dist(want_r) < 1e-9, "u {k}");
                } else {
                    let w_l = bump.eval(u0 + (u1 - u0) * t, v0 + (v1 - v0) * 0.5 * t).unwrap();
                    let w_r = bump.eval(u0 + (u1 - u0) * t, v0 + (v1 - v0) * (0.5 + 0.5 * t)).unwrap();
                    let [[a0, a1], [b0, b1]] = l.domain();
                    let g_l = l.eval(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t).unwrap();
                    let [[a0, a1], [b0, b1]] = r.domain();
                    let g_r = r.eval(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t).unwrap();
                    assert!(g_l.dist(w_l) < 1e-9 && g_r.dist(w_r) < 1e-9, "v {k}");
                }
            }
        }
    }

    #[test]
    fn a_refine_reaches_the_face_across_its_side() {
        // Two by two faces; u runs along x. Refining face 0 in u changes its rows, shared with
        // face 2 above it, so face 2 follows; faces 1 and 3 keep their points.
        let faces = flat_network(2, 2);
        let points = scan(2.0, 2.0, ripple2);
        let s = apply(&faces, &points, 1.0, 0, Fix::RefineU).unwrap();
        let mut touched = s.touched.clone();
        touched.sort_unstable();
        assert_eq!(touched, vec![0, 2]);
        assert_eq!(s.faces[0].count(), [5, 4]);
        assert_eq!(s.faces[2].count(), [5, 4]);
        assert_eq!(s.faces[1].count(), [4, 4]);
        assert!(network::joins(&s.faces).is_ok());
        assert!(s.rms < s.rms_before, "{} -> {}", s.rms_before, s.rms);
    }

    #[test]
    fn a_split_cuts_the_strip() {
        let faces = flat_network(2, 2);
        let points = scan(2.0, 2.0, ripple2);
        let s = apply(&faces, &points, 1.0, 0, Fix::SplitU).unwrap();
        assert_eq!(s.faces.len(), 6);
        assert!(network::joins(&s.faces).is_ok());
        let mut touched = s.touched.clone();
        touched.sort_unstable();
        assert_eq!(touched, vec![0, 2, 4, 5]);
    }

    #[test]
    fn suggestions_are_sorted_and_helpful() {
        let faces = flat_network(2, 2);
        let points = scan(2.0, 2.0, ripple2);
        let all = suggest(&faces, &points, 1.0, 2);
        assert!(all.iter().all(|s| s.rms < s.rms_before && network::joins(&s.faces).is_ok()));
        for face in 0..4 {
            let mine: Vec<f64> = all.iter().filter(|s| s.face == face).map(|s| s.rms).collect();
            assert!(mine.windows(2).all(|w| w[0] <= w[1]));
        }
    }

    #[test]
    fn nothing_in_nothing_out() {
        let faces = flat_network(1, 1);
        assert!(deviations(&faces, &[], 1.0).is_empty());
        assert!(suggest(&faces, &[], 1.0, 3).is_empty());
        assert!(suggest(&[], &scan(1.0, 1.0, ripple), 1.0, 3).is_empty());
        assert!(apply(&faces, &[], 1.0, 0, Fix::RefineU).is_none());
        assert!(apply(&faces, &scan(1.0, 1.0, ripple), 1.0, 7, Fix::RefineU).is_none());
        let bad = [Vec3::new(f64::NAN, 0.0, 0.0)];
        assert!(suggest(&faces, &bad, 1.0, 3).is_empty());
        assert!(suggest(&faces, &scan(1.0, 1.0, ripple), f64::NAN, 3).is_empty());
        assert!(suggest(&faces, &scan(1.0, 1.0, ripple), 1.0, 0).is_empty());
    }
}
