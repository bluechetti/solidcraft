//! Loft It's reading: a part whose section changes smoothly from one end to the other (a blade, a
//! handle, a horn) is read as sections lofted. The direction it runs is the one two planes at its
//! ends face apart along, when Find All found such end faces; else the direction it is longest
//! along. The scan is sliced square to it at even steps from end to end; each slice must be one
//! closed loop, which is laid out from a matching start round the same way, so the loft through
//! them does not twist.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::reconstruct::Face;
use crate::{Scan, in_plane};

/// Directions this close are one direction (a cosine: about 2°).
const SAME_DIRECTION: f64 = 0.9994;
/// An end face lies this near the scan's end along its normal, as a share of the part's length.
const AT_THE_END: f64 = 0.01;
/// The slices nearest the ends are this share of the length in from them.
const END_MARGIN: f64 = 0.01;
/// The fewest sections and the most.
pub const LEAST_SECTIONS: usize = 2;
pub const MOST_SECTIONS: usize = 30;
/// Points each section is laid out with.
const POINTS: usize = 36;
/// A loop shorter than this share of the longest in its slice is a speck of noise.
const SPECK: f64 = 0.05;

/// One section: the plane it lies in (its x and y the same at every section) and its points in
/// that plane's coordinates, counter-clockwise, the first where the one before's first was.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub plane: Plane,
    pub points: Vec<Vec2>,
    /// The section as a wing's or a blade's is described, when it is longer than it is thick.
    pub shape: Option<Aerofoil>,
}

/// A section as a wing's or a blade's is described: by its chord and what stands on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aerofoil {
    /// The two points of the section farthest apart, in the section's plane: its leading edge
    /// (the blunter end) and its trailing edge. Alike at both ends (an ellipse), the leading edge
    /// is the one further along the plane's x.
    pub leading: Vec2,
    pub trailing: Vec2,
    /// Their distance, in mm.
    pub chord: f64,
    /// The angle of the chord line (trailing to leading edge) from the plane's x, in radians,
    /// within a quarter turn either way of the section before's.
    pub twist: f64,
    /// The most the section measures across its chord line, in mm, and how far along the chord
    /// from the leading edge that is (0–1).
    pub thickness: f64,
    pub thickest_at: f64,
    /// The most the middle of the section lies off its chord line, in mm (0 for a section alike
    /// both sides of it).
    pub camber: f64,
    /// The NACA four-digit section it is within the tolerance asked of, when it is one.
    pub naca: Option<[u8; 3]>,
}

/// The stations along the chord a section is measured at.
const STATIONS: usize = 100;
/// A section whose middle lies off its chord line by less than this share of the chord is alike
/// both sides of it.
const FLAT: f64 = 0.005;
/// A section is described by its chord when it is at least this many times longer than thick.
const SLENDER: f64 = 2.0;

/// The stations along the chord an outline is laid out at ([`enveloped`]), closer toward its ends.
const OUTLINE_STATIONS: usize = 120;

/// The outline of the closed `ring` and the `specks` beside it (each closed or not) taken
/// together, as a slender section's is: from its two points farthest apart, the least and the
/// most it reaches across the line between them at each station along it. Counter-clockwise.
/// `None` when they have no length.
fn enveloped(ring: &[Vec2], specks: &[(&[Vec2], bool)]) -> Option<Vec<Vec2>> {
    let all: Vec<Vec2> = ring.iter().chain(specks.iter().flat_map(|(points, _)| points.iter())).copied().collect();
    // The two points farthest apart: among a thinned set, then among those near them.
    let step = (all.len() / 400).max(1);
    let coarse: Vec<Vec2> = all.iter().step_by(step).copied().collect();
    let (mut a, mut b, mut far) = (all[0], all[0], -1.0);
    for (i, p) in coarse.iter().enumerate() {
        for q in &coarse[i + 1..] {
            if p.dist(*q) > far {
                (a, b, far) = (*p, *q, p.dist(*q));
            }
        }
    }
    let (near_a, near_b): (Vec<Vec2>, Vec<Vec2>) =
        (all.iter().copied().filter(|p| p.dist(a) <= 0.05 * far).collect(), all.iter().copied().filter(|p| p.dist(b) <= 0.05 * far).collect());
    for p in &near_a {
        for q in &near_b {
            if p.dist(*q) > far {
                (a, b, far) = (*p, *q, p.dist(*q));
            }
        }
    }
    let chord = far;
    if !(chord > 0.0) {
        return None;
    }
    let along = (b - a) / chord;
    let up = Vec2::new(-along.y, along.x);
    let stations: Vec<f64> = (1..OUTLINE_STATIONS).map(|j| 0.5 * (1.0 - (std::f64::consts::PI * j as f64 / OUTLINE_STATIONS as f64).cos())).collect();
    let (mut most, mut least) = (vec![f64::NEG_INFINITY; stations.len()], vec![f64::INFINITY; stations.len()]);
    let mut span = |p: Vec2, q: Vec2| {
        let ((x0, v0), (x1, v1)) = (((p - a).dot(along) / chord, (p - a).dot(up)), ((q - a).dot(along) / chord, (q - a).dot(up)));
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        for (j, &x) in stations.iter().enumerate().filter(|(_, x)| **x >= lo && **x <= hi) {
            let v = if (x1 - x0).abs() > 1e-15 { v0 + (v1 - v0) * (x - x0) / (x1 - x0) } else { v0 };
            most[j] = most[j].max(v);
            least[j] = least[j].min(v);
        }
    };
    for (points, closed) in std::iter::once((ring, true)).chain(specks.iter().copied()) {
        let n = points.len();
        for i in 0..n.saturating_sub(usize::from(!closed)) {
            span(points[i], points[(i + 1) % n]);
        }
    }
    let at = |x: f64, v: f64| a + along * (x * chord) + up * v;
    let mut out = vec![a];
    out.extend(stations.iter().zip(&least).filter(|(_, v)| v.is_finite()).map(|(x, v)| at(*x, *v)));
    out.push(b);
    out.extend(stations.iter().zip(&most).rev().filter(|(_, v)| v.is_finite()).map(|(x, v)| at(*x, *v)));
    (out.len() >= 8).then_some(out)
}

/// The closed `ring` (counter-clockwise) read as a wing's section, `None` for one not at least
/// [`SLENDER`] times longer than it is thick. `tolerance` (mm) is what a NACA four-digit section
/// must fit it within to be named.
pub fn aerofoil(ring: &[Vec2], tolerance: f64) -> Option<Aerofoil> {
    let n = ring.len();
    if n < 8 {
        return None;
    }
    // The two points farthest apart: among a thinned ring first, then among their neighbours.
    let step = (n / 360).max(1);
    let coarse: Vec<usize> = (0..n).step_by(step).collect();
    let (mut a, mut b, mut far) = (0, 0, -1.0);
    for (i, &p) in coarse.iter().enumerate() {
        for &q in &coarse[i + 1..] {
            let d = ring[p].dist(ring[q]);
            if d > far {
                (a, b, far) = (p, q, d);
            }
        }
    }
    let near = |k: usize| (0..=2 * step).map(move |i| (k + n + i - step) % n);
    for p in near(a) {
        for q in near(b) {
            let d = ring[p].dist(ring[q]);
            if d > far {
                (a, b, far) = (p, q, d);
            }
        }
    }
    let chord = far;
    if !(chord > 0.0) {
        return None;
    }
    // Along the chord from one end (0) to the other (1), and across it: the most and the least
    // across at each station, each of the ring's sides read where it crosses the station.
    let (from, along) = (ring[a], (ring[b] - ring[a]) / chord);
    let place = |p: Vec2| ((p - from).dot(along) / chord, along.cross(p - from));
    let (mut most, mut least) = (vec![f64::NEG_INFINITY; STATIONS + 1], vec![f64::INFINITY; STATIONS + 1]);
    for i in 0..n {
        let ((x0, v0), (x1, v1)) = (place(ring[i]), place(ring[(i + 1) % n]));
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        let first = (lo * STATIONS as f64).ceil().max(0.0) as usize;
        let last = ((hi * STATIONS as f64).floor().max(-1.0) as isize).min(STATIONS as isize);
        for k in first as isize..=last {
            let x = k as f64 / STATIONS as f64;
            let v = if (x1 - x0).abs() > 1e-15 { v0 + (v1 - v0) * (x - x0) / (x1 - x0) } else { v0 };
            most[k as usize] = most[k as usize].max(v);
            least[k as usize] = least[k as usize].min(v);
        }
    }
    let across: Vec<Option<(f64, f64)>> = (0..=STATIONS).map(|k| (most[k] >= least[k]).then_some((most[k], least[k]))).collect();
    let thick = |k: usize| across.get(k).copied().flatten().map_or(0.0, |(hi, lo)| hi - lo);
    let thickest = (0..=STATIONS).max_by(|&i, &j| thick(i).total_cmp(&thick(j)))?;
    let thickness = thick(thickest);
    if !(thickness > 0.0) || chord < SLENDER * thickness {
        return None;
    }
    // The blunter end leads: the thicker a twentieth of the chord in. Alike, the end further along x.
    let (at_a, at_b) = (thick(STATIONS / 20), thick(STATIONS - STATIONS / 20));
    let a_leads = if (at_a - at_b).abs() > 0.1 * at_a.max(at_b) { at_a > at_b } else { ring[a].x >= ring[b].x };
    let (leading, trailing) = if a_leads { (ring[a], ring[b]) } else { (ring[b], ring[a]) };
    let from_leading = |k: usize| if a_leads { k as f64 / STATIONS as f64 } else { 1.0 - k as f64 / STATIONS as f64 };
    let camber = across.iter().flatten().map(|(hi, lo)| ((hi + lo) / 2.0).abs()).fold(0.0, f64::max);
    let direction = leading - trailing;
    // A section alike both sides of its chord has its chord line down its middle: the line through
    // the middles of its stations (from a tenth to nine tenths along) says which way that runs
    // better than its two end points do, one of which is a thin edge a scan wears short or bends.
    let level = if camber <= FLAT * chord {
        let middles: Vec<(f64, f64)> = (STATIONS / 10..=STATIONS - STATIONS / 10)
            .filter_map(|k| across[k].map(|(hi, lo)| (chord * k as f64 / STATIONS as f64, (hi + lo) / 2.0)))
            .collect();
        let count = middles.len().max(1) as f64;
        let (mx, my) = (middles.iter().map(|m| m.0).sum::<f64>() / count, middles.iter().map(|m| m.1).sum::<f64>() / count);
        let (sxx, sxy) = middles.iter().fold((0.0, 0.0), |(sxx, sxy), (x, v)| (sxx + (x - mx) * (x - mx), sxy + (x - mx) * (v - my)));
        if sxx > 0.0 { (sxy / sxx).atan() } else { 0.0 }
    } else {
        0.0
    };
    let mut found = Aerofoil {
        leading,
        trailing,
        chord,
        twist: direction.y.atan2(direction.x) + level,
        thickness,
        thickest_at: from_leading(thickest),
        camber,
        naca: None,
    };
    // A NACA four-digit section of that thickness, its camber line two parabolas meeting at the
    // camber's crest: named when every station is within the tolerance of it.
    let crest = (0..=STATIONS).max_by(|&i, &j| {
        let mid = |k: usize| across[k].map_or(0.0, |(hi, lo)| ((hi + lo) / 2.0).abs());
        mid(i).total_cmp(&mid(j))
    })?;
    let signed = across[crest].map_or(0.0, |(hi, lo)| (hi + lo) / 2.0);
    let (t, m) = (thickness / chord, signed / chord);
    let p = if m.abs() * chord > tolerance { from_leading(crest).clamp(0.1, 0.9) } else { 0.0 };
    let half = |x: f64| 5.0 * t * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3) - 0.1015 * x.powi(4));
    // The standard section is 20 % thick where its formula's `t` is 20 %, at 30 % of the chord.
    let line = |x: f64| {
        if p <= 0.0 {
            0.0
        } else if x < p {
            m / (p * p) * (2.0 * p * x - x * x)
        } else {
            m / ((1.0 - p) * (1.0 - p)) * ((1.0 - 2.0 * p) + 2.0 * p * x - x * x)
        }
    };
    // Measured from the line between the two points farthest apart, which on a cambered section
    // is a hair off the line the formula is written on: that line's own offset and slope against
    // ours (a straight line through what the middles differ by) is taken out first.
    let differ: Vec<(f64, f64, f64)> = (1..STATIONS)
        .filter_map(|k| {
            let (hi, lo) = across[k]?;
            let x = from_leading(k);
            Some((x, hi - (line(x) + half(x)) * chord, lo - (line(x) - half(x)) * chord))
        })
        .collect();
    let count = differ.len().max(1) as f64;
    let (sx, sy) = differ.iter().fold((0.0, 0.0), |(sx, sy), (x, hi, lo)| (sx + x, sy + (hi + lo) / 2.0));
    let (mx, my) = (sx / count, sy / count);
    let (sxx, sxy) = differ.iter().fold((0.0, 0.0), |(sxx, sxy), (x, hi, lo)| (sxx + (x - mx) * (x - mx), sxy + (x - mx) * ((hi + lo) / 2.0 - my)));
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let off = differ.iter().map(|(x, hi, lo)| (hi - my - slope * (x - mx)).abs().max((lo - my - slope * (x - mx)).abs())).fold(0.0, f64::max);
    if off <= tolerance {
        let digit = |v: f64, most: f64| v.round().clamp(0.0, most) as u8;
        found.naca = Some([digit(100.0 * m.abs(), 9.0), digit(10.0 * p, 9.0), digit(100.0 * t, 99.0)]);
    }
    Some(found)
}

/// A part read as sections lofted along `direction`.
#[derive(Clone, Debug, PartialEq)]
pub struct Loft {
    pub direction: Vec3,
    /// Whether the ends are faces Find All found (the loft then runs from one to the other).
    pub capped: bool,
    pub sections: Vec<Section>,
}

/// The direction two planes among `faces` face apart along, each at an end of the scan, and
/// where along it the two planes lie (the lower first); `None` without such a pair.
fn capped_direction(scan: &Scan, faces: &[Face]) -> Option<(Vec3, [f64; 2])> {
    let planes: Vec<(Vec3, Vec3, f64)> = faces
        .iter()
        .filter_map(|f| match f.shape {
            Shape::Plane { point, normal } if f.area > 0.0 => Some((point, normal.normalized()?, f.area)),
            _ => None,
        })
        .collect();
    let mut best: Option<(f64, Vec3, [f64; 2])> = None;
    for (i, a) in planes.iter().enumerate() {
        for b in &planes[i + 1..] {
            if a.1.dot(b.1) > -SAME_DIRECTION {
                continue;
            }
            let d = a.1;
            let (low, high) = scan.points().map(|p| p.dot(d)).fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| (l.min(x), h.max(x)));
            let length = high - low;
            let (la, lb) = (a.0.dot(d), b.0.dot(d));
            let ends = |x: f64| (x - low).abs() <= AT_THE_END * length || (high - x).abs() <= AT_THE_END * length;
            if length > 0.0 && ends(la) && ends(lb) && (la - lb).abs() >= 0.5 * length {
                let weight = a.2.min(b.2);
                if best.is_none_or(|(w, _, _)| weight > w) {
                    best = Some((weight, d, [la.min(lb), la.max(lb)]));
                }
            }
        }
    }
    best.map(|(_, d, levels)| (d, levels))
}

/// The loop's points evenly spaced along it, `count` of them from the point at `start`.
fn resampled(points: &[Vec2], start: usize, count: usize) -> Vec<Vec2> {
    let n = points.len();
    let ring: Vec<Vec2> = (0..=n).map(|k| points[(start + k) % n]).collect();
    let mut along = vec![0.0];
    for w in ring.windows(2) {
        along.push(along.last().copied().unwrap_or(0.0) + w[0].dist(w[1]));
    }
    let total = along.last().copied().unwrap_or(0.0);
    let mut out = Vec::with_capacity(count);
    let mut j = 0;
    for k in 0..count {
        let t = total * k as f64 / count as f64;
        while j + 2 < along.len() && along[j + 1] < t {
            j += 1;
        }
        let span = along[j + 1] - along[j];
        let f = if span > 0.0 { (t - along[j]) / span } else { 0.0 };
        out.push(ring[j] + (ring[j + 1] - ring[j]) * f);
    }
    out
}

fn signed_area(points: &[Vec2]) -> f64 {
    let n = points.len();
    0.5 * (0..n).map(|k| points[k].x * points[(k + 1) % n].y - points[(k + 1) % n].x * points[k].y).sum::<f64>()
}

fn middle(points: &[Vec2]) -> Vec2 {
    points.iter().fold(Vec2::ZERO, |s, p| s + *p) / points.len().max(1) as f64
}

/// The scan read as `count` sections lofted (see the module); why not, when it cannot be.
pub fn read(scan: &Scan, faces: &[Face], count: usize) -> Result<Loft, String> {
    read_within(scan, faces, count, 0.0)
}

/// [`read`], each section also read as a wing's ([`aerofoil`]) and named as a NACA section when
/// it is one within `tolerance` (mm).
pub fn read_within(scan: &Scan, faces: &[Face], count: usize, tolerance: f64) -> Result<Loft, String> {
    let count = count.clamp(LEAST_SECTIONS, MOST_SECTIONS);
    let points: Vec<Vec3> = crate::thinned(&scan.points().collect::<Vec<_>>(), 200_000);
    let (centre, axes, _) = crate::register::principal(&points).ok_or("the scan has no extent")?;
    let capped = capped_direction(scan, faces);
    let direction = capped.map_or(axes[0], |(d, _)| d);
    let (low, high) = points.iter().map(|p| p.dot(direction)).fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), x| (l.min(x), h.max(x)));
    let length = high - low;
    if !(length > 0.0) {
        return Err("the scan has no length".into());
    }
    let (x, y) = in_plane(direction);
    let weld = crate::welded(scan.positions);
    let mut sections: Vec<Section> = Vec::with_capacity(count);
    let mut previous: Option<Vec<Vec2>> = None;
    let mut twist_before: Option<f64> = None;
    let mut rings: Vec<Vec<Vec2>> = Vec::with_capacity(count);
    let mut sliced: Vec<f64> = Vec::with_capacity(count);
    for k in 0..count {
        let share = k as f64 / (count - 1) as f64;
        let at = low + length * (END_MARGIN + (1.0 - 2.0 * END_MARGIN) * share);
        let origin = centre + direction * (at - centre.dot(direction));
        let plane = Plane { origin, x, y };
        let runs = crate::section::section_welded(scan, &weld, &plane);
        let length_of = |p: &[Vec2], closed: bool| {
            let n = p.len();
            (0..n.saturating_sub(usize::from(!closed))).map(|i| p[i].dist(p[(i + 1) % n])).sum::<f64>()
        };
        let longest = runs.iter().map(|r| length_of(&r.points, r.closed)).fold(0.0, f64::max);
        let kept: Vec<_> = runs.iter().filter(|r| length_of(&r.points, r.closed) > SPECK * longest).collect();
        let [only] = kept.as_slice() else {
            return Err(format!(
                "the slice {:.0}% along the part is {} loops, not one: the part is not one section swept smoothly along it",
                100.0 * share,
                kept.len()
            ));
        };
        if !only.closed || only.points.len() < 3 {
            return Err(format!("the slice {:.0}% along the part does not close round: fill the scan's holes first", 100.0 * share));
        }
        let mut ring = only.points.clone();
        if signed_area(&ring) < 0.0 {
            ring.reverse();
        }
        // A thin trailing edge a scan's noise has tangled (its two sides cross, and the slice
        // closes short of it, the tip a loop of its own): the specks that lie against the loop
        // are part of the section, taken in by its outline along the chord.
        let against = SPECK * longest;
        let specks: Vec<(&[Vec2], bool)> = runs
            .iter()
            .filter(|r| !std::ptr::eq(*r, *only) && r.points.len() >= 2)
            .filter(|r| r.points.iter().any(|q| ring.iter().any(|p| p.dist(*q) <= 0.2 * against)))
            .map(|r| (r.points.as_slice(), r.closed))
            .collect();
        if !specks.is_empty() && aerofoil(&ring, tolerance).is_some() {
            ring = enveloped(&ring, &specks).unwrap_or(ring);
        }
        let m = middle(&ring);
        // The first starts at its furthest point along x; each after it where it lies nearest the
        // one before, point for point, seen from each one's middle (where one was started from
        // the point furthest along the last start's direction, a section that changed shape slid
        // round, and the loft between them twisted through itself).
        let start = (0..ring.len()).max_by(|&a, &b| (ring[a] - m).x.total_cmp(&(ring[b] - m).x)).unwrap_or(0);
        let laid = match &previous {
            Some(before) => matched(&resampled(&ring, start, POINTS), before),
            None => resampled(&ring, start, POINTS),
        };
        previous = Some(laid.iter().map(|p| *p - m).collect());
        // The ends' sections lie at the end faces, when there are such: where the faces were
        // fitted, not at the scan's outermost point (its noise stands a little proud of them).
        let plane = if let (Some((_, levels)), true) = (capped, k == 0 || k + 1 == count) {
            let end = if k == 0 { levels[0] } else { levels[1] };
            Plane { origin: centre + direction * (end - centre.dot(direction)), x, y }
        } else {
            plane
        };
        // Its chord line the same way round as the section before's.
        let shape = aerofoil(&ring, tolerance).map(|found| {
            let Some(before) = twist_before else { return found };
            let turn = (found.twist - before + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
            if turn.abs() > std::f64::consts::FRAC_PI_2 {
                Aerofoil {
                    leading: found.trailing,
                    trailing: found.leading,
                    twist: before + turn - turn.signum() * std::f64::consts::PI,
                    thickest_at: 1.0 - found.thickest_at,
                    ..found
                }
            } else {
                Aerofoil { twist: before + turn, ..found }
            }
        });
        twist_before = shape.map(|f| f.twist).or(twist_before);
        rings.push(ring);
        sliced.push(at);
        sections.push(Section { plane, points: laid, shape });
    }
    // Wing sections, all of them: laid out along their chords instead, from the trailing edge
    // round, closer toward both edges, so the loft keeps its edges where even steps round the
    // loop would round them off (a blade's trailing edge came out 1.3 mm short).
    if sections.iter().all(|s| s.shape.is_some()) {
        let laid: Vec<Option<Vec<Vec2>>> =
            sections.iter().zip(&rings).map(|(section, ring)| section.shape.and_then(|shape| by_chord(ring, &shape))).collect();
        if laid.iter().all(Option::is_some) {
            for (section, points) in sections.iter_mut().zip(laid.into_iter().flatten()) {
                section.points = points;
            }
            // An end's section was sliced a little in from the end face it is drawn on, and a blade
            // tapers and twists in that little: each of its points is carried on to the face along
            // the line from the next section's same point (they are laid out alike).
            if let (Some((_, levels)), true) = (capped, count >= 2) {
                for (end, next, level) in [(0, 1, levels[0]), (count - 1, count - 2, levels[1])] {
                    let step = sliced[end] - sliced[next];
                    if step.abs() > 1e-9 {
                        let on = (level - sliced[end]) / step;
                        let from = sections[next].points.clone();
                        for (p, q) in sections[end].points.iter_mut().zip(from) {
                            *p = *p + (*p - q) * on;
                        }
                    }
                }
            }
        }
    }
    // A loft only where the part is one: half way between each two sections, the scan's slice
    // lies near what the loft makes there (about their average, point for point). A part whose
    // section turns or changes shape between them is no loft, however its slices close.
    for k in 0..count - 1 {
        let at = 0.5 * (sliced[k] + sliced[k + 1]);
        let plane = Plane { origin: centre + direction * (at - centre.dot(direction)), x, y };
        let runs = crate::section::section_welded(scan, &weld, &plane);
        let Some(ring) = runs.iter().filter(|r| r.closed && r.points.len() >= 3).max_by(|a, b| {
            let l = |p: &[Vec2]| (0..p.len()).map(|i| p[i].dist(p[(i + 1) % p.len()])).sum::<f64>();
            l(&a.points).total_cmp(&l(&b.points))
        }) else {
            return Err(format!("the slice {:.0}% along the part does not close round", 100.0 * (at - low) / length));
        };
        let mut ring = ring.points.clone();
        if signed_area(&ring) < 0.0 {
            ring.reverse();
        }
        // Finely, and the slice's distance read to its own outline, not point to point: how the
        // points fall along a loop is no change of its shape.
        let (a, b) = (resampled(&rings[k], 0, CHECK_POINTS), resampled(&rings[k + 1], 0, CHECK_POINTS));
        let b = matched(&b, &a.iter().map(|p| *p - middle(&a)).collect::<Vec<_>>());
        let average: Vec<Vec2> = a.iter().zip(&b).map(|(p, q)| (*p + *q) * 0.5).collect();
        let to_ring = |p: Vec2| {
            (0..ring.len())
                .map(|i| {
                    let (u, v) = (ring[i], ring[(i + 1) % ring.len()]);
                    let d = v - u;
                    let t = if d.len2() > 0.0 { ((p - u).dot(d) / d.len2()).clamp(0.0, 1.0) } else { 0.0 };
                    p.dist(u + d * t)
                })
                .fold(f64::INFINITY, f64::min)
        };
        let off = average.iter().map(|&p| to_ring(p)).fold(0.0, f64::max);
        let size = average.iter().flat_map(|p| average.iter().map(move |q| p.dist(*q))).fold(0.0, f64::max);
        if off > (MIDWAY_OFF * size).max(4.0 * tolerance) {
            return Err(format!(
                "the part is no loft: half way between two of its sections the scan lies {off:.1} mm off what a loft through them makes ({:.0}% of the section's size): it turns or changes shape between them",
                100.0 * off / size.max(f64::MIN_POSITIVE)
            ));
        }
    }
    Ok(Loft { direction, capped: capped.is_some(), sections })
}

/// Points round each loop the check half way between sections reads with.
const CHECK_POINTS: usize = 360;

/// Half way between two sections, the scan's slice may lie at most this share of the section's
/// size (or four tolerances) from what the loft through them makes there: a section turning
/// evenly 15° from one to the next is off by half a percent of its size there; one turning 60°
/// in a tenth of the length, by nine percent.
const MIDWAY_OFF: f64 = 0.03;

/// `laid` (evenly spaced round a loop) turned round to start where it lies nearest `before` (a
/// loop of as many points, each from its middle), point for point, each seen from its own middle.
fn matched(laid: &[Vec2], before: &[Vec2]) -> Vec<Vec2> {
    let n = laid.len();
    if n == 0 || before.len() != n {
        return laid.to_vec();
    }
    let m = middle(laid);
    let cost = |shift: usize| (0..n).map(|i| ((laid[(i + shift) % n] - m) - before[i]).len2()).sum::<f64>();
    let best = (0..n).min_by(|&a, &b| cost(a).total_cmp(&cost(b))).unwrap_or(0);
    (0..n).map(|i| laid[(i + best) % n]).collect()
}

/// The counter-clockwise `ring` of a wing's section laid out along its chord with [`POINTS`]
/// points: the trailing edge, the side below the chord line (as it runs to the leading edge) at
/// stations closer toward both edges, the leading edge, and the other side back. `None` when a
/// station finds no side.
fn by_chord(ring: &[Vec2], shape: &Aerofoil) -> Option<Vec<Vec2>> {
    let side = POINTS / 2 - 1;
    let along = (shape.leading - shape.trailing).normalized()?;
    let up = Vec2::new(-along.y, along.x);
    let stations: Vec<f64> = (1..=side).map(|j| 0.5 * (1.0 - (std::f64::consts::PI * j as f64 / (side + 1) as f64).cos())).collect();
    let (mut most, mut least) = (vec![f64::NEG_INFINITY; side], vec![f64::INFINITY; side]);
    let n = ring.len();
    for i in 0..n {
        let place = |p: Vec2| ((p - shape.trailing).dot(along) / shape.chord, (p - shape.trailing).dot(up));
        let ((x0, v0), (x1, v1)) = (place(ring[i]), place(ring[(i + 1) % n]));
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        for (j, &x) in stations.iter().enumerate().filter(|(_, x)| **x >= lo && **x <= hi) {
            let v = if (x1 - x0).abs() > 1e-15 { v0 + (v1 - v0) * (x - x0) / (x1 - x0) } else { v0 };
            most[j] = most[j].max(v);
            least[j] = least[j].min(v);
        }
    }
    if most.iter().chain(&least).any(|v| !v.is_finite()) {
        return None;
    }
    let at = |x: f64, v: f64| shape.trailing + along * (x * shape.chord) + up * v;
    let mut out = vec![shape.trailing];
    out.extend(stations.iter().zip(&least).map(|(x, v)| at(*x, *v)));
    out.push(shape.leading);
    out.extend(stations.iter().zip(&most).rev().map(|(x, v)| at(*x, *v)));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from `lo` to `hi`, as twelve triangles wound outward.
    fn cuboid(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let p = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let positions = (0..8).map(p).collect();
        let triangles =
            vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        (positions, triangles)
    }

    /// A rounded bar 40 × 20 whose section turns 60° in 4 mm half way up its 60: every slice is
    /// one closed loop, but half way between two sections the scan lies far from what a loft
    /// through them makes, so it is no loft (T-LESS 17 was proposed as one, and its loft passed
    /// through itself). The same bar turning evenly, 60° over its whole length, is one.
    #[test]
    fn a_bar_whose_section_turns_suddenly_is_no_loft_and_one_turning_evenly_is() {
        let outline = crate::test_meshes::rounded_rectangle(40.0, 20.0, 2.0, 1.0);
        let bar = |angle_at: &dyn Fn(f64) -> f64| {
            let n = outline.len() as u32;
            let rows = 60u32;
            let mut positions = Vec::new();
            for j in 0..=rows {
                let z = f64::from(j);
                let (s, c) = angle_at(z).sin_cos();
                positions.extend(outline.iter().map(|&[x, y]| {
                    let (x, y) = (x - 20.0, y - 10.0);
                    Vec3::new(x * c - y * s, x * s + y * c, z)
                }));
            }
            let mut triangles = Vec::new();
            for j in 0..rows {
                for k in 0..n {
                    let (a, b) = (j * n + k, j * n + (k + 1) % n);
                    triangles.push([a, b, b + n]);
                    triangles.push([a, b + n, a + n]);
                }
            }
            for (z, top) in [(0.0, false), (f64::from(rows), true)] {
                let centre = positions.len() as u32;
                positions.push(Vec3::new(0.0, 0.0, z));
                let ring = if top { rows * n } else { 0 };
                for k in 0..n {
                    let (a, b) = (ring + k, ring + (k + 1) % n);
                    triangles.push(if top { [centre, a, b] } else { [centre, b, a] });
                }
            }
            (positions, triangles)
        };
        let sudden = |z: f64| (60f64.to_radians() * ((z - 28.0) / 4.0)).clamp(0.0, 60f64.to_radians());
        let (positions, triangles) = bar(&sudden);
        let refused = read(&Scan::new(&positions, &triangles), &[], 5).expect_err("no loft");
        assert!(refused.contains("no loft"), "{refused}");
        let even = |z: f64| 60f64.to_radians() * z / 60.0;
        let (positions, triangles) = bar(&even);
        let loft = read(&Scan::new(&positions, &triangles), &[], 5).expect("a loft");
        assert_eq!(loft.sections.len(), 5);
    }

    #[test]
    fn a_bar_is_lofted_along_its_length_between_its_end_faces() {
        let (positions, triangles) = cuboid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 6.0, 80.0));
        let scan = Scan::new(&positions, &triangles);
        let ends = [
            Face { shape: Shape::Plane { point: Vec3::new(5.0, 3.0, 0.0), normal: -Vec3::Z }, area: 60.0 },
            Face { shape: Shape::Plane { point: Vec3::new(5.0, 3.0, 80.0), normal: Vec3::Z }, area: 60.0 },
        ];
        let loft = read(&scan, &ends, 4).unwrap();
        assert!(loft.capped && loft.direction.z.abs() > 0.999, "{loft:?}");
        assert_eq!(loft.sections.len(), 4);
        let levels: Vec<f64> = loft.sections.iter().map(|s| s.plane.origin.z).collect();
        assert!((levels[0] - 0.0).abs() < 1e-9 || (levels[0] - 80.0).abs() < 1e-9, "{levels:?}");
        for s in &loft.sections {
            assert_eq!(s.points.len(), POINTS);
            assert!((signed_area(&s.points) - 60.0).abs() < 1.0, "{}", signed_area(&s.points));
        }
        // Without end faces, along its longest direction, 1% in from each end.
        let free = read(&scan, &[], 3).unwrap();
        assert!(!free.capped && free.direction.z.abs() > 0.999, "{free:?}");
    }

    /// A NACA 2412 section of chord 80, turned 12° and moved off: read with that chord and twist,
    /// 12 % thick at 30 % of its chord, cambered 2 %, and named. An ellipse 60 × 14.4 is read by
    /// its chord and thickness and is no NACA section; a square is no wing's section at all.
    #[test]
    fn a_wings_section_is_read_by_its_chord_twist_and_thickness() {
        let (chord, turn) = (80.0, 12f64.to_radians());
        let half = |x: f64| 5.0 * 0.12 * (0.2969 * x.sqrt() - 0.1260 * x - 0.3516 * x * x + 0.2843 * x.powi(3) - 0.1015 * x.powi(4));
        let line = |x: f64| if x < 0.4 { 0.02 / 0.16 * (0.8 * x - x * x) } else { 0.02 / 0.36 * (0.2 + 0.8 * x - x * x) };
        // From the trailing edge forward along the top, then back along the bottom; x from the
        // leading edge, the section's leading edge toward +x once turned half round.
        let stations: Vec<f64> = (0..=200).map(|i| 0.5 * (1.0 - (std::f64::consts::PI * f64::from(i) / 200.0).cos())).collect();
        let mut ring: Vec<Vec2> = stations.iter().rev().map(|&x| Vec2::new(-x, line(x) + half(x))).collect();
        ring.extend(stations.iter().skip(1).take(199).map(|&x| Vec2::new(-x, line(x) - half(x))));
        let placed: Vec<Vec2> = ring
            .iter()
            .rev()
            .map(|q| {
                let q = *q * chord;
                Vec2::new(q.x * turn.cos() - q.y * turn.sin() + 7.0, q.x * turn.sin() + q.y * turn.cos() - 3.0)
            })
            .collect();
        assert!(signed_area(&placed) > 0.0, "counter-clockwise");
        let found = aerofoil(&placed, 0.05).expect("a wing's section");
        assert!((found.chord - chord).abs() < 0.002 * chord, "{found:?}");
        assert!((found.twist - turn).abs() < 0.1f64.to_radians(), "{} deg", found.twist.to_degrees());
        assert!((found.thickness - 0.12 * chord).abs() < 0.05 && (found.thickest_at - 0.3).abs() < 0.03, "{found:?}");
        assert!((found.camber - 0.02 * chord).abs() < 0.05 && found.leading.dist(Vec2::new(7.0, -3.0)) < 0.05, "{found:?}");
        assert_eq!(found.naca, Some([2, 4, 12]), "{found:?}");
        let ellipse: Vec<Vec2> = (0..360).map(|k| f64::from(k).to_radians()).map(|a| Vec2::new(30.0 * a.cos(), 7.2 * a.sin())).collect();
        let found = aerofoil(&ellipse, 0.05).unwrap();
        assert!((found.chord - 60.0).abs() < 0.01 && (found.thickness - 14.4).abs() < 0.01 && found.camber < 0.01, "{found:?}");
        assert!(found.twist.abs() < 1e-3 && found.naca.is_none(), "{found:?}");
        let square = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let fine: Vec<Vec2> =
            (0..40).map(|k| square[k / 10] + (square[(k / 10 + 1) % 4] - square[k / 10]) * (f64::from((k % 10) as u8) / 10.0)).collect();
        assert_eq!(aerofoil(&fine, 0.05), None);
    }

    #[test]
    fn two_bars_side_by_side_are_no_loft() {
        let (mut positions, mut triangles) = cuboid(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 4.0, 80.0));
        let (more, tris) = cuboid(Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 4.0, 80.0));
        triangles.extend(tris.iter().map(|t| t.map(|i| i + 8)));
        positions.extend(more);
        let scan = Scan::new(&positions, &triangles);
        assert!(read(&scan, &[], 3).is_err());
    }
}
