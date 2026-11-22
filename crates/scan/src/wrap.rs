//! Wrap Surface (SurfaceCraft's, after QuickSurface's): a Free Form begun on a tube-like part of
//! the scan. A cylinder is fitted to the part round a point clicked on it (or about the world's X,
//! Y or Z, its line and radius fitted across it), a net of rectangles laid on the cylinder over the
//! part's reach, as many points the shorter way as Details says and enough the longer way to keep
//! the cells square, and the net fitted to the scan as a whole. A part that goes the whole way round
//! is left open at a seam at its back, as a Free Form is one net.

use std::f64::consts::{PI, TAU};

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::{self, Kind, Shape};
use crate::free_form::{self, Fitted, Fitting};
use crate::{Scan, in_plane};

/// The points along the shorter way, for Low, Medium and Fine.
pub const POINTS: [usize; 3] = [4, 8, 14];
/// The most points the longer way has.
pub const MOST: usize = 40;
/// The fewest triangles that are a part to wrap.
const LEAST_TRIANGLES: usize = 8;
/// A part reaching this far round (radians) is taken as the whole way: the net is left open at a
/// seam at its back.
const WHOLE_TURN: f64 = TAU - 0.35;
/// The scan is met no farther from the cylinder than this share of its radius: past that is an
/// opening, or another part.
const MEETS: f64 = 0.5;

/// A cylinder about the line through `point` along the unit `axis`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    pub point: Vec3,
    pub axis: Vec3,
    pub radius: f64,
}

impl Cylinder {
    /// How far `p` is from the cylinder.
    pub fn distance(&self, p: Vec3) -> f64 {
        let d = p - self.point;
        ((d - self.axis * d.dot(self.axis)).len() - self.radius).abs()
    }
}

/// What to wrap: the cylinder's axis (`None`: fitted to the part), how many points (`details`, 0
/// Low to 2 Fine), how far round (`arc`, radians; `None` for the part's own reach), and how stiff.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wrap {
    pub axis: Option<Vec3>,
    pub details: usize,
    pub arc: Option<f64>,
    pub stiffness: f64,
}

/// A net wrapped on the scan: the cylinder it was laid on, how far the part is from that on
/// average, and the face fitted.
#[derive(Clone, Debug, PartialEq)]
pub struct Wrapped {
    pub cylinder: Cylinder,
    pub off: f64,
    pub fitted: Fitted,
}

/// Why nothing could be wrapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapError {
    /// Too little of the scan round the point clicked.
    TooLittle,
    /// No cylinder fits the part.
    NoCylinder,
    /// The part has no width or no length round its cylinder.
    NoReach,
    /// The net met no scan, or could not be made.
    Failed,
}

impl std::fmt::Display for WrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WrapError::TooLittle => "there is too little of the scan round the point: click the middle of a tube-like part, or reach farther",
            WrapError::NoCylinder => "no cylinder fits the part round the point: click a part curved round one axis, or choose an axis of the world",
            WrapError::NoReach => "the part has no width or no length round its cylinder",
            WrapError::Failed => "the net met no scan: click on a tube-like part of it",
        })
    }
}

/// The angle `a` turned into (−π, π].
fn wrapped(a: f64) -> f64 {
    let a = a.rem_euclid(TAU);
    if a > PI { a - TAU } else { a }
}

/// The cylinder about the direction `axis` that fits `points` best: a circle through them seen
/// along it (Kåsa's), its centre a point of the axis. `None` for too few, or all in a line along it.
pub fn about(axis: Vec3, points: &[Vec3]) -> Option<Cylinder> {
    let axis = axis.normalized()?;
    if points.len() < 3 {
        return None;
    }
    let (u, v) = in_plane(axis);
    let mean = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len() as f64;
    let ([a, b], radius) = fit::kasa(points.iter().map(|p| ([(*p - mean).dot(u), (*p - mean).dot(v)], 1.0)))?;
    (radius.is_finite() && radius > 1e-9).then(|| Cylinder { point: mean + u * a + v * b, axis, radius })
}

/// The net of rectangles on the cylinder `c` over the reach of `points`, round it first (u) and
/// then along it (v), u fastest: as many points the shorter way as `details` says (0 Low to 2
/// Fine), the longer way as many as keep the cells square (up to [`MOST`]). `arc` (radians) sets
/// how far round it goes about the middle of the points. `None` for a reach with no width or
/// length.
pub fn net_on(c: &Cylinder, points: &[Vec3], details: usize, arc: Option<f64>) -> Option<([usize; 2], Vec<Vec3>)> {
    if points.len() < 3 || !(c.radius.is_finite() && c.radius > 1e-9) {
        return None;
    }
    let (u, v) = in_plane(c.axis);
    let (mut along_min, mut along_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut sin_sum, mut cos_sum) = (0.0, 0.0);
    let angles: Vec<f64> = points
        .iter()
        .map(|p| {
            let d = *p - c.point;
            let along = d.dot(c.axis);
            along_min = along_min.min(along);
            along_max = along_max.max(along);
            let angle = d.dot(v).atan2(d.dot(u));
            sin_sum += angle.sin();
            cos_sum += angle.cos();
            angle
        })
        .collect();
    let middle = sin_sum.atan2(cos_sum);
    let (mut low, mut high) = angles.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), a| {
        let from_middle = wrapped(a - middle);
        (lo.min(from_middle), hi.max(from_middle))
    });
    // An arc asked for: that far round about the middle of the points.
    if let Some(arc) = arc.filter(|a| *a > 0.0) {
        let half = 0.5 * arc.min(TAU);
        let centre = 0.5 * (low + high);
        (low, high) = (centre - half, centre + half);
    }
    // Nearly the whole way round: the net goes round from a seam at the back.
    if high - low >= WHOLE_TURN {
        (low, high) = (-PI + 0.02, PI - 0.02);
    }
    let (round, length) = (c.radius * (high - low), along_max - along_min);
    if !(round.is_finite() && length.is_finite() && round > 1e-9 && length > 1e-9) {
        return None;
    }
    let short = POINTS[details.min(POINTS.len() - 1)];
    let long = ((short as f64 * round.max(length) / round.min(length)).round() as usize).clamp(short, MOST);
    let (count_u, count_v) = if round <= length { (short, long) } else { (long, short) };
    let mut net = Vec::with_capacity(count_u * count_v);
    for j in 0..count_v {
        let along = along_min + length * j as f64 / (count_v - 1) as f64;
        for i in 0..count_u {
            let angle = middle + low + (high - low) * i as f64 / (count_u - 1) as f64;
            net.push(c.point + c.axis * along + (u * angle.cos() + v * angle.sin()) * c.radius);
        }
    }
    Some(([count_u, count_v], net))
}

/// A net wrapped on the part of the scan within `reach` of `at` (round triangle `seed`), as `wrap`
/// says; `snap` puts places on the scan, none farther than its second argument (`None` for a
/// place with none so near).
pub fn wrap(
    scan: &Scan,
    seed: usize,
    at: Vec3,
    reach: f64,
    wrap: &Wrap,
    snap: impl Fn(&[Vec3], f64) -> Vec<Option<Vec3>>,
) -> Result<Wrapped, WrapError> {
    wrap_region(scan, &fit::around(scan, seed, at, reach), wrap, snap)
}

/// A net wrapped on the triangles `region` of the scan (chosen by hand: selected, kept, grown
/// from a click), as [`wrap`] wraps the part round a click.
pub fn wrap_region(scan: &Scan, region: &[usize], wrap: &Wrap, snap: impl Fn(&[Vec3], f64) -> Vec<Option<Vec3>>) -> Result<Wrapped, WrapError> {
    let region: Vec<usize> = region.iter().copied().filter(|&t| t < scan.triangles.len()).collect();
    if region.len() < LEAST_TRIANGLES {
        return Err(WrapError::TooLittle);
    }
    let mut seen = vec![false; scan.positions.len()];
    let mut points = Vec::new();
    for &t in &region {
        for i in scan.triangles[t] {
            if let Some(s) = seen.get_mut(i as usize).filter(|s| !**s) {
                *s = true;
                points.push(scan.positions[i as usize]);
            }
        }
    }
    let cylinder = match wrap.axis {
        Some(axis) => about(axis, &points).ok_or(WrapError::NoCylinder)?,
        None => match fit::shape_of(scan, &region, Kind::Cylinder) {
            Some(Shape::Cylinder { point, axis, radius }) => Cylinder { point, axis, radius },
            _ => return Err(WrapError::NoCylinder),
        },
    };
    let off = points.iter().map(|p| cylinder.distance(*p)).sum::<f64>() / points.len() as f64;
    let (counts, net) = net_on(&cylinder, &points, wrap.details, wrap.arc).ok_or(WrapError::NoReach)?;
    let limit = MEETS * cylinder.radius;
    let fitting = Fitting { count: counts[0].min(counts[1]), stiffness: wrap.stiffness, overall: false };
    let fitted = free_form::fitted_net(net, counts, &fitting, |ps| snap(ps, limit)).ok_or(WrapError::Failed)?;
    if fitted.met == 0 {
        return Err(WrapError::Failed);
    }
    Ok(Wrapped { cylinder, off, fitted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Projector;
    use crate::surface_edit::{self, distance};

    /// A tube of radius 10 along Z, 40 long, from angle `from` to `to` round it.
    fn tube(from: f64, to: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (rings, segments) = (20u32, 72u32);
        let mut positions = Vec::new();
        for r in 0..=rings {
            for s in 0..=segments {
                let a = from + (to - from) * f64::from(s) / f64::from(segments);
                positions.push(Vec3::new(10.0 * a.cos(), 10.0 * a.sin(), 40.0 * f64::from(r) / f64::from(rings)));
            }
        }
        let mut triangles = Vec::new();
        for r in 0..rings {
            for s in 0..segments {
                let a = r * (segments + 1) + s;
                let (b, c) = (a + 1, a + segments + 1);
                triangles.push([a, b, c + 1]);
                triangles.push([a, c + 1, c]);
            }
        }
        (positions, triangles)
    }

    fn wrapped_on(positions: &[Vec3], triangles: &[[u32; 3]], wrap_as: &Wrap) -> Result<Wrapped, WrapError> {
        let scan = Scan::new(positions, triangles);
        let on = Projector::new(&scan);
        let at = Vec3::new(10.0 * (0.5f64).cos(), 10.0 * (0.5f64).sin(), 20.0);
        let seed = crate::base::nearest_triangle(&scan, at).unwrap();
        wrap(&scan, seed, at, 100.0, wrap_as, |ps, limit| ps.iter().map(|p| on.nearest(*p).filter(|(_, d)| *d <= limit).map(|(q, _)| q)).collect())
    }

    /// A quarter turn of a tube: the net goes the longer way along it with square cells, and the
    /// face fitted lies on the tube over the part's reach.
    #[test]
    fn a_net_is_wrapped_over_the_part_and_lies_on_it() {
        let (positions, triangles) = tube(0.0, PI / 2.0);
        let made = wrapped_on(&positions, &triangles, &Wrap { details: 1, ..Wrap::default() }).unwrap();
        assert!((made.cylinder.radius - 10.0).abs() < 0.05, "{:?}", made.cylinder);
        // 15.7 mm round, 40 mm along: 8 points the short way, 20 the long.
        assert_eq!(made.fitted.surface.count(), [8, 20]);
        let scan = Scan::new(&positions, &triangles);
        let under = surface_edit::covered(&scan, &made.fitted.surface, 1.0, 5_000);
        let (rms, max) = distance(&made.fitted.surface, &under);
        assert!(rms < 0.02 && max < 0.1, "on the tube: {rms} {max}");
        // The net reaches the part's ends.
        let surface = &made.fitted.surface;
        let (start, end) = (surface.eval(0.5, 0.0).unwrap(), surface.eval(0.5, 1.0).unwrap());
        assert!(start.z.abs() < 0.5 && (end.z - 40.0).abs() < 0.5, "{start:?} {end:?}");
    }

    /// About the world's Z the tube's cylinder is found across it; about X it is no cylinder of
    /// the tube; an arc sets how far round the net goes.
    #[test]
    fn an_axis_of_the_world_and_an_arc() {
        let (positions, _) = tube(0.0, PI / 2.0);
        let c = about(Vec3::Z, &positions).unwrap();
        assert!(Vec3::new(c.point.x, c.point.y, 0.0).len() < 0.05 && (c.radius - 10.0).abs() < 0.05, "{c:?}");
        let (_, net) = net_on(&c, &positions, 1, Some(PI)).unwrap();
        let angles: Vec<f64> = net.iter().map(|p| p.y.atan2(p.x).to_degrees()).collect();
        let (lo, hi) = angles.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), a| (lo.min(*a), hi.max(*a)));
        assert!((hi - lo - 180.0).abs() < 1e-6 && (lo + 45.0).abs() < 1e-6, "{lo} to {hi}");
    }

    /// A whole tube is left open at a seam; Details chooses how many points.
    #[test]
    fn a_whole_tube_is_open_at_a_seam_and_details_chooses_the_points() {
        let (positions, triangles) = tube(0.0, TAU);
        let counts = |details| wrapped_on(&positions, &triangles, &Wrap { details, ..Wrap::default() }).unwrap().fitted.surface.count();
        let (low, medium) = (counts(0), counts(1));
        assert!(low[0] * low[1] < medium[0] * medium[1], "{low:?} {medium:?}");
        let made = wrapped_on(&positions, &triangles, &Wrap { details: 1, ..Wrap::default() }).unwrap();
        let s = &made.fitted.surface;
        let (first, last) = (s.eval(0.0, 0.5).unwrap(), s.eval(1.0, 0.5).unwrap());
        assert!(first.dist(last) > 0.1 && first.dist(last) < 2.0, "{first:?} {last:?}");
        // A flat plate is no tube.
        let flat: Vec<Vec3> = positions.iter().map(|p| Vec3::new(p.x, 0.0, p.z)).collect();
        assert!(wrapped_on(&flat, &triangles, &Wrap::default()).is_err());
    }
}
