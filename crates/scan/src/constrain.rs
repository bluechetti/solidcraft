//! Constraint tools for the Free Form editor: hold selected points in a plane, make them a straight
//! line or a circle, or fit them to a cylinder, cone or sphere. Each fit is a least-squares fit of
//! the primitive to the points (distances measured square to the primitive), and `project` moves a
//! point to the nearest place on the fitted primitive, so the editor can move every selected point
//! there. Pure maths on positions: no scan, no mesh.
//!
//! Everything here is total: too few points, repeated points, points on a line (for a plane),
//! non-finite or enormous coordinates give `None` (or the input unchanged), never a panic.

// Small fixed-size matrix loops read clearer indexed.
#![allow(clippy::needless_range_loop)]

use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom;

/// Which primitive to fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A plane.
    Plane,
    /// A straight line.
    Line,
    /// A circle in space.
    Circle,
    /// A cylinder (infinitely long).
    Cylinder,
    /// A cone (the infinite double cone).
    Cone,
    /// A sphere.
    Sphere,
}

impl Kind {
    /// The fewest points `fit` needs for this kind.
    pub fn min_points(self) -> usize {
        match self {
            Kind::Line => 2,
            Kind::Plane | Kind::Circle => 3,
            Kind::Sphere => 4,
            Kind::Cylinder => 5,
            Kind::Cone => 6,
        }
    }
}

/// A fitted primitive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    /// A plane through `point`, square to the unit `normal`.
    Plane { point: Vec3, normal: Vec3 },
    /// A line through `point` along the unit `dir`.
    Line { point: Vec3, dir: Vec3 },
    /// A circle about `centre`, in the plane square to the unit `normal`.
    Circle { centre: Vec3, normal: Vec3, radius: f64 },
    /// A cylinder about the line through `point` along the unit `axis`.
    Cylinder { point: Vec3, axis: Vec3, radius: f64 },
    /// A double cone from `apex` along the unit `axis`; `half_angle` (radians, between 0 and a
    /// right angle) is the angle between the axis and the surface.
    Cone { apex: Vec3, axis: Vec3, half_angle: f64 },
    /// A sphere about `centre`.
    Sphere { centre: Vec3, radius: f64 },
}

/// A fit and how well it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The fitted primitive.
    pub primitive: Primitive,
    /// Root-mean-square distance of the points from the primitive.
    pub rms: f64,
    /// Largest distance of any point from the primitive.
    pub max: f64,
}

type V = [f64; 3];

/// Coordinates beyond this are refused (squares would overflow).
const HUGE: f64 = 1e120;
/// Largest radius, in units of the points' extent, a fit may have (a flatter fit is a plane).
const MAX_RADIUS: f64 = 1e3;

/// Fits `kind` to `points` by least squares. `None` when there are too few points, they are
/// degenerate for the kind (all equal, collinear for a plane, ...), contain non-finite or enormous
/// numbers, or no sensible fit is found.
pub fn fit(kind: Kind, points: &[Vec3]) -> Option<Fit> {
    if points.len() < kind.min_points() {
        return None;
    }
    let raw: Vec<V> = points.iter().map(|p| [p.x, p.y, p.z]).collect();
    if raw.iter().any(|p| p.iter().any(|c| !c.is_finite() || c.abs() > HUGE)) {
        return None;
    }
    let n = raw.len() as f64;
    let mut c = [0.0; 3];
    for p in &raw {
        c = add(c, *p);
    }
    c = scale(c, 1.0 / n);
    let mut s = 0.0f64;
    for p in &raw {
        let d = sub(*p, c);
        s = s.max(d[0].abs()).max(d[1].abs()).max(d[2].abs());
    }
    if !(s > 0.0) || !s.is_finite() {
        return None;
    }
    // A scale so small that `1/s` overflows is no usable extent either.
    let inv = 1.0 / s;
    if !inv.is_finite() {
        return None;
    }
    let q: Vec<V> = raw.iter().map(|p| scale(sub(*p, c), inv)).collect();
    let local = match kind {
        Kind::Plane => fit_plane(&q)?,
        Kind::Line => fit_line(&q)?,
        Kind::Circle => fit_circle(&q)?,
        Kind::Sphere => fit_sphere(&q)?,
        Kind::Cylinder => fit_cylinder(&q)?,
        Kind::Cone => fit_cone(&q)?,
    };
    let primitive = place(local, c, s);
    let (mut sum, mut max) = (0.0, 0.0f64);
    for p in points {
        let d = (project(&primitive, *p) - *p).len();
        if !d.is_finite() {
            return None;
        }
        sum += d * d;
        max = max.max(d);
    }
    let rms = (sum / n).sqrt();
    if !rms.is_finite() {
        return None;
    }
    Some(Fit { primitive, rms, max })
}

/// The nearest point to `p` on `primitive` (for a cone, on the infinite double cone). A non-finite
/// `p` comes back unchanged.
pub fn project(primitive: &Primitive, p: Vec3) -> Vec3 {
    let pv = [p.x, p.y, p.z];
    if pv.iter().any(|c| !c.is_finite()) {
        return p;
    }
    let out = match *primitive {
        Primitive::Plane { point, normal } => {
            let (o, n) = (v3(point), v3(normal));
            sub(pv, scale(n, dot(sub(pv, o), n)))
        }
        Primitive::Line { point, dir } => {
            let (o, d) = (v3(point), v3(dir));
            add(o, scale(d, dot(sub(pv, o), d)))
        }
        Primitive::Circle { centre, normal, radius } => {
            let (o, n) = (v3(centre), v3(normal));
            let w = sub(pv, o);
            let flat = sub(w, scale(n, dot(w, n)));
            let dir = normalized(flat).unwrap_or_else(|| frame(n).0);
            add(o, scale(dir, radius))
        }
        Primitive::Cylinder { point, axis, radius } => {
            let (o, a) = (v3(point), v3(axis));
            let w = sub(pv, o);
            let t = dot(w, a);
            let radial = sub(w, scale(a, t));
            let dir = normalized(radial).unwrap_or_else(|| frame(a).0);
            add(add(o, scale(a, t)), scale(dir, radius))
        }
        Primitive::Cone { apex, axis, half_angle } => {
            let (o, a) = (v3(apex), v3(axis));
            let w = sub(pv, o);
            let t = dot(w, a);
            let radial = sub(w, scale(a, t));
            let rho = norm(radial);
            let dir = normalized(radial).unwrap_or_else(|| frame(a).0);
            let (sn, cs) = half_angle.sin_cos();
            let side = if t < 0.0 { -1.0 } else { 1.0 };
            let along = (t.abs() * cs + rho * sn).max(0.0);
            add(o, add(scale(a, side * along * cs), scale(dir, along * sn)))
        }
        Primitive::Sphere { centre, radius } => {
            let o = v3(centre);
            let dir = normalized(sub(pv, o)).unwrap_or([1.0, 0.0, 0.0]);
            add(o, scale(dir, radius))
        }
    };
    Vec3::new(out[0], out[1], out[2])
}

/// `project` for every point.
pub fn project_all(primitive: &Primitive, points: &[Vec3]) -> Vec<Vec3> {
    points.iter().map(|p| project(primitive, *p)).collect()
}

/// The simplest of plane, cylinder, sphere and cone (in that order) that fits `points` with an rms
/// distance of at most `tolerance`; `None` when none does.
pub fn best(points: &[Vec3], tolerance: f64) -> Option<(Kind, Fit)> {
    if !(tolerance >= 0.0) {
        return None;
    }
    for kind in [Kind::Plane, Kind::Cylinder, Kind::Sphere, Kind::Cone] {
        if let Some(f) = fit(kind, points)
            && f.rms <= tolerance
        {
            return Some((kind, f));
        }
    }
    None
}

/// The points of an ordered chain moved onto their least-squares line, each keeping its order and
/// its position along the line. Returned unchanged when no line fits (fewer than two points, all
/// equal, non-finite).
pub fn straighten(points: &[Vec3]) -> Vec<Vec3> {
    match fit(Kind::Line, points) {
        Some(f) => project_all(&f.primitive, points),
        None => points.to_vec(),
    }
}

/// An ordered chain re-spaced evenly along its own polyline, the first and last points fixed.
/// Returned unchanged when it has fewer than three points, has no length or is not finite.
pub fn even_spacing(points: &[Vec3]) -> Vec<Vec3> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let pts: Vec<V> = points.iter().map(|p| v3(*p)).collect();
    if pts.iter().any(|p| p.iter().any(|c| !c.is_finite() || c.abs() > HUGE)) {
        return points.to_vec();
    }
    let mut cum = Vec::with_capacity(n);
    let mut total = 0.0;
    cum.push(0.0);
    for w in pts.windows(2) {
        total += norm(sub(w[1], w[0]));
        cum.push(total);
    }
    if !(total > 0.0) || !total.is_finite() {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(n);
    let mut seg = 0usize;
    for k in 0..n {
        if k == 0 {
            out.push(points[0]);
            continue;
        }
        if k + 1 == n {
            out.push(points[n - 1]);
            continue;
        }
        let target = total * k as f64 / (n - 1) as f64;
        while seg + 2 < n && cum[seg + 1] < target {
            seg += 1;
        }
        let (a, b) = (cum[seg], cum[seg + 1]);
        let f = if b > a { ((target - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        let p = add(pts[seg], scale(sub(pts[seg + 1], pts[seg]), f));
        out.push(Vec3::new(p[0], p[1], p[2]));
    }
    out
}

// ---- small vector helpers on [f64; 3] ----

fn v3(p: Vec3) -> V {
    [p.x, p.y, p.z]
}
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V, k: f64) -> V {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V) -> f64 {
    dot(a, a).sqrt()
}
fn normalized(a: V) -> Option<V> {
    let n = norm(a);
    if n.is_finite() && n > 1e-150 { Some(scale(a, 1.0 / n)) } else { None }
}
/// Two unit vectors square to the unit `a` and to each other.
fn frame(a: V) -> (V, V) {
    let helper = if a[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let e1 = normalized(cross(a, helper)).unwrap_or([1.0, 0.0, 0.0]);
    let e2 = cross(a, e1);
    (e1, e2)
}

/// A primitive in the normalised frame (centred, scaled by the extent), as plain arrays.
enum Local {
    Plane(V, V),
    Line(V, V),
    Circle(V, V, f64),
    Cylinder(V, V, f64),
    Cone(V, V, f64),
    Sphere(V, f64),
}

/// Back from the normalised frame: `centre` and `s` undo the centring and the scaling.
fn place(l: Local, c: V, s: f64) -> Primitive {
    let pt = |p: V| {
        let w = add(c, scale(p, s));
        Vec3::new(w[0], w[1], w[2])
    };
    let dir = |d: V| Vec3::new(d[0], d[1], d[2]);
    match l {
        Local::Plane(p, n) => Primitive::Plane { point: pt(p), normal: dir(n) },
        Local::Line(p, d) => Primitive::Line { point: pt(p), dir: dir(d) },
        Local::Circle(p, n, r) => Primitive::Circle { centre: pt(p), normal: dir(n), radius: r * s },
        Local::Cylinder(p, a, r) => Primitive::Cylinder { point: pt(p), axis: dir(a), radius: r * s },
        Local::Cone(p, a, h) => Primitive::Cone { apex: pt(p), axis: dir(a), half_angle: h },
        Local::Sphere(p, r) => Primitive::Sphere { centre: pt(p), radius: r * s },
    }
}

// ---- linear algebra ----

/// Eigenvalues (ascending) and the matching unit eigenvectors of a symmetric 3x3 matrix (Jacobi).
pub(crate) fn eigen3(m: [[f64; 3]; 3]) -> ([f64; 3], [V; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..60 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-300 {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in v.iter_mut() {
                let (vkp, vkq) = (row[p], row[q]);
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[i][i].partial_cmp(&a[j][j]).unwrap_or(std::cmp::Ordering::Equal));
    let vals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let col = |j: usize| [v[0][j], v[1][j], v[2][j]];
    (vals, [col(idx[0]), col(idx[1]), col(idx[2])])
}

/// Solves `a x = b` by Gaussian elimination with partial pivoting; `None` when singular.
fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    let mut biggest = 0.0f64;
    for row in &a {
        for x in row {
            biggest = biggest.max(x.abs());
        }
    }
    if !(biggest > 0.0) || !biggest.is_finite() {
        return None;
    }
    for col in 0..N {
        let mut piv = col;
        for r in col + 1..N {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() <= 1e-13 * biggest {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..N {
            let f = a[r][col] / a[col][col];
            for k in col..N {
                a[r][k] -= f * a[col][k];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for r in (0..N).rev() {
        let mut s = b[r];
        for k in r + 1..N {
            s -= a[r][k] * x[k];
        }
        x[r] = s / a[r][r];
        if !x[r].is_finite() {
            return None;
        }
    }
    Some(x)
}

/// Linear least squares `rows . x = rhs` through the normal equations.
fn least_squares<const N: usize>(rows: &[[f64; N]], rhs: &[f64]) -> Option<[f64; N]> {
    let mut ata = [[0.0; N]; N];
    let mut atb = [0.0; N];
    for (row, y) in rows.iter().zip(rhs) {
        for i in 0..N {
            atb[i] += row[i] * y;
            for j in 0..N {
                ata[i][j] += row[i] * row[j];
            }
        }
    }
    solve(ata, atb)
}

/// Levenberg-Marquardt from `x0` for the residual vector `resid` (central-difference Jacobian).
/// Returns the best parameters and their sum of squares; `None` if the start is not finite.
fn levenberg<const N: usize>(x0: [f64; N], resid: &dyn Fn(&[f64; N]) -> Vec<f64>) -> Option<([f64; N], f64)> {
    let cost_of = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let mut x = x0;
    let mut r = resid(&x);
    let mut cost = cost_of(&r);
    if !cost.is_finite() {
        return None;
    }
    let mut lambda = 1e-3;
    for _ in 0..80 {
        if cost < 1e-30 {
            break;
        }
        let mut jac = vec![[0.0; N]; r.len()];
        for k in 0..N {
            let h = 1e-6 * (1.0 + x[k].abs());
            let (mut hi, mut lo) = (x, x);
            hi[k] += h;
            lo[k] -= h;
            let (rh, rl) = (resid(&hi), resid(&lo));
            for (i, row) in jac.iter_mut().enumerate() {
                let (a, b) = (rh.get(i).copied().unwrap_or(0.0), rl.get(i).copied().unwrap_or(0.0));
                row[k] = (a - b) / (2.0 * h);
            }
        }
        let mut jtj = [[0.0; N]; N];
        let mut jtr = [0.0; N];
        for (row, ri) in jac.iter().zip(&r) {
            for i in 0..N {
                jtr[i] += row[i] * ri;
                for j in 0..N {
                    jtj[i][j] += row[i] * row[j];
                }
            }
        }
        let mut improved = false;
        for _ in 0..12 {
            let mut m = jtj;
            for i in 0..N {
                m[i][i] += lambda * (jtj[i][i] + 1e-9);
            }
            let rhs = jtr.map(|v| -v);
            if let Some(step) = solve(m, rhs) {
                let mut cand = x;
                for i in 0..N {
                    cand[i] += step[i];
                }
                let rc = resid(&cand);
                let cc = cost_of(&rc);
                if cc.is_finite() && cc < cost {
                    let gain = cost - cc;
                    x = cand;
                    r = rc;
                    let rel = gain / cost.max(1e-300);
                    cost = cc;
                    lambda = (lambda / 4.0).max(1e-12);
                    improved = true;
                    if rel < 1e-12 {
                        return Some((x, cost));
                    }
                    break;
                }
            }
            lambda *= 5.0;
            if lambda > 1e12 {
                break;
            }
        }
        if !improved {
            break;
        }
    }
    Some((x, cost))
}

// ---- the fits, all in the normalised frame (centroid at the origin, extent about 1) ----

fn covariance(q: &[V]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for p in q {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += p[i] * p[j];
            }
        }
    }
    let n = q.len().max(1) as f64;
    for row in m.iter_mut() {
        for x in row.iter_mut() {
            *x /= n;
        }
    }
    m
}

fn fit_plane(q: &[V]) -> Option<Local> {
    let (vals, vecs) = eigen3(covariance(q));
    if !(vals[1] > 1e-12 * vals[2]) {
        return None;
    }
    Some(Local::Plane([0.0; 3], normalized(vecs[0])?))
}

fn fit_line(q: &[V]) -> Option<Local> {
    let (vals, vecs) = eigen3(covariance(q));
    if !(vals[2] > 0.0) {
        return None;
    }
    Some(Local::Line([0.0; 3], normalized(vecs[2])?))
}

/// Algebraic (Kasa) circle in 2D: centre and radius.
fn kasa(pts: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    let rows: Vec<[f64; 3]> = pts.iter().map(|p| [2.0 * p[0], 2.0 * p[1], 1.0]).collect();
    let rhs: Vec<f64> = pts.iter().map(|p| p[0] * p[0] + p[1] * p[1]).collect();
    let x = least_squares(&rows, &rhs)?;
    let r2 = x[2] + x[0] * x[0] + x[1] * x[1];
    if !(r2 > 0.0) {
        return None;
    }
    Some(([x[0], x[1]], r2.sqrt()))
}

fn fit_circle(q: &[V]) -> Option<Local> {
    let Local::Plane(_, n) = fit_plane(q)? else { return None };
    let (e1, e2) = frame(n);
    let flat: Vec<[f64; 2]> = q.iter().map(|p| [dot(*p, e1), dot(*p, e2)]).collect();
    let (c0, r0) = kasa(&flat)?;
    let resid = |x: &[f64; 3]| -> Vec<f64> { flat.iter().map(|p| ((p[0] - x[0]).powi(2) + (p[1] - x[1]).powi(2)).sqrt() - x[2]).collect() };
    let (x, _) = levenberg([c0[0], c0[1], r0], &resid)?;
    let r = x[2].abs();
    if !(r > 0.0) || r > MAX_RADIUS {
        return None;
    }
    let centre = add(scale(e1, x[0]), scale(e2, x[1]));
    Some(Local::Circle(centre, n, r))
}

fn fit_sphere(q: &[V]) -> Option<Local> {
    let rows: Vec<[f64; 4]> = q.iter().map(|p| [2.0 * p[0], 2.0 * p[1], 2.0 * p[2], 1.0]).collect();
    let rhs: Vec<f64> = q.iter().map(|p| dot(*p, *p)).collect();
    let x = least_squares(&rows, &rhs)?;
    let c0 = [x[0], x[1], x[2]];
    let r2 = x[3] + dot(c0, c0);
    if !(r2 > 0.0) {
        return None;
    }
    let resid = |x: &[f64; 4]| -> Vec<f64> { q.iter().map(|p| norm(sub(*p, [x[0], x[1], x[2]])) - x[3]).collect() };
    let (x, _) = levenberg([c0[0], c0[1], c0[2], r2.sqrt()], &resid)?;
    let r = x[3].abs();
    if !(r > 0.0) || r > MAX_RADIUS {
        return None;
    }
    Some(Local::Sphere([x[0], x[1], x[2]], r))
}

/// About 200 unit directions spread over a hemisphere (an axis and its opposite are the same).
fn axis_candidates() -> Vec<V> {
    const COUNT: usize = 200;
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..COUNT)
        .map(|i| {
            let z = (i as f64 + 0.5) / COUNT as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = golden * i as f64;
            [r * phi.cos(), r * phi.sin(), z]
        })
        .collect()
}

/// Axis `a0` moved by `u` and `v` along the frame `(e1, e2)` square to it, renormalised.
fn tilt(a0: V, e1: V, e2: V, u: f64, v: f64) -> V {
    normalized(add(a0, add(scale(e1, u), scale(e2, v)))).unwrap_or(a0)
}

/// Keeps the `keep` cheapest candidates.
fn cheapest<T>(mut all: Vec<(f64, T)>, keep: usize) -> Vec<(f64, T)> {
    all.retain(|(c, _)| c.is_finite());
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    all.truncate(keep);
    all
}

fn fit_cylinder(q: &[V]) -> Option<Local> {
    let mut starts = Vec::new();
    for a in axis_candidates() {
        let (e1, e2) = frame(a);
        let flat: Vec<[f64; 2]> = q.iter().map(|p| [dot(*p, e1), dot(*p, e2)]).collect();
        let Some((o, r)) = kasa(&flat) else { continue };
        let cost: f64 = flat.iter().map(|p| (((p[0] - o[0]).powi(2) + (p[1] - o[1]).powi(2)).sqrt() - r).powi(2)).sum();
        starts.push((cost, (a, add(scale(e1, o[0]), scale(e2, o[1])), r)));
    }
    let mut result: Option<(f64, Local)> = None;
    for (_, (a0, p0, r0)) in cheapest(starts, 5) {
        let (e1, e2) = frame(a0);
        let resid = |x: &[f64; 5]| -> Vec<f64> {
            let a = tilt(a0, e1, e2, x[2], x[3]);
            let p = add(p0, add(scale(e1, x[0]), scale(e2, x[1])));
            q.iter()
                .map(|w| {
                    let d = sub(*w, p);
                    norm(sub(d, scale(a, dot(d, a)))) - x[4]
                })
                .collect()
        };
        let Some((x, cost)) = levenberg([0.0, 0.0, 0.0, 0.0, r0], &resid) else { continue };
        let r = x[4].abs();
        if !(r > 0.0) || r > MAX_RADIUS || result.as_ref().is_some_and(|(c, _)| *c <= cost) {
            continue;
        }
        let a = tilt(a0, e1, e2, x[2], x[3]);
        let p = add(p0, add(scale(e1, x[0]), scale(e2, x[1])));
        result = Some((cost, Local::Cylinder(p, a, r)));
    }
    result.map(|(_, l)| l)
}

/// Signed distance from the double cone to `w` (apex-relative), used as the residual.
fn cone_distance(w: V, a: V, half: f64) -> f64 {
    let t = dot(w, a);
    let rho = norm(sub(w, scale(a, t)));
    let (sn, cs) = half.sin_cos();
    rho * cs - t.abs() * sn
}

fn fit_cone(q: &[V]) -> Option<Local> {
    let mut starts = Vec::new();
    for a in axis_candidates() {
        let (e1, e2) = frame(a);
        // With the axis known, |q_perp|^2 = 2 o.q_perp + A t^2 + B t + C is linear in (o, A, B, C).
        let mut rows = Vec::with_capacity(q.len());
        let mut rhs = Vec::with_capacity(q.len());
        for p in q {
            let (u, v, t) = (dot(*p, e1), dot(*p, e2), dot(*p, a));
            rows.push([2.0 * u, 2.0 * v, t * t, t, 1.0]);
            rhs.push(u * u + v * v);
        }
        let Some(x) = least_squares(&rows, &rhs) else { continue };
        let (slope2, b2) = (x[2], x[3]);
        if !(slope2 > 1e-8) {
            continue;
        }
        let m = slope2.sqrt();
        let t0 = -(b2 / (2.0 * m)) / m;
        let apex = add(add(scale(e1, x[0]), scale(e2, x[1])), scale(a, t0));
        let half = m.atan();
        if !(norm(apex) < MAX_RADIUS) {
            continue;
        }
        let cost: f64 = q.iter().map(|p| cone_distance(sub(*p, apex), a, half).powi(2)).sum();
        starts.push((cost, (a, apex, half)));
    }
    let mut result: Option<(f64, Local)> = None;
    for (_, (a0, p0, h0)) in cheapest(starts, 6) {
        let (e1, e2) = frame(a0);
        let half_of = |h: f64| h.clamp(1e-3, std::f64::consts::FRAC_PI_2 - 1e-3);
        let resid = |x: &[f64; 6]| -> Vec<f64> {
            let a = tilt(a0, e1, e2, x[3], x[4]);
            let apex = [p0[0] + x[0], p0[1] + x[1], p0[2] + x[2]];
            let half = half_of(x[5]);
            q.iter().map(|w| cone_distance(sub(*w, apex), a, half)).collect()
        };
        let Some((x, cost)) = levenberg([0.0, 0.0, 0.0, 0.0, 0.0, h0], &resid) else { continue };
        let apex = [p0[0] + x[0], p0[1] + x[1], p0[2] + x[2]];
        if !(norm(apex) < MAX_RADIUS) || result.as_ref().is_some_and(|(c, _)| *c <= cost) {
            continue;
        }
        result = Some((cost, Local::Cone(apex, tilt(a0, e1, e2, x[3], x[4]), half_of(x[5]))));
    }
    result.map(|(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random numbers in `[-1, 1]`.
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        }
        fn jitter(&mut self, p: Vec3, amount: f64) -> Vec3 {
            Vec3::new(p.x + self.next() * amount, p.y + self.next() * amount, p.z + self.next() * amount)
        }
    }

    fn unit(x: f64, y: f64, z: f64) -> V {
        normalized([x, y, z]).unwrap_or([0.0, 0.0, 1.0])
    }

    fn vec(p: V) -> Vec3 {
        Vec3::new(p[0], p[1], p[2])
    }

    fn on_plane(n: usize) -> Vec<Vec3> {
        let (e1, e2) = frame(unit(1.0, 2.0, 3.0));
        (0..n)
            .map(|i| {
                let (u, v) = ((i as f64 * 0.7).sin() * 5.0, (i as f64 * 1.3).cos() * 3.0 + i as f64 * 0.1);
                vec(add([10.0, -4.0, 2.0], add(scale(e1, u), scale(e2, v))))
            })
            .collect()
    }

    fn on_sphere(n: usize) -> Vec<Vec3> {
        let c = [1.0, 2.0, 3.0];
        (0..n)
            .map(|i| {
                let a = i as f64 * 2.399963;
                let z = 0.9 - 1.5 * (i as f64 + 0.5) / n as f64;
                let r = (1.0 - z * z).sqrt();
                vec(add(c, scale([r * a.cos(), r * a.sin(), z], 4.0)))
            })
            .collect()
    }

    fn on_cylinder(n: usize) -> (Vec<Vec3>, V, V, f64) {
        let axis = unit(1.0, 1.0, 0.5);
        let (e1, e2) = frame(axis);
        let o = [3.0, -1.0, 2.0];
        let r = 2.5;
        let pts = (0..n)
            .map(|i| {
                let a = 0.3 + i as f64 * 0.37;
                let t = (i as f64 * 0.61).sin() * 6.0;
                vec(add(add(o, scale(axis, t)), scale(add(scale(e1, a.cos()), scale(e2, a.sin())), r)))
            })
            .collect();
        (pts, o, axis, r)
    }

    fn on_cone(n: usize) -> (Vec<Vec3>, V, V, f64) {
        let axis = unit(0.2, -0.4, 1.0);
        let (e1, e2) = frame(axis);
        let apex = [1.0, 1.0, -1.0];
        let half = 0.45f64;
        let pts = (0..n)
            .map(|i| {
                let a = 0.2 + i as f64 * 0.9;
                let t = 2.0 + (i as f64 * 0.77).sin().abs() * 6.0;
                let rho = t * half.tan();
                vec(add(add(apex, scale(axis, t)), scale(add(scale(e1, a.cos()), scale(e2, a.sin())), rho)))
            })
            .collect();
        (pts, apex, axis, half)
    }

    fn dist_to_line(p: V, o: V, d: V) -> f64 {
        let w = sub(p, o);
        norm(sub(w, scale(d, dot(w, d))))
    }

    #[test]
    fn plane_exact_and_projection() {
        let pts = on_plane(12);
        let f = fit(Kind::Plane, &pts).expect("plane");
        assert!(f.rms < 1e-9 && f.max < 1e-9);
        let Primitive::Plane { point, normal } = f.primitive else { panic!("kind") };
        let want = unit(1.0, 2.0, 3.0);
        assert!(dot(v3(normal), want).abs() > 1.0 - 1e-9);
        let off = Vec3::new(point.x + want[0], point.y + want[1], point.z + want[2]);
        let on = project(&f.primitive, off);
        assert!((on - point).len() < 1e-6 + 1e-9);
        assert!(((on - point).x * want[0] + (on - point).y * want[1] + (on - point).z * want[2]).abs() < 1e-9);
    }

    #[test]
    fn plane_noisy() {
        let mut n = Noise(7);
        let pts: Vec<Vec3> = on_plane(80).into_iter().map(|p| n.jitter(p, 0.01)).collect();
        let f = fit(Kind::Plane, &pts).expect("plane");
        assert!(f.rms < 0.02, "rms {}", f.rms);
    }

    #[test]
    fn line_and_straighten_keep_order() {
        let d = unit(1.0, -2.0, 0.5);
        let pts: Vec<Vec3> = (0..10).map(|i| vec(add([1.0, 1.0, 1.0], scale(d, i as f64 * i as f64 * 0.3)))).collect();
        let f = fit(Kind::Line, &pts).expect("line");
        assert!(f.rms < 1e-9);
        let mut n = Noise(3);
        let bent: Vec<Vec3> = pts.iter().map(|p| n.jitter(*p, 0.05)).collect();
        let s = straighten(&bent);
        assert_eq!(s.len(), bent.len());
        let Some(Fit { primitive: Primitive::Line { point, dir }, .. }) = fit(Kind::Line, &bent) else { panic!("line") };
        // The line's direction has no preferred sign: order must be monotone either way.
        let ts: Vec<f64> = s.iter().map(|p| dot(sub(v3(*p), v3(point)), v3(dir))).collect();
        for p in &s {
            assert!(dist_to_line(v3(*p), v3(point), v3(dir)) < 1e-9);
        }
        assert!(ts.windows(2).all(|w| w[1] > w[0]) || ts.windows(2).all(|w| w[1] < w[0]));
    }

    #[test]
    fn circle_exact_and_noisy() {
        let n = unit(0.3, -0.5, 1.0);
        let (e1, e2) = frame(n);
        let c = [2.0, 3.0, -1.0];
        let pts: Vec<Vec3> = (0..9)
            .map(|i| {
                let a = 0.4 + i as f64 * 0.5;
                vec(add(c, scale(add(scale(e1, a.cos()), scale(e2, a.sin())), 3.0)))
            })
            .collect();
        let f = fit(Kind::Circle, &pts).expect("circle");
        let Primitive::Circle { centre, radius, normal } = f.primitive else { panic!("kind") };
        assert!(f.rms < 1e-9);
        assert!((radius - 3.0).abs() < 1e-8);
        assert!(norm(sub(v3(centre), c)) < 1e-8);
        assert!(dot(v3(normal), n).abs() > 1.0 - 1e-9);
        let mut nz = Noise(11);
        let noisy: Vec<Vec3> = pts.iter().map(|p| nz.jitter(*p, 0.02)).collect();
        let g = fit(Kind::Circle, &noisy).expect("noisy circle");
        assert!(g.rms < 0.05);
        let Primitive::Circle { radius, .. } = g.primitive else { panic!("kind") };
        assert!((radius - 3.0).abs() < 0.1);
        // Projected points lie on the circle.
        for p in project_all(&g.primitive, &noisy) {
            assert!(((p - centre_of(&g.primitive)).len() - radius).abs() < 1e-9);
        }
    }

    fn centre_of(p: &Primitive) -> Vec3 {
        match p {
            Primitive::Circle { centre, .. } | Primitive::Sphere { centre, .. } => *centre,
            _ => Vec3::new(0.0, 0.0, 0.0),
        }
    }

    #[test]
    fn sphere_exact_and_noisy() {
        let pts = on_sphere(30);
        let f = fit(Kind::Sphere, &pts).expect("sphere");
        let Primitive::Sphere { centre, radius } = f.primitive else { panic!("kind") };
        assert!(f.rms < 1e-9 && (radius - 4.0).abs() < 1e-8);
        assert!(norm(sub(v3(centre), [1.0, 2.0, 3.0])) < 1e-8);
        let mut n = Noise(5);
        let noisy: Vec<Vec3> = pts.iter().map(|p| n.jitter(*p, 0.02)).collect();
        let g = fit(Kind::Sphere, &noisy).expect("noisy sphere");
        assert!(g.rms < 0.05);
        assert!(fit(Kind::Sphere, &pts[..4]).is_some());
    }

    #[test]
    fn cylinder_exact_and_noisy() {
        let (pts, o, axis, r) = on_cylinder(40);
        let f = fit(Kind::Cylinder, &pts).expect("cylinder");
        let Primitive::Cylinder { point, axis: a, radius } = f.primitive else { panic!("kind") };
        assert!(f.rms < 1e-7, "rms {}", f.rms);
        assert!((radius - r).abs() < 1e-6);
        assert!(dot(v3(a), axis).abs() > 1.0 - 1e-9);
        assert!(dist_to_line(v3(point), o, axis) < 1e-6);
        let mut n = Noise(9);
        let noisy: Vec<Vec3> = pts.iter().map(|p| n.jitter(*p, 0.02)).collect();
        let g = fit(Kind::Cylinder, &noisy).expect("noisy cylinder");
        assert!(g.rms < 0.05, "rms {}", g.rms);
        let Primitive::Cylinder { radius, .. } = g.primitive else { panic!("kind") };
        assert!((radius - r).abs() < 0.1);
        let (five, ..) = on_cylinder(5);
        assert!(fit(Kind::Cylinder, &five).is_some());
    }

    #[test]
    fn cone_exact_and_noisy() {
        let (pts, apex, axis, half) = on_cone(40);
        let f = fit(Kind::Cone, &pts).expect("cone");
        let Primitive::Cone { apex: ap, axis: a, half_angle } = f.primitive else { panic!("kind") };
        assert!(f.rms < 1e-6, "rms {}", f.rms);
        assert!((half_angle - half).abs() < 1e-5);
        assert!(dot(v3(a), axis).abs() > 1.0 - 1e-8);
        assert!(norm(sub(v3(ap), apex)) < 1e-4);
        let mut n = Noise(13);
        let noisy: Vec<Vec3> = pts.iter().map(|p| n.jitter(*p, 0.02)).collect();
        let g = fit(Kind::Cone, &noisy).expect("noisy cone");
        assert!(g.rms < 0.05, "rms {}", g.rms);
        let (six, ..) = on_cone(6);
        assert!(fit(Kind::Cone, &six).is_some());
    }

    #[test]
    fn projection_lands_on_primitive_and_is_idempotent() {
        let shapes = [
            fit(Kind::Plane, &on_plane(10)),
            fit(Kind::Sphere, &on_sphere(10)),
            fit(Kind::Cylinder, &on_cylinder(20).0),
            fit(Kind::Cone, &on_cone(20).0),
        ];
        for f in shapes.into_iter().flatten() {
            let moved = project_all(&f.primitive, &[Vec3::new(7.0, -3.0, 5.0), Vec3::new(-2.0, 8.0, 1.0)]);
            for p in &moved {
                let again = project(&f.primitive, *p);
                assert!((again - *p).len() < 1e-6, "{:?}", f.primitive);
            }
        }
    }

    #[test]
    fn projection_on_axis_and_centre_is_finite() {
        let cyl = Primitive::Cylinder { point: Vec3::new(0.0, 0.0, 0.0), axis: Vec3::new(0.0, 0.0, 1.0), radius: 2.0 };
        let p = project(&cyl, Vec3::new(0.0, 0.0, 3.0));
        assert!(((p.x * p.x + p.y * p.y).sqrt() - 2.0).abs() < 1e-12 && (p.z - 3.0).abs() < 1e-12);
        let sph = Primitive::Sphere { centre: Vec3::new(1.0, 1.0, 1.0), radius: 2.0 };
        assert!(((project(&sph, Vec3::new(1.0, 1.0, 1.0)) - Vec3::new(1.0, 1.0, 1.0)).len() - 2.0).abs() < 1e-12);
        let cone = Primitive::Cone { apex: Vec3::new(0.0, 0.0, 0.0), axis: Vec3::new(0.0, 0.0, 1.0), half_angle: 0.5 };
        let q = project(&cone, Vec3::new(0.0, 0.0, 0.0));
        assert!(q.x.is_finite() && q.len() < 1e-12);
        // The lower nappe takes a point below the apex.
        let below = project(&cone, Vec3::new(1.0, 0.0, -5.0));
        assert!(below.z < 0.0);
        // A non-finite point comes back as it was.
        assert!(project(&cone, Vec3::new(f64::NAN, 0.0, 0.0)).x.is_nan());
    }

    #[test]
    fn best_picks_simplest() {
        let plane = on_plane(20);
        assert_eq!(best(&plane, 1e-6).map(|b| b.0), Some(Kind::Plane));
        assert_eq!(best(&on_sphere(30), 1e-6).map(|b| b.0), Some(Kind::Sphere));
        assert_eq!(best(&on_cylinder(30).0, 1e-6).map(|b| b.0), Some(Kind::Cylinder));
        assert_eq!(best(&on_cone(30).0, 1e-5).map(|b| b.0), Some(Kind::Cone));
        let mut n = Noise(21);
        let rough: Vec<Vec3> = (0..40).map(|_| Vec3::new(n.next() * 5.0, n.next() * 5.0, n.next() * 5.0)).collect();
        assert!(best(&rough, 1e-3).is_none());
        assert!(best(&plane, f64::NAN).is_none());
    }

    #[test]
    fn too_few_points() {
        let p = on_sphere(10);
        assert!(fit(Kind::Line, &p[..1]).is_none());
        assert!(fit(Kind::Plane, &p[..2]).is_none());
        assert!(fit(Kind::Circle, &p[..2]).is_none());
        assert!(fit(Kind::Sphere, &p[..3]).is_none());
        assert!(fit(Kind::Cylinder, &p[..4]).is_none());
        assert!(fit(Kind::Cone, &p[..5]).is_none());
        assert!(fit(Kind::Plane, &[]).is_none());
    }

    #[test]
    fn degenerate_inputs() {
        let same = vec![Vec3::new(1.0, 2.0, 3.0); 8];
        for k in [Kind::Plane, Kind::Line, Kind::Circle, Kind::Cylinder, Kind::Cone, Kind::Sphere] {
            assert!(fit(k, &same).is_none(), "{k:?}");
        }
        assert_eq!(straighten(&same), same);
        assert_eq!(even_spacing(&same), same);
        let collinear: Vec<Vec3> = (0..6).map(|i| Vec3::new(i as f64, 2.0 * i as f64, 0.0)).collect();
        assert!(fit(Kind::Plane, &collinear).is_none());
        assert!(fit(Kind::Circle, &collinear).is_none());
        assert!(fit(Kind::Line, &collinear).is_some());
        let mut nan = on_plane(6);
        nan[2] = Vec3::new(f64::NAN, 0.0, 0.0);
        for k in [Kind::Plane, Kind::Line, Kind::Circle, Kind::Cylinder, Kind::Cone, Kind::Sphere] {
            assert!(fit(k, &nan).is_none());
        }
        nan[2] = Vec3::new(f64::INFINITY, 0.0, 0.0);
        assert!(fit(Kind::Plane, &nan).is_none());
        let huge: Vec<Vec3> = on_plane(6).into_iter().map(|p| Vec3::new(p.x * 1e200, p.y, p.z)).collect();
        assert!(fit(Kind::Plane, &huge).is_none());
        assert_eq!(straighten(&nan).len(), 6);
        assert_eq!(even_spacing(&nan).len(), 6);
    }

    #[test]
    fn coplanar_points_are_no_sphere_or_cone_crash() {
        // Whatever comes back for flat points must be finite, never a panic.
        let flat = on_plane(12);
        for k in [Kind::Cylinder, Kind::Cone, Kind::Sphere, Kind::Circle] {
            if let Some(f) = fit(k, &flat) {
                assert!(f.rms.is_finite() && f.max.is_finite());
            }
        }
    }

    #[test]
    fn tiny_and_offset_clouds() {
        let tiny: Vec<Vec3> = on_sphere(12).into_iter().map(|p| Vec3::new(p.x * 1e-9, p.y * 1e-9, p.z * 1e-9)).collect();
        let f = fit(Kind::Sphere, &tiny).expect("tiny sphere");
        assert!(f.rms < 1e-15);
        let far: Vec<Vec3> = on_sphere(12).into_iter().map(|p| Vec3::new(p.x + 1e7, p.y - 1e7, p.z)).collect();
        let g = fit(Kind::Sphere, &far).expect("far sphere");
        assert!(g.rms < 1e-5, "rms {}", g.rms);
    }

    #[test]
    fn even_spacing_keeps_ends_and_evens_gaps() {
        let pts: Vec<Vec3> = [0.0, 0.1, 0.5, 2.0, 2.2, 5.0, 10.0].iter().map(|x| Vec3::new(*x, 0.0, 0.0)).collect();
        let out = even_spacing(&pts);
        assert_eq!(out.len(), pts.len());
        assert_eq!(out[0], pts[0]);
        assert_eq!(out[6], pts[6]);
        for w in out.windows(2) {
            assert!(((w[1] - w[0]).len() - 10.0 / 6.0).abs() < 1e-9);
        }
        // A bent chain keeps its length along the polyline.
        let bent = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 3.0, 0.0), Vec3::new(1.0, 4.0, 0.0)];
        let e = even_spacing(&bent);
        assert!((e[1] - Vec3::new(1.0, 2.0 / 3.0, 0.0)).len() < 1e-9);
        assert!((e[2] - Vec3::new(1.0, 7.0 / 3.0, 0.0)).len() < 1e-9);
        assert_eq!(even_spacing(&bent[..2]).len(), 2);
    }
}
