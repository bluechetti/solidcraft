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

#[cfg(test)]
mod tests {
    use super::*;

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
