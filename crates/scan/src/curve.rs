//! A curve drawn on the scan: points clicked along the scan, the smooth line through them (the
//! centripetal Catmull–Rom spline a 3D sketch draws, which neither loops nor overshoots where the
//! points are unevenly spaced) laid on the scan by taking each of its places to the nearest place
//! of the scan, and the fewest fit points, taken from that line, whose own spline keeps within a
//! tolerance of it. The curve is then an ordinary 3D sketch spline through those points.
//!
//! Between two points the line must stay within reach of the scan: across a gap or a hole it is
//! refused, naming the span, so a point can be clicked between.

use solidcore_record::Vec3;
use solidcraft_geom::Mesh;
use solidcraft_geom::catmull_rom::{SPAN_SAMPLES, samples};
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};
use solidcraft_geom::prelude::*;

use crate::Scan;

/// The most places the line on the scan has; a long line is laid more coarsely.
const PATH_LIMIT: usize = 20_000;

/// The most fit points a curve keeps (a 3D sketch spline's own limit).
pub const MAX_FIT: usize = 500;

/// The least default tolerance, in mm.
const LEAST_TOLERANCE: f64 = 1e-3;

/// How far a curve may stray from the line laid on `scan` when no tolerance is given: the side of
/// its typical triangle, no more than a thousandth of its size. The line follows the scan's noise,
/// which is about as fine as its triangles: a tighter tolerance only keeps more points tracing it
/// (on a 2.7M-triangle laser scan, 26 to 142 points for a 30 mm curve at 0.02 mm, 4 to 12 at its
/// 0.14 mm triangles). The cap keeps a coarse mesh of a smooth part close to it.
pub fn default_tolerance(scan: &Scan) -> f64 {
    crate::mirror_fill::typical_edge(scan).min(scan.extent() / 1000.0).max(LEAST_TOLERANCE)
}

/// The scan, for the nearest place of it to a point.
pub struct Projector {
    tree: std::sync::Arc<TriangleTree>,
    /// The scan's points and triangles, shared with what holds them ([`Projector::sharing`]).
    mesh: std::sync::Arc<Mesh>,
    /// The side of the equilateral triangle of the scan's mean triangle area.
    pub edge: f64,
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Projector").field("triangles", &self.mesh.triangles.len()).field("edge", &self.edge).finish()
    }
}

impl Projector {
    pub fn new(scan: &Scan) -> Projector {
        let mesh = Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Mesh::default() };
        let tree = std::sync::Arc::new(TriangleTree::of(&mesh.positions, &mesh.triangles));
        Projector::sharing(std::sync::Arc::new(mesh), tree)
    }

    /// A projector onto `mesh` (its points and triangles), with `tree` built over them, both
    /// shared, not copied: a scan's tree is as large as the scan, and Select reads the same one.
    pub fn sharing(mesh: std::sync::Arc<Mesh>, tree: std::sync::Arc<TriangleTree>) -> Projector {
        let edge = crate::mirror_fill::typical_edge(&Scan::new(&mesh.positions, &mesh.triangles));
        Projector { tree, mesh, edge }
    }

    /// The nearest place of the scan to `p` and how far it is.
    pub fn nearest(&self, p: Vec3) -> Option<(Vec3, f64)> {
        let (d, t) = self.tree.nearest(p)?;
        let tri = self.mesh.triangles.get(t as usize)?;
        let at = |i: u32| self.mesh.positions.get(i as usize).copied();
        Some((closest_on_triangle(p, at(tri[0])?, at(tri[1])?, at(tri[2])?), d))
    }

    /// Where the line through `p` along `dir` (either way) meets the scan, and how far that is
    /// from `p`: from the nearest place of the scan, the line is met with the plane of the
    /// triangle there, again from the scan's nearest place to that, until it settles (a few steps
    /// on a smooth scan). `None` where the line runs along the scan (it grazes it) or settles off it,
    /// as over a hole: the scan's nearest place is the better guess there.
    pub fn along(&self, p: Vec3, dir: Vec3) -> Option<(Vec3, f64)> {
        let n = dir.normalized()?;
        let mut x = p;
        for _ in 0..8 {
            let (q, m) = self.nearest_and_normal(x)?;
            let c = n.dot(m);
            if c.abs() < 0.25 {
                return None;
            }
            let next = p + n * ((q - p).dot(m) / c);
            let settled = next.dist(x) <= 1e-9 * (1.0 + next.dist(p));
            x = next;
            if settled {
                break;
            }
        }
        let (_, off) = self.nearest(x)?;
        (off <= 0.25 * self.edge.max(1e-9) + 1e-9).then(|| (x, x.dist(p)))
    }

    /// The scan's nearest place to `p` and the unit normal of the triangle it lies on.
    fn nearest_and_normal(&self, p: Vec3) -> Option<(Vec3, Vec3)> {
        let (_, t) = self.tree.nearest(p)?;
        let tri = self.mesh.triangles.get(t as usize)?;
        let at = |i: u32| self.mesh.positions.get(i as usize).copied();
        let (a, b, c) = (at(tri[0])?, at(tri[1])?, at(tri[2])?);
        Some((closest_on_triangle(p, a, b, c), (b - a).cross(c - a).normalized()?))
    }

    /// The way the scan faces at its nearest place to `p`: that triangle's unit normal.
    pub fn normal(&self, p: Vec3) -> Option<Vec3> {
        let (_, t) = self.tree.nearest(p)?;
        let tri = self.mesh.triangles.get(t as usize)?;
        let at = |i: u32| self.mesh.positions.get(i as usize).copied();
        let (a, b, c) = (at(tri[0])?, at(tri[1])?, at(tri[2])?);
        (b - a).cross(c - a).normalized()
    }
}

/// Why a curve could not be drawn on the scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveError {
    /// Fewer than two points (three for a closed curve).
    TooFew,
    /// A point is nowhere near the scan.
    OffTheScan { point: usize },
    /// Between point `after` and the next the line leaves the scan (a gap or a hole).
    Gap { after: usize },
}

impl std::fmt::Display for CurveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurveError::TooFew => f.write_str("click at least two points on the scan, apart from each other (three for a closed curve)"),
            CurveError::OffTheScan { point } => write!(f, "point {} is not on the scan", point + 1),
            CurveError::Gap { after } => {
                write!(f, "between points {} and {} the curve leaves the scan (a gap or a hole): click a point between them", after + 1, after + 2)
            }
        }
    }
}

/// The line through `picks` laid on the scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Laid {
    /// The picks, each put on the scan.
    pub picks: Vec<Vec3>,
    /// The line's places on the scan, from the first pick to the last (back to the first when
    /// closed), and where each pick is among them.
    pub path: Vec<Vec3>,
    pub at: Vec<usize>,
    pub closed: bool,
}

/// The fit list of a spline through `points`, closed by repeating the first at the end.
fn fit_list(points: &[Vec3], closed: bool) -> Vec<Vec3> {
    let mut fit = points.to_vec();
    if closed && let Some(&first) = points.first() {
        fit.push(first);
    }
    fit
}

/// The smooth line through `picks` (each first put on the scan), laid on the scan. A pick farther
/// than `reach` from the scan is refused; between picks the line may cut in or stand off (it is
/// laid on the scan place by place), but where it would jump (a hole, another wall) it is refused.
pub fn lay(scan: &Projector, picks: &[Vec3], closed: bool, reach: f64) -> Result<Laid, CurveError> {
    if picks.len() < 2 || (closed && picks.len() < 3) {
        return Err(CurveError::TooFew);
    }
    let on: Vec<Vec3> = picks
        .iter()
        .enumerate()
        .map(|(i, p)| scan.nearest(*p).filter(|(_, d)| *d <= reach).map(|(q, _)| q).ok_or(CurveError::OffTheScan { point: i }))
        .collect::<Result<_, _>>()?;
    // Points clicked twice in one place say nothing more.
    let mut picks: Vec<Vec3> = Vec::with_capacity(on.len());
    for p in on {
        if picks.last().is_none_or(|q: &Vec3| q.dist(p) > 1e-6) {
            picks.push(p);
        }
    }
    if closed && picks.len() > 1 && picks.first().zip(picks.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6) {
        picks.pop();
    }
    if picks.len() < 2 || (closed && picks.len() < 3) {
        return Err(CurveError::TooFew);
    }
    let fit = fit_list(&picks, closed);
    let spline = samples(&fit);
    // Each span's samples cut finer, to about a triangle of the scan each (fewer on a long line).
    let length: f64 = spline.windows(2).map(|w| w[0].dist(w[1])).sum();
    let step = scan.edge.max(length / PATH_LIMIT as f64).max(1e-6);
    let jump = 4.0 * step + 3.0 * scan.edge;
    let spans = fit.len() - 1;
    let mut path = vec![picks[0]];
    let mut at = vec![0];
    for span in 0..spans {
        // Two fit points make one straight span, sampled at its ends only.
        let (first, last) = if fit.len() == 2 { (0, 1) } else { (span * SPAN_SAMPLES, (span + 1) * SPAN_SAMPLES) };
        for k in first..last {
            let (a, b) = (spline[k], spline[k + 1]);
            let n = ((a.dist(b) / step).ceil() as usize).clamp(1, PATH_LIMIT);
            for j in 1..=n {
                let q = a + (b - a) * (j as f64 / n as f64);
                let (on, _) = scan.nearest(q).ok_or(CurveError::Gap { after: span })?;
                // Neighbouring places a step apart land near each other on the scan unless the
                // scan is not there: across a hole, or flipping to another wall, they jump.
                if path.last().is_some_and(|last: &Vec3| last.dist(on) > jump) {
                    return Err(CurveError::Gap { after: span });
                }
                path.push(on);
            }
        }
        // The pick itself ends the span exactly.
        if let (Some(last), Some(&pick)) = (path.last_mut(), fit.get(span + 1)) {
            *last = pick;
        }
        at.push(path.len() - 1);
    }
    Ok(Laid { picks, path, at, closed })
}

/// The distance from `p` to the segment `a`–`b`.
fn to_segment(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = b - a;
    let l2 = ab.len2();
    let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + ab * t)
}

/// A curve on the scan: its fit points and how far its spline strays from the line on the scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    /// The fit points, the first repeated at the end when closed (as a 3D sketch spline takes them).
    pub fit: Vec<Vec3>,
    /// The farthest the spline through them is from the line on the scan, in mm.
    pub max: f64,
}

/// The fewest fit points taken from the line on the scan (every pick among them) whose spline keeps
/// within `tolerance` of it: where a span strays farther, the line's place it strays most from is
/// added, until every span keeps within it or the curve has [`MAX_FIT`] points.
pub fn fit(laid: &Laid, tolerance: f64) -> Fitted {
    let mut at = laid.at.clone();
    loop {
        // When closed, `at` ends at the path's end, the first pick again: the fit list closes.
        let fit: Vec<Vec3> = at.iter().filter_map(|&i| laid.path.get(i).copied()).collect();
        let spline = samples(&fit);
        let spans = fit.len().saturating_sub(1);
        let mut worst: Vec<(usize, f64)> = Vec::new();
        let mut max = 0.0f64;
        for span in 0..spans {
            let (first, last) = if fit.len() == 2 { (0, 1) } else { (span * SPAN_SAMPLES, (span + 1) * SPAN_SAMPLES) };
            let Some(piece) = spline.get(first..=last) else { continue };
            let (from, to) = (at[span], at[span + 1]);
            let mut far = (from, 0.0f64);
            for (i, &point) in laid.path.iter().enumerate().take(to).skip(from + 1) {
                let d = piece.windows(2).map(|w| to_segment(point, w[0], w[1])).fold(f64::INFINITY, f64::min);
                if d > far.1 {
                    far = (i, d);
                }
            }
            max = max.max(far.1);
            if far.1 > tolerance {
                worst.push(far);
            }
        }
        if worst.is_empty() || at.len() + worst.len() > MAX_FIT {
            return Fitted { fit, max };
        }
        at.extend(worst.iter().map(|w| w.0));
        at.sort_unstable();
        at.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cylinder of radius `r` along Y, from y = 0 to `length`, `n` × `n` squares round and along.
    fn cylinder(r: f64, length: f64, n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..n {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                positions.push(Vec3::new(r * a.cos(), length * f64::from(j) / f64::from(n), r * a.sin()));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let (a, b) = (j * n + i, j * n + (i + 1) % n);
                triangles.push([a, b, b + n]);
                triangles.push([a, b + n, a + n]);
            }
        }
        (positions, triangles)
    }

    /// A flat strip of `w` × `h` squares of side `side`, two triangles each.
    fn strip(w: u32, h: u32, side: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let positions = (0..=h).flat_map(|j| (0..=w).map(move |i| Vec3::new(side * f64::from(i), side * f64::from(j), 0.0))).collect();
        let at = |i: u32, j: u32| j * (w + 1) + i;
        let triangles = (0..h)
            .flat_map(|j| (0..w).flat_map(move |i| [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]]))
            .collect();
        (positions, triangles)
    }

    /// The default tolerance is as fine as the scan's triangles, no coarser than a thousandth of
    /// its size.
    #[test]
    fn the_default_tolerance_follows_the_triangles_up_to_a_thousandth_of_the_size() {
        // 2 m long in 1 mm squares: its triangles' side (that of an equilateral one of the same area).
        let (p, t) = strip(2000, 1, 1.0);
        let fine = default_tolerance(&Scan::new(&p, &t));
        assert!((fine - (2.0 / 3.0f64.sqrt()).sqrt()).abs() < 1e-9, "{fine}");
        // One 10 mm square: a thousandth of its diagonal.
        let (p, t) = strip(1, 1, 10.0);
        let coarse = default_tolerance(&Scan::new(&p, &t));
        assert!((coarse - 200.0f64.sqrt() / 1000.0).abs() < 1e-9, "{coarse}");
    }

    /// Two points half way round a cylinder: the line between them bends round the cylinder (it is
    /// laid on the scan, not cut through it), and the curve fitted to it keeps within the tolerance.
    #[test]
    fn a_curve_across_a_cylinder_follows_it() {
        let (positions, triangles) = cylinder(20.0, 40.0, 96);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let picks = [Vec3::new(20.0, 10.0, 0.0), Vec3::new(0.0, 20.0, 20.0), Vec3::new(-20.0, 30.0, 0.0)];
        let laid = lay(&on, &picks, false, 1.0).unwrap();
        assert_eq!(laid.at.len(), 3);
        assert!(laid.path.iter().all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 20.0).abs() < 0.05), "on the cylinder");
        let curve = fit(&laid, 0.02);
        assert!(curve.max <= 0.02, "within the tolerance: {}", curve.max);
        assert!(curve.fit.len() > 3 && curve.fit.len() < 60, "a few more points than clicked: {}", curve.fit.len());
        assert_eq!((curve.fit[0], *curve.fit.last().unwrap()), (laid.picks[0], laid.picks[2]), "through the ends clicked");
        for p in &laid.picks {
            assert!(curve.fit.contains(p), "through every point clicked");
        }
    }

    /// Round the cylinder, closed: the curve comes back to where it began.
    #[test]
    fn a_closed_curve_runs_round() {
        let (positions, triangles) = cylinder(20.0, 40.0, 96);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let picks = [Vec3::new(20.0, 20.0, 0.0), Vec3::new(-10.0, 20.0, 17.3), Vec3::new(-10.0, 20.0, -17.3)];
        let curve = fit(&lay(&on, &picks, true, 1.0).unwrap(), 0.05);
        assert_eq!(curve.fit.first(), curve.fit.last(), "closed");
        assert!(curve.max <= 0.05);
    }

    /// The tube is open at its ends: a line from one end past the other leaves the scan.
    #[test]
    fn a_line_off_the_scan_is_refused() {
        let (positions, triangles) = cylinder(20.0, 40.0, 48);
        let on = Projector::new(&Scan::new(&positions, &triangles));
        assert_eq!(lay(&on, &[Vec3::new(20.0, 5.0, 0.0)], false, 1.0).unwrap_err(), CurveError::TooFew);
        assert_eq!(lay(&on, &[Vec3::new(20.0, 5.0, 0.0), Vec3::new(80.0, 5.0, 0.0)], false, 1.0).unwrap_err(), CurveError::OffTheScan { point: 1 });
        // Straight across the inside of the tube: half way, the nearest wall flips to the other side.
        assert_eq!(lay(&on, &[Vec3::new(20.0, 5.0, 0.0), Vec3::new(-20.0, 5.0, 0.0)], false, 1.0).unwrap_err(), CurveError::Gap { after: 0 });
    }
}
