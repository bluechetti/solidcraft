//! A run of a scan's section as lines and arcs (SurfaceCraft's chain fitter, a sketch read off a
//! section): the run is cut where its curvature changes, each piece fitted by least squares
//! as a line or a circle, neighbours one line or circle fits joined, and each end put where its fit
//! meets the next one's. A piece averages its points, so the scan's noise does not stay in what it
//! makes, and a corner is where its two sides meet.
//!
//! A closed run starts where its curvature changes most (a corner, or where a line meets an arc),
//! so its seam falls on a joint; a closed run one circle fits is that circle.

use std::f64::consts::TAU;

use solidcore_record::Vec2;
use solidcraft_geom::prelude::*;

/// The most of a turn one arc may sweep: the rest of a whole loop is a second arc.
const MOST_SWEEP: f64 = 0.86 * TAU;

/// A line or an arc of the chain, end to end with the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    Line {
        from: Vec2,
        to: Vec2,
    },
    /// Round `center` from `from` to `to`, counter-clockwise when `ccw`.
    Arc {
        center: Vec2,
        radius: f64,
        from: Vec2,
        to: Vec2,
        ccw: bool,
    },
}

impl Piece {
    pub fn from(&self) -> Vec2 {
        match *self {
            Piece::Line { from, .. } | Piece::Arc { from, .. } => from,
        }
    }

    pub fn to(&self) -> Vec2 {
        match *self {
            Piece::Line { to, .. } | Piece::Arc { to, .. } => to,
        }
    }

    /// The way it runs at its start (`at_end` false) or its end, unit.
    pub fn direction(&self, at_end: bool) -> Option<Vec2> {
        match *self {
            Piece::Line { from, to } => (to - from).normalized(),
            Piece::Arc { center, from, to, ccw, .. } => {
                let radial = if at_end { to - center } else { from - center };
                let along = radial.perp();
                (if ccw { along } else { -along }).normalized()
            }
        }
    }
}

/// What a whole closed run is: a circle, or a chain of pieces.
#[derive(Clone, Debug, PartialEq)]
pub enum Outline {
    Circle { center: Vec2, radius: f64 },
    Chain(Vec<Piece>),
}

/// A line or a circle fitted to a run of points.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fit {
    Line { point: Vec2, direction: Vec2 },
    Circle { center: Vec2, radius: f64 },
}

impl Fit {
    fn distance(self, p: Vec2) -> f64 {
        match self {
            Fit::Line { point, direction } => (p - point).cross(direction).abs(),
            Fit::Circle { center, radius } => (p.dist(center) - radius).abs(),
        }
    }

    fn nearest(self, p: Vec2) -> Vec2 {
        match self {
            Fit::Line { point, direction } => point + direction * (p - point).dot(direction),
            Fit::Circle { center, radius } => center + (p - center).normalized().unwrap_or(Vec2::new(1.0, 0.0)) * radius,
        }
    }
}

/// A run of the points `from..=to` and the fit that holds it.
#[derive(Clone, Copy, Debug)]
struct Span {
    from: usize,
    to: usize,
    fit: Fit,
    /// The farthest of its points from the fit.
    off: f64,
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// The total least-squares line through `points`, and the farthest of them from it, if they run
/// along it one after another (going back by no more than the tolerance allows).
fn line_fit(points: &[Vec2], tolerance: f64) -> Option<(Fit, f64)> {
    let n = points.len() as f64;
    let mean = points.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) / n;
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for p in points {
        let d = *p - mean;
        xx += d.x * d.x;
        xy += d.x * d.y;
        yy += d.y * d.y;
    }
    // The way the points spread most: the larger eigenvector of their scatter.
    let mut direction = Vec2::from_angle(0.5 * (2.0 * xy).atan2(xx - yy));
    if direction.dot(points[points.len() - 1] - points[0]) < 0.0 {
        direction = -direction;
    }
    let line = Fit::Line { point: mean, direction };
    let (mut last, mut off) = (f64::NEG_INFINITY, 0.0f64);
    for p in points {
        let along = (*p - mean).dot(direction);
        if !(along >= last - 3.0 * tolerance) {
            return None;
        }
        last = last.max(along);
        off = off.max(line.distance(*p));
    }
    Some((line, off))
}

/// The circle that fits `points` best: an algebraic fit (Kåsa), then Gauss–Newton on the
/// distances themselves, which the algebraic fit leans small on a short noisy arc.
pub(crate) fn best_circle(points: &[Vec2], tolerance: f64) -> Option<(Vec2, f64)> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let mean = points.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) / n;
    // x² + y² + D x + E y + F = 0 in least squares, about the mean.
    let (mut normal, mut right) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
    for p in points {
        let d = *p - mean;
        let row = [d.x, d.y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                normal[i][j] += row[i] * row[j];
            }
            right[i] -= row[i] * d.len2();
        }
    }
    if !(det3(&normal).abs() > 1e-18 * n.powi(3)) {
        return None;
    }
    let [d, e, f] = crate::symmetry::solve3(&normal, right)?;
    let mut center = Vec2::new(-d / 2.0, -e / 2.0);
    let mut radius = (center.len2() - f).max(0.0).sqrt();
    center += mean;
    for _ in 0..8 {
        let (mut jtj, mut jtr) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
        for p in points {
            let d = *p - center;
            let length = d.len().max(1e-300);
            let row = [-d.x / length, -d.y / length, -1.0];
            let residual = length - radius;
            for i in 0..3 {
                for j in 0..3 {
                    jtj[i][j] += row[i] * row[j];
                }
                jtr[i] += row[i] * residual;
            }
        }
        let Some(step) = crate::symmetry::solve3(&jtj, jtr) else { break };
        center -= Vec2::new(step[0], step[1]);
        radius -= step[2];
        if (step[0] * step[0] + step[1] * step[1] + step[2] * step[2]).sqrt() < 1e-12 * radius.abs().max(1.0) {
            break;
        }
    }
    let extent = points.iter().map(|p| p.dist(points[0])).fold(points[0].dist(points[points.len() - 1]), f64::max);
    (radius.is_finite() && radius > 0.0 && radius < 1000.0 * extent.max(tolerance)).then_some((center, radius))
}

/// Which way the points go round `center`: +1 counter-clockwise.
fn turn(points: &[Vec2], center: Vec2) -> f64 {
    points.windows(2).map(|w| (w[0] - center).cross(w[1] - center)).sum::<f64>().signum()
}

/// [`best_circle`], if the points run round it in order by less than [`MOST_SWEEP`].
fn circle_fit(points: &[Vec2], tolerance: f64) -> Option<(Fit, f64)> {
    let (center, radius) = best_circle(points, tolerance)?;
    let way = turn(points, center);
    let start = (points[0] - center).angle();
    let slack = 3.0 * tolerance / radius;
    let (mut last, mut swept) = (0.0f64, 0.0f64);
    for p in points {
        let along = (way * ((*p - center).angle() - start)).rem_euclid(TAU);
        // Just behind the start reads as nearly a whole turn.
        let along = if along > TAU - slack { along - TAU } else { along };
        if !(along >= last - slack) {
            return None;
        }
        last = last.max(along);
        swept = swept.max(along);
    }
    if !(swept < MOST_SWEEP) {
        return None;
    }
    let circle = Fit::Circle { center, radius };
    Some((circle, points.iter().map(|p| circle.distance(*p)).fold(0.0, f64::max)))
}

/// The run `from..=to` as a line, or else a circle, if one fits it within `tolerance`.
fn fitted(points: &[Vec2], from: usize, to: usize, tolerance: f64) -> Option<Span> {
    let run = &points[from..=to];
    if run.len() == 2 {
        let direction = (run[1] - run[0]).normalized()?;
        return Some(Span { from, to, fit: Fit::Line { point: run[0], direction }, off: 0.0 });
    }
    let line = line_fit(run, tolerance).filter(|(_, off)| *off <= tolerance);
    let (fit, off) = line.or_else(|| circle_fit(run, tolerance).filter(|(_, off)| *off <= tolerance))?;
    Some(Span { from, to, fit, off })
}

/// The longest run from `from` one fit holds: doubling, then halving back.
fn longest(points: &[Vec2], from: usize, tolerance: f64) -> Option<Span> {
    let last = points.len() - 1;
    let mut good = fitted(points, from, from + 1, tolerance)?;
    let (mut step, mut bad) = (2, None);
    while good.to < last {
        let to = (from + step).min(last);
        match fitted(points, from, to, tolerance) {
            Some(span) => good = span,
            None => {
                bad = Some(to);
                break;
            }
        }
        step *= 2;
    }
    if let Some(mut bad) = bad {
        while bad - good.to > 1 {
            let middle = good.to + (bad - good.to) / 2;
            match fitted(points, from, middle, tolerance) {
                Some(span) => good = span,
                None => bad = middle,
            }
        }
    }
    Some(good)
}

/// Where a closed run of `points` starts: where its curvature changes most.
fn seam(points: &[Vec2]) -> usize {
    let n = points.len();
    let reach = 3.max(n / 60).min(n / 4).max(1);
    let k: Vec<f64> = (0..n)
        .map(|i| {
            let window: Vec<Vec2> = (0..=2 * reach).map(|j| points[(i + n + j - reach) % n]).collect();
            match best_circle(&window, f64::INFINITY) {
                Some((center, radius)) => turn(&window, center) / radius,
                None => 0.0,
            }
        })
        .collect();
    let change = |i: usize| (k[(i + reach) % n] - k[(i + n - reach) % n]).abs() + k[i].abs() * 1e-3;
    (0..n).max_by(|&a, &b| change(a).total_cmp(&change(b))).unwrap_or(0)
}

/// The sine of the angle under which a line or circle meeting a circle is taken to touch it, not
/// cross it: there the crossing runs off along the two, and the place they touch is the joint.
const TOUCHING: f64 = 0.25;

/// Where two fits beside each other meet, near `at` where the points pass from one to the other:
/// where they touch or cross, else where they come nearest.
fn joint(a: Fit, b: Fit, at: Vec2, near: f64) -> Vec2 {
    let crossings: Vec<Vec2> = match (a, b) {
        (Fit::Line { point: p, direction: u }, Fit::Line { point: q, direction: v }) => {
            let across = u.cross(v);
            if across.abs() > 0.5f64.to_radians().sin() { vec![p + u * ((q - p).cross(v) / across)] } else { Vec::new() }
        }
        (Fit::Line { point, direction }, Fit::Circle { center, radius }) | (Fit::Circle { center, radius }, Fit::Line { point, direction }) => {
            let foot = point + direction * (center - point).dot(direction);
            let half = radius * radius - foot.dist(center).powi(2);
            if half >= 0.0 && half.sqrt() > TOUCHING * radius {
                vec![foot - direction * half.sqrt(), foot + direction * half.sqrt()]
            } else {
                vec![foot]
            }
        }
        (Fit::Circle { center: c, radius: r }, Fit::Circle { center: d, radius: s }) => {
            let apart = c.dist(d);
            if apart > 1e-12 {
                let along = (apart * apart + r * r - s * s) / (2.0 * apart);
                let axis = (d - c) / apart;
                let base = c + axis * along;
                let half = r * r - along * along;
                if half >= 0.0 && half.sqrt() > TOUCHING * r.min(s) {
                    vec![base - axis.perp() * half.sqrt(), base + axis.perp() * half.sqrt()]
                } else {
                    // Touching: on the line through the centres, on the first circle, the joint's side.
                    let side = if (at - c).dot(axis) >= 0.0 { axis } else { -axis };
                    vec![c + side * r]
                }
            } else {
                Vec::new()
            }
        }
    };
    crossings
        .into_iter()
        .filter(|p| p.dist(at) <= near)
        .min_by(|p, q| p.dist(at).total_cmp(&q.dist(at)))
        .unwrap_or_else(|| (a.nearest(at) + b.nearest(at)) / 2.0)
}

/// The span made a piece from `from` to `to`: a line, or the arc round its circle's centre moved
/// square onto the chord between them, the way its points go round.
fn piece(span: &Span, points: &[Vec2], from: Vec2, to: Vec2) -> Piece {
    match span.fit {
        Fit::Line { .. } => Piece::Line { from, to },
        Fit::Circle { center, .. } => {
            let middle = (from + to) / 2.0;
            let square = (to - from).perp().normalized().unwrap_or(Vec2::new(0.0, 1.0));
            let center = middle + square * (center - middle).dot(square);
            let ccw = turn(&points[span.from..=span.to], center) >= 0.0;
            Piece::Arc { center, radius: center.dist(from), from, to, ccw }
        }
    }
}

/// `raw`, a run of points in order, as lines and arcs each within `tolerance` of its points, end
/// to end (a closed run ends where it starts; one a circle fits is that circle). `None` for fewer
/// than three points.
pub fn chain(raw: &[Vec2], closed: bool, tolerance: f64) -> Option<Outline> {
    let mut points: Vec<Vec2> = Vec::new();
    for &p in raw {
        if p.x.is_finite() && p.y.is_finite() && points.last().is_none_or(|l: &Vec2| l.dist(p) > 1e-6) {
            points.push(p);
        }
    }
    if closed && points.len() > 1 && points[0].dist(points[points.len() - 1]) <= 1e-6 {
        points.pop();
    }
    if points.len() < 3 || !(tolerance > 0.0) {
        return None;
    }
    if closed {
        if let Some((center, radius)) = best_circle(&points, tolerance)
            && points.iter().all(|p| (p.dist(center) - radius).abs() <= tolerance)
        {
            return Some(Outline::Circle { center, radius });
        }
        let start = seam(&points);
        points.rotate_left(start);
        points.push(points[0]);
    }
    let last = points.len() - 1;
    // Each span the longest one fit holds, from where the one before ended.
    let mut spans = vec![longest(&points, 0, tolerance)?];
    while spans[spans.len() - 1].to < last {
        let next = longest(&points, spans[spans.len() - 1].to, tolerance)?;
        spans.push(next);
    }
    // Neighbours one fit holds are one; each joint moved to where the two sides fit best.
    for _ in 0..3 {
        let mut k = 0;
        while k + 1 < spans.len() {
            if let Some(joined) = fitted(&points, spans[k].from, spans[k + 1].to, tolerance) {
                spans[k] = joined;
                spans.remove(k + 1);
            } else {
                k += 1;
            }
        }
        let mut moved = false;
        for k in 0..spans.len().saturating_sub(1) {
            let (a, b) = (spans[k], spans[k + 1]);
            let reach = 3.max((a.to - a.from).min(b.to - b.from) / 3);
            let (lowest, highest) = ((a.from + 1).max(a.to.saturating_sub(reach)), (b.to - 1).min(a.to + reach));
            let mut best = (a.off.max(b.off), a, b);
            for at in lowest..=highest {
                if at == a.to {
                    continue;
                }
                if let (Some(first), Some(second)) = (fitted(&points, a.from, at, tolerance), fitted(&points, at, b.to, tolerance))
                    && first.off.max(second.off) < best.0 - 1e-12
                {
                    best = (first.off.max(second.off), first, second);
                }
            }
            if best.1.to != a.to {
                (spans[k], spans[k + 1]) = (best.1, best.2);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    // A closed run's last and first spans may be one across the seam.
    if closed && spans.len() > 2 {
        let (first, final_span) = (spans[0], spans[spans.len() - 1]);
        let mut across: Vec<Vec2> = points[final_span.from..].to_vec();
        across.extend_from_slice(&points[1..=first.to]);
        if let Some(joined) = fitted(&across, 0, across.len() - 1, tolerance) {
            let count = points.len() - 1;
            let mut rotated: Vec<Vec2> = points[final_span.from..count].to_vec();
            rotated.extend_from_slice(&points[..final_span.from]);
            rotated.push(rotated[0]);
            let shift = count - final_span.from;
            let mut again = vec![Span { from: 0, to: first.to + shift, ..joined }];
            again.extend(spans[1..spans.len() - 1].iter().map(|r| Span { from: r.from + shift, to: r.to + shift, ..*r }));
            points = rotated;
            spans = again;
        }
    }
    if closed && spans.len() == 1 {
        return None;
    }
    // Each joint where the fits either side meet; an open chain's ends where its first and last
    // points fall on their fits.
    let near = |r: &Span| points[r.from].dist(points[r.to]).max(4.0 * tolerance);
    let joints: Vec<Vec2> = (0..spans.len() - 1)
        .map(|k| joint(spans[k].fit, spans[k + 1].fit, points[spans[k].to], near(&spans[k]).min(near(&spans[k + 1])) / 2.0))
        .collect();
    let (start, end) = if closed {
        let seam = joint(spans[spans.len() - 1].fit, spans[0].fit, points[0], near(&spans[0]).min(near(&spans[spans.len() - 1])) / 2.0);
        (seam, seam)
    } else {
        (spans[0].fit.nearest(points[0]), spans[spans.len() - 1].fit.nearest(points[points.len() - 1]))
    };
    let ends: Vec<Vec2> = std::iter::once(start).chain(joints).chain(std::iter::once(end)).collect();
    Some(Outline::Chain(spans.iter().enumerate().map(|(k, span)| piece(span, &points, ends[k], ends[k + 1])).collect()))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, PI};

    use super::*;

    /// A rounded rectangle of half sizes `w` and `h`, corners of `r`, counter-clockwise, `per_mm`
    /// points along each millimetre, each moved square to the outline by up to `noise`.
    fn rounded(w: f64, h: f64, r: f64, per_mm: f64, noise: f64) -> Vec<Vec2> {
        let (x, y) = (w - r, h - r);
        let mut out = Vec::new();
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut wobble = || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            noise * (((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0)
        };
        for (center, start, along, length) in [
            (Vec2::new(x, -y), -FRAC_PI_2, Vec2::new(0.0, 1.0), 2.0 * y),
            (Vec2::new(x, y), 0.0, Vec2::new(-1.0, 0.0), 2.0 * x),
            (Vec2::new(-x, y), FRAC_PI_2, Vec2::new(0.0, -1.0), 2.0 * y),
            (Vec2::new(-x, -y), PI, Vec2::new(1.0, 0.0), 2.0 * x),
        ] {
            let arc = (FRAC_PI_2 * r * per_mm).ceil() as usize;
            for k in 0..arc {
                let d = Vec2::from_angle(start + k as f64 * FRAC_PI_2 / arc as f64);
                out.push(center + d * (r + wobble()));
            }
            let first = center + Vec2::from_angle(start + FRAC_PI_2) * r;
            let count = (length * per_mm).ceil() as usize;
            for k in 0..count {
                out.push(first + along * (length * k as f64 / count as f64) + along.perp() * -wobble());
            }
        }
        out
    }

    /// A noisy rounded rectangle is four lines and four arcs, the arcs of about the corners'
    /// radius, each piece meeting the next where it ends.
    #[test]
    fn a_noisy_rounded_rectangle_is_four_lines_and_four_arcs() {
        let points = rounded(30.0, 20.0, 5.0, 4.0, 0.03);
        let Some(Outline::Chain(pieces)) = chain(&points, true, 0.1) else { panic!("a chain") };
        let lines = pieces.iter().filter(|p| matches!(p, Piece::Line { .. })).count();
        let arcs: Vec<f64> = pieces.iter().filter_map(|p| if let Piece::Arc { radius, .. } = p { Some(*radius) } else { None }).collect();
        assert_eq!((lines, arcs.len()), (4, 4), "{pieces:?}");
        assert!(arcs.iter().all(|r| (r - 5.0).abs() < 0.2), "{arcs:?}");
        for k in 0..pieces.len() {
            assert!(pieces[k].to().dist(pieces[(k + 1) % pieces.len()].from()) < 1e-9, "end to end");
        }
    }

    /// A noisy circle is one circle; too few points is nothing.
    #[test]
    fn a_noisy_circle_is_a_circle() {
        let points: Vec<Vec2> = (0..200)
            .map(|k| {
                let a = TAU * f64::from(k) / 200.0;
                Vec2::new(3.0, -2.0) + Vec2::from_angle(a) * (12.0 + 0.02 * (7.0 * a).sin())
            })
            .collect();
        let Some(Outline::Circle { center, radius }) = chain(&points, true, 0.1) else { panic!("a circle") };
        assert!(center.dist(Vec2::new(3.0, -2.0)) < 0.01 && (radius - 12.0).abs() < 0.01);
        assert!(chain(&points[..2], false, 0.1).is_none());
    }
}
