//! Shapes fitted to a scan and to each other together. A scan is a little off what was designed,
//! and the designer knows what was meant: this wall is square to Z, these two bores share an axis,
//! that floor is 25 mm below the ceiling. Held to such relations, the shapes are fitted to their
//! regions and to the relations at once, so each is the best fit that satisfies them, not a free
//! fit moved afterwards.
//!
//! The relations: parallel, perpendicular, coaxial (two round shapes on one axis), equal radius,
//! the distance between two parallel planes or axes, the angle between two directions, and a
//! radius held at a value. The world's axes, planes and origin stand in for a shape on the other
//! side (a bore on Z, a face square to X).
//!
//! The solve is Gauss–Newton damped as Levenberg–Marquardt, with numeric derivatives, over small
//! changes of each shape (two turns of its direction, its place, its size), each shape's points
//! weighted so that every shape counts alike whatever its size. The relations are residuals too,
//! weighted up in stages until they hold to far below the scan's noise.
//!
//! Suggested relations: what the shapes nearly are already, within 3° of parallel or
//! perpendicular (to each other or a world axis), on one axis within 5 % of the smaller radius
//! (at least 0.5 mm), radii within 3 %.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::Shape;

/// The most points of a region the solve reads (an even sample of more).
pub const MOST_POINTS: usize = 2000;
/// Within this many degrees, directions are suggested parallel or perpendicular.
pub const SUGGEST_DEGREES: f64 = 3.0;
/// Radii within this share of each other are suggested equal.
pub const SUGGEST_RADIUS: f64 = 0.03;

/// Something of the world a relation can hold a shape to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Datum {
    /// The world's axes, through the origin.
    X,
    Y,
    Z,
    /// The world's planes through the origin (XY square to Z, and so on).
    PlaneXy,
    PlaneYz,
    PlaneZx,
    Origin,
}

/// The other side of a relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Shape(usize),
    Datum(Datum),
}

/// What a relation holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Parallel,
    Perpendicular,
    /// Two round shapes (or one and a world axis) on one axis.
    Coaxial,
    EqualRadius,
    /// Two parallel planes this far apart, or two parallel axes, mm.
    Distance(f64),
    /// The angle between two directions, radians, 0 to a right angle.
    Angle(f64),
    /// A cylinder's or sphere's radius held at this, mm.
    Radius(f64),
    /// Touching: a plane (or a world plane) and a sphere or a cylinder (its axis along the plane,
    /// a radius off it), two spheres, or two parallel cylinders, outside or inside each other,
    /// whichever they are nearer.
    Tangent,
    /// `a` and `b`, of one kind, each other's reflection in the plane `about` (a fitted plane or a
    /// world plane): planes, spheres, cylinders, cones or tori.
    Mirror(Side),
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Parallel => "parallel",
            Kind::Perpendicular => "perpendicular",
            Kind::Coaxial => "coaxial",
            Kind::EqualRadius => "equal_radius",
            Kind::Distance(_) => "distance",
            Kind::Angle(_) => "angle",
            Kind::Radius(_) => "radius",
            Kind::Tangent => "tangent",
            Kind::Mirror(_) => "mirror",
        }
    }
}

/// A relation between shape `a` and `b` (none for a radius held at a value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relation {
    pub kind: Kind,
    pub a: usize,
    pub b: Option<Side>,
}

/// A shape to fit: where it starts (its free fit) and its region's points.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub shape: Shape,
    pub points: Vec<Vec3>,
}

/// Why the shapes could not be fitted together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelateError {
    /// A relation names a shape that is not there, or a shape that cannot take its place in it
    /// (a sphere has no direction, a plane no radius).
    Unfit { relation: usize, why: &'static str },
    /// A shape has too few points.
    TooFew { shape: usize },
}

impl std::fmt::Display for RelateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelateError::Unfit { relation, why } => write!(f, "relation {}: {why}", relation + 1),
            RelateError::TooFew { shape } => write!(f, "shape {} has too few points of the scan to fit", shape + 1),
        }
    }
}

/// The shapes fitted together.
#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    pub shapes: Vec<Shape>,
    /// Each shape's points' distance from it: root mean square and largest, mm.
    pub rms: Vec<f64>,
    pub max: Vec<f64>,
    /// How far each relation is from holding (mm, or radians for directions).
    pub off: Vec<f64>,
}

fn axis_of(d: Datum) -> Vec3 {
    match d {
        Datum::X | Datum::PlaneYz => Vec3::new(1.0, 0.0, 0.0),
        Datum::Y | Datum::PlaneZx => Vec3::new(0.0, 1.0, 0.0),
        Datum::Z | Datum::PlaneXy | Datum::Origin => Vec3::new(0.0, 0.0, 1.0),
    }
}

/// A shape's direction: a plane's normal, a round shape's axis.
fn direction(s: &Shape) -> Option<Vec3> {
    match *s {
        Shape::Plane { normal, .. } => Some(normal),
        Shape::Sphere { .. } => None,
        _ => s.axis().map(|(_, d)| d),
    }
}

/// A shape's point: on its plane, on its axis, its centre or apex.
fn point(s: &Shape) -> Vec3 {
    match *s {
        Shape::Plane { point, .. } | Shape::Cylinder { point, .. } => point,
        Shape::Sphere { center, .. } | Shape::Torus { center, .. } => center,
        Shape::Cone { apex, .. } => apex,
    }
}

fn radius(s: &Shape) -> Option<f64> {
    match *s {
        Shape::Cylinder { radius, .. } | Shape::Sphere { radius, .. } => Some(radius),
        _ => None,
    }
}

/// How many small changes a shape takes.
fn freedoms(s: &Shape) -> usize {
    match s {
        Shape::Plane { .. } => 3,
        Shape::Sphere { .. } => 4,
        Shape::Cylinder { .. } => 5,
        Shape::Cone { .. } => 6,
        Shape::Torus { .. } => 7,
    }
}

/// Two directions square to `d` and each other.
fn across(d: Vec3) -> (Vec3, Vec3) {
    let e1 = d.any_perp().normalized().unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    (e1, d.cross(e1))
}

fn turned(d: Vec3, u: f64, v: f64) -> Vec3 {
    let (e1, e2) = across(d);
    (d + e1 * u + e2 * v).normalized().unwrap_or(d)
}

/// The shape changed by `x` (its own number of small changes, [`freedoms`]).
fn changed(s: &Shape, x: &[f64]) -> Shape {
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    match *s {
        Shape::Plane { point, normal } => Shape::Plane { point: point + normal * at(2), normal: turned(normal, at(0), at(1)) },
        Shape::Sphere { center, radius } => Shape::Sphere { center: center + Vec3::new(at(0), at(1), at(2)), radius: radius + at(3) },
        Shape::Cylinder { point, axis, radius } => {
            let (e1, e2) = across(axis);
            Shape::Cylinder { point: point + e1 * at(2) + e2 * at(3), axis: turned(axis, at(0), at(1)), radius: radius + at(4) }
        }
        Shape::Cone { apex, axis, half_angle } => Shape::Cone {
            apex: apex + Vec3::new(at(0), at(1), at(2)),
            axis: turned(axis, at(3), at(4)),
            half_angle: (half_angle + at(5)).clamp(1e-6, std::f64::consts::FRAC_PI_2 - 1e-6),
        },
        Shape::Torus { center, axis, major, minor } => Shape::Torus {
            center: center + Vec3::new(at(0), at(1), at(2)),
            axis: turned(axis, at(3), at(4)),
            major: major + at(5),
            minor: minor + at(6),
        },
    }
}

/// The other side's direction and point.
fn side_of(shapes: &[Shape], side: Side) -> (Option<Vec3>, Vec3, Option<f64>) {
    match side {
        Side::Shape(i) => shapes.get(i).map_or((None, Vec3::ZERO, None), |s| (direction(s), point(s), radius(s))),
        Side::Datum(d) => (Some(axis_of(d)), Vec3::ZERO, None),
    }
}

/// The plane of `side`: a fitted plane's point and normal, or a world plane's.
fn plane_of(shapes: &[Shape], side: Side) -> Option<(Vec3, Vec3)> {
    match side {
        Side::Shape(i) => match shapes.get(i)? {
            Shape::Plane { point, normal } => Some((*point, *normal)),
            _ => None,
        },
        Side::Datum(d @ (Datum::PlaneXy | Datum::PlaneYz | Datum::PlaneZx)) => Some((Vec3::ZERO, axis_of(d))),
        Side::Datum(_) => None,
    }
}

/// Whether `a` and `b` (a shape, or a world plane when `b` is none) can touch: a plane and a
/// sphere or a cylinder (either way round), two spheres, two cylinders.
fn touchable(a: &Shape, b: Option<&Shape>, plane_b: bool) -> bool {
    let ball_or_tube = |s: &Shape| matches!(s, Shape::Sphere { .. } | Shape::Cylinder { .. });
    match (a, b) {
        (Shape::Plane { .. }, Some(b)) => ball_or_tube(b),
        (a, Some(Shape::Plane { .. })) => ball_or_tube(a),
        (a, None) => plane_b && ball_or_tube(a),
        (Shape::Sphere { .. }, Some(Shape::Sphere { .. })) | (Shape::Cylinder { .. }, Some(Shape::Cylinder { .. })) => true,
        _ => false,
    }
}

/// Checks a relation can be held by the shapes it names.
fn check(relations: &[Relation], shapes: &[Shape]) -> Result<(), RelateError> {
    for (k, r) in relations.iter().enumerate() {
        let unfit = |why| Err(RelateError::Unfit { relation: k, why });
        let Some(a) = shapes.get(r.a) else { return unfit("names a shape that is not there") };
        let b = match r.b {
            Some(Side::Shape(i)) if i == r.a => return unfit("holds a shape to itself"),
            Some(Side::Shape(i)) => match shapes.get(i) {
                Some(s) => Some(*s),
                None => return unfit("names a shape that is not there"),
            },
            _ => None,
        };
        let directed = |s: &Shape| direction(s).is_some();
        let round = |s: &Shape| s.axis().is_some();
        match r.kind {
            Kind::Parallel | Kind::Perpendicular | Kind::Angle(_) => {
                if !directed(a) || b.is_some_and(|b| !directed(&b)) || r.b.is_none() || r.b == Some(Side::Datum(Datum::Origin)) {
                    return unfit("needs two directions: planes, round shapes or world axes");
                }
            }
            Kind::Coaxial => {
                if !round(a)
                    || b.is_some_and(|b| !round(&b))
                    || !matches!(r.b, Some(Side::Shape(_)) | Some(Side::Datum(Datum::X | Datum::Y | Datum::Z)))
                {
                    return unfit("needs two round shapes, or one and a world axis");
                }
            }
            Kind::EqualRadius => {
                if radius(a).is_none() || !b.is_some_and(|b| radius(&b).is_some()) {
                    return unfit("needs two cylinders or spheres");
                }
            }
            Kind::Distance(v) => {
                let planes = matches!(a, Shape::Plane { .. })
                    && (b.is_some_and(|b| matches!(b, Shape::Plane { .. }))
                        || matches!(r.b, Some(Side::Datum(Datum::PlaneXy | Datum::PlaneYz | Datum::PlaneZx))));
                let axes = round(a) && (b.is_some_and(|b| round(&b)) || matches!(r.b, Some(Side::Datum(Datum::X | Datum::Y | Datum::Z))));
                if !(planes || axes) || !(v.is_finite() && v >= 0.0) {
                    return unfit("needs two planes or two axes, and a distance of 0 or more");
                }
            }
            Kind::Radius(v) => {
                if radius(a).is_none() || !(v.is_finite() && v > 0.0) {
                    return unfit("needs a cylinder or a sphere, and a radius above 0");
                }
            }
            Kind::Tangent => {
                let plane_b = r.b.is_some_and(|side| plane_of(shapes, side).is_some());
                if r.b.is_none() || !touchable(a, b.as_ref(), plane_b) {
                    return unfit("needs a plane and a sphere or a cylinder, two spheres or two cylinders");
                }
            }
            Kind::Mirror(about) => {
                let same = b.is_some_and(|b| b.kind() == a.kind());
                let names = |side: Side| matches!(side, Side::Shape(i) if i == r.a || Some(Side::Shape(i)) == r.b);
                if !same || plane_of(shapes, about).is_none() || names(about) {
                    return unfit("needs two shapes of one kind and a third plane (or a world plane) to mirror them in");
                }
            }
        }
        if let Kind::Angle(v) = r.kind
            && !(0.0..=std::f64::consts::FRAC_PI_2).contains(&v)
        {
            return unfit("needs an angle from 0 to 90°");
        }
    }
    Ok(())
}

/// What is off in relation `r` among `shapes`: zero when it holds.
fn off(r: &Relation, shapes: &[Shape]) -> Vec<f64> {
    let Some(a) = shapes.get(r.a) else { return Vec::new() };
    let (da, pa, ra) = (direction(a), point(a), radius(a));
    let (db, pb, rb) = r.b.map_or((None, Vec3::ZERO, None), |b| side_of(shapes, b));
    let cross = |x: Vec3, y: Vec3| {
        let c = x.cross(y);
        vec![c.x, c.y, c.z]
    };
    match (r.kind, da, db) {
        (Kind::Parallel, Some(x), Some(y)) => cross(x, y),
        (Kind::Perpendicular, Some(x), Some(y)) => vec![x.dot(y)],
        (Kind::Angle(v), Some(x), Some(y)) => vec![x.dot(y).abs() - v.cos()],
        (Kind::Coaxial, Some(x), Some(y)) => {
            let w = pb - pa;
            let apart = w - x * w.dot(x);
            let mut out = cross(x, y);
            out.extend([apart.x, apart.y, apart.z]);
            out
        }
        (Kind::EqualRadius, ..) => vec![ra.unwrap_or(0.0) - rb.unwrap_or(0.0)],
        (Kind::Radius(v), ..) => vec![ra.unwrap_or(v) - v],
        (Kind::Distance(v), Some(x), Some(y)) => {
            let w = pb - pa;
            let gap = if matches!(a, Shape::Plane { .. }) { w.dot(x).abs() } else { (w - x * w.dot(x)).len() };
            let mut out = cross(x, y);
            out.push(gap - v);
            out
        }
        (Kind::Tangent, ..) => r.b.map_or_else(Vec::new, |side| touching(a, side, shapes)),
        (Kind::Mirror(about), ..) => match (r.b, plane_of(shapes, about)) {
            (Some(Side::Shape(j)), Some(m)) => shapes.get(j).map_or_else(Vec::new, |b| mirrored(a, b, m)),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// How far `a` and the other `side` are from touching (see [`Kind::Tangent`]).
fn touching(a: &Shape, side: Side, shapes: &[Shape]) -> Vec<f64> {
    // The nearer of touching outside and inside, at the gap `d` between centres or axes.
    let nearer = |d: f64, ra: f64, rb: f64| {
        let (outside, inside) = (ra + rb, (ra - rb).abs());
        if (d - outside).abs() <= (d - inside).abs() { d - outside } else { d - inside }
    };
    let plane_and = |(p, n): (Vec3, Vec3), s: &Shape| match *s {
        Shape::Sphere { center, radius } => vec![(center - p).dot(n).abs() - radius],
        Shape::Cylinder { point, axis, radius } => vec![axis.dot(n), (point - p).dot(n).abs() - radius],
        _ => Vec::new(),
    };
    match (a, side) {
        (Shape::Plane { point, normal }, Side::Shape(j)) => shapes.get(j).map_or_else(Vec::new, |b| plane_and((*point, *normal), b)),
        (_, side) if plane_of(shapes, side).is_some() => plane_of(shapes, side).map_or_else(Vec::new, |m| plane_and(m, a)),
        (Shape::Sphere { center: ca, radius: ra }, Side::Shape(j)) => match shapes.get(j) {
            Some(Shape::Sphere { center: cb, radius: rb }) => vec![nearer(ca.dist(*cb), *ra, *rb)],
            _ => Vec::new(),
        },
        (Shape::Cylinder { point: pa, axis: da, radius: ra }, Side::Shape(j)) => match shapes.get(j) {
            Some(Shape::Cylinder { point: pb, axis: db, radius: rb }) => {
                let w = *pb - *pa;
                let c = da.cross(*db);
                vec![c.x, c.y, c.z, nearer((w - *da * w.dot(*da)).len(), *ra, *rb)]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// How far `b` is from `a`'s reflection in the plane `(m, n)`.
fn mirrored(a: &Shape, b: &Shape, (m, n): (Vec3, Vec3)) -> Vec<f64> {
    let point = |p: Vec3| p - n * (2.0 * (p - m).dot(n));
    let dir = |d: Vec3| d - n * (2.0 * d.dot(n));
    let v = |d: Vec3| vec![d.x, d.y, d.z];
    // A reflected axis lies along `b`'s and through it.
    let on_axis = |pa: Vec3, da: Vec3, pb: Vec3, db: Vec3| {
        let (q, e) = (point(pa), dir(da));
        let w = q - pb;
        let mut out = v(e.cross(db));
        out.extend(v(w - db * w.dot(db)));
        out
    };
    match (*a, *b) {
        (Shape::Plane { point: pa, normal: na }, Shape::Plane { point: pb, normal: nb }) => {
            let mut out = v(dir(na).cross(nb));
            out.push((point(pa) - pb).dot(nb));
            out
        }
        (Shape::Sphere { center: ca, radius: ra }, Shape::Sphere { center: cb, radius: rb }) => {
            let mut out = v(point(ca) - cb);
            out.push(ra - rb);
            out
        }
        (Shape::Cylinder { point: pa, axis: da, radius: ra }, Shape::Cylinder { point: pb, axis: db, radius: rb }) => {
            let mut out = on_axis(pa, da, pb, db);
            out.push(ra - rb);
            out
        }
        (Shape::Cone { apex: pa, axis: da, half_angle: ha }, Shape::Cone { apex: pb, axis: db, half_angle: hb }) => {
            let mut out = v(dir(da).cross(db));
            out.extend(v(point(pa) - pb));
            out.push(ha - hb);
            out
        }
        (Shape::Torus { center: ca, axis: da, major: ma, minor: na2 }, Shape::Torus { center: cb, axis: db, major: mb, minor: nb2 }) => {
            let mut out = v(dir(da).cross(db));
            out.extend(v(point(ca) - cb));
            out.extend([ma - mb, na2 - nb2]);
            out
        }
        _ => Vec::new(),
    }
}

fn length(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

/// `a x = b` for a small dense `a`, by Gaussian elimination with partial pivoting.
fn solve_dense(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if !(a[pivot][col].abs() > 1e-300) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f != 0.0 {
                for k in col..n {
                    a[row][k] -= f * a[col][k];
                }
                b[row] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// An even sample of at most [`MOST_POINTS`] of `points`.
fn sample(points: &[Vec3]) -> Vec<Vec3> {
    let step = points.len().div_ceil(MOST_POINTS).max(1);
    points.iter().step_by(step).copied().filter(|p| p.is_finite()).collect()
}

/// The shapes `held` fitted to their points and to `relations` together.
pub fn solve(held: &[Held], relations: &[Relation]) -> Result<Solved, RelateError> {
    let mut shapes: Vec<Shape> = held.iter().map(|h| h.shape).collect();
    check(relations, &shapes)?;
    let points: Vec<Vec<Vec3>> = held.iter().map(|h| sample(&h.points)).collect();
    for (i, p) in points.iter().enumerate() {
        if p.len() < freedoms(&shapes[i]) {
            return Err(RelateError::TooFew { shape: i });
        }
    }
    let sizes: Vec<usize> = shapes.iter().map(freedoms).collect();
    let starts: Vec<usize> = sizes
        .iter()
        .scan(0, |at, n| {
            let s = *at;
            *at += n;
            Some(s)
        })
        .collect();
    let unknowns: usize = sizes.iter().sum();
    // Where each shape's point rows start; the relations' rows come after them all.
    let rows: Vec<usize> = points
        .iter()
        .scan(0, |at, p| {
            let s = *at;
            *at += p.len();
            Some(s)
        })
        .collect();
    let point_rows: usize = points.iter().map(Vec::len).sum();
    let scale: Vec<f64> = points.iter().map(|p| 1.0 / (p.len() as f64).sqrt()).collect();
    let relation_rows = |shapes: &[Shape], w: f64| -> Vec<f64> { relations.iter().flat_map(|r| off(r, shapes)).map(|v| v * w).collect() };
    // The residuals: each shape's points (weighted so each shape counts alike), then each
    // relation's, weighted by `w`.
    let residuals = |shapes: &[Shape], w: f64| -> Vec<f64> {
        let mut out = Vec::with_capacity(point_rows);
        for (i, (s, pts)) in shapes.iter().zip(&points).enumerate() {
            out.extend(pts.iter().map(|p| s.distance(*p) * scale[i]));
        }
        out.extend(relation_rows(shapes, w));
        out
    };
    let moved = |shapes: &[Shape], x: &[f64]| -> Vec<Shape> {
        shapes.iter().enumerate().map(|(i, s)| changed(s, &x[starts[i]..starts[i] + sizes[i]])).collect()
    };
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    for w in [1e1, 1e3, 1e5, 1e7] {
        let mut lambda = 1e-3;
        for _ in 0..60 {
            let r0 = residuals(&shapes, w);
            let c0 = cost(&r0);
            // The derivatives, column by column: a change of shape `i` moves its own points' rows
            // and the relations' rows only (the rest of a column is zero).
            let h = 1e-7;
            let columns: Vec<Vec<(usize, f64)>> = (0..unknowns)
                .map(|k| {
                    let i = starts.iter().rposition(|&s0| s0 <= k).unwrap_or(0);
                    let mut x = vec![0.0; sizes[i]];
                    x[k - starts[i]] = h;
                    let mut now = shapes.clone();
                    now[i] = changed(&shapes[i], &x);
                    let mut col: Vec<(usize, f64)> = points[i]
                        .iter()
                        .enumerate()
                        .map(|(m, p)| (rows[i] + m, (now[i].distance(*p) - shapes[i].distance(*p)) * scale[i] / h))
                        .collect();
                    let before = &r0[point_rows..];
                    col.extend(relation_rows(&now, w).iter().zip(before).enumerate().map(|(m, (a, b))| (point_rows + m, (a - b) / h)));
                    col
                })
                .collect();
            let dense = |col: &[(usize, f64)]| {
                let mut v = vec![0.0; r0.len()];
                for &(row, x) in col {
                    if let Some(c) = v.get_mut(row) {
                        *c = x;
                    }
                }
                v
            };
            let full: Vec<Vec<f64>> = columns.iter().map(|c| dense(c)).collect();
            let mut jtj = vec![vec![0.0; unknowns]; unknowns];
            let mut jtr = vec![0.0; unknowns];
            for i in 0..unknowns {
                jtr[i] = -columns[i].iter().map(|&(row, a)| a * r0[row]).sum::<f64>();
                for j in i..unknowns {
                    let v: f64 = columns[i].iter().map(|&(row, a)| a * full[j][row]).sum();
                    jtj[i][j] = v;
                    jtj[j][i] = v;
                }
            }
            let mut improved = false;
            for _ in 0..12 {
                let mut damped = jtj.clone();
                for (i, row) in damped.iter_mut().enumerate() {
                    row[i] += lambda * (jtj[i][i] + 1e-12);
                }
                let Some(step) = solve_dense(damped, jtr.clone()) else {
                    lambda *= 10.0;
                    continue;
                };
                let next = moved(&shapes, &step);
                let c1 = cost(&residuals(&next, w));
                if c1 < c0 {
                    shapes = next;
                    lambda = (lambda / 3.0).max(1e-12);
                    improved = c0 - c1 > 1e-14 * c0.max(1e-300);
                    break;
                }
                lambda *= 4.0;
            }
            if !improved {
                break;
            }
        }
    }
    let fits: Vec<(f64, f64)> = shapes
        .iter()
        .zip(held)
        .map(|(s, h)| {
            let d: Vec<f64> = h.points.iter().map(|p| s.distance(*p).abs()).collect();
            let rms = (d.iter().map(|v| v * v).sum::<f64>() / d.len().max(1) as f64).sqrt();
            (rms, d.iter().copied().fold(0.0, f64::max))
        })
        .collect();
    Ok(Solved {
        off: relations.iter().map(|r| length(&off(r, &shapes))).collect(),
        rms: fits.iter().map(|f| f.0).collect(),
        max: fits.iter().map(|f| f.1).collect(),
        shapes,
    })
}

/// The relations `shapes` nearly hold already (within [`SUGGEST_DEGREES`] and [`SUGGEST_RADIUS`]).
pub fn suggest(shapes: &[Shape]) -> Vec<Relation> {
    let near = SUGGEST_DEGREES.to_radians();
    let mut out = Vec::new();
    for (i, a) in shapes.iter().enumerate() {
        if let Some(da) = direction(a) {
            for (d, axis) in [(Datum::X, Vec3::new(1.0, 0.0, 0.0)), (Datum::Y, Vec3::new(0.0, 1.0, 0.0)), (Datum::Z, Vec3::new(0.0, 0.0, 1.0))] {
                if da.dot(axis).abs().clamp(0.0, 1.0).acos() <= near {
                    out.push(Relation { kind: Kind::Parallel, a: i, b: Some(Side::Datum(d)) });
                }
            }
        }
        for (j, b) in shapes.iter().enumerate().skip(i + 1) {
            if let (Some(da), Some(db)) = (direction(a), direction(b)) {
                let angle = da.dot(db).abs().clamp(0.0, 1.0).acos();
                if angle <= near {
                    let coaxial = match (a.axis(), b.axis()) {
                        (Some((pa, _)), Some((pb, _))) => {
                            let w = pb - pa;
                            let apart = (w - da * w.dot(da)).len();
                            let smaller = radius(a).into_iter().chain(radius(b)).fold(f64::INFINITY, f64::min);
                            let reach = if smaller.is_finite() { (0.05 * smaller).max(0.5) } else { 0.5 };
                            apart <= reach
                        }
                        _ => false,
                    };
                    out.push(Relation { kind: if coaxial { Kind::Coaxial } else { Kind::Parallel }, a: i, b: Some(Side::Shape(j)) });
                } else if (std::f64::consts::FRAC_PI_2 - angle).abs() <= near {
                    out.push(Relation { kind: Kind::Perpendicular, a: i, b: Some(Side::Shape(j)) });
                }
            }
            if let (Some(ra), Some(rb)) = (radius(a), radius(b))
                && (ra - rb).abs() <= SUGGEST_RADIUS * ra.min(rb)
                && a.kind() == b.kind()
            {
                out.push(Relation { kind: Kind::EqualRadius, a: i, b: Some(Side::Shape(j)) });
            }
            if nearly_touching(a, Side::Shape(j), shapes) {
                out.push(Relation { kind: Kind::Tangent, a: i, b: Some(Side::Shape(j)) });
            }
            // Mirror images in a fitted plane among the others, or a world plane.
            if a.like(b) {
                let planes = shapes
                    .iter()
                    .enumerate()
                    .filter(|(k, s)| *k != i && *k != j && matches!(s, Shape::Plane { .. }))
                    .map(|(k, _)| Side::Shape(k))
                    .chain([Datum::PlaneXy, Datum::PlaneYz, Datum::PlaneZx].map(Side::Datum));
                for about in planes {
                    let Some(m) = plane_of(shapes, about) else { continue };
                    if nearly_mirrored(a, b, m) {
                        out.push(Relation { kind: Kind::Mirror(about), a: i, b: Some(Side::Shape(j)) });
                    }
                }
            }
        }
        for d in [Datum::PlaneXy, Datum::PlaneYz, Datum::PlaneZx] {
            if nearly_touching(a, Side::Datum(d), shapes) {
                out.push(Relation { kind: Kind::Tangent, a: i, b: Some(Side::Datum(d)) });
            }
        }
    }
    out
}

/// How near is near for a shape of size `r`: 5 % of it, at least 0.5 mm.
fn near_for(r: f64) -> f64 {
    (0.05 * r).max(0.5)
}

/// Whether `a` and `side` nearly touch: the directions within [`SUGGEST_DEGREES`] of what
/// touching needs, the gap within [`near_for`] the smaller radius.
fn nearly_touching(a: &Shape, side: Side, shapes: &[Shape]) -> bool {
    let b = match side {
        Side::Shape(j) => shapes.get(j),
        Side::Datum(_) => None,
    };
    if !touchable(a, b, plane_of(shapes, side).is_some()) {
        return false;
    }
    let r = radius(a).into_iter().chain(b.and_then(radius)).fold(f64::INFINITY, f64::min);
    let off = touching(a, side, shapes);
    let sin = SUGGEST_DEGREES.to_radians().sin();
    // The direction rows (a cylinder's axis against a plane's normal, two axes crossed) are
    // sines; the last row is the gap.
    match off.split_last() {
        Some((gap, dirs)) => gap.abs() <= near_for(r) && dirs.iter().all(|x| x.abs() <= sin),
        None => false,
    }
}

/// Whether `b` is nearly `a` reflected in the plane `m`.
fn nearly_mirrored(a: &Shape, b: &Shape, m: (Vec3, Vec3)) -> bool {
    let size = radius(a).unwrap_or(10.0);
    let sin = SUGGEST_DEGREES.to_radians().sin();
    let off = mirrored(a, b, m);
    if off.is_empty() {
        return false;
    }
    // Rows: direction sines (three) where the kind has a direction, then lengths; a sphere's
    // and a torus's sizes and a cone's angle are left to `like`, which `a` and `b` passed.
    let (dirs, rest): (&[f64], &[f64]) = match a {
        Shape::Plane { .. } | Shape::Cylinder { .. } | Shape::Cone { .. } | Shape::Torus { .. } => off.split_at(3.min(off.len())),
        Shape::Sphere { .. } => (&[], &off[..]),
    };
    // A plane's one offset; the others' first three lengths (a centre, an apex, an axis's offset).
    let lengths: Vec<f64> = match a {
        Shape::Plane { .. } => rest.to_vec(),
        _ => rest.iter().take(3).copied().collect(),
    };
    dirs.iter().all(|x| x.abs() <= sin) && length(&lengths) <= near_for(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points on a cylinder about `axis` through `point`, from `from` to `to` along it, shaken by
    /// `noise` in and out.
    fn cylinder(point: Vec3, axis: Vec3, radius: f64, from: f64, to: f64, noise: f64) -> Vec<Vec3> {
        let axis = axis.normalized().unwrap();
        let (e1, e2) = across(axis);
        let mut out = Vec::new();
        for i in 0..40 {
            for j in 0..30 {
                let a = std::f64::consts::TAU * f64::from(i) / 40.0;
                let t = from + (to - from) * f64::from(j) / 29.0;
                let shake = noise * ((f64::from(i * 31 + j * 17)).sin());
                out.push(point + axis * t + (e1 * a.cos() + e2 * a.sin()) * (radius + shake));
            }
        }
        out
    }

    /// Points on the plane through `point` square to `normal`, a square `size` across.
    fn plane(point: Vec3, normal: Vec3, size: f64, noise: f64) -> Vec<Vec3> {
        let normal = normal.normalized().unwrap();
        let (e1, e2) = across(normal);
        let mut out = Vec::new();
        for i in 0..30 {
            for j in 0..30 {
                let (u, v) = (size * (f64::from(i) / 29.0 - 0.5), size * (f64::from(j) / 29.0 - 0.5));
                let shake = noise * (f64::from(i * 13 + j * 7)).cos();
                out.push(point + e1 * u + e2 * v + normal * shake);
            }
        }
        out
    }

    /// Two bores scanned a hair off one axis and size, held coaxial and equal: one axis, one radius.
    #[test]
    fn two_bores_held_coaxial_and_equal_share_an_axis_and_a_radius() {
        let a = cylinder(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 5.0, 0.0, 10.0, 0.01);
        // The second 0.05 mm off the first's axis, parallel to it, and 0.04 mm larger.
        let b = cylinder(Vec3::new(0.05, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 5.04, 30.0, 40.0, 0.01);
        let held = [
            Held { shape: Shape::Cylinder { point: Vec3::ZERO, axis: Vec3::new(0.0, 0.0, 1.0), radius: 5.0 }, points: a },
            Held {
                shape: Shape::Cylinder { point: Vec3::new(0.05, 0.0, 30.0), axis: Vec3::new(0.01, 0.0, 1.0).normalized().unwrap(), radius: 5.04 },
                points: b,
            },
        ];
        let suggested = suggest(&held.iter().map(|h| h.shape).collect::<Vec<_>>());
        assert!(suggested.iter().any(|r| r.kind == Kind::Coaxial) && suggested.iter().any(|r| r.kind == Kind::EqualRadius), "{suggested:?}");
        let relations =
            [Relation { kind: Kind::Coaxial, a: 0, b: Some(Side::Shape(1)) }, Relation { kind: Kind::EqualRadius, a: 0, b: Some(Side::Shape(1)) }];
        let solved = solve(&held, &relations).unwrap();
        assert!(solved.off.iter().all(|o| *o < 1e-6), "{:?}", solved.off);
        let (Shape::Cylinder { axis: x, radius: r1, .. }, Shape::Cylinder { radius: r2, .. }) = (solved.shapes[0], solved.shapes[1]) else {
            panic!()
        };
        assert!((r1 - r2).abs() < 1e-6 && r1 > 5.0 && r1 < 5.04, "between the two: {r1} {r2}");
        // One axis through both: from the first's middle (z 5) to the second's (z 35, 0.05 off),
        // a slope of 0.05 / 30.
        assert!((x.cross(Vec3::new(0.0, 0.0, 1.0)).len() - 0.05 / 30.0).abs() < 3e-4, "{x:?}");
    }

    /// A face scanned 1° off square, held square to Z (parallel to the Z axis as its normal), ends
    /// up square, and two parallel faces held 25 mm apart are 25 mm apart.
    #[test]
    fn faces_held_square_and_a_distance_apart() {
        let tilt = Vec3::new(1.0f64.to_radians().sin(), 0.0, 1.0f64.to_radians().cos());
        let floor = plane(Vec3::ZERO, tilt, 40.0, 0.005);
        let ceiling = plane(Vec3::new(0.0, 0.0, 25.2), Vec3::new(0.0, 0.0, 1.0), 40.0, 0.005);
        let held = [
            Held { shape: Shape::Plane { point: Vec3::ZERO, normal: tilt }, points: floor },
            Held { shape: Shape::Plane { point: Vec3::new(0.0, 0.0, 25.2), normal: Vec3::new(0.0, 0.0, 1.0) }, points: ceiling },
        ];
        let relations = [
            Relation { kind: Kind::Parallel, a: 0, b: Some(Side::Datum(Datum::Z)) },
            Relation { kind: Kind::Distance(25.0), a: 0, b: Some(Side::Shape(1)) },
        ];
        let solved = solve(&held, &relations).unwrap();
        assert!(solved.off.iter().all(|o| *o < 1e-6), "{:?}", solved.off);
        let (Shape::Plane { point: p0, normal: n0 }, Shape::Plane { point: p1, .. }) = (solved.shapes[0], solved.shapes[1]) else { panic!() };
        assert!(n0.cross(Vec3::new(0.0, 0.0, 1.0)).len() < 1e-6);
        assert!(((p1 - p0).dot(n0).abs() - 25.0).abs() < 1e-6);
        // The tilted face, held square, is off its points by about its tilt over its half width.
        assert!(solved.max[0] > 0.2 && solved.max[0] < 0.5, "{}", solved.max[0]);
    }

    /// A cylinder lying 0.08 mm up off the plane it rests on, held tangent to it (suggested so),
    /// lies on it: its axis a radius above the plane, along it.
    #[test]
    fn a_cylinder_held_tangent_rests_on_its_plane() {
        let tube = cylinder(Vec3::new(0.0, 0.0, 5.08), Vec3::new(1.0, 0.0, 0.0), 5.0, -10.0, 10.0, 0.005);
        let floor = plane(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 40.0, 0.005);
        let held = [
            Held { shape: Shape::Cylinder { point: Vec3::new(0.0, 0.0, 5.08), axis: Vec3::new(1.0, 0.0, 0.0), radius: 5.0 }, points: tube },
            Held { shape: Shape::Plane { point: Vec3::ZERO, normal: Vec3::new(0.0, 0.0, 1.0) }, points: floor },
        ];
        let shapes: Vec<Shape> = held.iter().map(|h| h.shape).collect();
        assert!(suggest(&shapes).iter().any(|r| r.kind == Kind::Tangent && r.a == 0 && r.b == Some(Side::Shape(1))), "{:?}", suggest(&shapes));
        let solved = solve(&held, &[Relation { kind: Kind::Tangent, a: 0, b: Some(Side::Shape(1)) }]).unwrap();
        assert!(solved.off[0] < 1e-6, "{:?}", solved.off);
        let (Shape::Cylinder { point, axis, radius }, Shape::Plane { point: q, normal: n }) = (solved.shapes[0], solved.shapes[1]) else { panic!() };
        assert!(((point - q).dot(n).abs() - radius).abs() < 1e-6 && axis.dot(n).abs() < 1e-6);
        assert!((radius - 5.0).abs() < 0.05 && (point.z - 5.0).abs() < 0.06, "{point:?} r {radius}");
    }

    /// Two spheres scanned a hair off each other's reflection in the YZ plane, held mirrored in
    /// it (suggested so): their centres are each other's reflection and their radii one.
    #[test]
    fn two_spheres_held_mirrored_are_each_others_reflection() {
        let ball = |c: Vec3, r: f64| -> Vec<Vec3> {
            let mut out = Vec::new();
            for i in 0..20 {
                for j in 1..20 {
                    let (t, p) = (std::f64::consts::TAU * f64::from(i) / 20.0, std::f64::consts::PI * f64::from(j) / 20.0);
                    out.push(c + Vec3::new(p.sin() * t.cos(), p.sin() * t.sin(), p.cos()) * r);
                }
            }
            out
        };
        let (ca, cb) = (Vec3::new(-20.1, 0.0, 0.0), Vec3::new(19.9, 0.05, 0.0));
        let held = [
            Held { shape: Shape::Sphere { center: ca, radius: 5.0 }, points: ball(ca, 5.0) },
            Held { shape: Shape::Sphere { center: cb, radius: 5.04 }, points: ball(cb, 5.04) },
        ];
        let shapes: Vec<Shape> = held.iter().map(|h| h.shape).collect();
        let mirror = Relation { kind: Kind::Mirror(Side::Datum(Datum::PlaneYz)), a: 0, b: Some(Side::Shape(1)) };
        assert!(suggest(&shapes).contains(&mirror), "{:?}", suggest(&shapes));
        let solved = solve(&held, &[mirror]).unwrap();
        assert!(solved.off[0] < 1e-6, "{:?}", solved.off);
        let (Shape::Sphere { center: a, radius: ra }, Shape::Sphere { center: b, radius: rb }) = (solved.shapes[0], solved.shapes[1]) else {
            panic!()
        };
        assert!(a.dist(Vec3::new(-b.x, b.y, b.z)) < 1e-6 && (ra - rb).abs() < 1e-6, "{a:?} {b:?}");
        // Mirrored in their own sphere is refused: the plane must be a plane.
        let wrong = [Relation { kind: Kind::Mirror(Side::Shape(0)), a: 0, b: Some(Side::Shape(1)) }];
        assert!(matches!(solve(&held, &wrong), Err(RelateError::Unfit { .. })));
    }

    #[test]
    fn a_relation_a_shape_cannot_hold_is_refused() {
        let held = [Held { shape: Shape::Sphere { center: Vec3::ZERO, radius: 2.0 }, points: vec![Vec3::new(2.0, 0.0, 0.0); 10] }];
        let parallel = [Relation { kind: Kind::Parallel, a: 0, b: Some(Side::Datum(Datum::Z)) }];
        assert!(matches!(solve(&held, &parallel), Err(RelateError::Unfit { relation: 0, .. })));
        let missing = [Relation { kind: Kind::EqualRadius, a: 0, b: Some(Side::Shape(4)) }];
        assert!(matches!(solve(&held, &missing), Err(RelateError::Unfit { .. })));
        let radius = [Relation { kind: Kind::Radius(-1.0), a: 0, b: None }];
        assert!(matches!(solve(&held, &radius), Err(RelateError::Unfit { .. })));
    }
}
