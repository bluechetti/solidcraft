//! Furring strips for a scanned wall (or ceiling): strips bolted through the frame's own holes,
//! each strip's back following the scanned metal and its front on a flat plane, so flat panels
//! screw straight onto them. Made to be 3D printed: each strip is cut into segments that fit a
//! printer, laid flat on its front.
//!
//! The wall is looked at in its own frame: `up` (the way the strips run, world Z for a side wall),
//! `normal` (out of the wall, to the room) and `x = up × normal` along it. Heights are along `up`.
//! - The wall's front, cell by cell: the frontmost scanned surface toward the room.
//! - Bands: heights where the wall's front stays at one level (a wall whose lower rail stands
//!   further in than its upper frame has two); each band gets its own plane, set just in front of
//!   the metal (a high share of each row behind it), leaning with the wall when asked.
//! - Strips: along the columns the frame's holes make in a band, each bolted through those holes.
//!   A strip's back is the metal (a print gap off it), its front the band's plane.
//! - Segments: each strip cut between its holes into pieces no longer than the printer takes.

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::panel;

/// What the strips are made with.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The way the strips run (heights are along it): world Z for a side wall.
    pub up: Vec3,
    /// A point in the room: the wall's normal is turned toward it. None: the plane the wall rests
    /// on decides.
    pub toward: Option<Vec3>,
    /// The bands' heights `[from, to]` along `up` (world, mm); empty: found from the wall.
    pub bands: Vec<[f64; 2]>,
    /// Each band's plane leans with the wall (about the wall's own axis), at most [`MOST_LEAN`].
    pub lean: bool,
    /// The share of each row of the wall's front left behind the plane (0.5..1).
    pub tight: f64,
    /// How far the strips' fronts (the panel's back) stand in front of the frame (mm): 0 is
    /// flush, the panel on the frame itself and strips only where the metal lies behind it.
    pub standoff: f64,
    /// The strip's width, its least thickness and the deepest it reaches behind its front (mm).
    pub width: f64,
    pub thickness: f64,
    pub deepest: f64,
    /// The gap between the strip's back and the metal (mm): print tolerance.
    pub gap: f64,
    /// The panel's thickness (mm): the finished face is this far in front of the battens.
    pub panel: f64,
    /// Battens across the brackets: their thickness (mm; 0: none, the strips fill behind the
    /// panel instead), width and spacing (mm, middle to middle at most).
    pub batten: f64,
    pub batten_width: f64,
    pub batten_spacing: f64,
    /// The bolt's clearance hole, its head's counterbore and how deep that is (mm).
    pub bolt: f64,
    pub head: f64,
    pub head_depth: f64,
    /// The longest segment the printer takes (mm).
    pub segment: f64,
    /// Strips closer than this (mm, middle to middle) are not both made.
    pub spacing: f64,
    /// The most a strip leans off `up` to reach its holes (degrees).
    pub slant: f64,
    /// The cell of the wall's front (mm).
    pub cell: f64,
    /// Rectangles on the wall left out (each by two opposite corners, in space): a wheel arch, a
    /// door pillar; nothing there sets a plane, carries a strip or is reported proud.
    pub skip: Vec<[Vec3; 2]>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            up: Vec3::new(0.0, 0.0, 1.0),
            toward: None,
            bands: Vec::new(),
            lean: true,
            tight: 0.98,
            standoff: 0.0,
            width: 40.0,
            thickness: 8.0,
            deepest: 25.0,
            gap: 0.5,
            panel: 6.0,
            batten: 19.0,
            batten_width: 38.0,
            batten_spacing: 400.0,
            bolt: 6.6,
            head: 13.0,
            head_depth: 5.0,
            segment: 330.0,
            spacing: 250.0,
            slant: 3.0,
            cell: 2.0,
            skip: Vec::new(),
        }
    }
}

/// The most a band's plane leans off the wall's frame (degrees).
pub const MOST_LEAN: f64 = 6.0;
/// The most bands, strips, segments and places proud of the panel reported.
pub const MOST_BANDS: usize = 8;
pub const MOST_STRIPS: usize = 200;
pub const MOST_SEGMENTS: usize = 2_000;
pub const MOST_PROUD: usize = 100;
pub const MOST_SKIP: usize = 100;
/// The most of the frame's holes looked at for strips.
const MOST_HOLES: usize = 600;
/// The most cells the wall's front has: a larger wall gets larger cells.
const MOST_CELLS: f64 = 30_000_000.0;
/// A step in the wall's front this large (mm) over [`STEP_OVER`] (mm) parts two bands; a band
/// shorter than [`SHORTEST_BAND`] (mm) is left out.
const STEP: f64 = 30.0;
const STEP_OVER: f64 = 50.0;
const SHORTEST_BAND: f64 = 150.0;
/// A strip runs at least this far (mm). Larger holes than [`LARGEST_BOLT_HOLE`] are access
/// holes, not for bolts.
const COLUMN_SPAN: f64 = 80.0;
const LARGEST_BOLT_HOLE: f64 = 16.0;
/// Metal this far (mm) or more behind the plane needs a strip; a strip crosses less than
/// [`BLIP`] (mm) of metal on the plane, and runs at most [`MOST_RUN`] (mm) from its hole.
const RECESSED: f64 = 2.0;
/// A bracket carries on past its end holes this far (mm).
const BRACKET_END: f64 = 30.0;
const BLIP: f64 = 20.0;
const MOST_RUN: f64 = 3_000.0;
/// Holes in one strip closer than this (mm, centre to centre) keep only the first.
const HOLE_APART: f64 = 4.0;
/// The most a band's plane is moved out for its strips (mm).
const MOST_RAISE: f64 = 3.0;
/// The most a strip comes forward of the nearest band's plane to hug metal past the bands (mm).
const MOST_HUG: f64 = 200.0;
/// Cells either side looked at to keep a strip whole across a step.
const STEP_CELLS: i32 = 3;
/// How far a bolt's boss reaches round its head (mm).
const BOSS: f64 = 4.0;
/// The least material under a counterbore (mm); thinner, the bolt's head sits on the strip's front.
const UNDER_HEAD: f64 = 3.0;
/// A place proud of the panel is reported from this area (mm²).
const PROUD_AREA: f64 = 200.0;

/// Why there are no strips.
#[derive(Clone, Debug, PartialEq)]
pub enum FurringError {
    /// The scan has too few points, or `up` has no direction across the wall.
    NoWall,
    /// No band of the wall was found or given.
    NoBand,
    /// An option out of range.
    Bad(&'static str),
}

impl std::fmt::Display for FurringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FurringError::NoWall => write!(f, "no wall to fur: too few scanned points, or `up` runs square to the wall"),
            FurringError::NoBand => write!(f, "no band of the wall is scanned evenly enough for a plane: give `bands`"),
            FurringError::Bad(why) => write!(f, "{why}"),
        }
    }
}

/// The wall's frame and its front.
#[derive(Clone, Debug)]
pub struct Wall {
    /// `origin` lies at height 0; `x` along the wall, `y` = up, `normal` = x × y toward the room.
    pub frame: Plane,
    pub normal: Vec3,
    front: Front,
}

/// The frontmost scanned surface over the wall's frame, cell by cell (offset along the normal).
#[derive(Clone, Debug)]
struct Front {
    lo: Vec2,
    cell: f64,
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Front {
    fn get(&self, i: usize, j: usize) -> Option<f64> {
        if i >= self.w || j >= self.h {
            return None;
        }
        self.v.get(j * self.w + i).filter(|x| !x.is_nan()).map(|x| f64::from(*x))
    }
    fn index(&self, q: Vec2) -> Option<(usize, usize)> {
        let (fi, fj) = ((q.x - self.lo.x) / self.cell, (q.y - self.lo.y) / self.cell);
        (fi >= 0.0 && fj >= 0.0 && (fi as usize) < self.w && (fj as usize) < self.h).then_some((fi as usize, fj as usize))
    }
    /// The highest front within one cell of `q`: what a strip's back must clear there.
    fn back(&self, q: Vec2) -> Option<f64> {
        let (i, j) = self.index(q)?;
        let mut best: Option<f64> = None;
        for jj in j.saturating_sub(1)..=j + 1 {
            for ii in i.saturating_sub(1)..=i + 1 {
                if let Some(v) = self.get(ii, jj) {
                    best = Some(best.map_or(v, |b: f64| b.max(v)));
                }
            }
        }
        best
    }
    fn row_height(&self, j: usize) -> f64 {
        self.lo.y + (j as f64 + 0.5) * self.cell
    }
    fn col_u(&self, i: usize) -> f64 {
        self.lo.x + (i as f64 + 0.5) * self.cell
    }
}

/// The wall `scan` in its frame (see the module), its front on cells of `cell` (mm, grown on a
/// very large wall).
pub fn wall(scan: &Scan, up: Vec3, toward: Option<Vec3>, cell: f64, skip: &[[Vec3; 2]]) -> Result<Wall, FurringError> {
    let up = up.normalized().ok_or(FurringError::NoWall)?;
    let rest = crate::wall_outline::rest(scan).ok_or(FurringError::NoWall)?;
    let n0 = rest.frame.x.cross(rest.frame.y);
    let mut normal = (n0 - up * n0.dot(up)).normalized().ok_or(FurringError::NoWall)?;
    let (lo, hi) = scan.bounds().ok_or(FurringError::NoWall)?;
    let centre = (lo + hi) * 0.5;
    if let Some(t) = toward
        && (t - centre).dot(normal) < 0.0
    {
        normal = -normal;
    }
    let x = up.cross(normal).normalized().ok_or(FurringError::NoWall)?;
    let origin = centre - up * centre.dot(up);
    let frame = Plane { origin, x, y: up };
    let to = |p: Vec3| {
        let d = p - origin;
        (d.dot(x), d.dot(up), d.dot(normal))
    };
    let (mut ulo, mut uhi, mut hlo, mut hhi) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in scan.points() {
        let (u, h, _) = to(p);
        ulo = ulo.min(u);
        uhi = uhi.max(u);
        hlo = hlo.min(h);
        hhi = hhi.max(h);
    }
    if !(uhi > ulo && hhi > hlo) || !(cell.is_finite() && cell > 0.0) {
        return Err(FurringError::NoWall);
    }
    let cell = cell.max(((uhi - ulo) * (hhi - hlo) / MOST_CELLS).sqrt());
    let w = ((uhi - ulo) / cell) as usize + 2;
    let h = ((hhi - hlo) / cell) as usize + 2;
    let mut front = Front { lo: Vec2::new(ulo, hlo), cell, w, h, v: vec![f32::NAN; w * h] };
    let put = |front: &mut Front, i: usize, j: usize, val: f64| {
        if let Some(c) = front.v.get_mut(j * w + i)
            && (c.is_nan() || val as f32 > *c)
        {
            *c = val as f32;
        }
    };
    for t in 0..scan.triangle_count() {
        let Some(c) = scan.corners(t) else { continue };
        let q = c.map(to);
        for &(u, hh, ww) in &q {
            if let Some((i, j)) = front.index(Vec2::new(u, hh)) {
                put(&mut front, i, j, ww);
            }
        }
        // Cells whose middle the triangle covers, its height there.
        let (a, b, cc) = (q[0], q[1], q[2]);
        let det = (b.0 - a.0) * (cc.1 - a.1) - (cc.0 - a.0) * (b.1 - a.1);
        if !(det.abs() > 1e-9) {
            continue;
        }
        let (umin, umax) = (a.0.min(b.0).min(cc.0), a.0.max(b.0).max(cc.0));
        let (vmin, vmax) = (a.1.min(b.1).min(cc.1), a.1.max(b.1).max(cc.1));
        if umax - umin < cell && vmax - vmin < cell {
            continue;
        }
        let (i0, i1) = (((umin - ulo) / cell).floor().max(0.0) as usize, ((umax - ulo) / cell).ceil().max(0.0) as usize);
        let (j0, j1) = (((vmin - hlo) / cell).floor().max(0.0) as usize, ((vmax - hlo) / cell).ceil().max(0.0) as usize);
        for j in j0..=j1.min(h - 1) {
            for i in i0..=i1.min(w - 1) {
                let (pu, pv) = (front.col_u(i), front.row_height(j));
                let l1 = ((b.0 - pu) * (cc.1 - pv) - (cc.0 - pu) * (b.1 - pv)) / det;
                let l2 = ((cc.0 - pu) * (a.1 - pv) - (a.0 - pu) * (cc.1 - pv)) / det;
                let l3 = 1.0 - l1 - l2;
                if l1 >= -1e-9 && l2 >= -1e-9 && l3 >= -1e-9 {
                    put(&mut front, i, j, l1 * a.2 + l2 * b.2 + l3 * cc.2);
                }
            }
        }
    }
    // Pinholes in the scan filled from their neighbours (two passes: up to two cells).
    for _ in 0..2 {
        let old = front.v.clone();
        for j in 0..h {
            for i in 0..w {
                if !old[j * w + i].is_nan() {
                    continue;
                }
                let mut best = f32::NAN;
                let mut seen = 0;
                for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (ii, jj) = (i as i64 + di, j as i64 + dj);
                    if ii < 0 || jj < 0 || ii >= w as i64 || jj >= h as i64 {
                        continue;
                    }
                    let v = old[jj as usize * w + ii as usize];
                    if !v.is_nan() {
                        seen += 1;
                        best = if best.is_nan() { v } else { best.max(v) };
                    }
                }
                if seen >= 2 {
                    front.v[j * w + i] = best;
                }
            }
        }
    }
    for [a, b] in skip {
        let ((ua, ha, _), (ub, hb, _)) = (to(*a), to(*b));
        for j in 0..h {
            for i in 0..w {
                let (u, hh) = (front.col_u(i), front.row_height(j));
                if (ua.min(ub)..=ua.max(ub)).contains(&u) && (ha.min(hb)..=ha.max(hb)).contains(&hh) {
                    front.v[j * w + i] = f32::NAN;
                }
            }
        }
    }
    Ok(Wall { frame, normal, front })
}

/// One of the frame's holes a strip is bolted through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripHole {
    /// Its centre on the wall (u along, h up) and in space (on the metal), its scanned diameter.
    pub at: Vec2,
    pub center: Vec3,
    pub diameter: f64,
    /// How far along the strip from its start (mm).
    pub along: f64,
    /// The strip's thickness there (mm), and whether the bolt's head sits in a counterbore.
    pub thickness: f64,
    pub counterbore: bool,
}

/// A printed piece of a strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// Its ends (mm along the strip), the holes in it (indices into the strip's) and its volume (mm³).
    pub from: f64,
    pub to: f64,
    pub holes: Vec<usize>,
    pub volume: f64,
    /// The closed triangle mesh, in space.
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

/// A strip: a straight run on the wall from `start` to `end` (u, h), bolted through the frame's
/// holes under it, its back on the metal and its front on the bands' planes.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub start: Vec2,
    pub end: Vec2,
    pub holes: Vec<StripHole>,
    pub segments: Vec<Segment>,
    /// Its thinnest and thickest (mm).
    pub thinnest: f64,
    pub thickest: f64,
}

impl Strip {
    pub fn length(&self) -> f64 {
        self.start.dist(self.end)
    }
}

/// A place in a band where the metal stands in front of the plane the panel's back lies on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proud {
    pub center: Vec3,
    /// How far in front at most (mm), its area (mm²) and its size along and up the wall (mm).
    pub most: f64,
    pub area: f64,
    pub size: [f64; 2],
}

/// A band of the wall and its plane: strips' fronts lie on `front = level + lean·h` (offset along
/// the wall's normal), the finished panel `panel` further on.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub from: f64,
    pub to: f64,
    /// The plane of the strips' fronts: offset at height 0 and its rise per mm of height.
    pub level: f64,
    pub lean: f64,
    /// How far the plane was moved out so every strip has its least thickness (mm).
    pub raised: f64,
    /// The frame's holes found in the band.
    pub holes_found: usize,
    pub proud: Vec<Proud>,
}

impl Band {
    /// The strips' front plane's offset at height `h`.
    pub fn at(&self, h: f64) -> f64 {
        self.level + self.lean * h
    }
}

/// Furring for a whole wall.
#[derive(Clone, Debug)]
pub struct Plan {
    pub wall: Wall,
    pub bands: Vec<Band>,
    pub strips: Vec<Strip>,
    pub battens: Vec<Batten>,
    /// Batten lines left out because fewer than two brackets cross them.
    pub unsupported: usize,
    pub options: Options,
}

/// A wooden batten on the brackets' plane, across two or more brackets.
#[derive(Clone, Debug, PartialEq)]
pub struct Batten {
    /// Its ends (middle of its back, in space), its length (mm) and the brackets it crosses.
    pub start: Vec3,
    pub end: Vec3,
    pub length: f64,
    pub crosses: usize,
    /// A box, for showing it.
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl Plan {
    /// The point of `band`'s finished panel face (strips' fronts plus the panel) at (u, h), and
    /// the face's normal.
    pub fn face(&self, band: &Band, u: f64, h: f64) -> (Vec3, Vec3) {
        let f = &self.wall.frame;
        let p = f.origin + f.x * u + f.y * h + self.wall.normal * (band.at(h) + self.options.batten + self.options.panel);
        let n = (self.wall.normal - f.y * band.lean).normalized().unwrap_or(self.wall.normal);
        (p, n)
    }
    /// The band holding height `h`, if any.
    pub fn band_at(&self, h: f64) -> Option<&Band> {
        self.bands.iter().find(|b| (b.from..=b.to).contains(&h))
    }
    /// The middle of the wall along it (u, mm).
    pub fn middle(&self) -> f64 {
        self.wall.front.lo.x + self.wall.front.w as f64 * self.wall.front.cell / 2.0
    }
    /// The bands' planes at `h`: in a band its plane; between two, the one further to the room;
    /// past them all, the nearest band's.
    pub fn front(&self, h: f64) -> f64 {
        front_of(&self.bands, h)
    }
}

/// The deepest a strip's back reaches at `h` (offset along the normal): `deepest` behind the
/// band's plane; between two bands, behind the one further back, so the strip is whole through
/// the step.
fn floor_of(bands: &[Band], h: f64, deepest: f64) -> f64 {
    if let Some(b) = bands.iter().find(|b| (b.from..=b.to).contains(&h)) {
        return b.at(h) - deepest;
    }
    let below = bands.iter().filter(|b| b.to < h).max_by(|a, c| a.to.partial_cmp(&c.to).unwrap_or(std::cmp::Ordering::Equal));
    let above = bands.iter().filter(|b| b.from > h).min_by(|a, c| a.from.partial_cmp(&c.from).unwrap_or(std::cmp::Ordering::Equal));
    match (below, above) {
        (Some(a), Some(c)) => a.at(h).min(c.at(h)) - deepest,
        (Some(a), None) => a.at(h) - deepest,
        (None, Some(c)) => c.at(h) - deepest,
        (None, None) => -deepest,
    }
}

fn front_of(bands: &[Band], h: f64) -> f64 {
    if let Some(b) = bands.iter().find(|b| (b.from..=b.to).contains(&h)) {
        return b.at(h);
    }
    let below = bands.iter().filter(|b| b.to < h).max_by(|a, c| a.to.partial_cmp(&c.to).unwrap_or(std::cmp::Ordering::Equal));
    let above = bands.iter().filter(|b| b.from > h).min_by(|a, c| a.from.partial_cmp(&c.from).unwrap_or(std::cmp::Ordering::Equal));
    match (below, above) {
        (Some(a), Some(c)) => a.at(h).max(c.at(h)),
        (Some(a), None) => a.at(h),
        (None, Some(c)) => c.at(h),
        (None, None) => 0.0,
    }
}

fn percentile(v: &mut [f64], share: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let k = ((v.len() - 1) as f64 * share.clamp(0.0, 1.0)).round() as usize;
    v.get(k).copied()
}

/// Each row's level: the front `tight` of the row's scanned cells lie behind, where enough of the
/// row is scanned.
fn row_levels(front: &Front, tight: f64) -> Vec<Option<f64>> {
    (0..front.h)
        .map(|j| {
            let mut row: Vec<f64> = (0..front.w).filter_map(|i| front.get(i, j)).collect();
            if row.len() < 50 {
                return None;
            }
            percentile(&mut row, tight)
        })
        .collect()
}

/// Bands where the rows' levels have no step of [`STEP`] within [`STEP_OVER`].
fn found_bands(front: &Front, levels: &[Option<f64>]) -> Vec<[f64; 2]> {
    let k = ((STEP_OVER / front.cell).round() as usize).max(1);
    let mut cuts = vec![false; levels.len()];
    for j in 0..levels.len().saturating_sub(k) {
        if let (Some(a), Some(b)) = (levels[j], levels[j + k])
            && (a - b).abs() > STEP
        {
            for c in cuts.iter_mut().skip(j).take(k + 1) {
                *c = true;
            }
        }
    }
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for j in 0..=levels.len() {
        let inside = j < levels.len() && !cuts[j] && levels[j].is_some();
        match (inside, start) {
            (true, None) => start = Some(j),
            (false, Some(s)) => {
                let (a, b) = (front.row_height(s), front.row_height(j - 1));
                if b - a >= SHORTEST_BAND {
                    out.push([a, b]);
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// The line `level + lean·h` over every point (h, l), as close to them as it can be, leaning at
/// most [`MOST_LEAN`]: the edge of their upper hull over their mean height.
fn over_line(points: &[(f64, f64)], lean: bool) -> Option<(f64, f64)> {
    let top = points.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    if points.is_empty() || !top.is_finite() {
        return None;
    }
    if !lean || points.len() < 2 {
        return Some((top, 0.0));
    }
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut hull: Vec<(f64, f64)> = Vec::new();
    for p in pts {
        while hull.len() >= 2 {
            let (a, b) = (hull[hull.len() - 2], hull[hull.len() - 1]);
            // Upper hull: drop b where it lies on or under a → p.
            if (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0) >= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(p);
    }
    let mean = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let most = MOST_LEAN.to_radians().tan();
    let slope = hull
        .windows(2)
        .find(|w| w[0].0 <= mean && mean <= w[1].0 && w[1].0 > w[0].0)
        .map_or(0.0, |w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0))
        .clamp(-most, most);
    let level = points.iter().map(|p| p.1 - slope * p.0).fold(f64::MIN, f64::max);
    level.is_finite().then_some((level, slope))
}

/// The furring for the wall `scan`.
pub fn plan(scan: &Scan, options: &Options) -> Result<Plan, FurringError> {
    let o = options;
    let ok = |v: f64, lo: f64, hi: f64| v.is_finite() && v >= lo && v <= hi;
    if !ok(o.tight, 0.5, 1.0) {
        return Err(FurringError::Bad("`tight` must be a share from 0.5 to 1"));
    }
    if !ok(o.batten, 0.0, 100.0) || !ok(o.batten_width, 10.0, 200.0) || !ok(o.batten_spacing, 100.0, 2_000.0) {
        return Err(FurringError::Bad("batten 0–100 mm thick, 10–200 mm wide, 100–2000 mm apart"));
    }
    if !ok(o.standoff, 0.0, 200.0) {
        return Err(FurringError::Bad("`standoff` must be 0–200 mm"));
    }
    if !ok(o.width, 10.0, 300.0)
        || !ok(o.thickness, 2.0, 100.0)
        || !ok(o.deepest, o.thickness, 500.0)
        || !ok(o.gap, 0.0, 5.0)
        || !ok(o.panel, 0.0, 100.0)
    {
        return Err(FurringError::Bad("width 10–300 mm, thickness 2–100 mm, deepest from the thickness to 500 mm, gap 0–5 mm, panel 0–100 mm"));
    }
    if !ok(o.bolt, 1.0, 30.0) || !ok(o.head, o.bolt, 60.0) || !ok(o.head_depth, 0.0, 50.0) || o.head + 4.0 > o.width {
        return Err(FurringError::Bad("the bolt hole 1–30 mm, the head from the bolt to 60 mm and 4 mm narrower than the strip, its depth 0–50 mm"));
    }
    if !ok(o.segment, 50.0, 5_000.0) || !ok(o.spacing, 0.0, 10_000.0) || !ok(o.cell, 0.5, 50.0) || !ok(o.slant, 0.0, 45.0) {
        return Err(FurringError::Bad("segment 50–5000 mm, spacing 0–10000 mm, cell 0.5–50 mm, slant 0–45°"));
    }
    if o.bands.len() > MOST_BANDS || o.bands.iter().any(|b| !(b[0].is_finite() && b[1].is_finite() && b[1] > b[0])) {
        return Err(FurringError::Bad("each band is [from, to] heights, from below to"));
    }
    if o.skip.len() > MOST_SKIP || o.skip.iter().flatten().any(|p| !p.is_finite()) {
        return Err(FurringError::Bad("`skip` takes up to 100 rectangles, each two points [x, y, z]"));
    }
    let wall = wall(scan, o.up, o.toward, o.cell, &o.skip)?;
    let front = &wall.front;
    let levels = row_levels(front, o.tight);
    let mut heights = if o.bands.is_empty() { found_bands(front, &levels) } else { o.bands.clone() };
    heights.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut bands = Vec::new();
    let mut holes: Vec<panel::Hole> = Vec::new();
    let f = &wall.frame;
    for [from, to] in heights.into_iter().take(MOST_BANDS) {
        let pts: Vec<(f64, f64)> = (0..front.h)
            .filter(|&j| (from..=to).contains(&front.row_height(j)))
            .filter_map(|j| levels.get(j).copied().flatten().map(|l| (front.row_height(j), l)))
            .collect();
        let Some((level, lean)) = over_line(&pts, o.lean) else { continue };
        let mid = (from + to) / 2.0;
        let at = Plane { origin: f.origin + wall.normal * (level + lean * mid), x: f.x, y: f.y };
        let found: Vec<panel::Hole> =
            panel::frame_holes(scan, &at).into_iter().filter(|h| h.diameter <= LARGEST_BOLT_HOLE && (from..=to).contains(&h.at.y)).collect();
        bands.push(Band { from, to, level: level + o.standoff, lean, raised: 0.0, holes_found: found.len(), proud: Vec::new() });
        holes.extend(found);
    }
    if bands.is_empty() {
        return Err(FurringError::NoBand);
    }
    // The holes above the top band (a roof rail), on a plane at the metal there: strips run on to
    // them. (Below the bottom band is the floor's edge and the wheel arch.)
    let from = bands.iter().map(|b| b.to).fold(f64::MIN, f64::max);
    let to = front.row_height(front.h.saturating_sub(1));
    if to - from >= COLUMN_SPAN / 2.0 {
        let mut lv: Vec<f64> =
            (0..front.h).filter(|&j| (from..=to).contains(&front.row_height(j))).filter_map(|j| levels.get(j).copied().flatten()).collect();
        if let Some(level) = percentile(&mut lv, 0.5) {
            let at = Plane { origin: f.origin + wall.normal * level, x: f.x, y: f.y };
            holes.extend(panel::frame_holes(scan, &at).into_iter().filter(|h| h.diameter <= LARGEST_BOLT_HOLE && h.at.y > from && h.at.y < to));
        }
    }
    holes.sort_by(|a, b| (a.at.y, a.at.x).partial_cmp(&(b.at.y, b.at.x)).unwrap_or(std::cmp::Ordering::Equal));
    holes.dedup_by(|a, b| a.at.dist(b.at) < HOLE_APART);
    holes.truncate(MOST_HOLES);
    let runs = if o.batten > 0.0 { brackets(&holes, o) } else { runs(&holes, &bands, front, o) };
    // Standing off: each band's plane moved out (a little) where a strip in it would be thinner
    // than its least thickness. Flush, the plane stays on the frame.
    let half = o.width / 2.0;
    for b in bands.iter_mut().filter(|_| o.standoff > 0.0) {
        let mut raise: f64 = 0.0;
        for (a, c, _) in &runs {
            let (d, e) = axes(*a, *c);
            let len = a.dist(*c);
            let mut t = 0.0;
            while t <= len {
                let mut s = -half;
                while s <= half {
                    let q = *a + d * t + e * s;
                    if (b.from..=b.to).contains(&q.y)
                        && let Some(m) = front.back(q)
                    {
                        raise = raise.max(m + o.gap + o.thickness - b.at(q.y));
                    }
                    s += front.cell;
                }
                t += front.cell;
            }
        }
        // A small move keeps every strip whole; a bump larger than that is left proud (reported,
        // the strip thinner over it) rather than costing the room its width.
        b.raised = raise.clamp(0.0, MOST_RAISE);
        b.level += b.raised;
    }
    let strips: Vec<Strip> = runs.iter().map(|(a, c, on)| strip(&wall, &bands, *a, *c, on, o)).collect();
    for b in &mut bands {
        b.proud = proud(&wall, b);
    }
    let (battens, unsupported) = if o.batten > 0.0 { battens(&wall, &bands, &strips, o) } else { (Vec::new(), 0) };
    Ok(Plan { wall, bands, strips, battens, unsupported, options: o.clone() })
}

/// The unit along `a → c` and the unit across it, so (across, along) turns as (u, h) does.
fn axes(a: Vec2, c: Vec2) -> (Vec2, Vec2) {
    let d = (c - a).normalized().unwrap_or(Vec2::new(0.0, 1.0));
    (d, Vec2::new(d.y, -d.x))
}

/// Brackets: straight runs along the frame's members, through two or more of their holes in a
/// line within `slant` of up or of across (a pillar, a rail), the most holes and the longest
/// first, each carried on [`BRACKET_END`] past its end holes; a run beside a kept one of the same
/// way, closer than `spacing`, is left.
fn brackets(holes: &[panel::Hole], o: &Options) -> Vec<(Vec2, Vec2, Vec<panel::Hole>)> {
    let reach = o.width / 2.0 - o.bolt / 2.0 - 2.0;
    let most = o.slant.to_radians().tan();
    let under = |a: Vec2, d: Vec2, len: f64| -> Vec<panel::Hole> {
        let e = Vec2::new(d.y, -d.x);
        holes
            .iter()
            .copied()
            .filter(|h| {
                let r = h.at - a;
                (-1e-6..=len + 1e-6).contains(&r.dot(d)) && r.dot(e).abs() <= reach
            })
            .collect()
    };
    let mut candidates: Vec<(usize, f64, Vec2, Vec2)> = Vec::new();
    for (i, a) in holes.iter().enumerate() {
        for c in holes.iter().skip(i + 1) {
            let r = c.at - a.at;
            let len = r.len();
            if len < COLUMN_SPAN || !(r.x.abs() <= most * r.y.abs() || r.y.abs() <= most * r.x.abs()) {
                continue;
            }
            let (d, _) = axes(a.at, c.at);
            candidates.push((under(a.at, d, len).len(), len, a.at, c.at));
        }
    }
    candidates.sort_by(|p, q| q.0.cmp(&p.0).then(q.1.partial_cmp(&p.1).unwrap_or(std::cmp::Ordering::Equal)));
    let mut out: Vec<(Vec2, Vec2, Vec<panel::Hole>)> = Vec::new();
    for (n, _, a, c) in candidates {
        if n < 2 || out.len() >= MOST_STRIPS {
            break;
        }
        let (d, _) = axes(a, c);
        let mid = (a + c) * 0.5;
        let near = out.iter().any(|(p, q, _)| {
            let (dd, ee) = axes(*p, *q);
            if dd.dot(d).abs() < 0.9 {
                return false;
            }
            let (t0, t1) = ((a - *p).dot(dd), (c - *p).dot(dd));
            let len = p.dist(*q);
            let overlap = t0.min(t1) < len && t0.max(t1) > 0.0;
            overlap && (mid - *p).dot(ee).abs() < o.spacing
        });
        if near {
            continue;
        }
        let len = a.dist(c);
        let mut on = under(a, d, len);
        on.sort_by(|p, q| (p.at - a).dot(d).partial_cmp(&(q.at - a).dot(d)).unwrap_or(std::cmp::Ordering::Equal));
        out.push((a - d * BRACKET_END, c + d * BRACKET_END, on));
    }
    out
}

/// Battens on the brackets' plane, square to the brackets' main way (across rails, battens run
/// up; along pillars, they run across), `batten_spacing` apart at most over the brackets' spread,
/// each from the first bracket it crosses to the last; a line crossing fewer than two is left out
/// (counted).
fn battens(wall: &Wall, bands: &[Band], strips: &[Strip], o: &Options) -> (Vec<Batten>, usize) {
    let (mut across, mut up) = (0.0, 0.0);
    for s in strips {
        let r = s.end - s.start;
        if r.x.abs() > r.y.abs() { across += r.len() } else { up += r.len() }
    }
    // Battens run square to the brackets: a unit along them, and the coordinate across them.
    let vertical = across >= up;
    let (along, side) = if vertical { (Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0)) } else { (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)) };
    let crossing: Vec<&Strip> = strips.iter().filter(|s| (s.end - s.start).dot(side).abs() > (s.end - s.start).dot(along).abs()).collect();
    let (lo, hi) = crossing.iter().fold((f64::MAX, f64::MIN), |(lo, hi), s| {
        let (a, b) = (s.start.dot(side), s.end.dot(side));
        (lo.min(a.min(b)), hi.max(a.max(b)))
    });
    if crossing.len() < 2 || !(hi - lo > o.batten_width) {
        return (Vec::new(), 0);
    }
    let span = hi - lo - o.batten_width;
    let n = ((span / o.batten_spacing).ceil() as usize).max(1) + 1;
    let f = &wall.frame;
    let world = |q: Vec2, w: f64| f.origin + f.x * q.x + f.y * q.y + wall.normal * w;
    let (mut out, mut unsupported) = (Vec::new(), 0);
    for k in 0..n.min(MOST_STRIPS) {
        let x = lo + o.batten_width / 2.0 + span * k as f64 / (n - 1).max(1) as f64;
        // Where each crossing bracket meets the line side = x.
        let mut at: Vec<f64> = crossing
            .iter()
            .filter_map(|s| {
                let (a, b) = (s.start.dot(side), s.end.dot(side));
                if (x - a) * (x - b) > 0.0 || (b - a).abs() < 1e-9 {
                    return None;
                }
                let t = (x - a) / (b - a);
                Some((s.start + (s.end - s.start) * t).dot(along))
            })
            .collect();
        if at.len() < 2 {
            unsupported += 1;
            continue;
        }
        at.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        let (t0, t1) = (at[0] - o.width / 2.0, at[at.len() - 1] + o.width / 2.0);
        let p = |t: f64, s: f64| side * (x + s) + along * t;
        let corners = [p(t0, -o.batten_width / 2.0), p(t0, o.batten_width / 2.0), p(t1, o.batten_width / 2.0), p(t1, -o.batten_width / 2.0)];
        // Anticlockwise on the wall seen from the room, so the box's faces point out.
        let ccw = (corners[1] - corners[0]).cross(corners[2] - corners[0]) > 0.0;
        let ring: Vec<Vec2> = if ccw { corners.to_vec() } else { corners.iter().rev().copied().collect() };
        let mut positions: Vec<Vec3> = ring.iter().map(|q| world(*q, front_of(bands, q.y))).collect();
        positions.extend(ring.iter().map(|q| world(*q, front_of(bands, q.y) + o.batten)));
        let triangles =
            vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7]];
        let (s0, s1) = (p(t0, 0.0), p(t1, 0.0));
        out.push(Batten {
            start: world(s0, front_of(bands, s0.y)),
            end: world(s1, front_of(bands, s1.y)),
            length: t1 - t0,
            crosses: at.len(),
            positions,
            triangles,
        });
    }
    (out, unsupported)
}

/// The strips' runs: straight lines on the wall, rising at most `slant` off `up`, each through
/// one or more of the frame's holes lying behind the bands' planes and over the stretch where the
/// metal stays behind them (gaps under [`BLIP`] crossed), the most holes and the longest first,
/// kept `spacing` apart where they run side by side. Holes on the plane itself are left: the
/// panel is fastened through them directly.
fn runs(holes: &[panel::Hole], bands: &[Band], front: &Front, o: &Options) -> Vec<(Vec2, Vec2, Vec<panel::Hole>)> {
    let half = o.width / 2.0;
    let reach = half - o.bolt / 2.0 - 2.0;
    let most = o.slant.to_radians().tan();
    let (first, last) = bands.iter().fold((f64::MAX, f64::MIN), |(lo, hi), b| (lo.min(b.from), hi.max(b.to)));
    let recessed = |q: Vec2| front.back(q).is_none_or(|m| front_of(bands, q.y) - m >= RECESSED);
    let behind: Vec<panel::Hole> = holes.iter().copied().filter(|h| recessed(h.at)).collect();
    // Along `d` from `p`, the stretch [lo, hi] the metal stays behind the plane.
    let stretch = |p: Vec2, d: Vec2| -> (f64, f64) {
        let mut ends = [0.0f64; 2];
        for (k, sign) in [-1.0f64, 1.0].iter().enumerate() {
            let (mut t, mut last_in, mut out) = (0.0f64, 0.0f64, 0.0f64);
            while t < MOST_RUN {
                t += front.cell;
                let q = p + d * (sign * t);
                if q.y < first || q.y > last {
                    break;
                }
                if recessed(q) {
                    last_in = t;
                    out = 0.0;
                } else {
                    out += front.cell;
                    if out > BLIP {
                        break;
                    }
                }
            }
            ends[k] = sign * last_in;
        }
        (ends[0], ends[1])
    };
    let mut candidates: Vec<(usize, f64, Vec2, Vec2)> = Vec::new();
    let mut lines: Vec<(Vec2, Vec2)> = behind.iter().map(|h| (h.at, Vec2::new(0.0, 1.0))).collect();
    for (i, a) in behind.iter().enumerate() {
        for c in behind.iter().skip(i + 1) {
            let rise = c.at.y - a.at.y;
            if rise > 0.0 && (c.at.x - a.at.x).abs() <= most * rise + 1e-9 && (c.at.x - a.at.x).abs() > 1.0 {
                lines.push((a.at, axes(a.at, c.at).0));
            }
        }
    }
    for (p, d) in lines {
        let (lo, hi) = stretch(p, d);
        if hi - lo < COLUMN_SPAN {
            continue;
        }
        let e = Vec2::new(d.y, -d.x);
        let under = behind
            .iter()
            .filter(|h| {
                let r = h.at - p;
                (lo..=hi).contains(&r.dot(d)) && r.dot(e).abs() <= reach
            })
            .count();
        candidates.push((under, hi - lo, p + d * lo, p + d * hi));
    }
    candidates.sort_by(|p, q| q.0.cmp(&p.0).then(q.1.partial_cmp(&p.1).unwrap_or(std::cmp::Ordering::Equal)));
    let mut out: Vec<(Vec2, Vec2, Vec<panel::Hole>)> = Vec::new();
    for (under, _, a, c) in candidates {
        if under == 0 || out.len() >= MOST_STRIPS {
            break;
        }
        let mid = (a.x + c.x) / 2.0;
        // Apart from every strip beside it, and not crossing one.
        if out.iter().any(|(p, q, _)| {
            let m = (p.x + q.x) / 2.0;
            let beside = a.y < q.y && p.y < c.y;
            beside && ((m - mid).abs() < o.spacing || (a.x - p.x) * (c.x - q.x) < 0.0)
        }) {
            continue;
        }
        let (d, e) = axes(a, c);
        let len = a.dist(c);
        let mut on: Vec<panel::Hole> = behind
            .iter()
            .copied()
            .filter(|h| {
                let r = h.at - a;
                (-1e-6..=len + 1e-6).contains(&r.dot(d)) && r.dot(e).abs() <= reach
            })
            .collect();
        on.sort_by(|p, q| (p.at - a).dot(d).partial_cmp(&(q.at - a).dot(d)).unwrap_or(std::cmp::Ordering::Equal));
        out.push((a, c, on));
    }
    out.sort_by(|p, q| ((p.0.x + p.1.x) / 2.0).partial_cmp(&((q.0.x + q.1.x) / 2.0)).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn strip(wall: &Wall, bands: &[Band], a: Vec2, c: Vec2, on: &[panel::Hole], o: &Options) -> Strip {
    let front = &wall.front;
    let f = &wall.frame;
    let (d, e) = axes(a, c);
    let len = a.dist(c);
    let in_band = |h: f64| bands.iter().any(|b| (b.from..=b.to).contains(&h));
    let between = |h: f64| bands.iter().any(|b| b.to < h) && bands.iter().any(|b| b.from > h);
    let metal = |q: Vec2| front.back(q).map(|m| m + o.gap);
    // In and between bands the front is the bands' planes; past them all (a roof rail above the
    // top band) the strip keeps the nearest plane but comes forward over metal standing in front
    // of it, hugging it.
    let raw_top = |q: Vec2| {
        let f = front_of(bands, q.y);
        if in_band(q.y) || between(q.y) { f } else { metal(q).map_or(f, |m| f.max((m + o.thickness).min(f + MOST_HUG))) }
    };
    // Under each bolt a boss down to the metal, however deep: the bolt bears on the strip.
    let boss = o.head / 2.0 + BOSS;
    let raw_bottom = |q: Vec2| -> f64 {
        let t = raw_top(q);
        let low = if on.iter().any(|h| h.at.dist(q) <= boss) {
            f64::MIN
        } else if in_band(q.y) || between(q.y) {
            [-front.cell, 0.0, front.cell].iter().map(|k| floor_of(bands, q.y + k, o.deepest)).fold(f64::MAX, f64::min)
        } else {
            t - o.deepest
        };
        metal(q).map_or(low.max(t - o.deepest), |m| m.max(low))
    };
    // Where the front or the metal steps (a ledge), the strip is whole across the step: the front
    // past the bands is the highest and the back the deepest of the few cells round it, the back
    // never into the metal.
    let window: Vec<Vec2> = (-STEP_CELLS..=STEP_CELLS).map(|k| Vec2::new(0.0, k as f64 * front.cell)).collect();
    let top = |q: Vec2| {
        if in_band(q.y) { raw_top(q) } else { window.iter().map(|k| raw_top(q + *k)).fold(f64::MIN, f64::max) }
    };
    let bottom = |q: Vec2| -> f64 {
        let deepest = window.iter().map(|k| raw_bottom(q + *k)).fold(f64::MAX, f64::min);
        metal(q).map_or(deepest, |m| deepest.max(m)).min(top(q) - 1.0)
    };
    let holes: Vec<StripHole> = on
        .iter()
        .map(|h| {
            let thickness = top(h.at) - bottom(h.at);
            let across = (h.at - a).dot(e);
            let counterbore = o.head_depth > 0.0 && thickness - o.head_depth >= UNDER_HEAD && across.abs() + o.head / 2.0 + 1.5 <= o.width / 2.0;
            let metal = front.back(h.at).unwrap_or(top(h.at) - o.thickness);
            StripHole {
                at: h.at,
                center: f.origin + f.x * h.at.x + f.y * h.at.y + wall.normal * metal,
                diameter: h.diameter,
                along: (h.at - a).dot(d),
                thickness,
                counterbore,
            }
        })
        .collect();
    let (mut thinnest, mut thickest) = (f64::MAX, f64::MIN);
    let mut t = 0.0;
    while t <= len {
        for k in [-0.5, 0.0, 0.5] {
            let q = a + d * t + e * (k * o.width);
            let th = top(q) - bottom(q);
            thinnest = thinnest.min(th);
            thickest = thickest.max(th);
        }
        t += front.cell;
    }
    // Cuts between holes, each segment no longer than the printer takes.
    let keep_clear = o.head / 2.0 + 3.0;
    let clear = |x: f64| holes.iter().all(|hh| (hh.along - x).abs() >= keep_clear);
    let mut cuts = vec![0.0];
    let mut start = 0.0;
    while len - start > o.segment && cuts.len() < MOST_SEGMENTS {
        let reach = start + o.segment;
        let mids: Vec<f64> =
            holes.windows(2).map(|w| (w[0].along + w[1].along) / 2.0).filter(|m| *m > start + 50.0 && *m <= reach && clear(*m)).collect();
        // The farthest midpoint between two holes within reach, else the farthest clear point.
        let cut = mids.last().copied().or_else(|| {
            let mut x = reach;
            while x > start + 50.0 && !clear(x) {
                x -= 1.0;
            }
            (x > start + 50.0).then_some(x)
        });
        let Some(cut) = cut else { break };
        cuts.push(cut);
        start = cut;
    }
    cuts.push(len);
    let segments = cuts
        .windows(2)
        .map(|w| {
            let inside: Vec<usize> = (0..holes.len()).filter(|&k| holes[k].along > w[0] && holes[k].along < w[1]).collect();
            let place: Vec<StripHole> = inside.iter().map(|&k| holes[k]).collect();
            let (positions, triangles) = solid(wall, a, d, e, w[0], w[1], &place, o, &top, &bottom);
            let volume = volume(&positions, &triangles);
            Segment { from: w[0], to: w[1], holes: inside, volume, positions, triangles }
        })
        .collect();
    Strip { start: a, end: c, holes, segments, thinnest, thickest }
}

/// A segment's closed mesh: a block over a grid across (s) and along (t) the strip from `bottom`
/// to `top`, with a round hole (and counterbore) at each of `holes`.
#[allow(clippy::too_many_arguments)]
fn solid(
    wall: &Wall,
    a: Vec2,
    d: Vec2,
    e: Vec2,
    from: f64,
    to: f64,
    holes: &[StripHole],
    o: &Options,
    top_w: &dyn Fn(Vec2) -> f64,
    bottom: &dyn Fn(Vec2) -> f64,
) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let f = &wall.frame;
    let nu = ((o.width / 2.0).ceil() as usize).max(2);
    let nv = (((to - from) / 2.0).ceil() as usize).max(2);
    let du = o.width / nu as f64;
    let dv = (to - from) / nv as f64;
    let s0 = -o.width / 2.0;
    let gu = |i: usize| s0 + i as f64 * du;
    let gv = |j: usize| from + j as f64 * dv;
    // (across, along) on the wall.
    let wall_at = |s: f64, t: f64| a + d * t + e * s;
    // Each hole's block of cells, the circle well inside it.
    struct Block {
        i0: usize,
        i1: usize,
        j0: usize,
        j1: usize,
        c: Vec2,
        top_r: f64,
        bolt_r: f64,
        cb: bool,
    }
    let mut blocks: Vec<Block> = Vec::new();
    for h in holes {
        let c = Vec2::new((h.at - a).dot(e), h.along);
        let top_r = if h.counterbore { o.head / 2.0 } else { o.bolt / 2.0 };
        let r = top_r + 1.0;
        let i0 = ((c.x - r - s0) / du).floor();
        let i1 = ((c.x + r - s0) / du).ceil();
        let j0 = ((c.y - r - from) / dv).floor();
        let j1 = ((c.y + r - from) / dv).ceil();
        if i0 < 0.0 || j0 < 0.0 || i1 > nu as f64 || j1 > nv as f64 {
            continue;
        }
        let (i0, i1, j0, j1) = (i0 as usize, i1 as usize, j0 as usize, j1 as usize);
        if blocks.iter().any(|k| i0 <= k.i1 && k.i0 <= i1 && j0 <= k.j1 && k.j0 <= j1) {
            continue;
        }
        blocks.push(Block { i0, i1, j0, j1, c, top_r, bolt_r: o.bolt / 2.0, cb: h.counterbore });
    }
    let inside_block = |i: usize, j: usize| blocks.iter().any(|k| k.i0 < i && i < k.i1 && k.j0 < j && j < k.j1);
    let cell_in_block = |i: usize, j: usize| blocks.iter().any(|k| k.i0 <= i && i < k.i1 && k.j0 <= j && j < k.j1);
    let world = |q: Vec2, w: f64| f.origin + f.x * q.x + f.y * q.y + wall.normal * w;
    let mut pos: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let add = |p: Vec3, pos: &mut Vec<Vec3>| {
        pos.push(p);
        (pos.len() - 1) as u32
    };
    // Grid vertices, top and bottom.
    let mut top = vec![u32::MAX; (nu + 1) * (nv + 1)];
    let mut bot = vec![u32::MAX; (nu + 1) * (nv + 1)];
    for j in 0..=nv {
        for i in 0..=nu {
            if inside_block(i, j) {
                continue;
            }
            let q = wall_at(gu(i), gv(j));
            top[j * (nu + 1) + i] = add(world(q, top_w(q)), &mut pos);
            bot[j * (nu + 1) + i] = add(world(q, bottom(q)), &mut pos);
        }
    }
    let id = |i: usize, j: usize| j * (nu + 1) + i;
    for j in 0..nv {
        for i in 0..nu {
            if cell_in_block(i, j) {
                continue;
            }
            let (p, q, r, s) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            tris.push([top[p], top[q], top[r]]);
            tris.push([top[p], top[r], top[s]]);
            tris.push([bot[p], bot[r], bot[q]]);
            tris.push([bot[p], bot[s], bot[r]]);
        }
    }
    // Outer walls, round the grid's edge anticlockwise.
    let mut ring: Vec<usize> = Vec::new();
    ring.extend((0..nu).map(|i| id(i, 0)));
    ring.extend((0..nv).map(|j| id(nu, j)));
    ring.extend((1..=nu).rev().map(|i| id(i, nv)));
    ring.extend((1..=nv).rev().map(|j| id(0, j)));
    for k in 0..ring.len() {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        tris.push([bot[p], bot[q], top[q]]);
        tris.push([bot[p], top[q], top[p]]);
    }
    // Each hole: the block's edge joined to circles, the bore's walls and the counterbore's floor.
    for k in &blocks {
        let mut edge: Vec<usize> = Vec::new();
        edge.extend((k.i0..k.i1).map(|i| id(i, k.j0)));
        edge.extend((k.j0..k.j1).map(|j| id(k.i1, j)));
        edge.extend((k.i0 + 1..=k.i1).rev().map(|i| id(i, k.j1)));
        edge.extend((k.j0 + 1..=k.j1).rev().map(|j| id(k.i0, j)));
        // One circle point per edge point, at the edge point's angle round the centre.
        let angles: Vec<f64> = edge
            .iter()
            .map(|&g| {
                let (i, j) = (g % (nu + 1), g / (nu + 1));
                (gv(j) - k.c.y).atan2(gu(i) - k.c.x)
            })
            .collect();
        let circle = |r: f64, w: &dyn Fn(Vec2) -> f64, pos: &mut Vec<Vec3>| -> Vec<u32> {
            angles
                .iter()
                .map(|ang| {
                    let q = wall_at(k.c.x + r * ang.cos(), k.c.y + r * ang.sin());
                    pos.push(world(q, w(q)));
                    (pos.len() - 1) as u32
                })
                .collect()
        };
        let floor_w = |q: Vec2| top_w(q) - o.head_depth;
        let t_ring = circle(k.top_r, top_w, &mut pos);
        let b_ring = circle(k.bolt_r, bottom, &mut pos);
        let m = edge.len();
        for s in 0..m {
            let s1 = (s + 1) % m;
            let (ea, eb) = (edge[s], edge[s1]);
            tris.push([top[ea], top[eb], t_ring[s1]]);
            tris.push([top[ea], t_ring[s1], t_ring[s]]);
            tris.push([bot[ea], b_ring[s1], bot[eb]]);
            tris.push([bot[ea], b_ring[s], b_ring[s1]]);
        }
        // The bore: from the bottom up to the counterbore's floor (or the top).
        let wall_up = |lower: &[u32], upper: &[u32], tris: &mut Vec<[u32; 3]>| {
            for s in 0..m {
                let s1 = (s + 1) % m;
                tris.push([lower[s], upper[s], upper[s1]]);
                tris.push([lower[s], upper[s1], lower[s1]]);
            }
        };
        if k.cb {
            let f_out = circle(k.top_r, &floor_w, &mut pos);
            let f_in = circle(k.bolt_r, &floor_w, &mut pos);
            wall_up(&f_out, &t_ring, &mut tris);
            for s in 0..m {
                let s1 = (s + 1) % m;
                tris.push([f_out[s], f_out[s1], f_in[s1]]);
                tris.push([f_out[s], f_in[s1], f_in[s]]);
            }
            wall_up(&b_ring, &f_in, &mut tris);
        } else {
            wall_up(&b_ring, &t_ring, &mut tris);
        }
    }
    (pos, tris)
}
/// The volume a closed mesh holds (mm³).
pub fn volume(positions: &[Vec3], triangles: &[[u32; 3]]) -> f64 {
    triangles
        .iter()
        .filter_map(|t| Some((positions.get(t[0] as usize)?, positions.get(t[1] as usize)?, positions.get(t[2] as usize)?)))
        .map(|(a, b, c)| a.dot(b.cross(*c)) / 6.0)
        .sum::<f64>()
        .abs()
}

/// Places in the band where the metal stands in front of the strips' plane, largest first.
fn proud(wall: &Wall, b: &Band) -> Vec<Proud> {
    let front = &wall.front;
    let f = &wall.frame;
    let (w, h) = (front.w, front.h);
    let over = |i: usize, j: usize| {
        let hh = front.row_height(j);
        (b.from..=b.to).contains(&hh) && front.get(i, j).is_some_and(|v| v > b.at(hh))
    };
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    for j0 in 0..h {
        for i0 in 0..w {
            if seen[j0 * w + i0] || !over(i0, j0) {
                continue;
            }
            seen[j0 * w + i0] = true;
            let mut stack = vec![(i0, j0)];
            let (mut n, mut most, mut lo, mut hi) = (0usize, f64::MIN, Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
            let mut deepest = (i0, j0);
            while let Some((i, j)) = stack.pop() {
                n += 1;
                let (u, hh) = (front.col_u(i), front.row_height(j));
                lo = Vec2::new(lo.x.min(u), lo.y.min(hh));
                hi = Vec2::new(hi.x.max(u), hi.y.max(hh));
                let by = front.get(i, j).unwrap_or(0.0) - b.at(hh);
                if by > most {
                    most = by;
                    deepest = (i, j);
                }
                for (di, dj) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (ii, jj) = (i as i64 + di, j as i64 + dj);
                    if ii < 0 || jj < 0 || ii >= w as i64 || jj >= h as i64 {
                        continue;
                    }
                    let (ii, jj) = (ii as usize, jj as usize);
                    if !seen[jj * w + ii] && over(ii, jj) {
                        seen[jj * w + ii] = true;
                        stack.push((ii, jj));
                    }
                }
            }
            let area = n as f64 * front.cell * front.cell;
            if area >= PROUD_AREA {
                let (u, hh) = (front.col_u(deepest.0), front.row_height(deepest.1));
                let center = f.origin + f.x * u + f.y * hh + wall.normal * (b.at(hh) + most);
                out.push(Proud { center, most, area, size: [hi.x - lo.x + front.cell, hi.y - lo.y + front.cell] });
            }
        }
    }
    out.sort_by(|a, c| c.area.partial_cmp(&a.area).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(MOST_PROUD);
    out
}

/// The clear width between two furred walls at height `h` (world, along `a`'s up): from `a`'s
/// finished face at the middle of `a` (or at `u`), along `a`'s face normal, to `c`'s finished face.
pub fn clear_width(a: &Plan, c: &Plan, h: f64, u: Option<f64>) -> Option<f64> {
    let (ba, bc) = (a.band_at(h)?, c.band_at_point(a, h, u)?);
    let (p, n) = a.face(ba, u.unwrap_or_else(|| a.middle()), h);
    let (q, m) = c.face(bc, 0.0, 0.0);
    let denom = n.dot(m);
    if !(denom.abs() > 1e-6) {
        return None;
    }
    let t = (q - p).dot(m) / denom;
    (t.is_finite() && t > 0.0).then_some(t)
}

impl Plan {
    /// The band of this wall facing the point of `other`'s wall at height `h`: the band holding
    /// that point's height on this wall.
    fn band_at_point(&self, other: &Plan, h: f64, u: Option<f64>) -> Option<&Band> {
        let f = &other.wall.frame;
        let p = f.origin + f.x * u.unwrap_or_else(|| other.middle()) + f.y * h;
        let mine = (p - self.wall.frame.origin).dot(self.wall.frame.y);
        self.band_at(mine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ribbed wall: a flat sheet at x = 0 facing +x, a rail 40 mm proud below z = 300, round
    /// holes (8 mm) in columns at y = -400, 0 and 400 every 150 mm, a 100 mm deep bay between.
    fn ribbed() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut pos = Vec::new();
        let mut tris = Vec::new();
        let step = 2.0;
        let (ny, nz) = (600usize, 450usize);
        let depth = |y: f64, z: f64| {
            if z < 300.0 {
                40.0
            } else if (y.rem_euclid(400.0) - 200.0).abs() < 120.0 && z > 360.0 && z < 840.0 {
                -100.0
            } else {
                0.0
            }
        };
        let hole = |y: f64, z: f64| [-400.0f64, 0.0, 400.0].iter().any(|c| (0..6).any(|k| (y - c).hypot(z - (75.0 + 150.0 * k as f64)) < 4.0));
        for j in 0..=nz {
            for i in 0..=ny {
                let (y, z) = (-600.0 + i as f64 * step, j as f64 * step);
                pos.push(Vec3::new(depth(y, z), y, z));
            }
        }
        for j in 0..nz {
            for i in 0..ny {
                let (y, z) = (-600.0 + (i as f64 + 0.5) * step, (j as f64 + 0.5) * step);
                if hole(y, z) {
                    continue;
                }
                let a = (j * (ny + 1) + i) as u32;
                let (b, c, d) = (a + 1, a + 1 + (ny + 1) as u32, a + (ny + 1) as u32);
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        (pos, tris)
    }

    fn closed(tris: &[[u32; 3]]) -> bool {
        let mut edges = std::collections::HashMap::new();
        for t in tris {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0i32) += 1;
            }
        }
        edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1))
    }

    #[test]
    fn a_ribbed_wall_gets_two_bands_and_closed_strips() {
        let (pos, tris) = ribbed();
        let scan = Scan::new(&pos, &tris);
        let o = Options { toward: Some(Vec3::new(1000.0, 0.0, 400.0)), lean: false, standoff: 8.0, batten: 0.0, ..Options::default() };
        let plan = plan(&scan, &o).expect("furred");
        assert!(plan.wall.normal.x > 0.99, "normal {:?}", plan.wall.normal);
        assert_eq!(plan.bands.len(), 2, "{:?}", plan.bands.iter().map(|b| (b.from, b.to)).collect::<Vec<_>>());
        let (low, high) = (&plan.bands[0], &plan.bands[1]);
        // Lower band on the rail (40 proud), upper on the sheet: strips' fronts thickness in front.
        // Offsets are from the wall's middle: the strips' front is the sheet (x = 0) plus the
        // thickness and the print gap, the lower band 40 further on.
        let sheet = -plan.wall.frame.origin.x;
        assert!((high.at(600.0) - (sheet + 8.5)).abs() < 1.5, "high {}", high.at(600.0));
        assert!((low.at(150.0) - high.at(600.0) - 40.0).abs() < 1.5, "low {}", low.at(150.0));
        // One strip up each column, from the rail's holes to the sheet's.
        assert_eq!(plan.strips.len(), 3, "strips {}", plan.strips.len());
        for s in &plan.strips {
            assert_eq!(s.holes.len(), 6);
            assert!(s.thinnest >= 7.0, "thinnest {}", s.thinnest);
            assert!(s.holes.len() >= 2);
            for seg in &s.segments {
                assert!(seg.to - seg.from <= o.segment + 1e-6);
                assert!(!seg.holes.is_empty());
                assert!(closed(&seg.triangles), "segment not closed");
                assert!(seg.volume > 0.0);
            }
            for h in &s.holes {
                assert!(h.counterbore);
                let c = [-400.0f64, 0.0, 400.0].iter().map(|c| (h.center.y - c).abs()).fold(f64::MAX, f64::min);
                assert!(c < 0.6, "hole at {:?}", h.center);
            }
        }
        assert!(high.proud.is_empty() && low.proud.is_empty());
    }

    #[test]
    fn a_flush_plane_rests_on_the_rail_and_fills_only_behind_it() {
        let (pos, tris) = ribbed();
        let scan = Scan::new(&pos, &tris);
        let o = Options { toward: Some(Vec3::new(1000.0, 0.0, 400.0)), lean: false, bands: vec![[0.0, 900.0]], batten: 0.0, ..Options::default() };
        let plan = plan(&scan, &o).expect("furred");
        assert_eq!(plan.bands.len(), 1);
        let rail = 40.0 - plan.wall.frame.origin.x;
        assert!((plan.bands[0].at(450.0) - rail).abs() < 1.0, "plane {} rail {rail}", plan.bands[0].at(450.0));
        // One filler up each column over the sheet, bolted through the sheet's holes only (the
        // rail's are on the plane: the panel takes them).
        assert_eq!(plan.strips.len(), 3, "strips {}", plan.strips.len());
        for s in &plan.strips {
            assert!(s.start.y >= 295.0 && s.end.y >= 890.0, "{:?} {:?}", s.start, s.end);
            assert_eq!(s.holes.len(), 4);
            assert!(s.holes.iter().all(|h| h.center.z > 300.0));
            for seg in &s.segments {
                assert!(closed(&seg.triangles));
                // Its front on the plane, nowhere in front of it.
                let most = seg.positions.iter().map(|p| p.x).fold(f64::MIN, f64::max);
                assert!((most - 40.0).abs() < 0.01, "front {most}");
            }
        }
    }

    #[test]
    fn brackets_follow_the_members_and_battens_cross_them() {
        let (pos, tris) = ribbed();
        let scan = Scan::new(&pos, &tris);
        let o = Options { toward: Some(Vec3::new(1000.0, 0.0, 400.0)), lean: false, bands: vec![[0.0, 900.0]], ..Options::default() };
        let plan = plan(&scan, &o).expect("furred");
        // Each bracket through two or more holes in a line, up a column or along a row.
        assert!(!plan.strips.is_empty());
        for s in &plan.strips {
            assert!(s.holes.len() >= 2);
            let r = s.end - s.start;
            assert!(r.x.abs() < 0.06 * r.y.abs() || r.y.abs() < 0.06 * r.x.abs(), "{r:?}");
            assert!(s.segments.iter().all(|g| closed(&g.triangles)));
        }
        assert!(!plan.battens.is_empty());
        let plane = plan.bands[0].at(450.0) + plan.wall.frame.origin.x;
        for b in &plan.battens {
            assert!(b.crosses >= 2);
            assert!(closed(&b.triangles));
            assert!((volume(&b.positions, &b.triangles) - b.length * o.batten_width * o.batten).abs() < 1.0);
            let front = b.positions.iter().map(|p| p.x).fold(f64::MIN, f64::max);
            assert!((front - (plane + o.batten)).abs() < 0.01, "batten front {front} plane {plane}");
        }
    }

    #[test]
    fn hostile_options_are_refused_not_panicked() {
        let (pos, tris) = ribbed();
        let scan = Scan::new(&pos, &tris);
        for o in [
            Options { tight: f64::NAN, ..Options::default() },
            Options { width: 0.0, ..Options::default() },
            Options { standoff: -1.0, ..Options::default() },
            Options { batten_spacing: 0.0, ..Options::default() },
            Options { head: 100.0, ..Options::default() },
            Options { bands: vec![[5.0, 1.0]], ..Options::default() },
            Options { up: Vec3::new(0.0, 0.0, 0.0), ..Options::default() },
            Options { cell: f64::INFINITY, ..Options::default() },
        ] {
            assert!(plan(&scan, &o).is_err());
        }
        assert!(plan(&Scan::new(&[], &[]), &Options::default()).is_err());
    }

    #[test]
    fn two_facing_walls_measure_their_gap() {
        let (pos, tris) = ribbed();
        let other: Vec<Vec3> = pos.iter().map(|p| Vec3::new(1500.0 - p.x, p.y, p.z)).collect();
        let flipped: Vec<[u32; 3]> = tris.iter().map(|t| [t[0], t[2], t[1]]).collect();
        let (a, b) = (Scan::new(&pos, &tris), Scan::new(&other, &flipped));
        let o = Options { toward: Some(Vec3::new(750.0, 0.0, 400.0)), lean: false, batten: 0.0, ..Options::default() };
        let (pa, pb) = (plan(&a, &o).expect("a"), plan(&b, &o).expect("b"));
        // Flush: the panels (6 mm) on the frame; upper bands on sheets 1500 apart, lower on the
        // rails 40 further in each side.
        let w = clear_width(&pa, &pb, 600.0, None).expect("width");
        assert!((w - (1500.0 - 12.0)).abs() < 1.0, "width {w}");
        let w = clear_width(&pa, &pb, 150.0, None).expect("width");
        assert!((w - (1500.0 - 80.0 - 12.0)).abs() < 1.0, "width {w}");
    }
}
