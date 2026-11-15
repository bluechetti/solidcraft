//! A fillet's radius measured from the scan, after SurfaceCraft's fillet inference: the scan's
//! points near a model's edges, each seen in the plane square to the edge at its nearest station,
//! fitted with one round between the edge's two faces (each face free to sit a little off the
//! model's, where the scan's surface lies), how well it fits against the sharp edge, and how the
//! radius varies station by station.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

/// One place along an edge: the point, the edge's unit tangent, and its two faces, each as its
/// outward unit normal and the unit direction square to the edge that runs into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub point: Vec3,
    pub tangent: Vec3,
    pub faces: [(Vec3, Vec3); 2],
}

/// What the scan says about the rounding of the edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// The radius that fits best, and the one used (rounded to `step` when one is asked for).
    pub fitted: f64,
    pub radius: f64,
    pub step: f64,
    /// How far the points are from the round at `radius`, root mean square and most; and from
    /// the sharp edge.
    pub rms: f64,
    pub maximum: f64,
    pub sharp_rms: f64,
    /// Points used (after the furthest were set aside), of them on the round, and set aside.
    pub points: usize,
    pub on_round: usize,
    pub set_aside: usize,
    /// The lowest and highest radius fitted station by station.
    pub station_low: f64,
    pub station_high: f64,
    /// How far the scan's faces sit from the model's along their outward normals, two to an edge.
    pub face_offsets: Vec<f64>,
}

/// Why no round could be measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilletError {
    /// The search width or rounding step is no length.
    BadSearch,
    /// The faces at an edge meet almost flat or fold back.
    NoCorner,
    /// An edge has fewer than two stations.
    TooShort,
    /// Fewer than 20 points of the scan near the edges.
    TooFewPoints,
    /// The points near the edge are scattered.
    Scattered,
    /// The fitted round is wider than the search.
    TooWide,
    /// The scan shows a sharp edge.
    Sharp,
    /// Fewer than 6 points on the round itself.
    TooFewOnRound,
    /// A round fits no better than the sharp edge.
    NoBetter,
}

impl std::fmt::Display for FilletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FilletError::BadSearch => "the search width must be a length above zero",
            FilletError::NoCorner => "the faces at an edge meet almost flat or fold back: there is no corner to round",
            FilletError::TooShort => "an edge is too short to measure",
            FilletError::TooFewPoints => "too few scan points lie near the edge: align the scan to the model, or widen the search",
            FilletError::Scattered => "the scan near the edge is too scattered to fit a round",
            FilletError::TooWide => "the round looks wider than the search: widen it",
            FilletError::Sharp => "the scan shows a sharp edge here, not a round",
            FilletError::TooFewOnRound => "too few scan points lie on the round to measure it",
            FilletError::NoBetter => "a round fits the scan no better than the sharp edge does",
        })
    }
}

impl std::error::Error for FilletError {}

/// A station in its own plane: the rays along the faces and the faces' outward normals.
#[derive(Clone, Copy, Debug)]
struct Corner {
    d: (Vec2, Vec2),
    m: (Vec2, Vec2),
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    p: Vec3,
    t: Vec3,
    e1: Vec3,
    e2: Vec3,
    corner: Corner,
}

/// A point of the scan seen in the plane square to an edge at its nearest station.
#[derive(Clone, Copy, Debug)]
struct Section {
    edge: usize,
    station: usize,
    q: Vec2,
}

/// The most points fitted.
const MOST_POINTS: usize = 6_000;

/// The corners of the scan's triangles near `edges` (within `reach` of their stations' box)
/// that face out of the corner: each triangle's outward normal (its winding times `outward`)
/// not turned back against the two faces' outward normals at its nearest station. A round and the
/// faces it joins all face out of the corner; the inside of a thin wall near the edge (a shelled
/// part's, a few millimetres in) faces back into it, and read as part of the corner it makes any
/// round look sharp.
pub fn facing_out(scan: &crate::Scan, edges: &[Vec<Station>], reach: f64, outward: f64) -> Vec<Vec3> {
    let stations: Vec<&Station> = edges.iter().flatten().collect();
    let Some(first) = stations.first() else { return Vec::new() };
    let (lo, hi) = stations.iter().fold((first.point, first.point), |(lo, hi), s| (lo.min(s.point), hi.max(s.point)));
    let pad = Vec3::new(1.0, 1.0, 1.0) * reach;
    let (lo, hi) = (lo - pad, hi + pad);
    let inside = |q: Vec3| q.x >= lo.x && q.y >= lo.y && q.z >= lo.z && q.x <= hi.x && q.y <= hi.y && q.z <= hi.z;
    let mut out = Vec::new();
    for t in 0..scan.triangle_count() {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        if !(inside(a) || inside(b) || inside(c)) {
            continue;
        }
        let Some(normal) = ((b - a).cross(c - a) * outward).normalized() else { continue };
        let middle = (a + b + c) / 3.0;
        let Some(near) = stations.iter().min_by(|x, y| x.point.dist(middle).total_cmp(&y.point.dist(middle))) else { continue };
        if normal.dot(near.faces[0].0 + near.faces[1].0) >= 0.0 {
            out.extend([a, b, c]);
        }
    }
    out
}

/// One radius fitted to the scan's `points` near `edges` (each edge's stations, in order).
/// Points further than `reach` from an edge are not used; `step` > 0 rounds the radius used to a
/// multiple of it.
pub fn measure(points: &[Vec3], edges: &[Vec<Station>], reach: f64, step: f64) -> Result<Measured, FilletError> {
    if !(reach.is_finite() && reach > 0.0 && !edges.is_empty() && step >= 0.0 && step.is_finite()) {
        return Err(FilletError::BadSearch);
    }
    let unit2 = |v: Vec2| v.normalized().unwrap_or(Vec2::X);
    let mut frames: Vec<Vec<Frame>> = Vec::with_capacity(edges.len());
    for list in edges {
        let mut edge_frames = Vec::with_capacity(list.len());
        for s in list {
            let (Some(t), Some(d1), Some(d2)) = (s.tangent.normalized(), s.faces[0].1.normalized(), s.faces[1].1.normalized()) else {
                return Err(FilletError::NoCorner);
            };
            let e1 = d1;
            let e2 = t.cross(d1).normalized().ok_or(FilletError::NoCorner)?;
            let flat = |v: Vec3| unit2(Vec2::new(v.dot(e1), v.dot(e2)));
            let corner = Corner { d: (flat(d1), flat(d2)), m: (flat(s.faces[0].0), flat(s.faces[1].0)) };
            let angle = corner.d.0.dot(corner.d.1).clamp(-1.0, 1.0).acos();
            if !(angle > 0.05 && angle < PI - 0.05) {
                return Err(FilletError::NoCorner);
            }
            edge_frames.push(Frame { p: s.point, t, e1, e2, corner });
        }
        if edge_frames.len() < 2 {
            return Err(FilletError::TooShort);
        }
        frames.push(edge_frames);
    }
    // The points near each edge, at their nearest station, in its plane.
    let mut sections: Vec<Section> = Vec::new();
    for (edge, list) in frames.iter().enumerate() {
        let spacing = list.windows(2).map(|w| w[0].p.dist(w[1].p)).fold(0.0, f64::max);
        let (lo, hi) = list.iter().fold((list[0].p, list[0].p), |(lo, hi), f| (lo.min(f.p), hi.max(f.p)));
        let pad = Vec3::new(1.0, 1.0, 1.0) * (reach + spacing);
        let (lo, hi) = (lo - pad, hi + pad);
        for &q in points {
            if !(q.x >= lo.x && q.y >= lo.y && q.z >= lo.z && q.x <= hi.x && q.y <= hi.y && q.z <= hi.z) {
                continue;
            }
            let Some((best, f)) = list.iter().enumerate().min_by(|a, b| a.1.p.dist(q).total_cmp(&b.1.p.dist(q))) else { continue };
            let v = q - f.p;
            if !(v.dot(f.t).abs() <= 0.75 * spacing + 1e-9) {
                continue;
            }
            let flat = Vec2::new(v.dot(f.e1), v.dot(f.e2));
            if flat.len() < reach {
                sections.push(Section { edge, station: best, q: flat });
            }
        }
    }
    if sections.len() > MOST_POINTS {
        let stride = sections.len() as f64 / MOST_POINTS as f64;
        sections = (0..MOST_POINTS).map(|k| sections[(k as f64 * stride) as usize]).collect();
    }
    if sections.len() < 20 {
        return Err(FilletError::TooFewPoints);
    }
    let corner = |s: &Section| frames[s.edge][s.station].corner;
    // Parameters: the radius, then each edge's two face offsets.
    let residuals = |x: &[f64], list: &[Section]| -> Vec<f64> {
        list.iter().map(|s| distance(s.q, corner(s), x[0], (x[1 + 2 * s.edge], x[2 + 2 * s.edge]))).collect()
    };
    let cost = |x: &[f64], list: &[Section]| residuals(x, list).iter().map(|r| r * r).sum::<f64>();
    let mut x = vec![0.0; 1 + 2 * frames.len()];
    // A start: the best of radii spread from a hundredth of the reach to most of it.
    let mut best_start = f64::INFINITY;
    for i in 0..60 {
        let r = reach * 0.01 * 90f64.powf(f64::from(i) / 59.0);
        let mut trial = x.clone();
        trial[0] = r;
        let c = cost(&trial, &sections);
        if c < best_start {
            best_start = c;
            x[0] = r;
        }
    }
    x = least_squares(x, &sections, &residuals, reach);
    // Set aside points far off (another feature, noise, a hole's wall) and fit again.
    let first = residuals(&x, &sections);
    let rms0 = (first.iter().map(|r| r * r).sum::<f64>() / first.len() as f64).sqrt();
    let limit = (3.0 * rms0).max(1e-6 * reach);
    let kept: Vec<Section> = sections.iter().zip(&first).filter(|(_, r)| **r <= limit).map(|(s, _)| *s).collect();
    let set_aside = sections.len() - kept.len();
    if kept.len() < 20 {
        return Err(FilletError::Scattered);
    }
    x = least_squares(x, &kept, &residuals, reach);
    let fitted = x[0];
    if !(fitted.is_finite() && fitted > 1e-4 * reach && fitted < reach) {
        return Err(if fitted >= reach { FilletError::TooWide } else { FilletError::Sharp });
    }
    let radius = if step > 0.0 { step.max((fitted / step).round() * step) } else { fitted };
    let mut used = x.clone();
    used[0] = radius;
    let last = residuals(&used, &kept);
    let rms = (last.iter().map(|r| r * r).sum::<f64>() / last.len() as f64).sqrt();
    let mut sharp = x.clone();
    sharp[0] = 0.0;
    let sharp_rms = (cost(&sharp, &kept) / kept.len() as f64).sqrt();
    let offsets = |s: &Section| (x[1 + 2 * s.edge], x[2 + 2 * s.edge]);
    let on_round = kept.iter().filter(|s| on_arc(s.q, corner(s), fitted, offsets(s))).count();
    if on_round < 6 {
        return Err(FilletError::TooFewOnRound);
    }
    if !(sharp_rms > 1.5 * rms) {
        return Err(FilletError::NoBetter);
    }
    // Station by station, the radius alone, the faces where the whole fit put them.
    let mut by_station: BTreeMap<(usize, usize), Vec<Section>> = BTreeMap::new();
    for s in &kept {
        by_station.entry((s.edge, s.station)).or_default().push(*s);
    }
    let radii: Vec<f64> = by_station
        .values()
        .filter(|list| list.iter().filter(|s| on_arc(s.q, corner(s), fitted, offsets(s))).count() >= 3)
        .map(|list| {
            golden_section(0.0, 2.0 * fitted, |r| {
                let mut trial = x.clone();
                trial[0] = r;
                cost(&trial, list)
            })
        })
        .collect();
    Ok(Measured {
        fitted,
        radius,
        step,
        rms,
        maximum: last.iter().copied().fold(0.0, f64::max),
        sharp_rms,
        points: kept.len(),
        on_round,
        set_aside,
        station_low: radii.iter().copied().reduce(f64::min).unwrap_or(fitted),
        station_high: radii.iter().copied().reduce(f64::max).unwrap_or(fitted),
        face_offsets: x[1..].to_vec(),
    })
}

/// The corner with its faces moved `h` along their outward normals: where the two face lines
/// meet, and the round of radius `r` between them: (apex, centre, the two places it touches).
fn shape(c: Corner, r: f64, h: (f64, f64)) -> (Vec2, Vec2, Vec2, Vec2) {
    // apex · m1 = h1, apex · m2 = h2.
    let det = c.m.0.x * c.m.1.y - c.m.0.y * c.m.1.x;
    let apex = if det.abs() > 1e-12 { Vec2::new((h.0 * c.m.1.y - h.1 * c.m.0.y) / det, (c.m.0.x * h.1 - c.m.1.x * h.0) / det) } else { Vec2::ZERO };
    let angle = c.d.0.dot(c.d.1).clamp(-1.0, 1.0).acos();
    let along = r / (angle / 2.0).tan();
    let bisector = (c.d.0 + c.d.1).normalized().unwrap_or(Vec2::X);
    (apex, apex + bisector * (r / (angle / 2.0).sin()), apex + c.d.0 * along, apex + c.d.1 * along)
}

/// How far a point is from the rounded corner: the two faces from where the round touches them
/// on, and the round between.
fn distance(q: Vec2, c: Corner, r: f64, h: (f64, f64)) -> f64 {
    let (_, centre, t1, t2) = shape(c, r.max(0.0), h);
    let ray = |start: Vec2, d: Vec2| {
        let v = q - start;
        if v.dot(d) <= 0.0 { v.len() } else { (v.x * d.y - v.y * d.x).abs() }
    };
    let mut best = ray(t1, c.d.0).min(ray(t2, c.d.1));
    if r > 0.0 && in_sector(q, centre, t1, t2) {
        best = best.min((q.dist(centre) - r).abs());
    }
    best
}

fn on_arc(q: Vec2, c: Corner, r: f64, h: (f64, f64)) -> bool {
    let (_, centre, t1, t2) = shape(c, r, h);
    in_sector(q, centre, t1, t2)
}

/// Whether `q` lies in the (smaller) angle at `centre` between the directions to `a` and `b`.
fn in_sector(q: Vec2, centre: Vec2, a: Vec2, b: Vec2) -> bool {
    let cross = |x: Vec2, y: Vec2| x.x * y.y - x.y * y.x;
    let (u, w, v) = (a - centre, b - centre, q - centre);
    if cross(u, w) >= 0.0 { cross(u, v) >= 0.0 && cross(v, w) >= 0.0 } else { cross(u, v) <= 0.0 && cross(v, w) <= 0.0 }
}

/// Damped Gauss–Newton (Levenberg–Marquardt) on the residuals, the Jacobian by central
/// differences, the radius kept at 0 or more; at most 60 rounds.
fn least_squares(start: Vec<f64>, list: &[Section], residuals: &impl Fn(&[f64], &[Section]) -> Vec<f64>, reach: f64) -> Vec<f64> {
    let mut x = start;
    let mut r = residuals(&x, list);
    let squares = |v: &[f64]| v.iter().map(|a| a * a).sum::<f64>();
    let mut c = squares(&r);
    let mut lambda = 1e-3;
    let n = x.len();
    for _ in 0..60 {
        let h = 1e-6 * reach;
        let columns: Vec<Vec<f64>> = (0..n)
            .map(|j| {
                let (mut plus, mut minus) = (x.clone(), x.clone());
                plus[j] += h;
                minus[j] -= h;
                residuals(&plus, list).iter().zip(residuals(&minus, list)).map(|(a, b)| (a - b) / (2.0 * h)).collect()
            })
            .collect();
        let jtj: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| columns[i].iter().zip(&columns[j]).map(|(a, b)| a * b).sum()).collect()).collect();
        let jtr: Vec<f64> = (0..n).map(|i| columns[i].iter().zip(&r).map(|(a, b)| a * b).sum()).collect();
        let mut improved = false;
        for _ in 0..12 {
            let mut a = jtj.clone();
            for (i, row) in a.iter_mut().enumerate() {
                row[i] = row[i] * (1.0 + lambda) + 1e-12;
            }
            let Some(delta) = crate::solve_dense(a, jtr.iter().map(|v| -v).collect()) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial: Vec<f64> = x.iter().zip(&delta).map(|(a, b)| a + b).collect();
            trial[0] = trial[0].max(0.0);
            let tr = residuals(&trial, list);
            let tc = squares(&tr);
            if tc < c {
                improved = c - tc > 1e-14 * c.max(1e-30);
                (x, r, c) = (trial, tr, tc);
                lambda = (lambda / 3.0).max(1e-9);
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    x
}

/// The least of `f` on `[low, high]`, by golden section.
fn golden_section(low: f64, high: f64, mut f: impl FnMut(f64) -> f64) -> f64 {
    let g = (5.0f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (low, high);
    let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
    let (mut fc, mut fd) = (f(c), f(d));
    for _ in 0..60 {
        if fc < fd {
            (b, d, fd) = (d, c, fc);
            c = b - g * (b - a);
            fc = f(c);
        } else {
            (a, c, fc) = (c, d, fd);
            d = a + g * (b - a);
            fd = f(d);
        }
    }
    (a + b) / 2.0
}

/// A round measured where the scan was tapped, with no model: the triangles round the tap that
/// face square to one axis and lie on one circle seen along it (or, along a circular edge, on one
/// torus), the flat faces either side left out by how far they lean from the round.
#[derive(Clone, Debug, PartialEq)]
pub struct Tapped {
    /// The radius that fits best, and the one given (rounded to `step` when one is asked for).
    pub fitted: f64,
    pub radius: f64,
    pub step: f64,
    /// True for a round on an outside edge (the material inside its arc), false for one filling
    /// an inside corner.
    pub convex: bool,
    /// The angle the round turns through, radians.
    pub swept: f64,
    /// The round's unit axis at its middle, and the point on the axis level with its middle.
    pub axis: Vec3,
    pub centre: Vec3,
    /// The middle of the round, on it.
    pub middle: Vec3,
    /// Where the two faces the round joins would meet were the edge sharp; none when the round
    /// turns through more than 170° (a boss or a bore, no edge).
    pub corner: Option<Vec3>,
    /// How far the round runs along its axis (round a circular edge, along the circle).
    pub length: f64,
    /// How far the round's points lie off it, root mean square and most.
    pub rms: f64,
    pub maximum: f64,
    /// Round a circular edge: the radius of the circle the round's axis runs round.
    pub along_circle: Option<f64>,
    /// The scan's triangles on the round.
    pub triangles: Vec<usize>,
}

/// Why no round was found where the scan was tapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapError {
    /// The scan has no triangles.
    NotOnTheScan,
    /// The rounding step is not 0 or more.
    BadStep,
    /// The scan round the tap is flat.
    Flat,
    /// The scan round the tap is no round (a sharp edge, a freeform).
    NotRound,
}

impl std::fmt::Display for TapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TapError::NotOnTheScan => "the scan has no triangles",
            TapError::BadStep => "the rounding step must be 0 or more",
            TapError::Flat => "the scan is flat there: tap the middle of a round",
            TapError::NotRound => "the scan there is no round: a sharp edge, or a curve of changing radius",
        })
    }
}

impl std::error::Error for TapError {}

/// A triangle's normal may lean this far from the round's there and still be on it: past it, the
/// flat face the round runs into.
const ROUND_LEAN: f64 = 10.0 * PI / 180.0;
/// The normals round the tap must spread this far before an axis is read from them.
const SPREAD: f64 = 15.0 * PI / 180.0;
/// Past this share of the middle eigenvalue the normals do not all lie square to one axis.
const NOT_STRAIGHT: f64 = 0.05;

/// The round under `at` on `scan`, its radius rounded to `step` (0: exact); `outward` is
/// [`crate::reconstruct::outward`] of the scan, which way its triangles face out.
pub fn measure_at(scan: &crate::Scan, at: Vec3, step: f64, outward: f64) -> Result<Tapped, TapError> {
    if !(step.is_finite() && step >= 0.0) {
        return Err(TapError::BadStep);
    }
    let seed = crate::base::nearest_triangle(scan, at).ok_or(TapError::NotOnTheScan)?;
    let neighbours = crate::base::edge_neighbours(scan);
    let extent = scan.extent();
    let facet = |t: usize| -> Option<(Vec3, Vec3, f64)> {
        let [a, b, c] = scan.corners(t)?;
        let doubled = (b - a).cross(c - a) * outward;
        let area = doubled.len() / 2.0;
        Some(((a + b + c) / 3.0, doubled.normalized()?, area))
    };
    // Each triangle's direction read over it and the triangles beside it: a scan's noise turns a
    // single triangle's too far for the lean test.
    let smooth = |t: usize| -> Option<Vec3> {
        let (_, n, a) = facet(t)?;
        neighbours.of(scan, t).filter_map(facet).fold(n * a, |s, (_, m, w)| s + m * w).normalized()
    };
    let grow = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> {
        let mut seen = std::collections::HashSet::from([seed]);
        let mut found = vec![seed];
        let mut head = 0;
        while let Some(&t) = found.get(head) {
            head += 1;
            for n in neighbours.of(scan, t) {
                if !seen.contains(&n) && keep(n) {
                    seen.insert(n);
                    found.push(n);
                }
            }
        }
        found
    };
    let side = scan.corners(seed).map(|[a, b, c]| (a.dist(b) + b.dist(c) + c.dist(a)) / 3.0).filter(|s| *s > 0.0).ok_or(TapError::NotOnTheScan)?;
    // The start: the scan round the tap, widened until its directions spread enough to show an axis.
    let mut reach = 3.0 * side;
    let start = loop {
        let near = grow(&|t| facet(t).is_some_and(|(c, _, _)| c.dist(at) <= reach));
        let normals: Vec<Vec3> = near.iter().filter_map(|&t| smooth(t)).collect();
        let mean = normals.iter().fold(Vec3::ZERO, |s, n| s + *n).normalized();
        let spread = mean.map_or(0.0, |m| normals.iter().map(|n| n.dot(m).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max));
        if spread >= SPREAD && near.len() >= 12 {
            break near;
        }
        reach *= 1.5;
        if reach > 0.1 * extent {
            return Err(TapError::Flat);
        }
    };
    let straight_round = straight(&start, &facet, &smooth, &grow);
    // A round along a circular edge, fitted as a torus: taken when the normals do not lie square
    // to one axis, or when it fits clearly closer than the straight round (a short stretch of a
    // wide circle looks straight).
    let torus = crate::fit::fit(scan, seed, crate::fit::Kind::Torus, None).ok().and_then(|f| torus_round(f, &facet));
    match (straight_round, torus) {
        (Some((s, bent)), Some(t)) if bent > NOT_STRAIGHT || rms(&t.residuals) < 0.5 * rms(&s.residuals) => finish(t, step),
        (Some((s, _)), _) => finish(s, step),
        (None, Some(t)) => finish(t, step),
        (None, None) => Err(TapError::NotRound),
    }
}

fn rms(residuals: &[f64]) -> f64 {
    (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len().max(1) as f64).sqrt()
}

/// The round in its own terms before rounding: the radius, the centre and axis at the middle, the
/// points on it in the plane square to the axis there (x out along the round's middle), and so on.
struct Found {
    radius: f64,
    convex: bool,
    swept: f64,
    axis: Vec3,
    centre: Vec3,
    mid: Vec3,
    length: f64,
    residuals: Vec<f64>,
    along_circle: Option<f64>,
    triangles: Vec<usize>,
}

fn finish(f: Found, step: f64) -> Result<Tapped, TapError> {
    let rms = rms(&f.residuals);
    let maximum = f.residuals.iter().fold(0.0f64, |m, r| m.max(r.abs()));
    // A round must stand clear of its own scatter.
    if !(f.radius.is_finite() && f.radius > 4.0 * rms) {
        return Err(TapError::NotRound);
    }
    let radius = if step > 0.0 { ((f.radius / step).round() * step).max(step) } else { f.radius };
    let corner = (f.swept < 170f64.to_radians()).then(|| f.centre + f.mid * (f.radius / (f.swept / 2.0).cos()));
    Ok(Tapped {
        fitted: f.radius,
        radius,
        step,
        convex: f.convex,
        swept: f.swept,
        axis: f.axis,
        centre: f.centre,
        middle: f.centre + f.mid * f.radius,
        corner,
        length: f.length,
        rms,
        maximum,
        along_circle: f.along_circle,
        triangles: f.triangles,
    })
}

/// The span of `angles` (radians) round a circle: where it starts and how far it turns, across
/// the widest gap between them.
fn span(mut angles: Vec<f64>) -> Option<(f64, f64)> {
    angles.retain(|a| a.is_finite());
    angles.sort_by(f64::total_cmp);
    let (first, last) = (*angles.first()?, *angles.last()?);
    let (mut gap, mut start) = (first + 2.0 * PI - last, first);
    for w in angles.windows(2) {
        if w[1] - w[0] > gap {
            gap = w[1] - w[0];
            start = w[1];
        }
    }
    Some((start, 2.0 * PI - gap))
}

type Facet<'a> = dyn Fn(usize) -> Option<(Vec3, Vec3, f64)> + 'a;

/// A round along a straight edge grown from `start`, and how far its normals stray from lying
/// square to one axis (the smallest eigenvalue over the middle one).
fn straight(
    start: &[usize],
    facet: &Facet,
    smooth: &dyn Fn(usize) -> Option<Vec3>,
    grow: &dyn Fn(&dyn Fn(usize) -> bool) -> Vec<usize>,
) -> Option<(Found, f64)> {
    let axis_of = |region: &[usize]| -> Option<(Vec3, f64)> {
        let mut m = [[0.0f64; 3]; 3];
        for &t in region {
            let (Some((_, _, w)), Some(n)) = (facet(t), smooth(t)) else { continue };
            let n = [n.x, n.y, n.z];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += w * n[i] * n[j];
                }
            }
        }
        let (values, vectors) = crate::constrain::eigen3(m);
        let axis = Vec3::new(vectors[0][0], vectors[0][1], vectors[0][2]).normalized()?;
        Some((axis, if values[1] > 0.0 { values[0].max(0.0) / values[1] } else { 1.0 }))
    };
    let circle_of = |region: &[usize], axis: Vec3| -> Option<(Vec3, f64)> {
        let u = axis.any_perp().normalized()?;
        let v = axis.cross(u);
        let flat: Vec<Vec2> = region.iter().filter_map(|&t| facet(t)).map(|(c, _, _)| Vec2::new(c.dot(u), c.dot(v))).collect();
        let (c, r) = crate::chain::best_circle(&flat, 1e-9)?;
        Some((u * c.x + v * c.y, r))
    };
    let radial = |p: Vec3, centre: Vec3, axis: Vec3| -> (f64, Option<Vec3>) {
        let d = p - centre;
        let out = d - axis * d.dot(axis);
        (out.len(), out.normalized())
    };
    let mut region = start.to_vec();
    let (mut axis, mut straightness) = axis_of(&region)?;
    let (mut centre, mut radius) = circle_of(&region, axis)?;
    let convex_of = |region: &[usize], centre: Vec3, axis: Vec3| -> bool {
        region
            .iter()
            .filter_map(|&t| Some((facet(t)?, smooth(t)?)))
            .map(|((c, _, w), n)| radial(c, centre, axis).1.map_or(0.0, |o| w * n.dot(o)))
            .sum::<f64>()
            >= 0.0
    };
    let mut convex = convex_of(&region, centre, axis);
    for _ in 0..16 {
        let tolerance = (0.03 * radius).max(1e-6);
        let sign = if convex { 1.0 } else { -1.0 };
        let grown = grow(&|t| {
            let (Some((c, _, _)), Some(n)) = (facet(t), smooth(t)) else { return false };
            let (r, out) = radial(c, centre, axis);
            (r - radius).abs() <= tolerance && out.is_some_and(|o| (n.dot(o) * sign).clamp(-1.0, 1.0).acos() <= ROUND_LEAN)
        });
        if grown.len() < 6 {
            return None;
        }
        let same = grown.len() == region.len();
        region = grown;
        (axis, straightness) = axis_of(&region)?;
        (centre, radius) = circle_of(&region, axis)?;
        convex = convex_of(&region, centre, axis);
        if same {
            break;
        }
    }
    // Points far off the circle (> 2.5σ) set aside, and the circle fitted again.
    let off = |region: &[usize], centre: Vec3, radius: f64| -> Vec<f64> {
        region.iter().filter_map(|&t| facet(t)).map(|(c, _, _)| radial(c, centre, axis).0 - radius).collect()
    };
    let residuals = off(&region, centre, radius);
    let sigma = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len().max(1) as f64).sqrt();
    if sigma > 0.0 {
        let kept: Vec<usize> = region.iter().zip(&residuals).filter(|(_, r)| r.abs() <= 2.5 * sigma).map(|(t, _)| *t).collect();
        if kept.len() >= 6
            && let Some((c, r)) = circle_of(&kept, axis)
        {
            (centre, radius, region) = (c, r, kept);
        }
    }
    let u = axis.any_perp().normalized()?;
    let v = axis.cross(u);
    let points: Vec<Vec3> = region.iter().filter_map(|&t| facet(t)).map(|(c, _, _)| c).collect();
    // The turn read from the directions, not the places: the flat faces' first stretch, kept
    // within the lean, adds no turn.
    let sign = if convex { 1.0 } else { -1.0 };
    let (from, swept) = span(region.iter().filter_map(|&t| smooth(t)).map(|n| n * sign).map(|n| n.dot(v).atan2(n.dot(u))).collect())?;
    let along: Vec<f64> = points.iter().map(|&p| (p - centre).dot(axis)).collect();
    let (low, high) = along.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &t| (l.min(t), h.max(t)));
    let half = from + swept / 2.0;
    let found = Found {
        radius,
        convex,
        swept,
        axis,
        centre: centre + axis * ((low + high) / 2.0),
        mid: u * half.cos() + v * half.sin(),
        length: (high - low).max(0.0),
        residuals: off(&region, centre, radius),
        along_circle: None,
        triangles: region,
    };
    Some((found, straightness))
}

/// A torus fitted round the tap read as a round along a circular edge: its tube is the round.
fn torus_round(f: crate::fit::Fitted, facet: &Facet) -> Option<Found> {
    let crate::fit::Shape::Torus { center, axis, major, minor } = f.shape else { return None };
    if !(minor < major) {
        return None;
    }
    let points: Vec<(Vec3, Vec3)> = f.triangles.iter().filter_map(|&t| facet(t)).map(|(c, n, _)| (c, n)).collect();
    let u = axis.any_perp().normalized()?;
    let v = axis.cross(u);
    // Round the circle (azimuth), and round the tube in the plane through the axis (meridian).
    let azimuth: Vec<f64> = points.iter().map(|(p, _)| (*p - center).dot(v).atan2((*p - center).dot(u))).collect();
    let (from, turn) = span(azimuth)?;
    let middle = from + turn / 2.0;
    let w = u * middle.cos() + v * middle.sin();
    let tube = center + w * major;
    let meridian: Vec<f64> = points
        .iter()
        .map(|(p, _)| {
            let d = *p - center;
            let t = d.dot(axis);
            t.atan2((d - axis * t).len() - major)
        })
        .collect();
    let (start, swept) = span(meridian)?;
    let half = start + swept / 2.0;
    let mid = w * half.cos() + axis * half.sin();
    let convex = points
        .iter()
        .map(|(p, n)| {
            let d = *p - center;
            let t = d.dot(axis);
            let ring = center + (d - axis * t).normalized().unwrap_or(w) * major;
            n.dot(*p - ring)
        })
        .sum::<f64>()
        >= 0.0;
    Some(Found {
        radius: minor,
        convex,
        swept,
        axis: axis.cross(w).normalized()?,
        centre: tube,
        mid,
        length: major * turn,
        residuals: points.iter().map(|(p, _)| f.shape.distance(*p)).collect(),
        along_circle: Some(major),
        triangles: f.triangles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

    use crate::test_meshes::{extruded, revolved, rounded_rectangle};

    /// `positions` each moved up to `amount` in a fixed pseudo-random way, as a scan's noise.
    fn shaken(positions: &[Vec3], amount: f64) -> Vec<Vec3> {
        let mut seed = 11u64;
        let mut rand = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5
        };
        positions.iter().map(|p| *p + Vec3::new(rand(), rand(), rand()) * (2.0 * amount)).collect()
    }

    /// A tap on a block's rounded vertical corner reads its radius, an outside round turning a
    /// right angle, and where the sharp corner would be; noise moves it little.
    #[test]
    fn a_tap_on_an_outside_round_reads_its_radius() {
        let (positions, triangles) = extruded(&rounded_rectangle(60.0, 40.0, 5.0, 1.0), 0.0, 20.0, 20);
        let tap = Vec3::new(60.0 - 5.0 + 5.0 * FRAC_PI_4.cos(), 5.0 - 5.0 * FRAC_PI_4.sin(), 10.0);
        for (noise, close) in [(0.0, 0.03), (0.02, 0.15)] {
            let positions = shaken(&positions, noise);
            let scan = crate::Scan::new(&positions, &triangles);
            let m = measure_at(&scan, tap, 0.5, crate::reconstruct::outward(&scan)).unwrap();
            assert!((m.fitted - 5.0).abs() < close, "noise {noise}: {m:?}");
            assert_eq!(m.radius, 5.0);
            assert!(m.convex, "{m:?}");
            assert!((m.swept.to_degrees() - 90.0).abs() < 8.0, "{}", m.swept.to_degrees());
            assert!(m.axis.z.abs() > 0.99 && m.length > 15.0, "{m:?}");
            let corner = m.corner.unwrap();
            assert!(corner.x > 59.0 && corner.y < 1.0, "{corner:?}");
        }
    }

    /// An inside round (an L whose inner corner is rounded at 4) reads as concave.
    #[test]
    fn a_tap_on_an_inside_round_reads_concave() {
        let mut outline: Vec<[f64; 2]> = Vec::new();
        let edge = |from: [f64; 2], to: [f64; 2], out: &mut Vec<[f64; 2]>| {
            let n = ((to[0] - from[0]).hypot(to[1] - from[1]).ceil() as u32).max(1);
            out.extend(crate::test_meshes::steps(from, to, n).into_iter().take(n as usize));
        };
        edge([0.0, 0.0], [40.0, 0.0], &mut outline);
        edge([40.0, 0.0], [40.0, 15.0], &mut outline);
        edge([40.0, 15.0], [19.0, 15.0], &mut outline);
        for i in 0..24 {
            let a = -FRAC_PI_2 - FRAC_PI_2 * f64::from(i) / 24.0;
            outline.push([19.0 + 4.0 * a.cos(), 19.0 + 4.0 * a.sin()]);
        }
        edge([15.0, 19.0], [15.0, 40.0], &mut outline);
        edge([15.0, 40.0], [0.0, 40.0], &mut outline);
        edge([0.0, 40.0], [0.0, 0.0], &mut outline);
        let (positions, triangles) = extruded(&outline, 0.0, 10.0, 10);
        let scan = crate::Scan::new(&positions, &triangles);
        let tap = Vec3::new(19.0 - 4.0 * FRAC_PI_4.cos(), 19.0 - 4.0 * FRAC_PI_4.sin(), 5.0);
        let m = measure_at(&scan, tap, 0.0, crate::reconstruct::outward(&scan)).unwrap();
        assert!((m.radius - 4.0).abs() < 0.05, "{m:?}");
        assert!(!m.convex, "{m:?}");
        let corner = m.corner.unwrap();
        assert!((corner.x - 15.0).abs() < 0.5 && (corner.y - 15.0).abs() < 0.5, "{corner:?}");
    }

    /// A round along a circular edge (a disc's rim rounded at 3) reads as a round of 3 about a
    /// circle of 17.
    #[test]
    fn a_tap_on_a_rounded_rim_reads_the_round() {
        let mut profile = vec![[0.0, 0.0], [0.0, 20.0], [7.0, 20.0]];
        for i in 1..=12 {
            let a = FRAC_PI_2 * f64::from(i) / 12.0;
            profile.push([7.0 + 3.0 * a.sin(), 17.0 + 3.0 * a.cos()]);
        }
        profile.extend([[10.0, 0.0]]);
        let (positions, triangles) = revolved(&profile, 180, false);
        let scan = crate::Scan::new(&positions, &triangles);
        let tap = Vec3::new(17.0 + 3.0 * FRAC_PI_4.cos(), 0.0, 7.0 + 3.0 * FRAC_PI_4.sin());
        let m = measure_at(&scan, tap, 0.0, crate::reconstruct::outward(&scan)).unwrap();
        assert!((m.radius - 3.0).abs() < 0.1, "{m:?}");
        assert!(m.along_circle.is_some_and(|r| (r - 17.0).abs() < 0.3), "{m:?}");
        assert!(m.convex, "{m:?}");
    }

    /// A flat face and a sharp edge are no round.
    #[test]
    fn flat_and_sharp_are_no_round() {
        let (positions, triangles) = extruded(&rounded_rectangle(60.0, 40.0, 5.0, 1.0), 0.0, 20.0, 20);
        let scan = crate::Scan::new(&positions, &triangles);
        let outward = crate::reconstruct::outward(&scan);
        assert!(measure_at(&scan, Vec3::new(30.0, 0.0, 10.0), 0.5, outward).is_err());
        assert!(measure_at(&scan, Vec3::new(30.0, 0.0, 20.0), 0.5, outward).is_err());
        assert_eq!(measure_at(&scan, Vec3::new(30.0, 0.0, 10.0), -1.0, outward).err(), Some(TapError::BadStep));
        assert_eq!(measure_at(&crate::Scan::new(&[], &[]), Vec3::ZERO, 0.5, outward).err(), Some(TapError::NotOnTheScan));
    }

    /// Stations along a straight edge on the X axis between a face running +Y (facing up) and one
    /// running down at `angle` from it, and points sampled on the corner rounded at `r` with a
    /// little noise.
    fn corner_scan(angle: f64, r: f64, noise: f64) -> (Vec<Station>, Vec<Vec3>) {
        // Face 1 runs along +Y in z = 0, outward +Z; face 2 runs from the edge at `angle` below it.
        let d1 = Vec3::new(0.0, 1.0, 0.0);
        let d2 = Vec3::new(0.0, angle.cos(), -angle.sin());
        let n1 = Vec3::Z;
        // Face 2's outward normal: square to the edge and d2, away from face 1's side.
        // Face 2's outward normal: square to the edge and d2, away from face 1 (a convex corner).
        let n2 = Vec3::X.cross(d2).normalized().map(|n| if n.dot(d1) < 0.0 { n } else { -n }).unwrap();
        let stations: Vec<Station> =
            (0..=10).map(|k| Station { point: Vec3::new(f64::from(k) * 3.0, 0.0, 0.0), tangent: Vec3::X, faces: [(n1, d1), (n2, d2)] }).collect();
        // The round in the YZ plane: centre on the bisector, tangent to both rays.
        let (a, b) = (Vec2::new(1.0, 0.0), Vec2::new(angle.cos(), -angle.sin()));
        let bis = (a + b).normalized().unwrap();
        let theta = a.dot(b).acos();
        let centre = bis * (r / (theta / 2.0).sin());
        let along = r / (theta / 2.0).tan();
        let (t1, t2) = (a * along, b * along);
        let mut seed = 7u64;
        let mut rand = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) - 0.5
        };
        let mut points = Vec::new();
        for i in 0..30 {
            let x = f64::from(i) + 0.3;
            // Along each face from the round out, and round the arc.
            for k in 0..20 {
                let s = f64::from(k) * 0.4;
                for (start, d) in [(t1, a), (t2, b)] {
                    let q = start + d * s;
                    points.push(Vec3::new(x, q.x + noise * rand(), q.y + noise * rand()));
                }
                let (u, w) = (t1 - centre, t2 - centre);
                let phi = (k as f64) / 19.0;
                let q = centre + (u * (1.0 - phi) + w * phi).normalized().unwrap() * r;
                points.push(Vec3::new(x, q.x + noise * rand(), q.y + noise * rand()));
            }
        }
        (stations, points)
    }

    #[test]
    fn a_rounded_edge_measures_its_radius() {
        let (stations, points) = corner_scan(PI / 2.0, 3.0, 0.02);
        let m = measure(&points, &[stations], 10.0, 0.5).unwrap();
        assert!((m.fitted - 3.0).abs() < 0.05, "{m:?}");
        assert_eq!(m.radius, 3.0);
        assert!(m.sharp_rms > 3.0 * m.rms);
        assert!(m.station_low > 2.8 && m.station_high < 3.2, "{m:?}");
    }

    #[test]
    fn a_sharp_edge_is_no_round() {
        let (stations, points) = corner_scan(PI / 2.0, 0.0001, 0.005);
        assert!(measure(&points, std::slice::from_ref(&stations), 10.0, 0.0).is_err());
        assert_eq!(measure(&points, &[stations], 0.0, 0.0).err(), Some(FilletError::BadSearch));
        assert_eq!(measure(&[], &[], 10.0, 0.0).err(), Some(FilletError::BadSearch));
    }
}
