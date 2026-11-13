//! A flat template of a wall (or any panel) from its scan: the outline the user picked, laid on the
//! panel's plane, with a cut-out for each part that stands off the plane (an outlet, a pipe, a
//! bracket) and the scan's own edge drawn on the same plane so the template can be checked
//! against the scan.
//!
//! Nothing follows the scan except where it is asked to: the outline is exactly the points picked,
//! put onto one plane, and the plane is the one the scan lies in, fitted with the parts far off it
//! left out ([`crate::free_form::flat_plane`]), so a protrusion does not tilt it. A part that
//! stands more than `tolerance` off the plane (on the side asked for) is found where the plane
//! `tolerance` away cuts the scan ([`crate::section::section_welded`]): each closed run is that
//! part's footprint, and becomes a cut-out. A run that leaves the scan (the part runs off its
//! edge) is closed with a straight line and said to touch the scan's edge.
//!
//! A footprint that crosses the outline is clipped to it when the outline is convex; for an outline
//! that is not, it is kept whole and said to cross.

use std::collections::HashMap;

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::profile::{area as twice_area, contains};
use crate::section::section_welded;

/// The most points an outline has, the most cut-outs kept and the most loops of the scan's edge.
pub const MOST_OUTLINE: usize = 1_000;
pub const MOST_CUTOUTS: usize = 1_000;
pub const MOST_EDGE_LOOPS: usize = 1_000;

/// The least a cut-out's area is by default (mm²): specks of noise are not parts.
pub const LEAST_AREA: f64 = 25.0;

/// Which side of the plane a part stands off: along the plane's normal, against it, or either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Out,
    In,
    Both,
}

/// Why a template could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// The outline has fewer than three points, too many, a point that is not a place, or it
    /// encloses no area or crosses itself.
    Outline(&'static str),
    /// The plane has no direction.
    Plane,
    /// The tolerance or the least area is not a number in range.
    Number(&'static str),
    /// The scan has no triangles.
    EmptyScan,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Outline(why) => write!(f, "the outline {why}"),
            TemplateError::Plane => f.write_str("the plane has no direction"),
            TemplateError::Number(what) => write!(f, "{what} must be a number above 0"),
            TemplateError::EmptyScan => f.write_str("the scan has no triangles"),
        }
    }
}

/// A part standing off the plane, as the cut-out it makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Cutout {
    /// The footprint on the plane, counter-clockwise, in the template's coordinates.
    pub ring: Vec<Vec2>,
    /// Which side of the plane it stands off (never `Both`).
    pub side: Side,
    /// The footprint's area (mm²), and how far the part stands off the plane at most (mm).
    pub area: f64,
    pub height: f64,
    /// The part runs off the scan's edge: the footprint was closed with a straight line.
    pub at_scan_edge: bool,
    /// The footprint crosses the outline: clipped to it, or (an outline that is not convex) kept
    /// whole.
    pub crosses: bool,
    pub clipped: bool,
}

/// A template.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// The plane's frame: its origin, x and y; `x × y` is the normal `Side::Out` goes along.
    pub frame: Plane,
    /// The outline picked, on the plane, in the template's coordinates, counter-clockwise.
    pub outline: Vec<Vec2>,
    pub cutouts: Vec<Cutout>,
    /// The loops of the scan's open edges on the plane, to check the template against.
    pub scan_edge: Vec<Vec<Vec2>>,
    /// The outline's area and the area the cut-outs take from it (mm²).
    pub area: f64,
    pub cut_area: f64,
    /// The footprints found, those left out for being under the least area, and those left out
    /// for lying wholly outside the outline.
    pub found: usize,
    pub specks: usize,
    pub outside: usize,
}

impl Template {
    /// A point of the template in space.
    pub fn at(&self, p: Vec2) -> Vec3 {
        self.frame.origin + self.frame.x * p.x + self.frame.y * p.y
    }
}

/// The frame of the plane through `point` square to `normal`, its x along the first outline edge
/// that lies in the plane (so a rectangle comes out square to the axes).
fn frame_of(point: Vec3, normal: Vec3, outline: &[Vec3]) -> Option<Plane> {
    let n = normal.normalized()?;
    let mut edges = outline.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
    edges.push(*outline.first()? - *outline.last()?);
    let x = edges.into_iter().map(|e| e - n * e.dot(n)).find_map(Vec3::normalized).or_else(|| {
        let axis = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        (axis - n * axis.dot(n)).normalized()
    })?;
    let y = n.cross(x).normalized()?;
    Some(Plane { origin: point, x, y })
}

fn to_plane(f: &Plane, p: Vec3) -> Vec2 {
    let d = p - f.origin;
    Vec2::new(d.dot(f.x), d.dot(f.y))
}

fn cross2(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// The area a ring encloses, negative for one that runs clockwise.
fn signed_area(ring: &[Vec2]) -> f64 {
    twice_area(ring) / 2.0
}

fn ccw(mut ring: Vec<Vec2>) -> Vec<Vec2> {
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// Whether segments `a`–`b` and `c`–`d` cross at a point inside both.
fn crossing(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2, d3, d4) = (cross2(a, b, c), cross2(a, b, d), cross2(c, d, a), cross2(c, d, b));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn edges_of(ring: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    ring.iter().copied().zip(ring.iter().copied().cycle().skip(1))
}

fn simple(ring: &[Vec2]) -> bool {
    let n = ring.len();
    for i in 0..n {
        for j in (i + 1)..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if crossing(ring[i], ring[(i + 1) % n], ring[j], ring[(j + 1) % n]) {
                return false;
            }
        }
    }
    true
}

fn convex(ring: &[Vec2]) -> bool {
    edges_of(ring).zip(ring.iter().copied().cycle().skip(2)).all(|((a, b), c)| cross2(a, b, c) >= -1e-9)
}

fn meet(p: Vec2, q: Vec2, sp: f64, sc: f64) -> Vec2 {
    let t = sp / (sp - sc);
    Vec2::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t)
}

/// `subject` clipped to the convex counter-clockwise `clip` (Sutherland–Hodgman).
fn clipped(subject: &[Vec2], clip: &[Vec2]) -> Vec<Vec2> {
    let mut out = subject.to_vec();
    for (a, b) in edges_of(clip) {
        let input = std::mem::take(&mut out);
        let Some(&last) = input.last() else { break };
        let mut prev = last;
        for &cur in &input {
            let (sp, sc) = (cross2(a, b, prev), cross2(a, b, cur));
            if sc >= 0.0 {
                if sp < 0.0 {
                    out.push(meet(prev, cur, sp, sc));
                }
                out.push(cur);
            } else if sp >= 0.0 {
                out.push(meet(prev, cur, sp, sc));
            }
            prev = cur;
        }
    }
    out
}

/// The loops of the scan's open edges (edges of one triangle), on the plane.
pub(crate) fn scan_edges(scan: &Scan, weld: &[u32], frame: &Plane) -> Vec<Vec<Vec2>> {
    let id = |v: u32| weld.get(v as usize).copied().unwrap_or(v);
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for t in scan.triangles {
        for k in 0..3 {
            let (a, b) = (id(t[k]), id(t[(k + 1) % 3]));
            if a != b {
                *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut place: HashMap<u32, Vec3> = HashMap::new();
    for t in scan.triangles {
        for k in 0..3 {
            let (a, b) = (id(t[k]), id(t[(k + 1) % 3]));
            if a != b && uses.get(&(a.min(b), a.max(b))) == Some(&1) {
                next.entry(a).or_default().push(b);
                for v in [t[k], t[(k + 1) % 3]] {
                    if let Some(p) = scan.positions.get(v as usize) {
                        place.entry(id(v)).or_insert(*p);
                    }
                }
            }
        }
    }
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for start in starts {
        if loops.len() >= MOST_EDGE_LOOPS {
            break;
        }
        let mut chain = Vec::new();
        let mut at = start;
        while let Some(to) = next.get_mut(&at).and_then(Vec::pop) {
            chain.push(at);
            at = to;
            if at == start || chain.len() > scan.positions.len() {
                break;
            }
        }
        if at == start && chain.len() >= 3 {
            loops.push(chain.iter().filter_map(|v| place.get(v)).map(|p| to_plane(frame, *p)).collect());
        }
    }
    loops
}

/// A spread to judge noise by: 1.48 times the median absolute distance from the median of
/// `heights` (what a standard deviation would be, were they normal, but a protrusion does not
/// move it); 0 with none.
pub fn spread(heights: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = heights.iter().copied().filter(|h| h.is_finite()).collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let mut away: Vec<f64> = sorted.iter().map(|h| (h - median).abs()).collect();
    away.sort_by(f64::total_cmp);
    1.4826 * away[away.len() / 2]
}

/// The wall under an outline: its plane, how far off it counts as standing off, and how many of
/// the scan's points it was fitted to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wall {
    pub point: Vec3,
    /// Unit, turned so the parts standing farthest off the plane are on its positive side.
    pub normal: Vec3,
    /// Four times the noise of the wall about the plane, and at least 1 mm.
    pub tolerance: f64,
    /// The scan's points under the outline, and those the plane was fitted to (the rest, far off
    /// it, are protrusions).
    pub under: usize,
    pub kept: usize,
}

/// The most of the scan's points looked at to fit the plane (the rest are thinned evenly).
const MOST_FITTED: usize = 50_000;

/// The plane the scan lies in under `outline` (three or more points): fitted to the scan's points
/// whose place over the outline's own plane is inside it, with the points far off the plane left
/// out (so what stands off the wall does not tilt it). `None` when the outline or the scan under
/// it makes no plane.
pub fn wall(scan: &Scan, outline: &[Vec3]) -> Option<Wall> {
    if outline.len() < 3 || outline.len() > MOST_OUTLINE || !outline.iter().all(|p| p.is_finite()) {
        return None;
    }
    let (point0, normal0, _) = crate::free_form::flat_plane(outline)?;
    let frame = frame_of(point0, normal0, outline)?;
    let ring: Vec<Vec2> = outline.iter().map(|p| to_plane(&frame, *p)).collect();
    let step = (scan.positions.len() / MOST_FITTED).max(1);
    let under: Vec<Vec3> = scan.positions.iter().step_by(step).copied().filter(|p| p.is_finite() && contains(&ring, to_plane(&frame, *p))).collect();
    if under.len() < 3 {
        return None;
    }
    let (point, normal, kept) = crate::free_form::flat_plane(&under)?;
    let heights: Vec<f64> = under.iter().map(|p| (*p - point).dot(normal)).collect();
    let tolerance = (4.0 * spread(&heights)).max(1.0);
    let (most, least) = heights.iter().fold((0.0_f64, 0.0_f64), |(hi, lo), h| (hi.max(*h), lo.min(*h)));
    let normal = if -least > most { -normal } else { normal };
    Some(Wall { point, normal, tolerance, under: under.len(), kept })
}

/// The template of the scan over the `outline` picked (three or more points, put onto the plane
/// through `point` square to `normal`), with a cut-out for each part standing more than
/// `tolerance` off the plane on `side` and covering at least `least_area`.
pub fn template(
    scan: &Scan,
    point: Vec3,
    normal: Vec3,
    outline: &[Vec3],
    tolerance: f64,
    side: Side,
    least_area: f64,
) -> Result<Template, TemplateError> {
    if outline.len() < 3 {
        return Err(TemplateError::Outline("needs at least three points"));
    }
    if outline.len() > MOST_OUTLINE {
        return Err(TemplateError::Outline("has too many points"));
    }
    if !(point.is_finite() && outline.iter().all(|p| p.is_finite())) {
        return Err(TemplateError::Outline("has a point that is not a place"));
    }
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(TemplateError::Number("`tolerance`"));
    }
    if !(least_area.is_finite() && least_area >= 0.0) {
        return Err(TemplateError::Number("`least_area`"));
    }
    if scan.triangles.is_empty() {
        return Err(TemplateError::EmptyScan);
    }
    let frame = frame_of(point, normal, outline).ok_or(TemplateError::Plane)?;
    let ring: Vec<Vec2> = outline.iter().map(|p| to_plane(&frame, *p)).collect();
    let size = ring.iter().fold(0.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    if !(signed_area(&ring).abs() > 1e-9 * (1.0 + size * size)) {
        return Err(TemplateError::Outline("encloses no area"));
    }
    if !simple(&ring) {
        return Err(TemplateError::Outline("crosses itself"));
    }
    let outline2 = ccw(ring);
    let outline_area = signed_area(&outline2);
    let is_convex = convex(&outline2);

    let weld = crate::welded(scan.positions);
    let normal = frame.x.cross(frame.y);
    let sides: &[Side] = match side {
        Side::Out => &[Side::Out],
        Side::In => &[Side::In],
        Side::Both => &[Side::Out, Side::In],
    };
    let mut cutouts = Vec::new();
    let (mut found, mut specks, mut outside) = (0usize, 0usize, 0usize);
    'sides: for &which in sides {
        let sign = if which == Side::Out { 1.0 } else { -1.0 };
        let cutting = Plane { origin: frame.origin + normal * (sign * tolerance), x: frame.x, y: frame.y };
        for run in section_welded(scan, &weld, &cutting) {
            if run.points.len() < 3 {
                continue;
            }
            found += 1;
            let at_scan_edge = !run.closed;
            let footprint = ccw(run.points);
            if signed_area(&footprint) < least_area.max(f64::MIN_POSITIVE) {
                specks += 1;
                continue;
            }
            let inside = footprint.iter().filter(|p| contains(&outline2, **p)).count();
            let meets = edges_of(&footprint).any(|(a, b)| edges_of(&outline2).any(|(c, d)| crossing(a, b, c, d)));
            let swallows = outline2.iter().any(|p| contains(&footprint, *p));
            if inside == 0 && !meets && !swallows {
                outside += 1;
                continue;
            }
            let crosses = inside < footprint.len() || meets || swallows;
            let (ring, was_clipped) = if crosses && is_convex {
                let cut = clipped(&footprint, &outline2);
                if cut.len() < 3 {
                    outside += 1;
                    continue;
                }
                (cut, true)
            } else {
                (footprint, false)
            };
            let ring_area = signed_area(&ring);
            if ring_area < least_area.max(f64::MIN_POSITIVE) {
                specks += 1;
                continue;
            }
            cutouts.push(Cutout { ring, side: which, area: ring_area, height: tolerance, at_scan_edge, crosses, clipped: was_clipped });
            if cutouts.len() >= MOST_CUTOUTS {
                break 'sides;
            }
        }
    }
    // How far each part stands off: the most of the scan's vertices inside its footprint.
    for c in &mut cutouts {
        let (lo, hi) = c.ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
            (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
        });
        let sign = if c.side == Side::Out { 1.0 } else { -1.0 };
        for p in scan.positions {
            let h = sign * (*p - frame.origin).dot(normal);
            if h <= c.height {
                continue;
            }
            let q = to_plane(&frame, *p);
            if q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y && contains(&c.ring, q) {
                c.height = c.height.max(h);
            }
        }
    }
    let cut_area = cutouts.iter().map(|c| c.area).sum::<f64>().min(outline_area);
    let scan_edge = scan_edges(scan, &weld, &frame);
    Ok(Template { frame, outline: outline2, cutouts, scan_edge, area: outline_area, cut_area, found, specks, outside })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall in the XY plane, 400 × 300 mm, `n` squares a side, `z = f(x, y)`.
    fn wall_mesh(n: u32, f: impl Fn(f64, f64) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (400.0 * f64::from(i) / f64::from(n), 300.0 * f64::from(j) / f64::from(n));
                positions.push(Vec3::new(x, y, f(x, y)));
            }
        }
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        (positions, triangles)
    }

    fn box_at(x: f64, y: f64, w: f64, h: f64, z: f64) -> impl Fn(f64, f64) -> f64 {
        move |px, py| if (x..=x + w).contains(&px) && (y..=y + h).contains(&py) { z } else { 0.0 }
    }

    fn square(lo: f64, hi: f64) -> Vec<Vec3> {
        vec![Vec3::new(lo, lo, 0.0), Vec3::new(hi, lo, 0.0), Vec3::new(hi, hi, 0.0), Vec3::new(lo, hi, 0.0)]
    }

    /// A wall with an outlet box standing 25 mm off it: the template is the outline picked, exactly
    /// flat, with one cut-out the size of the box, and the scan's edge drawn.
    #[test]
    fn a_protrusion_becomes_a_cutout_and_the_scan_edge_is_shown() {
        let (positions, triangles) = wall_mesh(160, box_at(100.0, 100.0, 60.0, 80.0, 25.0));
        let scan = Scan::new(&positions, &triangles);
        let t = template(&scan, Vec3::ZERO, Vec3::Z, &square(20.0, 280.0), 3.0, Side::Both, LEAST_AREA).unwrap();
        assert_eq!(t.cutouts.len(), 1, "{:?}", t.cutouts.iter().map(|c| c.area).collect::<Vec<_>>());
        let c = &t.cutouts[0];
        // The box's sides are a ramp one cell wide, which the tolerance cuts a little out from its foot.
        assert!((c.area - 60.0 * 80.0).abs() < 0.12 * 60.0 * 80.0, "the box's footprint: {}", c.area);
        assert!((c.height - 25.0).abs() < 1e-9 && c.side == Side::Out && !c.crosses && !c.at_scan_edge, "{c:?}");
        assert!((t.area - 260.0 * 260.0).abs() < 1e-6);
        // The outline is flat by construction: every point back in space is on the plane.
        assert!(t.outline.iter().all(|p| t.at(*p).z.abs() < 1e-9));
        // The scan's own edge: one loop, the wall's 400 mm wide rectangle.
        assert_eq!(t.scan_edge.len(), 1);
        let (lo, hi) = t.scan_edge[0].iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.x), h.max(p.x)));
        assert!(((hi - lo) - 400.0).abs() < 1e-6, "{lo} {hi}");
    }

    /// Bumps under the tolerance and a speck under the least area are not cut.
    #[test]
    fn small_bumps_and_specks_are_not_cut() {
        let rough = |x: f64, y: f64| {
            0.4 * ((x * 0.9).sin() * (y * 1.1).cos()) + if (200.0..204.0).contains(&x) && (50.0..53.0).contains(&y) { 4.0 } else { 0.0 }
        };
        let (positions, triangles) = wall_mesh(200, rough);
        let scan = Scan::new(&positions, &triangles);
        let t = template(&scan, Vec3::ZERO, Vec3::Z, &square(10.0, 290.0), 3.0, Side::Both, LEAST_AREA).unwrap();
        assert!(t.cutouts.is_empty(), "{}", t.cutouts.len());
        assert!(t.specks >= 1, "the small bump is a speck: {}", t.specks);
    }

    /// A part that stands off outside the outline is left out; one across its edge is clipped to it
    /// (a convex outline).
    #[test]
    fn a_part_across_the_outline_is_clipped_and_one_outside_left_out() {
        let proud = |x: f64, y: f64| {
            let a = (250.0..330.0).contains(&x) && (100.0..160.0).contains(&y);
            let b = (330.0..380.0).contains(&x) && (200.0..260.0).contains(&y);
            if a || b { 20.0 } else { 0.0 }
        };
        let (positions, triangles) = wall_mesh(160, proud);
        let scan = Scan::new(&positions, &triangles);
        let t = template(&scan, Vec3::ZERO, Vec3::Z, &square(20.0, 280.0), 3.0, Side::Out, LEAST_AREA).unwrap();
        assert_eq!(t.cutouts.len(), 1, "{:?}", t.cutouts.iter().map(|c| c.area).collect::<Vec<_>>());
        assert_eq!(t.outside, 1);
        let c = &t.cutouts[0];
        assert!(c.crosses && c.clipped);
        let hi = c.ring.iter().fold(f64::MIN, |m, p| m.max(p.x));
        assert!(hi <= 280.0 + 1e-6, "clipped to the outline's edge at x = 280: {hi}");
        assert!((c.area - 30.0 * 60.0).abs() < 0.1 * 1800.0, "{}", c.area);
    }

    /// A recess is cut only when asked for.
    #[test]
    fn the_side_asked_for_is_the_side_cut() {
        let (positions, triangles) = wall_mesh(160, box_at(100.0, 100.0, 60.0, 80.0, -15.0));
        let scan = Scan::new(&positions, &triangles);
        let outline = square(20.0, 280.0);
        let go = |side| template(&scan, Vec3::ZERO, Vec3::Z, &outline, 3.0, side, LEAST_AREA).unwrap();
        let (both, out, inn) = (go(Side::Both), go(Side::Out), go(Side::In));
        assert_eq!((both.cutouts.len(), out.cutouts.len(), inn.cutouts.len()), (1, 0, 1));
        assert!((inn.cutouts[0].height - 15.0).abs() < 1e-9 && inn.cutouts[0].side == Side::In);
    }

    /// Hostile input: no panics, plain errors.
    #[test]
    fn hostile_input_is_refused() {
        let (positions, triangles) = wall_mesh(8, |_, _| 0.0);
        let scan = Scan::new(&positions, &triangles);
        let ok = square(10.0, 100.0);
        let go = |o: &[Vec3], tol: f64, n: Vec3| template(&scan, Vec3::ZERO, n, o, tol, Side::Both, LEAST_AREA);
        assert!(go(&ok[..2], 3.0, Vec3::Z).is_err(), "two points");
        assert!(go(&ok, 0.0, Vec3::Z).is_err() && go(&ok, f64::NAN, Vec3::Z).is_err() && go(&ok, -1.0, Vec3::Z).is_err());
        assert!(go(&ok, 3.0, Vec3::ZERO).is_err(), "no normal");
        let in_line = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(5.0, 0.0, 0.0), Vec3::new(9.0, 0.0, 0.0)];
        assert!(go(&in_line, 3.0, Vec3::Z).is_err(), "no area");
        let bow = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0)];
        assert!(go(&bow, 3.0, Vec3::Z).is_err(), "a bow tie crosses itself");
        let mut nan = ok.clone();
        nan[1] = Vec3::new(f64::NAN, 0.0, 0.0);
        assert!(go(&nan, 3.0, Vec3::Z).is_err());
        let empty = Scan::new(&positions, &[]);
        assert!(template(&empty, Vec3::ZERO, Vec3::Z, &ok, 3.0, Side::Both, 1.0).is_err());
        assert!(go(&ok, 3.0, Vec3::Z).unwrap().cutouts.is_empty(), "a flat wall has nothing to cut");
    }

    /// Picks off the plane are put on it.
    #[test]
    fn picks_off_the_plane_are_put_on_it() {
        let (positions, triangles) = wall_mesh(8, |_, _| 0.0);
        let scan = Scan::new(&positions, &triangles);
        let lifted = vec![Vec3::new(10.0, 10.0, 2.0), Vec3::new(100.0, 10.0, -1.0), Vec3::new(100.0, 90.0, 3.0), Vec3::new(10.0, 90.0, 1.0)];
        let t = template(&scan, Vec3::ZERO, Vec3::Z, &lifted, 3.0, Side::Both, LEAST_AREA).unwrap();
        assert!(t.outline.iter().all(|p| t.at(*p).z.abs() < 1e-9));
        assert!((t.area - 90.0 * 80.0).abs() < 1e-6, "{}", t.area);
    }

    /// The wall is found under the outline with the outlet left out, tilted or not, and the
    /// normal turned toward the protrusion.
    #[test]
    fn the_wall_is_fitted_without_its_protrusions() {
        let tilt = |x: f64, y: f64| 0.05 * x + box_at(100.0, 100.0, 60.0, 80.0, 25.0)(x, y);
        let (positions, triangles) = wall_mesh(120, tilt);
        let scan = Scan::new(&positions, &triangles);
        let found = wall(&scan, &square(20.0, 280.0)).unwrap();
        let want = Vec3::new(0.05, 0.0, -1.0).normalized().unwrap();
        // The outlet stands off the +z side: the normal points that way, square to the tilted wall.
        assert!(found.normal.dot(-want) > 1.0 - 1e-6, "{:?}", found.normal);
        assert!(found.kept < found.under, "the outlet's points left out: {} of {}", found.kept, found.under);
        assert!((1.0..=2.0).contains(&found.tolerance), "a clean wall has the least tolerance: {}", found.tolerance);
        let flat = |p: Vec3| (p - found.point).dot(found.normal);
        assert!(positions.iter().filter(|p| (30.0..90.0).contains(&p.x) && (30.0..90.0).contains(&p.y)).all(|p| flat(*p).abs() < 1e-6));
        assert!(wall(&scan, &square(20.0, 280.0)[..2]).is_none());
    }
}
