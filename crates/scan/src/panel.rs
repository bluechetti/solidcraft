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

/// How a cut-out is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// The box round it: four straight sides.
    Box,
    /// Its outline with every dent up to [`STRAIGHTEN`] deep filled: straight sides, never inside
    /// what the scan found.
    Straight,
    /// Its outline as the scan found it.
    Scanned,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Box => "box",
            Shape::Straight => "straight",
            Shape::Scanned => "scanned",
        }
    }
    pub fn from_name(name: &str) -> Option<Shape> {
        match name {
            "box" => Some(Shape::Box),
            "straight" => Some(Shape::Straight),
            "scanned" => Some(Shape::Scanned),
            _ => None,
        }
    }
}

/// A cut-out narrower than this along its edge (mm), or smaller than this (mm²), is cut as a box.
const BOX_UNDER: f64 = 150.0;
const BOX_AREA_UNDER: f64 = 25_000.0;

/// Straight sides: dents in a cut-out up to this deep are filled (mm).
pub const STRAIGHTEN: f64 = 15.0;

/// The gap left at every edge of the panel, round every cut-out and at every seam (mm): plywood
/// moves with moisture, and a panel cut to the scan exactly would bind.
pub const CLEARANCE: f64 = 3.0;

/// Edges are fastened this often (mm): a stretch longer than this with no hole near gets fixing
/// points of its own.
pub const SPACING: f64 = 300.0;

/// The saw's cut between pieces laid out on one sheet (mm).
pub const KERF: f64 = 3.0;

/// Holes this large or larger (mm) are access holes, not ones to fasten through.
pub const ACCESS_HOLE: f64 = 16.0;

/// A cut-out proposed.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// Its outline as found, counter-clockwise, in the template's coordinates.
    pub ring: Vec<Vec2>,
    /// A notch in an edge (rather than a cut-out inside the panel), and which edges it opens on (0
    /// bottom, 1 right, 2 top, 3 left).
    pub notch: bool,
    pub edges: Vec<usize>,
    /// How far it reaches in from the edge (mm; a cut-out inside: its height), its area (mm²).
    pub depth: f64,
    pub area: f64,
    /// How it is proposed to be cut.
    pub shape: Shape,
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

/// A new place to fasten the panel: flat metal behind it, where an edge had no hole near.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fixing {
    pub at: Vec2,
    /// How far the metal is behind the panel's back face (mm; less than 0: proud of it).
    pub behind: f64,
}

/// What a proposal is asked to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options<'a> {
    /// The straight edges (default: where the outline's sides lie).
    pub edges: Option<Edges>,
    /// The sheet, long × short (mm).
    pub sheet: [f64; 2],
    /// The grid the edges are rounded to (mm; 0: as found).
    pub round: f64,
    /// The gap at edges, cut-outs and seams (mm).
    pub clearance: f64,
    /// Propose access holes too.
    pub access_holes: bool,
    /// How often the edges are fastened (mm; 0: no fixing points proposed).
    pub spacing: f64,
    /// The seams to use (moved by hand) instead of finding them; the holes near them and the fixing
    /// points are worked out for these.
    pub seams: Option<&'a [Seam]>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options { edges: None, sheet: SHEET, round: 5.0, clearance: CLEARANCE, access_holes: false, spacing: SPACING, seams: None }
    }
}

/// Everything proposed for a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// The rectangle the edges make (before the gap).
    pub edges: Edges,
    pub cuts: Vec<Cut>,
    pub seams: Vec<Seam>,
    pub holes: Vec<Hole>,
    pub fixings: Vec<Fixing>,
    /// Notches shallower than [`LEAST_NOTCH`] the straight edges go over, and how deep the deepest is.
    pub flattened: usize,
    pub flattened_depth: f64,
    /// The sheet asked for, and how far the longest piece is past it (0: they all fit).
    pub sheet: [f64; 2],
    pub over: f64,
    /// The gap at edges, cut-outs and seams (mm).
    pub clearance: f64,
    /// Places an edge needed fastening but no flat metal was found behind it.
    pub unbacked: usize,
}

/// What is kept of a proposal: a mark for each cut-out (and how it is cut), seam, hole and fixing
/// point, and the seams as moved. A mark missing is kept; a shape missing is as proposed; no seams
/// moved is the proposal's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keep {
    pub cuts: Vec<bool>,
    pub shapes: Vec<Shape>,
    pub seams: Vec<bool>,
    pub holes: Vec<bool>,
    pub fixings: Vec<bool>,
    pub moved: Vec<Seam>,
}

impl Keep {
    /// Everything kept, as proposed.
    pub fn all(p: &Proposal) -> Keep {
        Keep {
            cuts: vec![true; p.cuts.len()],
            shapes: p.cuts.iter().map(|c| c.shape).collect(),
            seams: vec![true; p.seams.len()],
            holes: vec![true; p.holes.len()],
            fixings: vec![true; p.fixings.len()],
            moved: p.seams.clone(),
        }
    }
    fn mark(marks: &[bool], i: usize) -> bool {
        marks.get(i).copied().unwrap_or(true)
    }
    fn seams<'a>(&'a self, p: &'a Proposal) -> &'a [Seam] {
        if self.moved.len() == p.seams.len() { &self.moved } else { &p.seams }
    }
}

/// The proposal for `made` (a whole wall's outline): its straight edges, the cut-outs they need
/// (small ones as boxes, larger ones with straight sides), seams for the sheet along the rows of
/// `holes`, the holes near the edges and seams, and fixing points on flat metal (`backing`) where an
/// edge has no hole near.
pub fn propose(made: &Template, holes: &[Hole], backing: Option<&Backing>, options: &Options) -> Option<Proposal> {
    let edges = match options.edges {
        Some(e) if e.ok() => e,
        Some(_) => return None,
        None => edges_for(&made.outline, options.round)?,
    };
    let clearance = options.clearance;
    if !(options.sheet.iter().all(|s| s.is_finite() && *s > 0.0)) || !(clearance.is_finite() && clearance >= 0.0) {
        return None;
    }
    if !(edges.width() > 2.0 * clearance && edges.height() > 2.0 * clearance) {
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
        cuts.push(Cut { area: twice_area(&ring).abs() / 2.0, ring, notch: false, edges: Vec::new(), depth: hi.y - lo.y, shape: Shape::Scanned });
    }
    for c in &mut cuts {
        c.shape = proposed_shape(c);
    }
    let mut p = Proposal {
        edges,
        cuts,
        seams: Vec::new(),
        holes: Vec::new(),
        fixings: Vec::new(),
        flattened,
        flattened_depth,
        sheet: options.sheet,
        over: 0.0,
        clearance,
        unbacked: 0,
    };
    let panel = lay(&p, &Keep::all(&p)).outline;
    let inside: Vec<Hole> = holes.iter().copied().filter(|h| contains(&panel, h.at) && (options.access_holes || h.diameter < ACCESS_HOLE)).collect();
    let (seams, over) = match options.seams {
        Some(given) => {
            if given.len() > MOST_SEAMS || !given.iter().all(|s| s.from.is_finite() && s.to.is_finite()) {
                return None;
            }
            let mut p2 = p.clone();
            p2.seams = given.to_vec();
            let k = Keep::all(&p2);
            let (long, short) = (options.sheet[0].max(options.sheet[1]), options.sheet[0].min(options.sheet[1]));
            let over = piece_sizes(&p2, &k).iter().map(|[w, h]| (w.max(*h) - long).max(w.min(*h) - short).max(0.0)).fold(0.0, f64::max);
            (given.to_vec(), over)
        }
        None => seams_for(&inside, &edges, options.sheet),
    };
    let half = clearance / 2.0;
    let near: Vec<Hole> = inside
        .into_iter()
        .filter(|h| {
            let to_edge = ring_distance(&panel, h.at);
            let to_seam = seams.iter().map(|s| segment_distance(h.at, s.from, s.to) - half).fold(f64::INFINITY, f64::min);
            let reach = to_edge.min(to_seam);
            reach <= HOLE_BAND && reach - h.diameter / 2.0 >= HOLE_WOOD
        })
        .collect();
    p.seams = seams;
    p.over = over;
    p.holes = near;
    if let Some(backing) = backing.filter(|_| options.spacing.is_finite() && options.spacing > 0.0) {
        let at = |q: Vec2| made.at(q);
        let (fixings, unbacked) = fixings_for(&panel, &p.seams, clearance, &p.holes, &at, backing, options.spacing);
        p.fixings = fixings;
        p.unbacked = unbacked;
    }
    Some(p)
}

/// How a cut-out is proposed to be cut: a small one as a box, a larger one with straight sides.
fn proposed_shape(c: &Cut) -> Shape {
    let Some((lo, hi)) = bounds(&c.ring) else { return Shape::Scanned };
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let along = match (c.edges.iter().all(|e| *e == 0 || *e == 2), c.edges.iter().all(|e| *e == 1 || *e == 3)) {
        (true, false) => w,
        (false, true) => h,
        _ => w.min(h),
    };
    if along < BOX_UNDER || c.area < BOX_AREA_UNDER { Shape::Box } else { Shape::Straight }
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
            cuts.push(Cut { ring, notch: true, edges, depth, area: area2.abs() / 2.0, shape: Shape::Scanned });
        }
        k += len;
    }
    (cuts, flattened, deepest)
}

/// How far in from the rectangle's nearest side a point inside it is.
fn distance_in(p: Vec2, e: &Edges) -> f64 {
    (p.x - e.left).min(e.right - p.x).min(p.y - e.bottom).min(e.top - p.y).max(0.0)
}

/// `ring` (counter-clockwise) with every dent up to `deep` filled: a corner that turns in is taken
/// away while it lies no farther than that from the line past it, so the shape only grows and keeps
/// its straight sides.
pub fn straightened(ring: &[Vec2], deep: f64) -> Vec<Vec2> {
    let mut r: Vec<Vec2> = ring.to_vec();
    for _ in 0..ring.len().saturating_mul(2) {
        if r.len() <= 3 {
            break;
        }
        let n = r.len();
        let mut best: Option<(f64, usize)> = None;
        for k in 0..n {
            let (a, b, c) = (r[(k + n - 1) % n], r[k], r[(k + 1) % n]);
            let turn = (b - a).x * (c - b).y - (b - a).y * (c - b).x;
            let off = segment_distance(b, a, c);
            // A dent (a right turn on a counter-clockwise ring) not too deep, or a corner in line.
            let takes = (turn < 0.0 && off <= deep) || off <= 0.25;
            if takes && best.is_none_or(|(o, _)| off < o) {
                best = Some((off, k));
            }
        }
        match best {
            Some((_, k)) => {
                r.remove(k);
            }
            None => break,
        }
    }
    r
}

/// `ring` (counter-clockwise) grown by `by` all round, each corner moved out along its bisector (a
/// sharp corner's move held to four times `by`).
pub fn grown(ring: &[Vec2], by: f64) -> Vec<Vec2> {
    if by <= 0.0 || ring.len() < 3 {
        return ring.to_vec();
    }
    let n = ring.len();
    let normal = |a: Vec2, b: Vec2| {
        let d = b - a;
        let l = (d.x * d.x + d.y * d.y).sqrt().max(1e-12);
        Vec2::new(d.y / l, -d.x / l)
    };
    (0..n)
        .map(|k| {
            let (a, b, c) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
            let (n1, n2) = (normal(a, b), normal(b, c));
            let sum = n1 + n2;
            let cos = n1.x * n2.x + n1.y * n2.y;
            let mut m = sum * (by / (1.0 + cos).max(1e-9));
            let len = (m.x * m.x + m.y * m.y).sqrt();
            if len > 4.0 * by {
                m = m * (4.0 * by / len);
            }
            b + m
        })
        .collect()
}

/// The cut-out as it is to be cut: its shape, grown by the gap.
fn shaped(c: &Cut, shape: Shape, clearance: f64) -> Vec<Vec2> {
    let base = match shape {
        Shape::Box => match bounds(&c.ring) {
            Some((lo, hi)) => vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)],
            None => c.ring.clone(),
        },
        Shape::Straight => straightened(&c.ring, STRAIGHTEN),
        Shape::Scanned => c.ring.clone(),
    };
    grown(&base, clearance)
}

/// The runs of `ring` (clipped to the rectangle) off its sides, each from where it leaves a side
/// to where it comes back; none when no side of the ring lies along the rectangle (a cut-out
/// inside it).
fn runs_off(ring: &[Vec2], e: &Edges) -> Option<Vec<Vec<Vec2>>> {
    let n = ring.len();
    let seg_on = |k: usize| (0..4).any(|t| on_side(ring[k], e, t) && on_side(ring[(k + 1) % n], e, t));
    let start = (0..n).find(|&k| seg_on(k))?;
    let mut runs = Vec::new();
    let mut k = 1;
    while k < n {
        let i = (start + k) % n;
        if seg_on(i) {
            k += 1;
            continue;
        }
        let mut len = 1;
        while k + len < n && !seg_on((i + len) % n) {
            len += 1;
        }
        runs.push((0..=len).map(|t| ring[(i + t) % n]).collect());
        k += len;
    }
    Some(runs)
}

/// The rectangle with `regions` (each counter-clockwise) cut out of it: its outline, going in
/// round each region that reaches an edge, and the regions wholly inside it.
fn cut_out_of(e: &Edges, regions: &[Vec<Vec2>]) -> (Vec<Vec2>, Vec<Vec<Vec2>>) {
    let total = 2.0 * (e.width() + e.height());
    let mut regions: Vec<Vec<Vec2>> = regions.iter().map(|r| clip(r, e)).filter(|r| r.len() >= 3 && twice_area(r).abs() > 1e-6).collect();
    // Regions whose ways round overlap along the edge are cut as one box.
    for _ in 0..regions.len().max(1) {
        let mut paths: Vec<(f64, f64, usize)> = Vec::new();
        for (i, r) in regions.iter().enumerate() {
            for run in runs_off(r, e).unwrap_or_default() {
                if let (Some(a), Some(b)) = (run.last(), run.first()) {
                    paths.push((round_at(*a, e), round_at(*b, e), i));
                }
            }
        }
        let span = |a: f64, b: f64| (b - a).rem_euclid(total);
        let mut clash = None;
        'find: for (x, p) in paths.iter().enumerate() {
            for q in paths.iter().skip(x + 1) {
                if p.2 != q.2 && (span(p.0, q.0) < span(p.0, p.1) || span(q.0, p.0) < span(q.0, q.1)) {
                    clash = Some((p.2, q.2));
                    break 'find;
                }
            }
        }
        let Some((a, b)) = clash else { break };
        let both: Vec<Vec2> = regions[a].iter().chain(&regions[b]).copied().collect();
        let Some((lo, hi)) = bounds(&both) else { break };
        let joined = clip(&[lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)], e);
        let (first, second) = (a.min(b), a.max(b));
        regions.remove(second);
        regions[first] = joined;
    }
    let mut inside = Vec::new();
    // Each way round: (where it leaves the edge, where it comes back, the points between).
    let mut paths: Vec<(f64, f64, Vec<Vec2>)> = Vec::new();
    for r in &regions {
        match runs_off(r, e) {
            None => inside.push(r.clone()),
            Some(runs) => {
                for mut run in runs {
                    // The panel goes round the region the other way.
                    run.reverse();
                    if let (Some(a), Some(b)) = (run.first(), run.last()) {
                        paths.push((round_at(*a, e), round_at(*b, e), run));
                    }
                }
            }
        }
    }
    let corners = e.corners();
    let corner_at = [0.0, e.width(), e.width() + e.height(), 2.0 * e.width() + e.height()];
    let within =
        |s: f64| paths.iter().any(|(a, b, _)| (s - a).rem_euclid(total) > 1e-9 && (s - a).rem_euclid(total) < (b - a).rem_euclid(total) - 1e-9);
    let start = corner_at.iter().copied().chain(paths.iter().map(|p| p.1)).find(|s| !within(*s)).unwrap_or(0.0);
    // Corners and ways round, in order round from the start.
    let mut events: Vec<(f64, Option<usize>, usize)> = Vec::new();
    for (k, s) in corner_at.iter().enumerate() {
        if !within(*s) {
            events.push(((s - start).rem_euclid(total), None, k));
        }
    }
    for (i, (a, _, _)) in paths.iter().enumerate() {
        events.push(((a - start).rem_euclid(total), Some(i), 0));
    }
    events.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Vec2> = Vec::new();
    let mut past = -1.0;
    for (at, path, k) in events {
        if at < past {
            continue;
        }
        match path {
            None => out.push(corners[k]),
            Some(i) => {
                let (a, b, pts) = &paths[i];
                out.extend(pts.iter().copied());
                past = at + (b - a).rem_euclid(total);
            }
        }
    }
    (tidied(out), inside)
}

/// Repeated points and points in line with their neighbours taken out.
fn tidied(out: Vec<Vec2>) -> Vec<Vec2> {
    let mut clean: Vec<Vec2> = Vec::with_capacity(out.len());
    for p in out {
        if clean.last().is_none_or(|q: &Vec2| q.dist(p) > 1e-6) {
            clean.push(p);
        }
    }
    while clean.len() > 1 && clean.first().zip(clean.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-6) {
        clean.pop();
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
    /// Its outline, counter-clockwise (template coordinates), the gap taken off.
    pub outline: Vec<Vec2>,
    /// The cut-outs inside it kept, the gap added.
    pub inside: Vec<Vec<Vec2>>,
    /// Each seam kept, as the stretches over the panel of its two cut lines (the gap between them).
    pub seams: Vec<Vec<(Vec2, Vec2)>>,
    /// The holes kept that are in the panel: centre and diameter.
    pub holes: Vec<(Vec2, f64)>,
    /// The fixing points kept that are in the panel.
    pub fixings: Vec<Vec2>,
}

/// The panel from `proposal` as `keep` says.
pub fn lay(proposal: &Proposal, keep: &Keep) -> Laid {
    let c = proposal.clearance;
    let e = proposal.edges;
    let inset = Edges { left: e.left + c, right: e.right - c, bottom: e.bottom + c, top: e.top - c };
    let regions: Vec<Vec<Vec2>> = proposal
        .cuts
        .iter()
        .enumerate()
        .filter(|(i, _)| Keep::mark(&keep.cuts, *i))
        .map(|(i, cut)| shaped(cut, keep.shapes.get(i).copied().unwrap_or(cut.shape), c))
        .collect();
    let (outline, inside) = if inset.ok() { cut_out_of(&inset, &regions) } else { (e.corners().to_vec(), Vec::new()) };
    let seams: Vec<Vec<(Vec2, Vec2)>> = keep
        .seams(proposal)
        .iter()
        .enumerate()
        .filter(|(i, _)| Keep::mark(&keep.seams, *i))
        .map(|(_, s)| {
            let shift = if s.upright { Vec2::new(c / 2.0, 0.0) } else { Vec2::new(0.0, c / 2.0) };
            let mut both = over_ring(&outline, s.from - shift, s.to - shift);
            if c > 0.0 {
                both.extend(over_ring(&outline, s.from + shift, s.to + shift));
            }
            both
        })
        .filter(|r| !r.is_empty())
        .collect();
    let clear = |q: Vec2| contains(&outline, q) && !inside.iter().any(|r| contains(r, q));
    let holes =
        proposal.holes.iter().enumerate().filter(|(i, h)| Keep::mark(&keep.holes, *i) && clear(h.at)).map(|(_, h)| (h.at, h.diameter)).collect();
    let fixings = proposal.fixings.iter().enumerate().filter(|(i, f)| Keep::mark(&keep.fixings, *i) && clear(f.at)).map(|(_, f)| f.at).collect();
    Laid { outline, inside, seams, holes, fixings }
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

/// Each piece's blank between the seams kept (the box it is cut from: width × height, mm, the gaps
/// taken off; a sloping seam's rise included), top row first, for checking against the sheet.
pub fn piece_sizes(proposal: &Proposal, keep: &Keep) -> Vec<[f64; 2]> {
    let e = &proposal.edges;
    let c = proposal.clearance;
    let kept: Vec<&Seam> = keep.seams(proposal).iter().enumerate().filter(|(i, _)| Keep::mark(&keep.seams, *i)).map(|(_, s)| s).collect();
    let mut xs: Vec<(f64, f64)> = kept.iter().filter(|s| s.upright).map(|s| (s.from.x, s.to.x)).collect();
    let mut ys: Vec<(f64, f64)> = kept.iter().filter(|s| !s.upright).map(|s| (s.from.y, s.to.y)).collect();
    xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Between the edges (the gap c in from each) and the seams (half the gap back from each).
    let spans = |cuts: &[(f64, f64)], lo: f64, hi: f64| -> Vec<f64> {
        let mut out = Vec::new();
        let mut prev = (lo + c, lo + c);
        for (k, cut) in cuts.iter().chain(std::iter::once(&(hi - c, hi - c))).enumerate() {
            let near = if k < cuts.len() { c / 2.0 } else { 0.0 };
            let after = if k > 0 { c / 2.0 } else { 0.0 };
            // The blank: from the lower end of the seam before to the higher end of the next, so a
            // sloping seam's piece is cut from a sheet that takes all of it.
            out.push(cut.0.max(cut.1) - prev.0.min(prev.1) - near - after);
            prev = *cut;
        }
        out
    };
    let ws = spans(&xs, e.left, e.right);
    let hs = spans(&ys, e.bottom, e.top);
    hs.iter().rev().flat_map(|h| ws.iter().map(move |w| [*w, *h])).collect()
}

/// Seam moves that would bring every piece within the sheet: (seam, by mm), each moving one seam
/// so the piece past the sheet fits and the one on its other side still does.
pub fn fit_moves(proposal: &Proposal, keep: &Keep) -> Vec<(usize, f64)> {
    let (long, short) = (proposal.sheet[0].max(proposal.sheet[1]), proposal.sheet[0].min(proposal.sheet[1]));
    let e = &proposal.edges;
    let (most_w, most_h) = if e.width() >= e.height() { (long, short) } else { (short, long) };
    let c = proposal.clearance;
    let seams = keep.seams(proposal);
    let mut out = Vec::new();
    for upright in [true, false] {
        let most = if upright { most_w } else { most_h };
        let (lo, hi) = if upright { (e.left + c, e.right - c) } else { (e.bottom + c, e.top - c) };
        // The kept seams this way, by where they are at their widest.
        let mut at: Vec<(usize, f64, f64)> = seams
            .iter()
            .enumerate()
            .filter(|(i, s)| Keep::mark(&keep.seams, *i) && s.upright == upright)
            .map(|(i, s)| if upright { (i, s.from.x.min(s.to.x), s.from.x.max(s.to.x)) } else { (i, s.from.y.min(s.to.y), s.from.y.max(s.to.y)) })
            .collect();
        at.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        for (k, (i, low, high)) in at.iter().enumerate() {
            let before = if k == 0 { lo } else { at[k - 1].2 + c / 2.0 };
            let after = at.get(k + 1).map_or(hi, |n| n.1 - c / 2.0);
            let left_span = high - c / 2.0 - before;
            let right_span = after - (low + c / 2.0);
            if left_span > most {
                let by = -(left_span - most).ceil();
                if right_span - by <= most {
                    out.push((*i, by));
                }
            } else if right_span > most {
                let by = (right_span - most).ceil();
                if left_span + by <= most {
                    out.push((*i, by));
                }
            }
        }
    }
    out
}

/// The scan's frontmost surface over the panel's plane, cell by cell: what a screw behind the panel
/// would meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Backing {
    origin: Vec3,
    x: Vec3,
    y: Vec3,
    normal: Vec3,
    lo: Vec2,
    cell: f64,
    w: usize,
    h: usize,
    front: Vec<f32>,
}

/// The backing's cell (mm) and the most cells it has.
const BACKING_CELL: f64 = 4.0;
const MOST_BACKING_CELLS: f64 = 4_000_000.0;

/// Metal to fasten into: flat within this (mm) over a square this wide (mm), no more than this
/// far behind the panel (mm) and no more than [`HOLE_PROUD`] proud of it.
const FIX_FLAT: f64 = 1.5;
const FIX_SQUARE: f64 = 20.0;
const FIX_DEEPEST: f64 = 45.0;

/// A fixing point goes this far in from the edge at least and at most (mm), and this far along
/// it either way at most from where it is wanted (mm).
const FIX_IN: f64 = 20.0;
const FIX_IN_MOST: f64 = 80.0;
const FIX_ALONG: f64 = 60.0;

/// A hole within this of an edge (mm) fastens it there; a stretch with none this long or longer
/// gets fixing points.
const FIX_REACH: f64 = 100.0;
const FIX_STRETCH: f64 = 150.0;

/// The most fixing points proposed.
pub const MOST_FIXINGS: usize = 500;

/// The backing of the scan over `frame` (whose `x × y` points out of the wall to the room).
pub fn backing(scan: &Scan, frame: &Plane) -> Option<Backing> {
    let normal = frame.x.cross(frame.y).normalized()?;
    let flat: Vec<(Vec2, f64)> = scan.points().map(|p| (to_plane(frame, p), (p - frame.origin).dot(normal))).collect();
    let (lo, hi) = flat.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), (q, _)| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    if !(hi.x > lo.x && hi.y > lo.y) {
        return None;
    }
    let cell = BACKING_CELL.max(((hi.x - lo.x) * (hi.y - lo.y) / MOST_BACKING_CELLS).sqrt());
    let w = ((hi.x - lo.x) / cell) as usize + 1;
    let h = ((hi.y - lo.y) / cell) as usize + 1;
    let mut front = vec![f32::NAN; w * h];
    for (q, off) in flat {
        let (i, j) = (((q.x - lo.x) / cell) as usize, ((q.y - lo.y) / cell) as usize);
        if let Some(f) = front.get_mut(j * w + i.min(w - 1))
            && (f.is_nan() || off as f32 > *f)
        {
            *f = off as f32;
        }
    }
    Some(Backing { origin: frame.origin, x: frame.x, y: frame.y, normal, lo, cell, w, h, front })
}

impl Backing {
    /// The face of flat metal at `p` (along the normal from the plane, mm), if the scan is flat
    /// within [`FIX_FLAT`] over a [`FIX_SQUARE`] square round it, scanned all over.
    pub fn flat_face(&self, p: Vec3) -> Option<f64> {
        let d = p - self.origin;
        let q = Vec2::new(d.dot(self.x), d.dot(self.y));
        let (ci, cj) = ((q.x - self.lo.x) / self.cell, (q.y - self.lo.y) / self.cell);
        if !(ci >= 0.0 && cj >= 0.0) {
            return None;
        }
        let (ci, cj) = (ci as usize, cj as usize);
        let r = ((FIX_SQUARE / 2.0) / self.cell).ceil() as usize;
        let (mut low, mut high, mut sum, mut n) = (f32::MAX, f32::MIN, 0.0f64, 0usize);
        for j in cj.checked_sub(r)?..=cj + r {
            for i in ci.checked_sub(r)?..=ci + r {
                if i >= self.w || j >= self.h {
                    return None;
                }
                let v = *self.front.get(j * self.w + i)?;
                if v.is_nan() {
                    return None;
                }
                low = low.min(v);
                high = high.max(v);
                sum += f64::from(v);
                n += 1;
            }
        }
        (f64::from(high - low) <= FIX_FLAT && n > 0).then(|| sum / n as f64)
    }
    #[cfg(test)]
    fn normal(&self) -> Vec3 {
        self.normal
    }
}

/// Fixing points for every stretch of the panel's edges and seams (each side of a seam its own)
/// longer than [`FIX_STRETCH`] with no hole within [`FIX_REACH`], `spacing` apart, each moved to
/// the nearest flat metal behind the panel in from the edge; and how many found none.
fn fixings_for(
    panel: &[Vec2],
    seams: &[Seam],
    clearance: f64,
    holes: &[Hole],
    at: &dyn Fn(Vec2) -> Vec3,
    backing: &Backing,
    spacing: f64,
) -> (Vec<Fixing>, usize) {
    let mut covered: Vec<Vec2> = holes.iter().filter(|h| h.diameter < ACCESS_HOLE).map(|h| h.at).collect();
    // Each edge to fasten: its ends and the way in.
    let mut edges: Vec<(Vec2, Vec2, Vec2)> = Vec::new();
    for k in 0..panel.len() {
        let (a, b) = (panel[k], panel[(k + 1) % panel.len()]);
        let d = b - a;
        let l = (d.x * d.x + d.y * d.y).sqrt();
        if l > 1e-9 {
            edges.push((a, b, Vec2::new(-d.y / l, d.x / l)));
        }
    }
    for s in seams {
        for (a, b) in over_ring(panel, s.from, s.to) {
            let d = b - a;
            let l = (d.x * d.x + d.y * d.y).sqrt();
            if l > 1e-9 {
                let n = Vec2::new(-d.y / l, d.x / l);
                edges.push((a + n * (clearance / 2.0), b + n * (clearance / 2.0), n));
                edges.push((a - n * (clearance / 2.0), b - n * (clearance / 2.0), n * -1.0));
            }
        }
    }
    let seam_clear = |q: Vec2| seams.iter().all(|s| segment_distance(q, s.from, s.to) >= FIX_IN * 0.75 + clearance / 2.0);
    let (mut out, mut unbacked) = (Vec::new(), 0);
    for (a, b, inward) in edges {
        let d = b - a;
        let length = (d.x * d.x + d.y * d.y).sqrt();
        if length < FIX_STRETCH || out.len() >= MOST_FIXINGS {
            continue;
        }
        let along = d * (1.0 / length);
        let steps = (length / 10.0) as usize;
        let near = |s: f64, pts: &[Vec2]| {
            let q = a + along * s;
            pts.iter().any(|h| h.dist(q) <= FIX_REACH && (h.x - q.x) * inward.x + (h.y - q.y) * inward.y > 0.0)
        };
        // Stretches with nothing near.
        let mut runs: Vec<(f64, f64)> = Vec::new();
        let mut open: Option<f64> = None;
        for i in 0..=steps {
            let s = length * i as f64 / steps.max(1) as f64;
            match (near(s, &covered), open) {
                (false, None) => open = Some(s),
                (true, Some(o)) => {
                    runs.push((o, s));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(o) = open {
            runs.push((o, length));
        }
        for (s0, s1) in runs.into_iter().filter(|(s0, s1)| s1 - s0 >= FIX_STRETCH) {
            let n = ((s1 - s0) / spacing).ceil().max(1.0) as usize;
            for i in 0..n {
                let s = s0 + (s1 - s0) * (i as f64 + 0.5) / n as f64;
                let mut found = None;
                'search: for step in 0..=((FIX_ALONG / 10.0) as i32) {
                    for sign in [1.0, -1.0] {
                        let ds = sign * f64::from(step) * 10.0;
                        let mut inn = FIX_IN;
                        while inn <= FIX_IN_MOST {
                            let q = a + along * (s + ds) + inward * inn;
                            if contains(panel, q)
                                && ring_distance(panel, q) >= FIX_IN * 0.75
                                && seam_clear(q)
                                && let Some(face) = backing.flat_face(at(q))
                                && (-HOLE_PROUD..=FIX_DEEPEST).contains(&-face)
                            {
                                found = Some(Fixing { at: q, behind: -face });
                                break 'search;
                            }
                            inn += 5.0;
                        }
                        if step == 0 {
                            break;
                        }
                    }
                }
                match found {
                    Some(f) if out.len() < MOST_FIXINGS => {
                        covered.push(f.at);
                        out.push(f);
                    }
                    Some(_) => {}
                    None => unbacked += 1,
                }
            }
        }
    }
    (out, unbacked)
}

/// What a point on a piece is: a hole of the frame to fasten through, or a new fixing point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointKind {
    Hole,
    Fixing,
}

/// A point to drill on a piece, in the piece's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PiecePoint {
    pub kind: PointKind,
    /// Numbered on its piece: holes 1, 2, …, fixing points 1, 2, … (drawn F1, F2, …).
    pub number: usize,
    pub at: Vec2,
    pub diameter: f64,
    /// How far the metal is behind the panel's back face (mm).
    pub behind: f64,
    /// Where it is on the panel (template coordinates).
    pub on_panel: Vec2,
}

/// One piece of the panel, cut from its own blank.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// Numbered from 1, top row first, left to right (as [`piece_sizes`] lists them).
    pub number: usize,
    /// Where the piece's 0, 0 is on the panel (template coordinates): the blank's bottom-left
    /// corner, on the seam or edge below it.
    pub origin: Vec2,
    /// Its outline and the cut-outs inside it, counter-clockwise, in its own coordinates.
    pub outline: Vec<Vec2>,
    pub inside: Vec<Vec<Vec2>>,
    pub points: Vec<PiecePoint>,
    /// Its blank: the box its outline fits in (mm).
    pub blank: [f64; 2],
}

/// `ring` with what lies on the far side of the line through `a` along `d` cut off: what is kept
/// is to the left of the direction `d` (Sutherland–Hodgman against one half-plane).
fn clip_half(ring: &[Vec2], a: Vec2, d: Vec2) -> Vec<Vec2> {
    let side = |p: Vec2| d.x * (p.y - a.y) - d.y * (p.x - a.x);
    let mut out = Vec::with_capacity(ring.len() + 2);
    for k in 0..ring.len() {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        let (sp, sq) = (side(p), side(q));
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp >= 0.0) != (sq >= 0.0) {
            let t = sp / (sp - sq);
            out.push(p + (q - p) * t);
        }
    }
    tidied(out)
}

/// The panel laid as `keep` says, cut into its pieces along the seams kept (each piece stopping
/// half the gap short of a seam), numbered top row first; each with its holes and fixing points
/// in its own coordinates. Pieces smaller than half a percent of the panel (a sliver a seam cuts
/// off a notch) are left out.
pub fn pieces(proposal: &Proposal, keep: &Keep) -> Vec<Piece> {
    let laid = lay(proposal, keep);
    let c = proposal.clearance;
    let seams: Vec<Seam> = keep.seams(proposal).iter().enumerate().filter(|(i, _)| Keep::mark(&keep.seams, *i)).map(|(_, s)| *s).collect();
    // Seams across, bottom first, and up, left first (by their middles).
    let mid = |s: &Seam| (s.from + s.to) * 0.5;
    let mut across: Vec<Seam> = seams.iter().copied().filter(|s| !s.upright).collect();
    let mut up: Vec<Seam> = seams.iter().copied().filter(|s| s.upright).collect();
    across.sort_by(|a, b| mid(a).y.partial_cmp(&mid(b).y).unwrap_or(std::cmp::Ordering::Equal));
    up.sort_by(|a, b| mid(a).x.partial_cmp(&mid(b).x).unwrap_or(std::cmp::Ordering::Equal));
    // A seam's line moved half the gap towards `toward` (+1 up or right), running so that what is
    // kept (to its left) is that side.
    let line = |s: &Seam, toward: f64| {
        let (a, b) = if s.upright {
            if s.from.y <= s.to.y { (s.from, s.to) } else { (s.to, s.from) }
        } else if s.from.x <= s.to.x {
            (s.from, s.to)
        } else {
            (s.to, s.from)
        };
        let d = b - a;
        let l = (d.x * d.x + d.y * d.y).sqrt().max(1e-12);
        // Left of a→b: up for a seam across, left for a seam up.
        let left = Vec2::new(-d.y / l, d.x / l);
        let shift = left * (c / 2.0) * if s.upright { -toward } else { toward };
        let (a, b, d) = (a + shift, b + shift, d);
        // Keep the side `toward` points to.
        let keep_left = if s.upright { toward < 0.0 } else { toward > 0.0 };
        if keep_left { (a, d) } else { (b, d * -1.0) }
    };
    let total = twice_area(&laid.outline).abs() / 2.0;
    let mut found: Vec<(usize, usize, Vec<Vec2>)> = Vec::new();
    for row in 0..=across.len() {
        for col in 0..=up.len() {
            let mut ring = laid.outline.clone();
            if let Some(s) = row.checked_sub(1).and_then(|r| across.get(r)) {
                let (a, d) = line(s, 1.0);
                ring = clip_half(&ring, a, d);
            }
            if let Some(s) = across.get(row) {
                let (a, d) = line(s, -1.0);
                ring = clip_half(&ring, a, d);
            }
            if let Some(s) = col.checked_sub(1).and_then(|k| up.get(k)) {
                let (a, d) = line(s, 1.0);
                ring = clip_half(&ring, a, d);
            }
            if let Some(s) = up.get(col) {
                let (a, d) = line(s, -1.0);
                ring = clip_half(&ring, a, d);
            }
            if ring.len() >= 3 && twice_area(&ring).abs() / 2.0 > 0.005 * total {
                found.push((row, col, ring));
            }
        }
    }
    // Top row first, then left to right.
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let e = proposal.edges;
    let mut out = Vec::new();
    for (n, (row, _, ring)) in found.into_iter().enumerate() {
        let Some((lo, hi)) = bounds(&ring) else { continue };
        // The blank's corner: on the seam (or the edge) below, at the piece's left.
        let base = match row.checked_sub(1).and_then(|r| across.get(r)) {
            Some(s) => {
                let (a, b) = (s.from, s.to);
                let t = if (b.x - a.x).abs() > 1e-12 { (lo.x - a.x) / (b.x - a.x) } else { 0.0 };
                a.y + (b.y - a.y) * t + c / 2.0
            }
            None => e.bottom + c,
        }
        .min(lo.y);
        let origin = Vec2::new(lo.x, base);
        let local = |q: Vec2| q - origin;
        let inside: Vec<Vec<Vec2>> =
            laid.inside.iter().filter(|r| r.first().is_some_and(|q| contains(&ring, *q))).map(|r| r.iter().map(|q| local(*q)).collect()).collect();
        let mut points: Vec<PiecePoint> = Vec::new();
        let near = |q: Vec2| contains(&ring, q);
        let mut holes: Vec<(Vec2, f64, f64)> = laid
            .holes
            .iter()
            .filter(|(q, _)| near(*q))
            .map(|(q, d)| (*q, *d, proposal.holes.iter().find(|h| h.at.dist(*q) < 1e-9).map_or(0.0, |h| h.behind)))
            .collect();
        let row_of = |q: Vec2| ((q.y - base) / 45.0).round() as i64;
        holes.sort_by(|a, b| (row_of(a.0), a.0.x).partial_cmp(&(row_of(b.0), b.0.x)).unwrap_or(std::cmp::Ordering::Equal));
        for (k, (q, d, behind)) in holes.into_iter().enumerate() {
            points.push(PiecePoint { kind: PointKind::Hole, number: k + 1, at: local(q), diameter: d, behind, on_panel: q });
        }
        let mut fixings: Vec<(Vec2, f64)> = laid
            .fixings
            .iter()
            .filter(|q| near(**q))
            .map(|q| (*q, proposal.fixings.iter().find(|f| f.at.dist(*q) < 1e-9).map_or(0.0, |f| f.behind)))
            .collect();
        fixings.sort_by(|a, b| (row_of(a.0), a.0.x).partial_cmp(&(row_of(b.0), b.0.x)).unwrap_or(std::cmp::Ordering::Equal));
        for (k, (q, behind)) in fixings.into_iter().enumerate() {
            points.push(PiecePoint { kind: PointKind::Fixing, number: k + 1, at: local(q), diameter: 0.0, behind, on_panel: q });
        }
        let outline: Vec<Vec2> = ring.iter().map(|q| local(*q)).collect();
        out.push(Piece { number: n + 1, origin, blank: [hi.x - lo.x, hi.y - base], outline, inside, points });
    }
    out
}

/// A distance to measure in the van before cutting: between two holes of the frame far apart, so
/// a tape across the wall checks the scan and where the panel will sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Check {
    /// (piece number, hole number) at each end.
    pub from: (usize, usize),
    pub to: (usize, usize),
    pub distance: f64,
    /// How far across and up from the first to the second (mm).
    pub across: f64,
    pub up: f64,
}

/// Up to three distances to check: the two holes farthest apart across the panel, the two
/// farthest apart up it, and the two farthest apart in all.
pub fn checks(pieces: &[Piece]) -> Vec<Check> {
    let holes: Vec<(usize, usize, Vec2)> =
        pieces.iter().flat_map(|p| p.points.iter().filter(|q| q.kind == PointKind::Hole).map(move |q| (p.number, q.number, q.on_panel))).collect();
    let mut out: Vec<Check> = Vec::new();
    let best = |score: &dyn Fn(Vec2, Vec2) -> f64| {
        let mut top: Option<(f64, usize, usize)> = None;
        for i in 0..holes.len() {
            for j in i + 1..holes.len() {
                let v = score(holes[i].2, holes[j].2);
                if top.is_none_or(|(t, _, _)| v > t) {
                    top = Some((v, i, j));
                }
            }
        }
        top
    };
    // Across: far apart in x, close in height (one tape along a row); up: the other way.
    let along_x = |a: Vec2, b: Vec2| (a.x - b.x).abs() - 4.0 * (a.y - b.y).abs();
    let along_y = |a: Vec2, b: Vec2| (a.y - b.y).abs() - 4.0 * (a.x - b.x).abs();
    let far = |a: Vec2, b: Vec2| a.dist(b);
    for score in [&along_x as &dyn Fn(Vec2, Vec2) -> f64, &along_y, &far] {
        if let Some((_, i, j)) = best(score) {
            let (a, b) = if holes[i].2.x <= holes[j].2.x { (holes[i], holes[j]) } else { (holes[j], holes[i]) };
            let c = Check { from: (a.0, a.1), to: (b.0, b.1), distance: a.2.dist(b.2), across: b.2.x - a.2.x, up: b.2.y - a.2.y };
            if !out.iter().any(|o| o.from == c.from && o.to == c.to) {
                out.push(c);
            }
        }
    }
    out
}

/// A piece laid on a sheet: which, where its corner goes, its size as laid, and whether it is
/// turned a quarter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub piece: usize,
    pub at: Vec2,
    pub size: [f64; 2],
    pub turned: bool,
}

/// One sheet and the pieces cut from it; `too_big` when its one piece does not fit any sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetPlan {
    pub pieces: Vec<Placed>,
    pub too_big: bool,
}

/// The pieces (width × height each) laid on sheets of `sheet` with `kerf` between them, in rows
/// a saw cuts straight across (first fit, tallest first); a piece too big for a sheet on one of its
/// own.
pub fn sheets(sizes: &[[f64; 2]], sheet: [f64; 2], kerf: f64) -> Vec<SheetPlan> {
    let (long, short) = (sheet[0].max(sheet[1]), sheet[0].min(sheet[1]));
    let mut order: Vec<(usize, f64, f64, bool)> =
        sizes.iter().enumerate().map(|(i, [w, h])| if w >= h { (i, *w, *h, false) } else { (i, *h, *w, true) }).collect();
    order.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut plans: Vec<SheetPlan> = Vec::new();
    // Each sheet's rows: (y, height, x used).
    let mut rows: Vec<Vec<(f64, f64, f64)>> = Vec::new();
    for (piece, pw, ph, turned) in order {
        if pw > long + 1e-6 || ph > short + 1e-6 {
            plans.push(SheetPlan { pieces: vec![Placed { piece, at: Vec2::new(0.0, 0.0), size: [pw, ph], turned }], too_big: true });
            rows.push(Vec::new());
            continue;
        }
        let mut placed = false;
        'sheets: for (s, plan) in plans.iter_mut().enumerate() {
            if plan.too_big {
                continue;
            }
            for row in rows[s].iter_mut() {
                let x = if row.2 > 0.0 { row.2 + kerf } else { 0.0 };
                if ph <= row.1 + 1e-6 && x + pw <= long + 1e-6 {
                    plan.pieces.push(Placed { piece, at: Vec2::new(x, row.0), size: [pw, ph], turned });
                    row.2 = x + pw;
                    placed = true;
                    break 'sheets;
                }
            }
            let y = rows[s].last().map_or(0.0, |r| r.0 + r.1 + kerf);
            if y + ph <= short + 1e-6 {
                plan.pieces.push(Placed { piece, at: Vec2::new(0.0, y), size: [pw, ph], turned });
                rows[s].push((y, ph, pw));
                placed = true;
                break;
            }
        }
        if !placed {
            plans.push(SheetPlan { pieces: vec![Placed { piece, at: Vec2::new(0.0, 0.0), size: [pw, ph], turned }], too_big: false });
            rows.push(vec![(0.0, ph, pw)]);
        }
    }
    plans
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

    /// No gap, every cut-out as scanned: exact areas.
    fn exact() -> Options<'static> {
        Options { clearance: 0.0, ..Options::default() }
    }

    fn area_of(l: &Laid) -> f64 {
        twice_area(&l.outline).abs() / 2.0 - l.inside.iter().map(|r| twice_area(r).abs() / 2.0).sum::<f64>()
    }

    #[test]
    fn the_wheel_well_and_the_top_slot_are_notches_and_dropping_one_covers_it() {
        let made = template_of(wall_with_well());
        let p = propose(&made, &[], None, &exact()).expect("proposal");
        let notches: Vec<(f64, f64)> = p.cuts.iter().filter(|c| c.notch).map(|c| (c.depth, c.area)).collect();
        // The well: 280 deep over the bottom edge at 20, 800 × 280; the slot: 40 × 100.
        assert_eq!(notches.len(), 2, "{notches:?}");
        assert!(notches.iter().any(|(d, a)| (d - 280.0).abs() < 1e-6 && (a - 800.0 * 280.0).abs() < 1e-3), "{notches:?}");
        assert!(notches.iter().any(|(d, a)| (d - 40.0).abs() < 1e-6 && (a - 100.0 * 40.0).abs() < 1e-3), "{notches:?}");
        let full = area_of(&lay(&p, &Keep::all(&p)));
        let well = p.cuts.iter().position(|c| (c.depth - 280.0).abs() < 1e-6).expect("well");
        let mut keep = Keep::all(&p);
        keep.cuts[well] = false;
        let covered = area_of(&lay(&p, &keep));
        assert!((full - (4000.0 * 1980.0 - 800.0 * 280.0 - 4000.0)).abs() < 1e-3, "{full}");
        assert!((covered - full - 800.0 * 280.0).abs() < 1e-3, "{covered} {full}");
    }

    /// A slot 100 wide is cut as a box; the wheel well, 800 wide, with straight sides. A ragged
    /// notch is straightened by filling its dents, never cutting inside what was found.
    #[test]
    fn small_cut_outs_are_boxes_and_large_ones_get_straight_sides() {
        let mut outline = wall_with_well();
        // A ragged slot in the top: 3 mm teeth along its bottom.
        let top = outline.iter().position(|q| *q == Vec2::new(2100.0, 1960.0)).expect("slot");
        outline.splice(top..=top, [Vec2::new(2100.0, 1960.0), Vec2::new(2066.0, 1963.0), Vec2::new(2033.0, 1960.0)]);
        let made = template_of(outline);
        let p = propose(&made, &[], None, &exact()).expect("proposal");
        let slot = p.cuts.iter().find(|c| c.depth < 50.0).expect("slot");
        let well = p.cuts.iter().find(|c| c.depth > 200.0).expect("well");
        assert_eq!((slot.shape, well.shape), (Shape::Box, Shape::Straight));
        // The slot as a box: 100 × 40 whole, its tooth gone.
        let l = lay(&p, &Keep::all(&p));
        assert!((area_of(&l) - (4000.0 * 1980.0 - 800.0 * 280.0 - 4000.0)).abs() < 1e-3, "{}", area_of(&l));
        // Straightened: never inside the ring found, and dents filled.
        let ragged = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 50.0),
            Vec2::new(60.0, 50.0),
            Vec2::new(50.0, 40.0),
            Vec2::new(40.0, 50.0),
            Vec2::new(0.0, 50.0),
        ];
        let st = straightened(&ragged, 15.0);
        assert_eq!(st.len(), 4, "the 10 mm dent filled: {st:?}");
        assert!(ragged.iter().all(|q| contains(&st, *q) || ring_distance(&st, *q) < 1e-9), "{st:?}");
    }

    /// The gap: the panel comes in 3 mm from every edge, a box cut-out grows 3 mm all round, and a
    /// seam is two cut lines 3 mm apart.
    #[test]
    fn the_gap_comes_off_every_edge_cut_out_and_seam() {
        let rect = vec![Vec2::new(0.0, 0.0), Vec2::new(3000.0, 0.0), Vec2::new(3000.0, 1000.0), Vec2::new(0.0, 1000.0)];
        let mut outline = rect.clone();
        outline.splice(1..1, [Vec2::new(1000.0, 0.0), Vec2::new(1000.0, 100.0), Vec2::new(1100.0, 100.0), Vec2::new(1100.0, 0.0)]);
        let made = template_of(outline);
        let p = propose(&made, &[], None, &Options::default()).expect("proposal");
        assert_eq!(p.cuts.len(), 1, "{:?}", p.cuts);
        let mut keep = Keep::all(&p);
        keep.moved = vec![Seam { from: Vec2::new(2000.0, 0.0), to: Vec2::new(2000.0, 1000.0), upright: true, between_rows: false }];
        keep.seams = vec![true];
        let mut p2 = p.clone();
        p2.seams = keep.moved.clone();
        let l = lay(&p2, &keep);
        let want = 2994.0 * 994.0 - 106.0 * (100.0 + 3.0 - 3.0);
        assert!((area_of(&l) - want).abs() < 1e-3, "{} vs {want}", area_of(&l));
        let lines = &l.seams[0];
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!((lines[0].0.x - lines[1].0.x).abs() - 3.0 < 1e-9, "{lines:?}");
        // The pieces: 2000 less 3 and 1.5, and 1000 less 1.5 and 3; 1000 less 6 high.
        let sizes = piece_sizes(&p2, &keep);
        assert_eq!(sizes, vec![[1995.5, 994.0], [995.5, 994.0]]);
    }

    /// Flat metal 20 mm behind the panel all over but a 400 mm gap: fixing points every 300 mm or
    /// closer along the edges, on the metal, 20 mm in at least; where the metal is missing, none.
    #[test]
    fn fixing_points_go_on_flat_metal_where_an_edge_has_no_hole() {
        let rect = vec![Vec2::new(0.0, 0.0), Vec2::new(1000.0, 0.0), Vec2::new(1000.0, 600.0), Vec2::new(0.0, 600.0)];
        let made = template_of(rect);
        let mut positions = Vec::new();
        for j in 0..=300 {
            for i in 0..=500 {
                let (x, y) = (2.0 * f64::from(i), 2.0 * f64::from(j));
                if !(300.0..700.0).contains(&x) || (150.0..450.0).contains(&y) {
                    positions.push(Vec3::new(x, y, -20.0));
                }
            }
        }
        let scan = Scan::new(&positions, &[]);
        let b = backing(&scan, &made.frame).expect("backing");
        assert!(b.normal().dist(Vec3::Z) < 1e-12);
        let p = propose(&made, &[], Some(&b), &Options::default()).expect("proposal");
        assert!(p.fixings.len() >= 6 && p.unbacked >= 2, "{} fixings, {} unbacked", p.fixings.len(), p.unbacked);
        let inset = vec![Vec2::new(3.0, 3.0), Vec2::new(997.0, 3.0), Vec2::new(997.0, 597.0), Vec2::new(3.0, 597.0)];
        for f in &p.fixings {
            assert!((f.behind - 20.0).abs() < 1e-3, "{f:?}");
            assert!(ring_distance(&inset, f.at) >= 15.0 - 1e-9, "{f:?}");
            assert!(!((300.0..700.0).contains(&f.at.x) && !(150.0..450.0).contains(&f.at.y)), "on the metal: {f:?}");
        }
        // A hole near an edge fastens it: no fixing point within its reach on that side.
        let hole = Hole { at: Vec2::new(150.0, 40.0), diameter: 8.0, behind: 20.0 };
        let p = propose(&made, &[hole], Some(&b), &Options::default()).expect("proposal");
        assert!(p.fixings.iter().all(|f| f.at.dist(hole.at) > 60.0), "{:?}", p.fixings);
    }

    /// Six pieces on 2440 × 1220 sheets in rows a saw cuts straight: four sheets, none overlapping
    /// or off its sheet; a piece longer than the sheet on one of its own.
    #[test]
    fn pieces_are_laid_on_sheets_in_straight_rows() {
        let sizes = [[2400.0, 411.0], [1251.0, 400.0], [2400.0, 700.0], [1513.0, 1042.0], [2400.0, 529.0], [1506.0, 536.0]];
        let plans = sheets(&sizes, SHEET, KERF);
        assert_eq!(plans.len(), 4, "{plans:?}");
        for plan in &plans {
            for (k, a) in plan.pieces.iter().enumerate() {
                assert!(a.at.x + a.size[0] <= 2440.0 + 1e-6 && a.at.y + a.size[1] <= 1220.0 + 1e-6, "{a:?}");
                for b in plan.pieces.iter().skip(k + 1) {
                    let apart =
                        a.at.x + a.size[0] <= b.at.x || b.at.x + b.size[0] <= a.at.x || a.at.y + a.size[1] <= b.at.y || b.at.y + b.size[1] <= a.at.y;
                    assert!(apart, "{a:?} {b:?}");
                }
            }
        }
        let long = sheets(&[[2468.0, 529.0], [1000.0, 500.0]], SHEET, KERF);
        assert!(long.iter().any(|p| p.too_big && p.pieces[0].piece == 0), "{long:?}");
    }

    /// A seam that leaves a piece 50 mm past the sheet: the move offered brings it within.
    #[test]
    fn a_seam_move_is_offered_when_a_piece_is_longer_than_the_sheet() {
        let rect = vec![Vec2::new(0.0, 0.0), Vec2::new(3900.0, 0.0), Vec2::new(3900.0, 1000.0), Vec2::new(0.0, 1000.0)];
        let made = template_of(rect);
        let mut p = propose(&made, &[], None, &Options::default()).expect("proposal");
        p.seams = vec![Seam { from: Vec2::new(2490.0, 0.0), to: Vec2::new(2490.0, 1000.0), upright: true, between_rows: true }];
        let mut keep = Keep::all(&p);
        assert!(piece_sizes(&p, &keep).iter().any(|[w, _]| *w > 2440.0));
        let moves = fit_moves(&p, &keep);
        assert_eq!(moves.len(), 1, "{moves:?}");
        let (i, by) = moves[0];
        keep.moved[i].from.x += by;
        keep.moved[i].to.x += by;
        assert!(piece_sizes(&p, &keep).iter().all(|[w, h]| *w <= 2440.0 && *h <= 1220.0), "{:?}", piece_sizes(&p, &keep));
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
        let p = propose(&made, &holes, None, &Options::default()).expect("proposal");
        let across: Vec<&Seam> = p.seams.iter().filter(|s| !s.upright).collect();
        let up: Vec<&Seam> = p.seams.iter().filter(|s| s.upright).collect();
        assert_eq!(up.len(), 1, "{:?}", p.seams);
        assert!((up[0].from.x - 2420.0).abs() < 1.0 && up[0].between_rows, "{:?}", up[0]);
        // Height 1980 needs one seam across at least; the paired rows give the one between them.
        assert!(across.iter().any(|s| s.between_rows && (s.from.y - 517.0).abs() < 2.0), "{across:?}");
        let sizes = piece_sizes(&p, &Keep::all(&p));
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
        let with = |edges: Option<Edges>, sheet: [f64; 2], clearance: f64| Options { edges, sheet, clearance, ..Options::default() };
        assert!(propose(&made, &[], None, &with(Some(Edges { left: 10.0, right: 5.0, bottom: 0.0, top: 1.0 }), SHEET, 3.0)).is_none());
        assert!(propose(&made, &[], None, &with(None, [f64::NAN, 1.0], 3.0)).is_none());
        assert!(propose(&made, &[], None, &with(Some(Edges { left: 5000.0, right: 6000.0, bottom: 0.0, top: 10.0 }), SHEET, 3.0)).is_none());
        assert!(propose(&made, &[], None, &with(None, SHEET, f64::NAN)).is_none());
        assert!(propose(&made, &[], None, &with(None, SHEET, 5000.0)).is_none());
    }

    /// Cut along a seam across and one up, with the gap: four pieces, numbered top row first, each
    /// stopping half the gap short of the seams; holes go with the piece they are on, in its own
    /// coordinates from its blank's corner.
    #[test]
    fn the_panel_is_cut_into_pieces_along_the_seams() {
        let rect = vec![Vec2::new(0.0, 0.0), Vec2::new(2000.0, 0.0), Vec2::new(2000.0, 1000.0), Vec2::new(0.0, 1000.0)];
        let made = template_of(rect);
        let holes = [
            Hole { at: Vec2::new(100.0, 480.0), diameter: 8.0, behind: 12.0 },
            Hole { at: Vec2::new(1900.0, 520.0), diameter: 8.0, behind: 12.0 },
            Hole { at: Vec2::new(1020.0, 900.0), diameter: 8.0, behind: 12.0 },
        ];
        let mut p = propose(&made, &holes, None, &Options { spacing: 0.0, ..Options::default() }).expect("proposal");
        p.seams = vec![
            Seam { from: Vec2::new(0.0, 500.0), to: Vec2::new(2000.0, 500.0), upright: false, between_rows: true },
            Seam { from: Vec2::new(1000.0, 0.0), to: Vec2::new(1000.0, 1000.0), upright: true, between_rows: true },
        ];
        let keep = Keep::all(&p);
        let ps = pieces(&p, &keep);
        assert_eq!(ps.len(), 4, "{ps:?}");
        // Top left: x 3..998.5, y 501.5..997.
        let tl = &ps[0];
        assert!((tl.origin.x - 3.0).abs() < 1e-9 && (tl.origin.y - 501.5).abs() < 1e-9, "{:?}", tl.origin);
        assert!((tl.blank[0] - 995.5).abs() < 1e-9 && (tl.blank[1] - 495.5).abs() < 1e-9, "{:?}", tl.blank);
        let areas: f64 = ps.iter().map(|q| twice_area(&q.outline).abs() / 2.0).sum();
        assert!((areas - (1994.0 * 994.0 - 3.0 * 1994.0 - 3.0 * 991.0)).abs() < 1e-6, "{areas}");
        // The hole at (1020, 900) is on the top right piece, 18.5 in from its left edge.
        let tr = &ps[1];
        let h = tr.points.iter().find(|q| q.on_panel == Vec2::new(1020.0, 900.0)).expect("hole");
        // Numbered by rows from the bottom: the hole at (1900, 520), 18.5 up, is the first.
        assert!((h.at.x - 18.5).abs() < 1e-9 && (h.at.y - 398.5).abs() < 1e-9 && h.number == 2, "{h:?}");
        let cs = checks(&ps);
        assert!(!cs.is_empty() && cs.iter().all(|c| c.distance > 0.0), "{cs:?}");
        assert!(cs.iter().any(|c| (c.distance - Vec2::new(100.0, 480.0).dist(Vec2::new(1900.0, 520.0))).abs() < 1e-9), "{cs:?}");
    }
}
