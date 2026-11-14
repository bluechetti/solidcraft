//! Coil It's reading: a coil spring (a wire wound round an axis as it climbs) is read off the
//! pieces Find All fits to it: short stretches of a helix fit tori alike (the wire's radius, the
//! coil's), each leaning by the helix's angle, centred on one axis. The axis is their mean; the
//! pitch is the climb per turn that lines every point of the wire up on one helix (searched over
//! the points' angles round the axis and heights along it); the turns are as many as the scan's
//! height holds.

use std::collections::BTreeSet;
use std::f64::consts::{PI, TAU};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::hole_form::snapped;
use crate::in_plane;
use crate::reconstruct::Face;

/// Tori this near alike in either radius (a share) are pieces of one coil.
const ALIKE: f64 = 0.05;
/// The fewest tori a coil is read from, and the least share of the shapes' area they cover.
const LEAST_PIECES: usize = 4;
const LEAST_SHARE: f64 = 0.6;
/// A torus whose axis is further than this from the coil's (a cosine, about 25°) is no piece.
const LEANS: f64 = 0.9;
/// Steps the pitch is searched in, and the most points it is searched with.
const PITCH_STEPS: usize = 600;
const MOST_POINTS: usize = 20_000;
/// How tightly the points must line up on one helix for a coil to be read: the length of their
/// mean phase (1 for all on one line; points round a wire half a pitch thick, all on one helix,
/// give about 0.2 to 0.5; points that line up on no helix, about none).
const LEAST_LINED_UP: f64 = 0.15;

/// A coil spring as Coil makes one.
#[derive(Clone, Debug, PartialEq)]
pub struct Coil {
    /// On the axis at the height the wire's centre starts at, and the unit direction it climbs.
    pub base: Vec3,
    pub axis: Vec3,
    /// From the axis toward where the wire's centre starts (unit, square to the axis).
    pub start: Vec3,
    /// The coil's diameter (to the wire's centre) and the wire's.
    pub diameter: f64,
    pub wire: f64,
    /// The climb per turn, and how many turns.
    pub pitch: f64,
    pub turns: f64,
    /// Turning clockwise about `axis` as it climbs (a left-hand coil).
    pub clockwise: bool,
    pub faces: BTreeSet<usize>,
}

fn unit(v: Vec3) -> Vec3 {
    v.normalized().unwrap_or(Vec3::ZERO)
}

/// The coil the tori among `faces` make, if they make one; `points` gives a face's points.
pub fn read(faces: &[Face], points: &dyn Fn(usize) -> Vec<Vec3>, tolerance: f64) -> Option<Coil> {
    let tori: Vec<(usize, Vec3, Vec3, f64, f64)> = faces
        .iter()
        .enumerate()
        .filter(|(_, f)| f.area > 0.0)
        .filter_map(|(k, f)| match f.shape {
            Shape::Torus { center, axis, major, minor } if major > minor => Some((k, center, unit(axis), major, minor)),
            _ => None,
        })
        .collect();
    if tori.len() < LEAST_PIECES {
        return None;
    }
    // The most common torus, by area: the others alike with it.
    let largest = tori.iter().max_by(|a, b| faces[a.0].area.total_cmp(&faces[b.0].area))?;
    let (major, minor) = (largest.3, largest.4);
    let mut lean = largest.2;
    let alike: Vec<&(usize, Vec3, Vec3, f64, f64)> = tori
        .iter()
        .filter(|t| (t.3 - major).abs() <= ALIKE * major && (t.4 - minor).abs() <= ALIKE * minor + tolerance && t.2.dot(lean).abs() >= LEANS)
        .collect();
    if alike.len() < LEAST_PIECES {
        return None;
    }
    let total: f64 = faces.iter().map(|f| f.area).sum();
    let covered: f64 = alike.iter().map(|t| faces[t.0].area).sum();
    if covered < LEAST_SHARE * total {
        return None;
    }
    // The axis: the pieces' axes turned one way and averaged (their leans cancel round a turn),
    // through their centres' middle.
    let mut sum = Vec3::ZERO;
    for t in &alike {
        sum += if t.2.dot(lean) < 0.0 { -t.2 } else { t.2 };
    }
    lean = unit(sum);
    let middle = alike.iter().fold(Vec3::ZERO, |s, t| s + t.1) / alike.len() as f64;
    let axis = lean;
    let (u, v) = in_plane(axis);
    let mut wire_points: Vec<Vec3> = alike.iter().flat_map(|t| points(t.0)).collect();
    if wire_points.len() > MOST_POINTS {
        let step = wire_points.len() / MOST_POINTS + 1;
        wire_points = wire_points.into_iter().step_by(step).collect();
    }
    if wire_points.len() < 50 {
        return None;
    }
    // The axis through the middle of the points round it (the tori's centres are pulled by their
    // lean): a circle fitted to them seen along the axis.
    let flat: Vec<(f64, f64)> = wire_points.iter().map(|p| ((*p - middle).dot(u), (*p - middle).dot(v))).collect();
    let (cx, cy) = circle_centre(&flat)?;
    let centre = middle + u * cx + v * cy;
    let samples: Vec<(f64, f64)> = wire_points
        .iter()
        .map(|p| {
            let d = *p - centre;
            (d.dot(v).atan2(d.dot(u)), d.dot(axis))
        })
        .collect();
    let (low, high) = samples.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), s| (l.min(s.1), h.max(s.1)));
    // The pitch the lean says, then searched about it (both hands) for the one that lines the
    // points up best on one helix.
    let leaning: f64 = alike.iter().map(|t| t.2.dot(axis).abs().clamp(-1.0, 1.0).acos()).sum::<f64>() / alike.len() as f64;
    let guess = (TAU * major * leaning.tan()).max(2.0 * minor);
    let lined_up = |pitch: f64| {
        let (mut c, mut s) = (0.0, 0.0);
        for &(theta, h) in &samples {
            let phase = TAU * h / pitch - theta;
            c += phase.cos();
            s += phase.sin();
        }
        ((c * c + s * s).sqrt() / samples.len() as f64, s.atan2(c))
    };
    let mut best = (0.0, 0.0, 0.0);
    for hand in [1.0, -1.0] {
        for k in 0..=PITCH_STEPS {
            let pitch = hand * guess * (0.5 + 1.5 * k as f64 / PITCH_STEPS as f64);
            let (r, phase) = lined_up(pitch);
            if r > best.0 {
                best = (r, pitch, phase);
            }
        }
    }
    // Refined about the best step.
    let step = guess * 1.5 / PITCH_STEPS as f64;
    for k in -50..=50 {
        let pitch = best.1 + step * f64::from(k) / 50.0;
        let (r, phase) = lined_up(pitch);
        if r > best.0 {
            best = (r, pitch, phase);
        }
    }
    let (strength, coarse, coarse_phase) = best;
    if strength < LEAST_LINED_UP || coarse == 0.0 {
        return None;
    }
    // Refined on the pieces: each piece's middle (its points' mean height, at their mean angle)
    // lies on the wire's centre line, the angle unwound by the coarse helix; a line through them
    // climbs by the pitch a turn.
    let mids: Vec<(f64, f64)> = alike
        .iter()
        .filter_map(|t| {
            let ps = points(t.0);
            if ps.is_empty() {
                return None;
            }
            let (mut c, mut s, mut h) = (0.0, 0.0, 0.0);
            for p in &ps {
                let d = *p - centre;
                let a = d.dot(v).atan2(d.dot(u));
                c += a.cos();
                s += a.sin();
                h += d.dot(axis);
            }
            let h = h / ps.len() as f64;
            let theta = s.atan2(c);
            let expected = TAU * h / coarse - coarse_phase;
            Some((theta + TAU * ((expected - theta) / TAU).round(), h))
        })
        .collect();
    let (pitch, phase) = match line_through(&mids) {
        // h = a + b θ: a turn climbs 2π b; the phase is 2π a / pitch.
        Some((a, b)) if b != 0.0 && (TAU * b / coarse - 1.0).abs() < 0.25 => (TAU * b, TAU * a / (TAU * b)),
        _ => (coarse, coarse_phase),
    };
    let clockwise = pitch < 0.0;
    let wire = snapped(2.0 * minor, (0.01 * 2.0 * minor).max(0.25 * tolerance));
    let diameter = snapped(2.0 * major, 0.01 * 2.0 * major);
    let climb = snapped(pitch.abs(), 0.01 * pitch.abs());
    let r = 0.5 * wire;
    let start_height = low + r;
    let turns = ((high - low - wire) / pitch.abs() * 4.0).round() / 4.0;
    if !(turns > 0.0) {
        return None;
    }
    // On the helix, 2π h / pitch − θ = phase: the angle the wire starts at, at the start height.
    let theta = TAU * start_height / pitch - phase;
    let start = unit(u * theta.cos() + v * theta.sin());
    let base = centre + axis * start_height;
    Some(Coil { base, axis, start, diameter, wire, pitch: climb, turns, clockwise, faces: alike.iter().map(|t| t.0).collect() })
        .filter(|c| c.diameter > c.wire && (c.turns * PI).is_finite())
}

/// The line `y = a + b x` through `points` (least squares), `None` for fewer than two or all at
/// one `x`.
fn line_through(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let (mx, my) = (points.iter().map(|p| p.0).sum::<f64>() / n, points.iter().map(|p| p.1).sum::<f64>() / n);
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    (sxx > 0.0).then(|| (my - sxy / sxx * mx, sxy / sxx))
}

/// The centre of the circle fitted to `points` (least squares, algebraic).
fn circle_centre(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    // x² + y² + D x + E y + F = 0.
    let mut a = vec![vec![0.0; 3]; 3];
    let mut b = vec![0.0; 3];
    for &(x, y) in points {
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += row[i] * row[j];
            }
            b[i] += row[i] * rhs;
        }
    }
    let s = crate::solve_dense(a, b)?;
    Some((-0.5 * s[0], -0.5 * s[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points on a wire of radius `r` wound `turns` round Z at radius `big`, climbing `pitch`
    /// a turn from height 0, and the tori a fit to every quarter turn would give.
    fn spring(big: f64, r: f64, pitch: f64, turns: f64) -> (Vec<Face>, Vec<Vec<Vec3>>) {
        let quarters = (turns * 4.0) as usize;
        let lean = (pitch / (TAU * big)).atan();
        let mut faces = Vec::new();
        let mut points = Vec::new();
        for q in 0..quarters {
            let mut ps = Vec::new();
            for i in 0..40 {
                let t = (q as f64 + i as f64 / 40.0) * PI / 2.0;
                let along = Vec3::new(-t.sin(), t.cos(), pitch / (TAU * big));
                let out = Vec3::new(t.cos(), t.sin(), 0.0);
                let up = along.cross(out).normalized().unwrap();
                let c = Vec3::new(big * t.cos(), big * t.sin(), pitch * t / TAU);
                for j in 0..12 {
                    let a = TAU * j as f64 / 12.0;
                    ps.push(c + (out * a.cos() + up * a.sin()) * r);
                }
            }
            let t = (q as f64 + 0.5) * PI / 2.0;
            let tilt = Vec3::new(-t.sin(), t.cos(), 0.0) * -lean.sin() + Vec3::Z * lean.cos();
            faces.push(Face {
                shape: Shape::Torus { center: Vec3::new(0.0, 0.0, pitch * t / TAU), axis: tilt, major: big * 1.005, minor: r },
                area: 100.0,
            });
            points.push(ps);
        }
        (faces, points)
    }

    #[test]
    fn a_spring_is_read_with_its_pitch_turns_and_hand() {
        let (faces, points) = spring(15.0, 2.0, 8.0, 5.0);
        let coil = read(&faces, &|k| points[k].clone(), 0.06).unwrap();
        assert!(coil.axis.z > 0.999, "{coil:?}");
        assert_eq!((coil.diameter, coil.wire, coil.pitch, coil.turns, coil.clockwise), (30.0, 4.0, 8.0, 5.0, false), "{coil:?}");
        assert!(coil.base.dist(Vec3::ZERO) < 0.2, "{coil:?}");
        assert!(coil.start.dist(Vec3::X) < 0.05, "{coil:?}");
        // Wound the other way.
        let mirrored: Vec<Vec<Vec3>> = points.iter().map(|ps| ps.iter().map(|p| Vec3::new(p.x, -p.y, p.z)).collect()).collect();
        let left = read(&faces, &|k| mirrored[k].clone(), 0.06).unwrap();
        assert!(left.clockwise && left.pitch == 8.0, "{left:?}");
    }
}
