//! A wall's outline made into a straight panel cut from sheets: what is proposed, for someone to
//! look through and confirm one by one, and the panel those choices make.
//!
//! From the whole wall's outline ([`crate::wall_outline`]) on the plane it rests on:
//! - four straight edges, a rectangle square to the outline's sides, each where the outline lies
//!   clear of the innermost level the outline runs along for a good share of that side (a tear in
//!   the scan or a short step is a notch, not a reason to move the whole edge);
//! - the cut-outs the rectangle then needs: each stretch where the outline turns in from an edge (a
//!   wheel well, a pillar's bracket, a slot for a ceiling bow, a raised corner) and each cut-out
//!   inside it;
//! - the frame's holes, from the scan's open edges: the small round rings lying flat a little
//!   behind the panel (holes in the frame to fasten it through), with their centres from a circle
//!   fitted to each ring;
//! - seams where the sheets meet, along the rows and columns those holes make, so the sheets on
//!   both sides of a seam can be fastened; as few as keep every piece within the sheet.
//!
//! [`lay`] makes the panel from what is kept: a cut-out dropped is covered by the straight edge, a
//! seam dropped joins two pieces into one, a hole dropped is not drawn.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::holes::survey;
use crate::profile::{area as twice_area, contains};
use crate::template::Template;

/// The default sheet: 2440 × 1220 mm (8 × 4 ft).
pub const SHEET: [f64; 2] = [2440.0, 1220.0];

/// A level of the outline along a side counts where it runs along this share of the side, its
/// points within this of one another (mm).
const LEVEL_SHARE: f64 = 0.15;
const LEVEL_SPREAD: f64 = 2.0;

/// How far in from the outline's box a side of the outline still counts as lying along that edge
/// (mm); farther in it is a notch (a wheel well), not where the edge is.
const EDGE_BAND: f64 = 150.0;

/// Notches shallower than this are not cut: the straight edge goes over them (mm).
pub const LEAST_NOTCH: f64 = 8.0;

/// Holes this close to an edge or a seam (centre, mm) are proposed: they fasten that edge.
pub const HOLE_BAND: f64 = 100.0;

/// The least wood between a hole and the edge (mm): closer, the sheet would split.
const HOLE_WOOD: f64 = 8.0;

/// The frame's holes: their size (mm) and how far behind the panel's face they may be (mm).
const HOLE_SMALLEST: f64 = 6.0;
const HOLE_LARGEST: f64 = 30.0;
const HOLE_DEEPEST: f64 = 60.0;
const HOLE_PROUD: f64 = 30.0;

/// The most holes, seams and cut-outs proposed.
pub const MOST_HOLES: usize = 2_000;
pub const MOST_SEAMS: usize = 8;
pub const MOST_CUTOUTS: usize = 500;

/// A row of holes needs this many, spread over this share of the panel.
const ROW_HOLES: usize = 5;
const ROW_SPREAD: f64 = 0.25;
const COLUMN_HOLES: usize = 4;
const COLUMN_SPREAD: f64 = 0.15;

/// Two rows this far apart are a pair a seam runs between (mm); a seam beside a single row runs
/// this far from it.
const PAIR_NEAREST: f64 = 20.0;
const PAIR_FARTHEST: f64 = 80.0;
const BESIDE_ROW: f64 = 20.0;

/// The narrowest piece a seam may leave (mm).
const NARROWEST_PIECE: f64 = 150.0;

/// One of the frame's holes, on the panel's plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hole {
    /// Its centre, in the template's coordinates, and its diameter (mm).
    pub at: Vec2,
    pub diameter: f64,
    /// How far its rim lies behind the panel's back face (mm; less than 0: that far proud of it).
    pub behind: f64,
}

/// The frame's holes in the scan, on `frame` (whose `x × y` points out of the wall, to the room).
pub fn frame_holes(scan: &Scan, frame: &Plane) -> Vec<Hole> {
    let Some(normal) = frame.x.cross(frame.y).normalized() else { return Vec::new() };
    let rings = survey(scan).rings;
    let mut out = Vec::new();
    for ring in rings.iter().filter(|r| !r.rim) {
        let perimeter = ring.perimeter;
        if !(std::f64::consts::PI * HOLE_SMALLEST * 0.9..=std::f64::consts::PI * HOLE_LARGEST * 1.2).contains(&perimeter) {
            continue;
        }
        let pts: Vec<Vec3> = ring.points.iter().filter_map(|&i| scan.positions.get(i as usize).copied()).filter(|p| p.is_finite()).collect();
        if pts.len() < 6 || pts.len() != ring.points.len() {
            continue;
        }
        let flat: Vec<Vec2> = pts.iter().map(|p| to_plane(frame, *p)).collect();
        let off: Vec<f64> = pts.iter().map(|p| (*p - frame.origin).dot(normal)).collect();
        let mean_off = off.iter().sum::<f64>() / off.len() as f64;
        let spread = (off.iter().map(|o| (o - mean_off).powi(2)).sum::<f64>() / off.len() as f64).sqrt();
        // A hole in a face square to the plane (a rim seen edge on) or deep in an opening is not one
        // to fasten through.
        if spread > 4.0 || !(-mean_off <= HOLE_DEEPEST && -mean_off >= -HOLE_PROUD) {
            continue;
        }
        let area = twice_area(&flat).abs() / 2.0;
        let length: f64 = (0..flat.len()).map(|k| flat[k].dist(flat[(k + 1) % flat.len()])).sum();
        if !(length > 0.0) || 4.0 * std::f64::consts::PI * area / (length * length) < 0.6 {
            continue;
        }
        let Some((centre, radius, rms)) = fit_circle(&flat) else { continue };
        let diameter = 2.0 * radius;
        if !(HOLE_SMALLEST..=HOLE_LARGEST).contains(&diameter) || rms > 0.15 * diameter {
            continue;
        }
        out.push(Hole { at: centre, diameter, behind: -mean_off });
        if out.len() >= MOST_HOLES {
            break;
        }
    }
    out.sort_by(|a, b| (a.at.y, a.at.x).partial_cmp(&(b.at.y, b.at.x)).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn to_plane(f: &Plane, p: Vec3) -> Vec2 {
    let d = p - f.origin;
    Vec2::new(d.dot(f.x), d.dot(f.y))
}

/// The circle through `pts` by least squares (Kåsa): centre, radius and the rms of the points off it.
fn fit_circle(pts: &[Vec2]) -> Option<(Vec2, f64, f64)> {
    let n = pts.len() as f64;
    let c = pts.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) / n;
    // Centred: u² + v² = 2a·u + 2b·v + k.
    let (mut suu, mut suv, mut svv, mut su3, mut sv3, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (u, v) = (p.x - c.x, p.y - c.y);
        suu += u * u;
        suv += u * v;
        svv += v * v;
        su3 += u * u * u;
        sv3 += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if !(det.abs() > 1e-12) {
        return None;
    }
    let r1 = 0.5 * (su3 + suvv);
    let r2 = 0.5 * (sv3 + svuu);
    let a = (r1 * svv - r2 * suv) / det;
    let b = (r2 * suu - r1 * suv) / det;
    let radius = (a * a + b * b + (suu + svv) / n).sqrt();
    let centre = Vec2::new(c.x + a, c.y + b);
    let rms = (pts.iter().map(|p| (p.dist(centre) - radius).powi(2)).sum::<f64>() / n).sqrt();
    (radius.is_finite() && centre.x.is_finite() && centre.y.is_finite()).then_some((centre, radius, rms))
}

/// The template turned in its plane by `quarters` quarter turns counter-clockwise (seen from the
/// side its normal points to), and the holes with it: the same places in space, the side that was
/// on the left now at the bottom for one quarter turn.
pub fn turned(made: &Template, holes: &[Hole], quarters: u8) -> (Template, Vec<Hole>) {
    let k = quarters % 4;
    // (x, y) -> the turned coordinates, and the frame's axes so each point stays in place.
    let turn = |p: Vec2| match k {
        1 => Vec2::new(-p.y, p.x),
        2 => Vec2::new(-p.x, -p.y),
        3 => Vec2::new(p.y, -p.x),
        _ => p,
    };
    let (x, y) = (made.frame.x, made.frame.y);
    let (nx, ny) = match k {
        1 => (y * -1.0, x),
        2 => (x * -1.0, y * -1.0),
        3 => (y, x * -1.0),
        _ => (x, y),
    };
    let ring = |r: &[Vec2]| r.iter().map(|p| turn(*p)).collect::<Vec<_>>();
    let mut out = made.clone();
    out.frame = Plane { origin: made.frame.origin, x: nx, y: ny };
    out.outline = ring(&made.outline);
    for c in &mut out.cutouts {
        c.ring = ring(&c.ring);
    }
    out.scan_edge = made.scan_edge.iter().map(|l| ring(l)).collect();
    let holes = holes.iter().map(|h| Hole { at: turn(h.at), ..*h }).collect();
    (out, holes)
}

/// The quarter turns that put `up` (a direction in space) most nearly up the template.
pub fn quarters_for(made: &Template, up: Vec3) -> u8 {
    let (x, y) = (made.frame.x, made.frame.y);
    // Up the template after k quarter turns is its new y: y, x, -y, -x.
    let ups = [y, x, y * -1.0, x * -1.0];
    (0..4u8).max_by(|a, b| ups[*a as usize].dot(up).partial_cmp(&ups[*b as usize].dot(up)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0)
}

/// The panel's straight edges, in the template's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edges {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
}

impl Edges {
    fn ok(&self) -> bool {
        [self.left, self.right, self.bottom, self.top].iter().all(|v| v.is_finite()) && self.right > self.left && self.top > self.bottom
    }
    fn corners(&self) -> [Vec2; 4] {
        [Vec2::new(self.left, self.bottom), Vec2::new(self.right, self.bottom), Vec2::new(self.right, self.top), Vec2::new(self.left, self.top)]
    }
    fn width(&self) -> f64 {
        self.right - self.left
    }
    fn height(&self) -> f64 {
        self.top - self.bottom
    }
}

/// Where the edges go for `outline` (counter-clockwise, its sides squared to the axes): each at the
/// innermost level the outline runs along for [`LEVEL_SHARE`] of the side, the stretches more than
/// [`EDGE_BAND`] in from the outline's box not counted. On the grid `round` (0: as found).
pub fn edges_for(outline: &[Vec2], round: f64) -> Option<Edges> {
    let (lo, hi) = bounds(outline)?;
    let step = 2.0f64.max((hi.x - lo.x).max(hi.y - lo.y) / 4000.0);
    // At each sample across, the outline's extent along the other axis.
    let along = |horizontal: bool, at: f64| -> Option<(f64, f64)> {
        let mut hits: Vec<f64> = Vec::new();
        for k in 0..outline.len() {
            let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
            let (a0, a1, b0, b1) = if horizontal { (a.x, a.y, b.x, b.y) } else { (a.y, a.x, b.y, b.x) };
            if (a0 <= at && at < b0) || (b0 <= at && at < a0) {
                let t = (at - a0) / (b0 - a0);
                hits.push(a1 + t * (b1 - a1));
            }
        }
        let low = hits.iter().copied().fold(f64::INFINITY, f64::min);
        let high = hits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (low.is_finite() && high.is_finite()).then_some((low, high))
    };
    let samples = |horizontal: bool| -> Vec<(f64, f64)> {
        let (from, to) = if horizontal { (lo.x, hi.x) } else { (lo.y, hi.y) };
        let n = (((to - from) / step) as usize).clamp(1, 20_000);
        (0..n).filter_map(|i| along(horizontal, from + (i as f64 + 0.5) * (to - from) / n as f64)).collect()
    };
    let across_x = samples(true);
    let across_y = samples(false);
    // Along each side the outline runs at levels (a floor, a step, a raised stretch): the edge
    // goes to the innermost level the outline runs along for [`EDGE_SHARE`] of the side, the
    // stretches more than [`EDGE_BAND`] in (a wheel well) not counted, a slope never a level.
    let pick = |values: Vec<f64>, outer: f64, inward: f64| -> f64 {
        let mut near: Vec<f64> = values.into_iter().map(|v| (v - outer) * inward).filter(|d| *d <= EDGE_BAND).collect();
        if near.is_empty() {
            return outer;
        }
        near.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let least = (near.len() as f64 * LEVEL_SHARE).max(1.0);
        let mut best = 0.0f64;
        let mut i = 0;
        while i < near.len() {
            let mut j = i + 1;
            while j < near.len() && near[j] - near[i] <= LEVEL_SPREAD {
                j += 1;
            }
            if (j - i) as f64 >= least {
                best = best.max(near[j - 1]);
            }
            i = j;
        }
        outer + best * inward
    };
    let mut e = Edges {
        bottom: pick(across_x.iter().map(|s| s.0).collect(), lo.y, 1.0),
        top: pick(across_x.iter().map(|s| s.1).collect(), hi.y, -1.0),
        left: pick(across_y.iter().map(|s| s.0).collect(), lo.x, 1.0),
        right: pick(across_y.iter().map(|s| s.1).collect(), hi.x, -1.0),
    };
    if round > 0.0 && round.is_finite() {
        // In whole steps, never out past where the edge was found.
        e.left = lo.x + ((e.left - lo.x) / round - 1e-9).ceil() * round;
        e.bottom = lo.y + ((e.bottom - lo.y) / round - 1e-9).ceil() * round;
        e.right = lo.x + ((e.right - lo.x) / round + 1e-9).floor() * round;
        e.top = lo.y + ((e.top - lo.y) / round + 1e-9).floor() * round;
    }
    e.ok().then_some(e)
}

fn bounds(ring: &[Vec2]) -> Option<(Vec2, Vec2)> {
    let (lo, hi) = ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    (ring.len() >= 3 && hi.x > lo.x && hi.y > lo.y).then_some((lo, hi))
}

/// The outline clipped to the rectangle (Sutherland–Hodgman against each side), points on a side
/// put exactly on it.
fn clip(outline: &[Vec2], e: &Edges) -> Vec<Vec2> {
    let mut ring: Vec<Vec2> = outline.to_vec();
    // (axis x?, value, keep the side above?)
    for (on_x, v, above) in [(true, e.left, true), (true, e.right, false), (false, e.bottom, true), (false, e.top, false)] {
        if ring.is_empty() {
            break;
        }
        let inside = |p: Vec2| {
            let c = if on_x { p.x } else { p.y };
            if above { c >= v } else { c <= v }
        };
        let cross = |a: Vec2, b: Vec2| {
            let (ca, cb) = if on_x { (a.x, b.x) } else { (a.y, b.y) };
            let t = (v - ca) / (cb - ca);
            let p = a + (b - a) * t;
            if on_x { Vec2::new(v, p.y) } else { Vec2::new(p.x, v) }
        };
        let mut out = Vec::with_capacity(ring.len() + 4);
        for k in 0..ring.len() {
            let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
            match (inside(a), inside(b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cross(a, b)),
                (false, true) => {
                    out.push(cross(a, b));
                    out.push(b);
                }
                (false, false) => {}
            }
        }
        ring = out;
    }
    // Repeated points out.
    let mut clean: Vec<Vec2> = Vec::with_capacity(ring.len());
    for p in ring {
        if clean.last().is_none_or(|q: &Vec2| q.dist(p) > 1e-6) {
            clean.push(p);
        }
    }
    while clean.len() > 1 && clean.first().zip(clean.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6) {
        clean.pop();
    }
    clean
}

/// Which side of the rectangle a point lies on: 0 bottom, 1 right, 2 top, 3 left (the first, at a
/// corner, going round counter-clockwise from the bottom-left), or none.
fn side_of(p: Vec2, e: &Edges) -> Option<usize> {
    const ON: f64 = 1e-6;
    if (p.y - e.bottom).abs() <= ON {
        Some(0)
    } else if (p.x - e.right).abs() <= ON {
        Some(1)
    } else if (p.y - e.top).abs() <= ON {
        Some(2)
    } else if (p.x - e.left).abs() <= ON {
        Some(3)
    } else {
        None
    }
}

fn on_side(p: Vec2, e: &Edges, s: usize) -> bool {
    const ON: f64 = 1e-6;
    match s {
        0 => (p.y - e.bottom).abs() <= ON,
        1 => (p.x - e.right).abs() <= ON,
        2 => (p.y - e.top).abs() <= ON,
        _ => (p.x - e.left).abs() <= ON,
    }
}

/// How far round the rectangle a point on it is, counter-clockwise from the bottom-left corner.
fn round_at(p: Vec2, e: &Edges) -> f64 {
    let (w, h) = (e.width(), e.height());
    match side_of(p, e) {
        Some(0) => p.x - e.left,
        Some(1) => w + p.y - e.bottom,
        Some(2) => w + h + e.right - p.x,
        _ => 2.0 * w + h + e.top - p.y,
    }
}

/// The rectangle's corners passed going counter-clockwise from `a` to `b` (both on it).
fn corners_between(a: Vec2, b: Vec2, e: &Edges) -> Vec<Vec2> {
    let (w, h) = (e.width(), e.height());
    let total = 2.0 * (w + h);
    let (sa, mut sb) = (round_at(a, e), round_at(b, e));
    if sb <= sa {
        sb += total;
    }
    let c = e.corners();
    let at = [0.0, w, w + h, 2.0 * w + h];
    let mut out = Vec::new();
    for lap in [0.0, total] {
        for (k, s) in at.iter().enumerate() {
            let s = s + lap;
            if s > sa + 1e-9 && s < sb - 1e-9 {
                out.push(c[k]);
            }
        }
    }
    out
}

/// A cut-out proposed.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// Its outline, counter-clockwise, in the template's coordinates.
    pub ring: Vec<Vec2>,
    /// A notch in an edge (rather than a cut-out inside the panel), and which edges it opens on (0
    /// bottom, 1 right, 2 top, 3 left).
    pub notch: bool,
    pub edges: Vec<usize>,
    /// How far it reaches in from the edge (mm; a cut-out inside: its height), its area (mm²).
    pub depth: f64,
    pub area: f64,
    /// For a notch: where it leaves the clipped outline and where it comes back (indices).
    from: usize,
    to: usize,
}

/// A seam proposed: a straight line across the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    /// Its ends, on the rectangle (template coordinates).
    pub from: Vec2,
    pub to: Vec2,
    /// Up the panel (a column of pieces either side) rather than across.
    pub upright: bool,
    /// It runs between two rows of holes (each piece gets one), rather than beside one.
    pub between_rows: bool,
}

/// Everything proposed for a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub edges: Edges,
    /// The outline clipped to the rectangle.
    clipped: Vec<Vec2>,
    pub cuts: Vec<Cut>,
    pub seams: Vec<Seam>,
    pub holes: Vec<Hole>,
    /// Notches shallower than [`LEAST_NOTCH`] the straight edges go over, and how deep the deepest is.
    pub flattened: usize,
    pub flattened_depth: f64,
    /// The sheet asked for, and how far the longest piece is past it (0: they all fit).
    pub sheet: [f64; 2],
    pub over: f64,
}

/// The proposal for `made` (a whole wall's outline): its straight edges (or `edges`), the cut-outs
/// they need, seams for sheets of `sheet` (long × short, mm) along the rows of `holes`, and the
/// holes near the edges and seams.
pub fn propose(made: &Template, holes: &[Hole], edges: Option<Edges>, sheet: [f64; 2], round: f64) -> Option<Proposal> {
    let edges = match edges {
        Some(e) if e.ok() => e,
        Some(_) => return None,
        None => edges_for(&made.outline, round)?,
    };
    if !(sheet.iter().all(|s| s.is_finite() && *s > 0.0)) {
        return None;
    }
    let clipped = clip(&made.outline, &edges);
    if clipped.len() < 3 {
        return None;
    }
    let (mut cuts, flattened, flattened_depth) = notches(&clipped, &edges);
    for c in made.cutouts.iter().filter(|c| c.ring.len() >= 3) {
        let ring = clip(&c.ring, &edges);
        let Some((lo, hi)) = bounds(&ring) else { continue };
        if cuts.len() >= MOST_CUTOUTS {
            break;
        }
        // A cut-out that reaches the edge is part of a notch already.
        if ring.iter().any(|p| side_of(*p, &edges).is_some()) {
            continue;
        }
        cuts.push(Cut { area: twice_area(&ring).abs() / 2.0, ring, notch: false, edges: Vec::new(), depth: hi.y - lo.y, from: 0, to: 0 });
    }
    let panel = laid_outline(&clipped, &cuts, &vec![true; cuts.len()], &edges);
    let inside: Vec<Hole> = holes.iter().copied().filter(|h| contains(&panel, h.at)).collect();
    let (seams, over) = seams_for(&inside, &edges, sheet);
    let near: Vec<Hole> = inside
        .into_iter()
        .filter(|h| {
            let to_edge = ring_distance(&panel, h.at);
            let to_seam = seams.iter().map(|s| segment_distance(h.at, s.from, s.to)).fold(f64::INFINITY, f64::min);
            let reach = to_edge.min(to_seam);
            reach <= HOLE_BAND && reach - h.diameter / 2.0 >= HOLE_WOOD
        })
        .collect();
    Some(Proposal { edges, clipped, cuts, seams, holes: near, flattened, flattened_depth, sheet, over })
}

/// The notches of the clipped outline: each run of it off the rectangle's sides, with the stretch
/// of the rectangle it replaces; and how many shallower than [`LEAST_NOTCH`] were left to the edge.
fn notches(clipped: &[Vec2], e: &Edges) -> (Vec<Cut>, usize, f64) {
    let n = clipped.len();
    let seg_on = |k: usize| {
        let (a, b) = (clipped[k], clipped[(k + 1) % n]);
        (0..4).any(|s| on_side(a, e, s) && on_side(b, e, s))
    };
    // Start at a point on the rectangle whose next side runs along it.
    let Some(start) = (0..n).find(|&k| seg_on(k)) else { return (Vec::new(), 0, 0.0) };
    let mut cuts = Vec::new();
    let (mut flattened, mut deepest) = (0, 0.0f64);
    let mut k = 0;
    while k < n {
        let i = (start + k) % n;
        if seg_on(i) || side_of(clipped[i], e).is_none() {
            k += 1;
            continue;
        }
        // A run off the sides from point i until it is back on one.
        let mut len = 1;
        while len < n && side_of(clipped[(i + len) % n], e).is_none() {
            len += 1;
        }
        let j = (i + len) % n;
        let detour: Vec<Vec2> = (0..=len).map(|t| clipped[(i + t) % n]).collect();
        let depth = detour.iter().map(|p| distance_in(*p, e)).fold(0.0, f64::max);
        if depth < LEAST_NOTCH {
            flattened += 1;
            deepest = deepest.max(depth);
        } else if cuts.len() < MOST_CUTOUTS {
            let mut ring = detour.clone();
            let mut back = corners_between(clipped[i], clipped[j], e);
            back.reverse();
            ring.extend(back);
            let area2 = twice_area(&ring);
            if area2 < 0.0 {
                ring.reverse();
            }
            let mut edges: Vec<usize> = [side_of(clipped[i], e), side_of(clipped[j], e)].into_iter().flatten().collect();
            edges.sort_unstable();
            edges.dedup();
            cuts.push(Cut { ring, notch: true, edges, depth, area: area2.abs() / 2.0, from: i, to: j });
        }
        k += len;
    }
    (cuts, flattened, deepest)
}

/// How far in from the rectangle's nearest side a point inside it is.
fn distance_in(p: Vec2, e: &Edges) -> f64 {
    (p.x - e.left).min(e.right - p.x).min(p.y - e.bottom).min(e.top - p.y).max(0.0)
}

/// The panel's outline: the clipped outline with each notch not kept replaced by the rectangle's
/// side it opens on.
fn laid_outline(clipped: &[Vec2], cuts: &[Cut], keep: &[bool], e: &Edges) -> Vec<Vec2> {
    let n = clipped.len();
    let mut replace: Vec<Option<(usize, Vec<Vec2>)>> = vec![None; n];
    for (c, kept) in cuts.iter().zip(keep) {
        if c.notch && !kept {
            let len = (c.to + n - c.from) % n;
            if let (Some(a), Some(b)) = (clipped.get(c.from), clipped.get(c.to)) {
                replace[c.from] = Some((len, corners_between(*a, *b, e)));
            }
        }
    }
    let mut out = Vec::with_capacity(n + 8);
    let mut k = 0;
    // Start where no notch is open, so each replacement is whole.
    let start = (0..n).find(|&i| !in_any(i, cuts, n)).unwrap_or(0);
    while k < n {
        let i = (start + k) % n;
        out.push(clipped[i]);
        if let Some((len, between)) = &replace[i] {
            out.extend(between.iter().copied());
            k += (*len).max(1);
        } else {
            k += 1;
        }
    }
    // Points in a straight line on one side folded away.
    let mut clean: Vec<Vec2> = Vec::with_capacity(out.len());
    for p in out {
        if clean.last().is_none_or(|q: &Vec2| q.dist(p) > 1e-6) {
            clean.push(p);
        }
    }
    let mut changed = true;
    while changed && clean.len() > 3 {
        changed = false;
        for k in 0..clean.len() {
            let m = clean.len();
            let (a, b, c) = (clean[(k + m - 1) % m], clean[k], clean[(k + 1) % m]);
            if ((b - a).x * (c - b).y - (b - a).y * (c - b).x).abs() <= 1e-9 * (a.dist(b) * b.dist(c)).max(1e-12) {
                clean.remove(k);
                changed = true;
                break;
            }
        }
    }
    clean
}

fn in_any(i: usize, cuts: &[Cut], n: usize) -> bool {
    cuts.iter().filter(|c| c.notch).any(|c| {
        let len = (c.to + n - c.from) % n;
        let off = (i + n - c.from) % n;
        off > 0 && off < len
    })
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.x * d.x + d.y * d.y;
    if l2 <= 0.0 {
        return p.dist(a);
    }
    let t = (((p - a).x * d.x + (p - a).y * d.y) / l2).clamp(0.0, 1.0);
    p.dist(a + d * t)
}

fn ring_distance(ring: &[Vec2], p: Vec2) -> f64 {
    (0..ring.len()).map(|k| segment_distance(p, ring[k], ring[(k + 1) % ring.len()])).fold(f64::INFINITY, f64::min)
}

/// Lines of holes running nearly along the x axis (`upright` false) or the y axis: the slope that
/// lines most of them up, and each line's offset (at 0 along) and how many holes are on it.
fn lines_of(holes: &[Hole], upright: bool, least: usize, spread: f64) -> (f64, Vec<(f64, usize)>) {
    let pts: Vec<(f64, f64)> = holes.iter().map(|h| if upright { (h.at.y, h.at.x) } else { (h.at.x, h.at.y) }).collect();
    if pts.len() < least {
        return (0.0, Vec::new());
    }
    const BIN: f64 = 4.0;
    let mut best: (f64, f64) = (f64::NEG_INFINITY, 0.0);
    for k in -150..=150 {
        let slope = (k as f64 * 0.02f64).to_radians().tan();
        let mut bins: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for (a, c) in &pts {
            *bins.entry(((c - slope * a) / BIN).floor() as i64).or_insert(0) += 1;
        }
        let score: f64 = bins.values().map(|&v| (v * v) as f64).sum();
        // Of slopes that line the holes up as well, the squarest.
        if score > best.0 || (score == best.0 && slope.abs() < best.1.abs()) {
            best = (score, slope);
        }
    }
    let slope = best.1;
    let mut off: Vec<(f64, f64)> = pts.iter().map(|(a, c)| (c - slope * a, *a)).collect();
    off.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), (a, _)| (lo.min(*a), hi.max(*a)));
    let span = (hi - lo).max(1.0);
    let mut lines = Vec::new();
    let mut i = 0;
    while i < off.len() {
        let mut j = i + 1;
        while j < off.len() && off[j].0 - off[j - 1].0 <= BIN {
            j += 1;
        }
        let group = &off[i..j];
        let (a0, a1) = group.iter().fold((f64::MAX, f64::MIN), |(lo, hi), (_, a)| (lo.min(*a), hi.max(*a)));
        if group.len() >= least && (a1 - a0) >= spread * span {
            let mid = group[group.len() / 2].0;
            lines.push((mid, group.len()));
        }
        i = j;
    }
    (slope, lines)
}

/// Seams across (along the rows of holes) and up (along their columns), as few as keep each piece
/// within `sheet`; and how far the longest piece is past the sheet when no choice fits.
fn seams_for(holes: &[Hole], e: &Edges, sheet: [f64; 2]) -> (Vec<Seam>, f64) {
    let (long, short) = (sheet[0].max(sheet[1]), sheet[0].min(sheet[1]));
    // The panel's long side along the sheet's.
    let (most_w, most_h) = if e.width() >= e.height() { (long, short) } else { (short, long) };
    let mut seams = Vec::new();
    let mut over = 0.0f64;
    for upright in [false, true] {
        let (least, spread) = if upright { (COLUMN_HOLES, COLUMN_SPREAD) } else { (ROW_HOLES, ROW_SPREAD) };
        let (slope, lines) = lines_of(holes, upright, least, spread);
        // Along the seam: from..to; across: low..high, the most a piece may be.
        let (a0, a1, low, high, most) = if upright { (e.bottom, e.top, e.left, e.right, most_w) } else { (e.left, e.right, e.bottom, e.top, most_h) };
        // Candidates: (offset at 0 along, runs between two lines).
        let mut cands: Vec<(f64, bool)> = Vec::new();
        for (k, (c, _)) in lines.iter().enumerate() {
            cands.push((c - BESIDE_ROW, false));
            cands.push((c + BESIDE_ROW, false));
            if let Some((d, _)) = lines.get(k + 1)
                && (PAIR_NEAREST..=PAIR_FARTHEST).contains(&(d - c))
            {
                cands.push(((c + d) / 2.0, true));
            }
        }
        // Where the seam is across the panel, at its two ends.
        let at = |c: f64, along: f64| c + slope * along;
        let cands: Vec<(f64, bool)> = cands
            .into_iter()
            .filter(|(c, _)| {
                let (p, q) = (at(*c, a0), at(*c, a1));
                p.min(q) >= low + NARROWEST_PIECE && p.max(q) <= high - NARROWEST_PIECE
            })
            .collect();
        // The longest piece across for a choice of seams (sorted).
        let longest = |chosen: &[(f64, bool)]| -> f64 {
            let mut worst = 0.0f64;
            for along in [a0, a1] {
                let mut prev = low;
                for (c, _) in chosen {
                    let x = at(*c, along);
                    worst = worst.max(x - prev);
                    prev = x;
                }
                worst = worst.max(high - prev);
            }
            worst
        };
        let total = high - low;
        let mut best: Option<(f64, Vec<(f64, bool)>)> = None;
        let mut consider = |chosen: Vec<(f64, bool)>| {
            let past = (longest(&chosen) - most).max(0.0);
            let between = chosen.iter().filter(|(_, b)| *b).count() as f64;
            let score = 1000.0 * chosen.len() as f64 + 20.0 * past - 10.0 * between;
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, chosen));
            }
        };
        if total <= most {
            consider(Vec::new());
        } else {
            let n = cands.len().min(40);
            let mut sorted = cands[..n].to_vec();
            sorted.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
            let apart = |s: &[(f64, bool)]| s.windows(2).all(|w| w[1].0 - w[0].0 >= NARROWEST_PIECE);
            for i in 0..n {
                consider(vec![sorted[i]]);
                for j in i + 1..n {
                    let two = vec![sorted[i], sorted[j]];
                    if apart(&two) {
                        consider(two);
                    }
                    for k in j + 1..n {
                        let three = vec![sorted[i], sorted[j], sorted[k]];
                        if apart(&three) {
                            consider(three);
                        }
                    }
                }
            }
        }
        if let Some((_, chosen)) = best {
            over = over.max((longest(&chosen) - most).max(0.0));
            for (c, between_rows) in chosen.into_iter().take(MOST_SEAMS) {
                let (p, q) = (at(c, a0), at(c, a1));
                let (from, to) = if upright { (Vec2::new(p, a0), Vec2::new(q, a1)) } else { (Vec2::new(a0, p), Vec2::new(a1, q)) };
                seams.push(Seam { from, to, upright, between_rows });
            }
        } else {
            over = over.max((total - most).max(0.0));
        }
    }
    (seams, over)
}

/// The panel laid out from what is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Laid {
    /// Its outline, counter-clockwise (template coordinates).
    pub outline: Vec<Vec2>,
    /// The cut-outs inside it kept.
    pub inside: Vec<Vec<Vec2>>,
    /// Each seam kept, as the stretches of it over the panel.
    pub seams: Vec<Vec<(Vec2, Vec2)>>,
    /// The holes kept that are in the panel: centre and diameter.
    pub holes: Vec<(Vec2, f64)>,
}

/// The panel from `proposal` with the cut-outs, seams and holes marked kept (a mark missing: kept).
/// The seams may be given moved (`seams`, in place of the proposal's).
pub fn lay(proposal: &Proposal, keep_cuts: &[bool], keep_seams: &[bool], keep_holes: &[bool], seams: Option<&[Seam]>) -> Laid {
    let kept = |marks: &[bool], i: usize| marks.get(i).copied().unwrap_or(true);
    let keep: Vec<bool> = (0..proposal.cuts.len()).map(|i| kept(keep_cuts, i)).collect();
    let outline = laid_outline(&proposal.clipped, &proposal.cuts, &keep, &proposal.edges);
    let inside: Vec<Vec<Vec2>> = proposal.cuts.iter().zip(&keep).filter(|(c, k)| !c.notch && **k).map(|(c, _)| c.ring.clone()).collect();
    let all_seams = seams.unwrap_or(&proposal.seams);
    let seams: Vec<Vec<(Vec2, Vec2)>> = all_seams
        .iter()
        .enumerate()
        .filter(|(i, _)| kept(keep_seams, *i))
        .map(|(_, s)| over_ring(&outline, s.from, s.to))
        .filter(|r| !r.is_empty())
        .collect();
    let holes = proposal
        .holes
        .iter()
        .enumerate()
        .filter(|(i, h)| kept(keep_holes, *i) && contains(&outline, h.at) && !inside.iter().any(|r| contains(r, h.at)))
        .map(|(_, h)| (h.at, h.diameter))
        .collect();
    Laid { outline, inside, seams, holes }
}

/// The stretches of the segment a–b inside `ring`.
fn over_ring(ring: &[Vec2], a: Vec2, b: Vec2) -> Vec<(Vec2, Vec2)> {
    let d = b - a;
    let mut ts = vec![0.0, 1.0];
    for k in 0..ring.len() {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        let r = q - p;
        let den = d.x * r.y - d.y * r.x;
        if den.abs() <= 1e-12 {
            continue;
        }
        let t = ((p - a).x * r.y - (p - a).y * r.x) / den;
        let u = ((p - a).x * d.y - (p - a).y * d.x) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            ts.push(t);
        }
    }
    ts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    ts.dedup_by(|x, y| (*x - *y).abs() <= 1e-9);
    ts.windows(2).filter(|w| w[1] - w[0] > 1e-6 && contains(ring, a + d * ((w[0] + w[1]) / 2.0))).map(|w| (a + d * w[0], a + d * w[1])).collect()
}

/// Each piece's size between the seams kept (its box: width × height, mm), for checking against
/// the sheet.
pub fn piece_sizes(proposal: &Proposal, keep_seams: &[bool], seams: Option<&[Seam]>) -> Vec<[f64; 2]> {
    let e = &proposal.edges;
    let all = seams.unwrap_or(&proposal.seams);
    let kept: Vec<&Seam> = all.iter().enumerate().filter(|(i, _)| keep_seams.get(*i).copied().unwrap_or(true)).map(|(_, s)| s).collect();
    let mut xs: Vec<(f64, f64)> = kept.iter().filter(|s| s.upright).map(|s| (s.from.x, s.to.x)).collect();
    let mut ys: Vec<(f64, f64)> = kept.iter().filter(|s| !s.upright).map(|s| (s.from.y, s.to.y)).collect();
    xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let spans = |cuts: &[(f64, f64)], lo: f64, hi: f64| -> Vec<f64> {
        let mut out = Vec::new();
        let mut prev = (lo, lo);
        for c in cuts.iter().chain(std::iter::once(&(hi, hi))) {
            out.push((c.0 - prev.0).max(c.1 - prev.1));
            prev = *c;
        }
        out
    };
    let ws = spans(&xs, e.left, e.right);
    let hs = spans(&ys, e.bottom, e.top);
    hs.iter().rev().flat_map(|h| ws.iter().map(move |w| [*w, *h])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall_with_well() -> Vec<Vec2> {
        // A 4000 × 2000 wall: a wheel well 800 wide, 300 high, from x 1000 on the floor; the bottom
        // 20 mm higher on the right half; a 40 mm slot in the top.
        [
            (0.0, 0.0),
            (1000.0, 0.0),
            (1000.0, 300.0),
            (1800.0, 300.0),
            (1800.0, 0.0),
            (2000.0, 0.0),
            (2000.0, 20.0),
            (4000.0, 20.0),
            (4000.0, 2000.0),
            (2100.0, 2000.0),
            (2100.0, 1960.0),
            (2000.0, 1960.0),
            (2000.0, 2000.0),
            (0.0, 2000.0),
        ]
        .iter()
        .map(|&(x, y)| Vec2::new(x, y))
        .collect()
    }

    fn template_of(outline: Vec<Vec2>) -> Template {
        Template {
            frame: Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y },
            area: twice_area(&outline).abs() / 2.0,
            outline,
            cutouts: Vec::new(),
            scan_edge: Vec::new(),
            cut_area: 0.0,
            found: 0,
            specks: 0,
            outside: 0,
        }
    }

    #[test]
    fn the_edges_sit_where_most_of_each_side_lies_and_a_short_step_is_a_notch() {
        let e = edges_for(&wall_with_well(), 5.0).expect("edges");
        // The bottom: 0 along 1200 of the 3200 sampled outside the well, 20 along 2000: the edge
        // goes to 20, clear of the higher floor on the right.
        assert_eq!((e.left, e.right, e.bottom, e.top), (0.0, 4000.0, 20.0, 2000.0));
    }

    #[test]
    fn the_wheel_well_and_the_top_slot_are_notches_and_dropping_one_covers_it() {
        let made = template_of(wall_with_well());
        let p = propose(&made, &[], None, SHEET, 5.0).expect("proposal");
        let notches: Vec<(f64, f64)> = p.cuts.iter().filter(|c| c.notch).map(|c| (c.depth, c.area)).collect();
        // The well: 280 deep over the bottom edge at 20, 800 × 280; the slot: 40 × 100.
        assert_eq!(notches.len(), 2, "{notches:?}");
        assert!(notches.iter().any(|(d, a)| (d - 280.0).abs() < 1e-6 && (a - 800.0 * 280.0).abs() < 1e-3), "{notches:?}");
        assert!(notches.iter().any(|(d, a)| (d - 40.0).abs() < 1e-6 && (a - 100.0 * 40.0).abs() < 1e-3), "{notches:?}");
        let full = twice_area(&lay(&p, &[], &[], &[], None).outline).abs() / 2.0;
        let well = p.cuts.iter().position(|c| (c.depth - 280.0).abs() < 1e-6).expect("well");
        let mut keep = vec![true; p.cuts.len()];
        keep[well] = false;
        let covered = twice_area(&lay(&p, &keep, &[], &[], None).outline).abs() / 2.0;
        assert!((full - (4000.0 * 1980.0 - 800.0 * 280.0 - 4000.0)).abs() < 1e-3, "{full}");
        assert!((covered - full - 800.0 * 280.0).abs() < 1e-3, "{covered} {full}");
    }

    #[test]
    fn a_seam_runs_between_two_rows_of_holes_and_the_pieces_fit_the_sheet() {
        let made = template_of(wall_with_well());
        // Two rows 34 mm apart at 500 and 534, rising 0.3° to the right; a column pair at 2400/2440.
        let slope = 0.3f64.to_radians().tan();
        let mut holes = Vec::new();
        for i in 0..12 {
            let x = 150.0 + 300.0 * i as f64;
            for y in [500.0, 534.0, 1500.0] {
                holes.push(Hole { at: Vec2::new(x, y + slope * x), diameter: 8.0, behind: 20.0 });
            }
        }
        for j in 0..6 {
            let y = 700.0 + 120.0 * j as f64;
            holes.push(Hole { at: Vec2::new(2400.0, y), diameter: 8.0, behind: 30.0 });
            holes.push(Hole { at: Vec2::new(2440.0, y), diameter: 8.0, behind: 30.0 });
        }
        // Two holes in the middle of the wall, far from any edge or seam.
        let far = [Vec2::new(1000.0, 1000.0), Vec2::new(3200.0, 1100.0)];
        holes.extend(far.iter().map(|&at| Hole { at, diameter: 8.0, behind: 20.0 }));
        let p = propose(&made, &holes, None, SHEET, 5.0).expect("proposal");
        let across: Vec<&Seam> = p.seams.iter().filter(|s| !s.upright).collect();
        let up: Vec<&Seam> = p.seams.iter().filter(|s| s.upright).collect();
        assert_eq!(up.len(), 1, "{:?}", p.seams);
        assert!((up[0].from.x - 2420.0).abs() < 1.0 && up[0].between_rows, "{:?}", up[0]);
        // Height 1980 needs one seam across at least; the paired rows give the one between them.
        assert!(across.iter().any(|s| s.between_rows && (s.from.y - 517.0).abs() < 2.0), "{across:?}");
        let sizes = piece_sizes(&p, &[], None);
        assert!(sizes.iter().all(|[w, h]| *w <= 2440.0 + 1e-6 && *h <= 1220.0 + 1e-6) || p.over > 0.0, "{sizes:?}");
        // The holes kept are near an edge or a seam.
        assert!(p.holes.len() >= 40 && p.holes.iter().all(|h| !far.contains(&h.at)), "{}", p.holes.len());
    }

    #[test]
    fn a_circle_fit_finds_the_centre_of_a_ragged_ring() {
        let ring: Vec<Vec2> = (0..18)
            .map(|k| {
                let a = k as f64 / 18.0 * std::f64::consts::TAU;
                let r = 4.0 + if k % 3 == 0 { 0.3 } else { -0.1 };
                Vec2::new(10.0 + r * a.cos(), -5.0 + r * a.sin())
            })
            .collect();
        let (c, r, rms) = fit_circle(&ring).expect("circle");
        assert!(c.dist(Vec2::new(10.0, -5.0)) < 0.05 && (r - 4.03).abs() < 0.05 && rms < 0.3, "{c:?} {r} {rms}");
    }

    #[test]
    fn a_turned_template_keeps_every_point_in_place_and_puts_up_up() {
        let made = template_of(wall_with_well());
        let holes = [Hole { at: Vec2::new(150.0, 500.0), diameter: 8.0, behind: 10.0 }];
        for k in 0..4u8 {
            let (t, h) = turned(&made, &holes, k);
            for (a, b) in made.outline.iter().zip(&t.outline) {
                assert!(made.at(*a).dist(t.at(*b)) < 1e-9, "turn {k}");
            }
            assert!(made.at(holes[0].at).dist(t.at(h[0].at)) < 1e-9, "turn {k}");
            assert!((twice_area(&t.outline) - twice_area(&made.outline)).abs() < 1e-6, "still counter-clockwise, turn {k}");
        }
        // Up in space along -x: a quarter turn the other way (three), and its y is then up.
        let k = quarters_for(&made, Vec3::new(-1.0, 0.0, 0.0));
        let (t, _) = turned(&made, &[], k);
        assert!(t.frame.y.dist(Vec3::new(-1.0, 0.0, 0.0)) < 1e-12, "{k}");
    }

    #[test]
    fn hostile_edges_and_sheets_are_refused() {
        let made = template_of(wall_with_well());
        assert!(propose(&made, &[], Some(Edges { left: 10.0, right: 5.0, bottom: 0.0, top: 1.0 }), SHEET, 5.0).is_none());
        assert!(propose(&made, &[], None, [f64::NAN, 1.0], 5.0).is_none());
        assert!(propose(&made, &[], Some(Edges { left: 5000.0, right: 6000.0, bottom: 0.0, top: 10.0 }), SHEET, 5.0).is_none());
    }
}
