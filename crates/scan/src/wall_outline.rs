//! The whole outline of a wall from its scan, for a flat template: nothing is picked. The wall's
//! plane is the face it rests on (the outermost large flat part, so a template laid against the
//! frame spans over the recessed panels), the outline is where the scan covers that plane, and
//! every part standing more than a tolerance off it on the side asked for is taken out: a part at
//! the scan's edge (a wheel well, a corner turning away) becomes a notch in the outline, one inside
//! it a cut-out.
//!
//! The scan's own edge is torn and ragged, so the outline is not traced along it. The scan is laid
//! on a grid of `step` square cells on the plane; tears and notches in what it covers narrower than
//! `gap` are bridged (a closing), and holes in it are filled. The outline and the cut-outs are the
//! boundaries of what is left, simplified to within a cell, so they are as true as `step`.

use std::collections::{HashMap, VecDeque};

use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::profile::{area as twice_area, contains};
use crate::template::{Cutout, MOST_CUTOUTS, MOST_OUTLINE, Side, Template, TemplateError, spread};

/// Default width of the tears and notches in the scan's edge bridged (mm).
pub const GAP: f64 = 50.0;
/// Default size of a grid cell (mm): how true the outline and the cut-outs are.
pub const STEP: f64 = 2.0;
/// The most cells in the grid; a larger scan gets larger cells.
const MOST_CELLS: f64 = 8_000_000.0;
/// The most of the scan's points looked at to find the plane (the rest are thinned evenly).
const MOST_FITTED: usize = 200_000;
/// The most cells a triangle's box may span for its cells to be tested one by one.
const MOST_CELLS_PER_TRIANGLE: f64 = 4096.0;
/// The most points sampled across one triangle each way.
const MOST_SAMPLES: usize = 64;

/// The plane a wall rests on, from its whole scan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rest {
    /// Its origin and axes: `x` along the wall's longest spread, `x × y` its normal, turned toward
    /// the side the parts stand farthest off.
    pub frame: Plane,
    /// Four times the noise of the face about the plane, and at least 1 mm.
    pub tolerance: f64,
    /// The points looked at, and those on the face the plane was fitted to.
    pub under: usize,
    pub kept: usize,
}

fn finite_thinned(scan: &Scan) -> Vec<Vec3> {
    let step = (scan.positions.len() / MOST_FITTED).max(1);
    scan.positions.iter().step_by(step).copied().filter(|p| p.is_finite()).collect()
}

/// An in-plane frame through `point` square to `normal` with x along the points' longest spread.
fn spread_frame(point: Vec3, normal: Vec3, points: &[Vec3]) -> Option<Plane> {
    let n = normal.normalized()?;
    let axis = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = (axis - n * axis.dot(n)).normalized()?;
    let v = n.cross(u).normalized()?;
    let count = points.len().max(1) as f64;
    let (mu, mv) = points.iter().fold((0.0, 0.0), |(a, b), p| (a + (*p - point).dot(u), b + (*p - point).dot(v)));
    let (mu, mv) = (mu / count, mv / count);
    let (mut cuu, mut cvv, mut cuv) = (0.0, 0.0, 0.0);
    for p in points {
        let (a, b) = ((*p - point).dot(u) - mu, (*p - point).dot(v) - mv);
        cuu += a * a;
        cvv += b * b;
        cuv += a * b;
    }
    let angle = 0.5 * (2.0 * cuv).atan2(cuu - cvv);
    let x = (u * angle.cos() + v * angle.sin()).normalized()?;
    let y = n.cross(x).normalized()?;
    Some(Plane { origin: point, x, y })
}

/// The plane the wall rests on: fitted to the whole scan, turned toward the side the parts stand
/// farthest off, then moved onto the outermost flat face holding a fair share of the scan (at
/// least a fifth of the largest) and fitted again to that face alone. `None` when the scan makes
/// no plane.
pub fn rest(scan: &Scan) -> Option<Rest> {
    let points = finite_thinned(scan);
    if points.len() < 3 {
        return None;
    }
    let (point, normal, _) = crate::free_form::flat_plane(&points)?;
    let heights: Vec<f64> = points.iter().map(|p| (*p - point).dot(normal)).collect();
    let (hi, lo) = heights.iter().fold((f64::MIN, f64::MAX), |(hi, lo), h| (hi.max(*h), lo.min(*h)));
    let (normal, heights, hi, lo) =
        if -lo > hi { (-normal, heights.iter().map(|h| -h).collect::<Vec<_>>(), -lo, -hi) } else { (normal, heights, hi, lo) };
    // The faces are the peaks of how the heights spread: a histogram, smoothed a little.
    let width = ((hi - lo) / 4_000.0).max(1.0);
    let bins = (((hi - lo) / width).ceil() as usize).clamp(1, 4_001);
    let mut count = vec![0.0_f64; bins];
    for h in &heights {
        let at = (((h - lo) / width) as usize).min(bins - 1);
        count[at] += 1.0;
    }
    let smooth: Vec<f64> = (0..bins).map(|i| count[i.saturating_sub(2)..(i + 3).min(bins)].iter().sum::<f64>()).collect();
    let most = smooth.iter().copied().fold(0.0, f64::max);
    let peak = (0..bins)
        .rev()
        .find(|&i| {
            let s = smooth[i];
            s >= 0.2 * most && (i == 0 || smooth[i - 1] <= s) && (i + 1 == bins || smooth[i + 1] <= s)
        })
        .unwrap_or(0);
    let face_height = lo + (peak as f64 + 0.5) * width;
    let band = (3.0 * width).max(5.0);
    let face: Vec<Vec3> = points.iter().zip(&heights).filter(|(_, h)| (**h - face_height).abs() <= band).map(|(p, _)| *p).collect();
    let (point, normal) = match crate::free_form::flat_plane(&face) {
        Some((p, n, _)) => (p, if n.dot(normal) < 0.0 { -n } else { n }),
        None => (point + normal * face_height, normal),
    };
    let noise: Vec<f64> = face.iter().map(|p| (*p - point).dot(normal)).collect();
    let tolerance = (4.0 * spread(&noise)).max(1.0);
    let frame = spread_frame(point, normal, &points)?;
    Some(Rest { frame, tolerance, under: points.len(), kept: face.len() })
}

/// A grid of square cells on the plane.
struct Grid {
    lo: Vec2,
    cell: f64,
    w: usize,
    h: usize,
}

impl Grid {
    fn at(&self, p: Vec2) -> Option<usize> {
        let (x, y) = ((p.x - self.lo.x) / self.cell, (p.y - self.lo.y) / self.cell);
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        (x < self.w && y < self.h).then_some(y * self.w + x)
    }
    fn corner(&self, x: usize, y: usize) -> Vec2 {
        Vec2::new(self.lo.x + x as f64 * self.cell, self.lo.y + y as f64 * self.cell)
    }
}

/// Every cell within `r` cells (a square) of a set cell, along rows then columns.
pub(crate) fn dilate(m: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return m.to_vec();
    }
    let run = |line: &[bool]| -> Vec<bool> {
        let n = line.len();
        let mut sum = vec![0u32; n + 1];
        for i in 0..n {
            sum[i + 1] = sum[i] + u32::from(line[i]);
        }
        (0..n).map(|i| sum[(i + r + 1).min(n)] > sum[i.saturating_sub(r)]).collect()
    };
    let mut rows = vec![false; w * h];
    for y in 0..h {
        let out = run(&m[y * w..(y + 1) * w]);
        rows[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    let mut out = vec![false; w * h];
    let mut col = vec![false; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = rows[y * w + x];
        }
        for (y, v) in run(&col).into_iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

/// Gaps narrower than `2r + 1` cells filled: dilated, then eroded back.
pub(crate) fn close(m: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let grown = dilate(m, w, h, r);
    let outside: Vec<bool> = grown.iter().map(|b| !b).collect();
    dilate(&outside, w, h, r).into_iter().map(|b| !b).collect()
}

fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
}

/// The cells not reachable from the grid's border through unset cells: the set ones and the holes
/// they enclose.
pub(crate) fn filled(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut queue = VecDeque::new();
    for i in (0..w).chain((0..w).map(|x| (h - 1) * w + x)).chain((0..h).map(|y| y * w)).chain((0..h).map(|y| y * w + w - 1)) {
        if !m[i] && !outside[i] {
            outside[i] = true;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        for j in neighbours(i, w, h) {
            if !m[j] && !outside[j] {
                outside[j] = true;
                queue.push_back(j);
            }
        }
    }
    outside.into_iter().map(|o| !o).collect()
}

/// The largest piece of set cells joined side to side.
fn largest(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut label = vec![0u32; w * h];
    let (mut best, mut best_size, mut next) = (0u32, 0usize, 0u32);
    let mut queue = VecDeque::new();
    for start in 0..w * h {
        if !m[start] || label[start] != 0 {
            continue;
        }
        next += 1;
        label[start] = next;
        queue.push_back(start);
        let mut size = 0usize;
        while let Some(i) = queue.pop_front() {
            size += 1;
            for j in neighbours(i, w, h) {
                if m[j] && label[j] == 0 {
                    label[j] = next;
                    queue.push_back(j);
                }
            }
        }
        if size > best_size {
            (best, best_size) = (next, size);
        }
    }
    label.into_iter().map(|l| best != 0 && l == best).collect()
}

/// `m` without its fingers: the parts narrower than `2r + 1` cells (what an opening by `r` takes
/// away) that reach more than `depth` cells from what the opening keeps, between parts standing
/// out (nine tenths of their border, past what is kept, is inside the scan: `scanned`). A part the
/// opening takes that reaches less far (a corner, the wall between two shallow slots) is kept, and
/// so is a strip along the scan's edge (the wall between a hole and the edge), and one with a cell
/// asked to be covered (`asked`).
fn without_fingers(m: &[bool], scanned: &[bool], asked: &[bool], w: usize, h: usize, r: usize, depth: usize) -> Vec<bool> {
    if r == 0 {
        return m.to_vec();
    }
    let kept = largest(&open(m, w, h, r), w, h);
    if !kept.iter().any(|k| *k) {
        return m.to_vec();
    }
    // How far each part the opening took lies from what it kept, in steps through that part.
    let mut far = vec![usize::MAX; w * h];
    let mut queue = VecDeque::new();
    for i in 0..w * h {
        if kept[i] {
            far[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        for j in neighbours(i, w, h) {
            if m[j] && !kept[j] && far[j] == usize::MAX {
                far[j] = far[i] + 1;
                queue.push_back(j);
            }
        }
    }
    // The tips: what reaches further than `depth`, each judged by its own border. A tip between
    // parts standing out (nine tenths of its border, past `m`, inside the scan) is a finger's.
    let tip: Vec<bool> = (0..w * h).map(|i| m[i] && !kept[i] && far[i] != usize::MAX && far[i] > depth).collect();
    let mut owner = vec![0u8; w * h]; // 0 none, 1 a finger's (cut off), 2 kept
    let mut queue = VecDeque::new();
    for start in 0..w * h {
        if !tip[start] || owner[start] != 0 {
            continue;
        }
        let mut piece = vec![start];
        owner[start] = 3;
        let mut k = 0;
        while let Some(&i) = piece.get(k) {
            k += 1;
            for j in neighbours(i, w, h) {
                if tip[j] && owner[j] == 0 {
                    owner[j] = 3;
                    piece.push(j);
                }
            }
        }
        let (mut inside, mut edge) = (0usize, 0usize);
        for &i in &piece {
            for j in neighbours(i, w, h) {
                if !m[j] {
                    if scanned[j] { inside += 1 } else { edge += 1 }
                }
            }
        }
        let finger = 10 * edge <= inside + edge && !piece.iter().any(|&i| asked.get(i).copied().unwrap_or(false));
        for i in piece {
            owner[i] = if finger { 1 } else { 2 };
            queue.push_back(i);
        }
    }
    // The rest of what the opening took goes with the tip nearest it, through it: a finger's neck
    // is cut off with it, while what leads to a kept strip, or to no tip at all, stays.
    while let Some(i) = queue.pop_front() {
        for j in neighbours(i, w, h) {
            if m[j] && !kept[j] && owner[j] == 0 {
                owner[j] = owner[i];
                queue.push_back(j);
            }
        }
    }
    let out: Vec<bool> = (0..w * h).map(|i| kept[i] || (m[i] && owner[i] != 1)).collect();
    largest(&out, w, h)
}

/// The closed boundaries of the set cells along cell sides, each with the set cells on its left: an
/// outer boundary runs counter-clockwise, a hole's clockwise. Where two set cells touch only at a
/// corner the boundary turns left, keeping them apart.
pub(crate) fn boundaries(m: &[bool], w: usize, h: usize) -> Vec<Vec<(usize, usize)>> {
    let set = |x: isize, y: isize| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize];
    let mut out: HashMap<(usize, usize), Vec<(usize, usize)>> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            if !m[y * w + x] {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            if !set(xi, yi - 1) {
                out.entry((x, y)).or_default().push((x + 1, y));
            }
            if !set(xi + 1, yi) {
                out.entry((x + 1, y)).or_default().push((x + 1, y + 1));
            }
            if !set(xi, yi + 1) {
                out.entry((x + 1, y + 1)).or_default().push((x, y + 1));
            }
            if !set(xi - 1, yi) {
                out.entry((x, y + 1)).or_default().push((x, y));
            }
        }
    }
    let mut starts: Vec<(usize, usize)> = out.keys().copied().collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for start in starts {
        while let Some(first) = out.get_mut(&start).and_then(Vec::pop) {
            let mut ring = vec![start];
            let (mut from, mut at) = (start, first);
            while at != start {
                ring.push(at);
                let Some(choices) = out.get_mut(&at) else { break };
                if choices.is_empty() {
                    break;
                }
                let din = (at.0 as isize - from.0 as isize, at.1 as isize - from.1 as isize);
                let pick = (0..choices.len())
                    .max_by_key(|&k| {
                        let to = choices[k];
                        let dout = (to.0 as isize - at.0 as isize, to.1 as isize - at.1 as isize);
                        din.0 * dout.1 - din.1 * dout.0
                    })
                    .unwrap_or(0);
                let to = choices.swap_remove(pick);
                (from, at) = (at, to);
            }
            if at == start && ring.len() >= 4 {
                loops.push(ring);
            }
        }
    }
    loops
}

fn line_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let len2 = d.x * d.x + d.y * d.y;
    if len2 <= 0.0 {
        return p.dist(a);
    }
    let t = (((p - a).x * d.x + (p - a).y * d.y) / len2).clamp(0.0, 1.0);
    p.dist(a + d * t)
}

/// The points of the open run `pts` kept so no dropped point is more than `eps` off the line
/// between those kept either side (Douglas–Peucker), its ends always kept.
fn simplify_run(pts: &[Vec2], eps: f64, keep: &mut Vec<Vec2>) {
    let mut stack = vec![(0usize, pts.len().saturating_sub(1))];
    let mut kept = vec![false; pts.len()];
    if let Some(k) = kept.first_mut() {
        *k = true;
    }
    if let Some(k) = kept.last_mut() {
        *k = true;
    }
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut far, mut at) = (0.0, a);
        for i in a + 1..b {
            let d = line_distance(pts[i], pts[a], pts[b]);
            if d > far {
                (far, at) = (d, i);
            }
        }
        if far > eps {
            kept[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    keep.extend(pts.iter().zip(&kept).filter(|(_, k)| **k).map(|(p, _)| *p));
}

/// A closed ring simplified to within `eps`, split at its first point and the point farthest from it.
pub(crate) fn simplify_ring(ring: &[Vec2], eps: f64) -> Vec<Vec2> {
    let Some(&first) = ring.first() else { return Vec::new() };
    let far = (0..ring.len()).max_by(|&i, &j| ring[i].dist(first).total_cmp(&ring[j].dist(first))).unwrap_or(0);
    let mut out = Vec::new();
    simplify_run(&ring[..=far], eps, &mut out);
    out.pop();
    let mut back: Vec<Vec2> = ring[far..].to_vec();
    back.push(first);
    simplify_run(&back, eps, &mut out);
    out.pop();
    out
}

pub(crate) fn crosses_itself(ring: &[Vec2]) -> bool {
    let n = ring.len();
    let cross2 = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let crossing = |a: Vec2, b: Vec2, c: Vec2, d: Vec2| {
        let (d1, d2, d3, d4) = (cross2(a, b, c), cross2(a, b, d), cross2(c, d, a), cross2(c, d, b));
        ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    };
    (0..n).any(|i| ((i + 2)..n).any(|j| !(i == 0 && j == n - 1) && crossing(ring[i], ring[(i + 1) % n], ring[j], ring[(j + 1) % n])))
}

/// `ring` simplified to within `eps` or as little more as keeps it to `MOST_OUTLINE` points and
/// not crossing itself; `None` when nothing does.
fn tidy(ring: &[Vec2], eps: f64) -> Option<Vec<Vec2>> {
    let mut eps = eps;
    for _ in 0..24 {
        let out = simplify_ring(ring, eps);
        if out.len() >= 3 && out.len() <= MOST_OUTLINE && !crosses_itself(&out) {
            return Some(out);
        }
        eps *= 1.5;
    }
    None
}

/// What [`template`] made it from: the plane and the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Made {
    pub point: Vec3,
    pub tolerance: f64,
    pub fitted: bool,
    pub under: usize,
    pub kept: usize,
    /// The size of a grid cell used (mm).
    pub cell: f64,
    /// The most a corner of the outline or a cut-out was moved by squaring and rounding (mm); 0
    /// when not rounded.
    pub moved: f64,
}

/// Default width under which cut-outs and slivers of the template are smoothed away (mm): a cable
/// or a bolt inside it goes behind the panel; a notch in its edge (a pillar's bracket, a wheel
/// well) is still cut round.
pub const SMOOTH: f64 = 100.0;

/// Slivers of the template narrower than this are cut off when smoothing (mm): thinner than a
/// panel can be cut to, or the scan's noise.
const SLIVER: f64 = 20.0;

/// Default width under which a finger of the template reaching in between parts standing out is
/// cut off (mm), and how far it must reach to be one.
pub const FINGER: f64 = 300.0;
const FINGER_DEPTH: f64 = 100.0;

/// Default grid the template's corners are rounded to, from its corner (mm): its sides measure in
/// whole steps of it.
pub const ROUND: f64 = 5.0;

/// Sides within this of level or plumb are made exactly so (radians: 2°).
const SQUARE_WITHIN: f64 = 2.0 * std::f64::consts::PI / 180.0;

/// Level sides this close to one another (or plumb sides) are put on one line (mm).
const LINE_UP_WITHIN: f64 = 5.0;

/// A side that rises no more than this (mm) while running at least [`FLAT_RATIO`] times as far is
/// level (and the same way plumb): straight to the eye though a little more than 2° off.
const LOOKS_LEVEL: f64 = 10.0;
const FLAT_RATIO: f64 = 6.0;

/// A side between two level sides (or two plumb ones) is a square step when it moves no more than
/// this one way or the other (mm).
const JOG: f64 = 15.0;

/// How a whole outline is found and how clean it is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Tears in the scan's edge narrower than this are bridged (mm).
    pub gap: f64,
    /// The grid's cell (mm): how true the outline is.
    pub step: f64,
    /// Cut-outs narrower than this are smoothed away (mm): the template goes over a part that thin
    /// inside it rather than round it. A notch in its edge is always kept, and so is any strip of
    /// wall wider than 20 mm. 0 keeps them all.
    pub smooth: f64,
    /// Cut-outs under this area are left in (mm²).
    pub least_area: f64,
    /// The template tidied (mm): sides that are nearly level or plumb made exactly so, those nearly
    /// on one line put on it, and every corner rounded to this grid from the template's corner; no
    /// side is added or taken away. 0 leaves it as found.
    pub round: f64,
    /// Fingers of the template narrower than this reaching more than [`FINGER_DEPTH`] in between
    /// parts standing out are cut off (mm): a tongue between a raised corner and a ridge, a lobe
    /// into a pocket in a raised frame. A wide or shallow one (the wall between two slots in its
    /// edge) is kept. 0 keeps them all.
    pub finger: f64,
    /// Also give the scan's own edge loops.
    pub edge: bool,
    /// The way the template's x runs (a direction in space, laid on the plane), in place of the way
    /// the outline's sides run: the outline is squared and tidied to it.
    pub square_dir: Option<Vec3>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            gap: GAP,
            step: STEP,
            smooth: SMOOTH,
            least_area: crate::template::LEAST_AREA,
            round: ROUND,
            finger: FINGER,
            edge: false,
            square_dir: None,
        }
    }
}

/// The cells left after removing every part of the set narrower than `2r + 1` cells: eroded, then
/// dilated back.
fn open(m: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let outside: Vec<bool> = m.iter().map(|b| !b).collect();
    let shrunk: Vec<bool> = dilate(&outside, w, h, r).into_iter().map(|b| !b).collect();
    dilate(&shrunk, w, h, r)
}

/// The template of the whole scan (see the module): on `plane` ({point, normal}) or the plane the
/// wall rests on, a part a cut-out or notch where it stands more than `tolerance` (default: four
/// times the face's noise) off the plane on `side`, made as `options` ask.
pub fn template(
    scan: &Scan,
    plane: Option<(Vec3, Vec3)>,
    tolerance: Option<f64>,
    side: Side,
    options: Options,
) -> Result<(Template, Made), TemplateError> {
    template_covering(scan, plane, tolerance, side, options, &[])
}

/// The most places asked to be covered.
pub const MOST_COVER: usize = 100;

/// How far from a place picked to cover the part standing out there is looked for (mm).
const COVER_REACH: f64 = 10.0;

/// How far above or below the place picked a part standing out may be and still be covered with
/// it (mm): a plateau and a ridge on it, not the corner block it rises to.
const COVER_BAND: f64 = 15.0;

/// [`template`], with the parts standing out at each place in `keep_over` covered rather than cut
/// round: the raised area there, as far as it stays within [`COVER_BAND`] of its height at the
/// place (a plateau and a low ridge on it, not the part it rises steeply to). A place where
/// nothing stands out changes nothing.
pub fn template_covering(
    scan: &Scan,
    plane: Option<(Vec3, Vec3)>,
    tolerance: Option<f64>,
    side: Side,
    options: Options,
    keep_over: &[Vec3],
) -> Result<(Template, Made), TemplateError> {
    if keep_over.len() > MOST_COVER {
        return Err(TemplateError::Outline("is asked to cover too many places"));
    }
    if !keep_over.iter().all(|p| p.is_finite()) {
        return Err(TemplateError::Outline("is asked to cover a place that is not one"));
    }
    let Options { gap, step, smooth, least_area, round, finger, edge, square_dir } = options;
    if !(finger.is_finite() && finger >= 0.0) {
        return Err(TemplateError::Number("`finger`"));
    }
    if !(round.is_finite() && round >= 0.0) {
        return Err(TemplateError::Number("`round`"));
    }
    if !(smooth.is_finite() && smooth >= 0.0) {
        return Err(TemplateError::Number("`smooth`"));
    }
    if !(gap.is_finite() && gap >= 0.0) {
        return Err(TemplateError::Number("`gap`"));
    }
    if !(step.is_finite() && step > 0.0) {
        return Err(TemplateError::Number("`step`"));
    }
    if !(least_area.is_finite() && least_area >= 0.0) {
        return Err(TemplateError::Number("`least_area`"));
    }
    if let Some(t) = tolerance
        && !(t.is_finite() && t > 0.0)
    {
        return Err(TemplateError::Number("`tolerance`"));
    }
    if scan.triangles.is_empty() {
        return Err(TemplateError::EmptyScan);
    }
    let (frame, tolerance, fitted, under, kept) = match plane {
        Some((point, normal)) => {
            if !point.is_finite() {
                return Err(TemplateError::Outline("has a point that is not a place"));
            }
            let frame = spread_frame(point, normal, &finite_thinned(scan)).ok_or(TemplateError::Plane)?;
            (frame, tolerance.unwrap_or(1.0), false, 0, 0)
        }
        None => {
            let r = rest(scan).ok_or(TemplateError::Outline("cannot be found: the scan makes no plane"))?;
            (r.frame, tolerance.unwrap_or(r.tolerance), true, r.under, r.kept)
        }
    };
    let normal = frame.x.cross(frame.y);
    let flat = |p: Vec3| {
        let d = p - frame.origin;
        Vec2::new(d.dot(frame.x), d.dot(frame.y))
    };

    // The grid, with room round the scan for the closing.
    let (lo, hi) = scan
        .positions
        .iter()
        .filter(|p| p.is_finite())
        .map(|p| flat(*p))
        .fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
            (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
        });
    if !(hi.x > lo.x && hi.y > lo.y) {
        return Err(TemplateError::Outline("cannot be found: the scan covers no area on the plane"));
    }
    let span = hi - lo;
    let mut cell = step;
    // Room for the closings (of the tears and of the smoothing) to grow into and shrink back from.
    let pad = |cell: f64| gap.max(smooth).max(finger) / 2.0 + 3.0 * cell;
    while ((span.x + 2.0 * pad(cell)) / cell) * ((span.y + 2.0 * pad(cell)) / cell) > MOST_CELLS {
        cell *= 1.25;
    }
    let lo = lo - Vec2::new(pad(cell), pad(cell));
    let grid =
        Grid { lo, cell, w: ((span.x + 2.0 * pad(cell)) / cell).ceil() as usize + 1, h: ((span.y + 2.0 * pad(cell)) / cell).ceil() as usize + 1 };
    let (w, h) = (grid.w, grid.h);

    // What the scan covers: the cells whose middle lies on a triangle, so the outline traced round
    // them is within half a cell of the scan's edge, in or out (a cell any part of a triangle
    // touched would put it up to a whole cell outside). A triangle over very many cells (a coarse
    // scan, or hostile input) is sampled instead.
    let mut cover = vec![false; w * h];
    for t in scan.triangles {
        let corners: Option<Vec<Vec2>> = t.iter().map(|&v| scan.positions.get(v as usize).filter(|p| p.is_finite()).map(|p| flat(*p))).collect();
        let Some([a, b, c]) = corners.as_deref() else { continue };
        let (a, b, c) = (*a, *b, *c);
        let (x0, x1) = (a.x.min(b.x).min(c.x), a.x.max(b.x).max(c.x));
        let (y0, y1) = (a.y.min(b.y).min(c.y), a.y.max(b.y).max(c.y));
        // The cells whose middles lie in the triangle's box: middle x = lo.x + (i + 0.5) cell.
        let first = |v: f64, from: f64, n: usize| (((v - from) / cell - 0.5).ceil().max(0.0) as usize).min(n);
        let last = |v: f64, from: f64, n: usize| {
            let k = ((v - from) / cell - 0.5).floor();
            if k < 0.0 { None } else { Some((k as usize).min(n.saturating_sub(1))) }
        };
        let (i0, j0) = (first(x0, grid.lo.x, w), first(y0, grid.lo.y, h));
        let (Some(i1), Some(j1)) = (last(x1, grid.lo.x, w), last(y1, grid.lo.y, h)) else { continue };
        if i1 < i0 || j1 < j0 {
            continue;
        }
        if ((i1 - i0 + 1) * (j1 - j0 + 1)) as f64 <= MOST_CELLS_PER_TRIANGLE {
            let twice = (b - a).cross(c - a);
            if twice == 0.0 || !twice.is_finite() {
                continue;
            }
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let q = grid.corner(i, j) + Vec2::new(cell / 2.0, cell / 2.0);
                    // Its barycentric weights: on the triangle when none is negative.
                    let (u, v) = ((q - a).cross(c - a) / twice, (b - a).cross(q - a) / twice);
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                        cover[j * w + i] = true;
                    }
                }
            }
        } else {
            let longest = a.dist(b).max(b.dist(c)).max(c.dist(a));
            let n = ((longest / (cell * 0.5)).ceil() as usize).clamp(1, MOST_SAMPLES);
            for i in 0..=n {
                for j in 0..=(n - i) {
                    let (s, r) = (i as f64 / n as f64, j as f64 / n as f64);
                    if let Some(k) = grid.at(a + (b - a) * s + (c - a) * r) {
                        cover[k] = true;
                    }
                }
            }
        }
    }
    let mut height = vec![f64::NEG_INFINITY; w * h];
    let mut standing = vec![false; w * h];
    for p in scan.positions.iter().filter(|p| p.is_finite()) {
        let off = (*p - frame.origin).dot(normal);
        let stands = match side {
            Side::Out => off,
            Side::In => -off,
            Side::Both => off.abs(),
        };
        if let Some(k) = grid.at(flat(*p)) {
            height[k] = height[k].max(stands);
            if stands > tolerance {
                standing[k] = true;
            }
        }
    }

    let reach = (gap / 2.0 / cell).round() as usize;
    let whole = filled(&close(&cover, w, h, reach), w, h);
    // Each place to cover: the raised area there, within the band of its height, is not cut. The
    // raised cells are first joined up (a sparse scan marks only the cells its points fall in), and
    // a cell joined in with no point of its own takes the height of the highest within two cells.
    let mut standing = close(&standing, w, h, 2);
    if !keep_over.is_empty() {
        let own = height.clone();
        for k in 0..w * h {
            if standing[k] && !own[k].is_finite() {
                let (x, y) = ((k % w) as isize, (k / w) as isize);
                let mut most = f64::NEG_INFINITY;
                for dy in -2..=2_isize {
                    for dx in -2..=2_isize {
                        let (i, j) = (x + dx, y + dy);
                        if i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h {
                            most = most.max(own[j as usize * w + i as usize]);
                        }
                    }
                }
                height[k] = most;
            }
        }
    }
    let mut asked = vec![false; w * h];
    for place in keep_over {
        let Some(start) = grid.at(flat(*place)) else { continue };
        // The raised cell nearest the place, within [`COVER_REACH`] (a click just beside a narrow
        // ridge still finds it).
        let reach = (COVER_REACH / cell).ceil() as isize;
        let (sx, sy) = ((start % w) as isize, (start / w) as isize);
        let mut seed: Option<(isize, usize)> = None;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let (x, y) = (sx + dx, sy + dy);
                if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
                    continue;
                }
                let k = y as usize * w + x as usize;
                let d = dx * dx + dy * dy;
                if standing[k] && height[k].is_finite() && d <= reach * reach && seed.is_none_or(|(e, _)| d < e) {
                    seed = Some((d, k));
                }
            }
        }
        let Some((_, seed)) = seed else { continue };
        let at = height[seed];
        let (lo_h, hi_h) = (at - COVER_BAND, at + COVER_BAND);
        let mut queue = VecDeque::from([seed]);
        let mut done = vec![false; w * h];
        done[seed] = true;
        while let Some(k) = queue.pop_front() {
            standing[k] = false;
            asked[k] = true;
            for j in neighbours(k, w, h) {
                if !done[j] && standing[j] && height[j] >= lo_h && height[j] <= hi_h {
                    done[j] = true;
                    queue.push_back(j);
                }
            }
        }
    }
    let standing = standing;
    let region: Vec<bool> = whole.iter().zip(&standing).map(|(a, s)| *a && !s).collect();
    let region = largest(&region, w, h);
    // Smoothed: holes in the template narrower than `smooth` filled (a cable, a bolt: the template
    // goes over them), then slivers narrower than [`SLIVER`] cut off (too thin to cut, or noise).
    // A strip of wall wider than that between two parts is kept, and a notch in its edge is never
    // closed, however narrow: what stands out there (a pillar's bracket, a ridge) is cut round.
    let even = (smooth / 2.0 / cell).round() as usize;
    let region = if even > 0 {
        let holes: Vec<bool> = filled(&region, w, h).iter().zip(&region).map(|(f, r)| *f && !r).collect();
        let closed = close(&region, w, h, even);
        let covered: Vec<bool> = region.iter().zip(&closed).zip(&holes).map(|((r, c), hole)| *r || (*c && *hole)).collect();
        let thin = (smooth.min(SLIVER) / 2.0 / cell).round() as usize;
        largest(&open(&covered, w, h, thin), w, h)
    } else {
        region
    };
    let region = if finger > 0.0 {
        without_fingers(&region, &whole, &asked, w, h, (finger / 2.0 / cell).round() as usize, (FINGER_DEPTH / cell).round() as usize)
    } else {
        region
    };
    let eps = cell.max(smooth / 20.0);
    if !region.iter().any(|b| *b) {
        return Err(TemplateError::Outline("cannot be found: all of the scan stands off the plane"));
    }

    let corners = |r: &[(usize, usize)]| -> Vec<Vec2> { r.iter().map(|&(x, y)| grid.corner(x, y)).collect() };
    let mut outer: Option<Vec<Vec2>> = None;
    let mut holes = Vec::new();
    for r in boundaries(&region, w, h) {
        let ring = corners(&r);
        let a = twice_area(&ring) / 2.0;
        if a > 0.0 {
            if outer.as_ref().is_none_or(|o| twice_area(o) / 2.0 < a) {
                outer = Some(ring);
            }
        } else {
            holes.push((-a, r, ring));
        }
    }
    let outline = outer.and_then(|o| tidy(&o, eps)).ok_or(TemplateError::Outline("cannot be found: the scan's outline is too ragged"))?;
    let area = twice_area(&outline) / 2.0;

    let (mut cutouts, mut specks, outside) = (Vec::new(), 0usize, 0usize);
    let found = holes.len();
    holes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = vec![false; w * h];
    for (hole_area, cells, raw) in holes {
        if hole_area < least_area.max(f64::MIN_POSITIVE) {
            specks += 1;
            continue;
        }
        if cutouts.len() >= MOST_CUTOUTS {
            break;
        }
        let Some(mut ring) = tidy(&raw, eps) else { continue };
        if twice_area(&ring) < 0.0 {
            ring.reverse();
        }
        // How far it stands off: the most over the hole's cells, flooded from the cell on the right
        // of its first side (a hole's boundary has the region on its left).
        let mut most = tolerance;
        if let [(ax, ay), (bx, by), ..] = cells[..] {
            let seed = match (bx as isize - ax as isize, by as isize - ay as isize) {
                (1, 0) => (ax < w && ay > 0).then(|| (ay - 1) * w + ax),
                (-1, 0) => (bx < w && ay < h).then(|| ay * w + bx),
                (0, 1) => (ay < h && ax < w).then(|| ay * w + ax),
                (0, -1) => (ax > 0 && by < h).then(|| by * w + ax - 1),
                _ => None,
            };
            let mut queue: VecDeque<usize> = seed.filter(|&k| k < w * h && !region[k] && !seen[k]).into_iter().collect();
            for &k in &queue {
                seen[k] = true;
            }
            while let Some(k) = queue.pop_front() {
                if height[k].is_finite() {
                    most = most.max(height[k]);
                }
                for j in neighbours(k, w, h) {
                    if !region[j] && !seen[j] {
                        seen[j] = true;
                        queue.push_back(j);
                    }
                }
            }
        }
        let crosses = !ring.iter().all(|q| contains(&outline, *q));
        let ring_area = twice_area(&ring) / 2.0;
        let which = if side == Side::In { Side::In } else { Side::Out };
        cutouts.push(Cutout { ring, side: which, area: ring_area, height: most, at_scan_edge: false, crosses, clipped: false });
    }
    // Square to its own edges: the frame turned in its plane to the way the outline's sides run
    // (or to the way asked).
    let turn = match square_dir.filter(|d| d.is_finite()) {
        Some(d) if d.dot(frame.x).hypot(d.dot(frame.y)) > 1e-9 => d.dot(frame.y).atan2(d.dot(frame.x)),
        _ => squaring_angle(&outline),
    };
    let (c, sn) = (turn.cos(), turn.sin());
    let frame = Plane { origin: frame.origin, x: frame.x * c + frame.y * sn, y: frame.y * c - frame.x * sn };
    let spin = |ring: &mut Vec<Vec2>| {
        for q in ring.iter_mut() {
            *q = Vec2::new(q.x * c + q.y * sn, q.y * c - q.x * sn);
        }
    };
    let mut outline = outline;
    spin(&mut outline);
    for cut in &mut cutouts {
        spin(&mut cut.ring);
    }
    // Tidied: squared and rounded, the areas measured again.
    let mut moved = 0.0_f64;
    let (mut outline, mut area) = (outline, area);
    if round > 0.0 {
        let low = outline.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
        let mut rings: Vec<Vec<Vec2>> = std::iter::once(outline.clone()).chain(cutouts.iter().map(|c| c.ring.clone())).collect();
        let lines = lines_up(&rings, low, round);
        for (k, ring) in rings.iter_mut().enumerate() {
            if let Some(tidied) = square_up(ring, k, &lines, low, round) {
                moved = moved.max(apart(ring, &tidied));
                *ring = tidied;
            }
        }
        let mut rings = rings.into_iter();
        if let Some(o) = rings.next() {
            outline = o;
            area = twice_area(&outline) / 2.0;
        }
        for (cut, ring) in cutouts.iter_mut().zip(rings) {
            cut.area = twice_area(&ring) / 2.0;
            cut.ring = ring;
        }
    }
    let cut_area = cutouts.iter().map(|c| c.area).sum::<f64>().min(area);
    let scan_edge = if edge { crate::template::scan_edges(scan, &crate::welded(scan.positions), &frame) } else { Vec::new() };
    let made = Made { point: frame.origin, tolerance, fitted, under, kept, cell, moved };
    Ok((Template { frame, outline, cutouts, scan_edge, area, cut_area, found, specks, outside }, made))
}

/// How a side runs: level, plumb, or neither.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Run {
    Level,
    Plumb,
    Slant,
}

fn run_of(a: Vec2, b: Vec2) -> Run {
    let d = b - a;
    let (across, up) = (d.x.abs(), d.y.abs());
    let slack = SQUARE_WITHIN.tan();
    if across == 0.0 && up == 0.0 {
        Run::Slant
    } else if up <= across * slack || (up <= LOOKS_LEVEL && FLAT_RATIO * up <= across) {
        Run::Level
    } else if across <= up * slack || (across <= LOOKS_LEVEL && FLAT_RATIO * across <= up) {
        Run::Plumb
    } else {
        Run::Slant
    }
}

/// How each side of `ring` runs (side `i` from corner `i` to the next): by its angle, and a slanted
/// side between two level sides that moves no more than [`JOG`] one way or the other is a plumb step
/// (a slot's wall), as is one between two plumb sides a level step. A side among slanted ones (a
/// curve) keeps its slant.
fn runs(ring: &[Vec2]) -> Vec<Run> {
    let n = ring.len();
    let mut kinds: Vec<Run> = (0..n).map(|i| run_of(ring[i], ring[(i + 1) % n])).collect();
    // A step found can make its neighbour a step in turn: again until nothing changes.
    for _ in 0..n.min(8) {
        let next: Vec<Run> = (0..n)
            .map(|i| {
                let (before, after) = (kinds[(i + n - 1) % n], kinds[(i + 1) % n]);
                let d = ring[(i + 1) % n] - ring[i];
                match kinds[i] {
                    Run::Slant if before == Run::Level && after == Run::Level && d.x.abs().min(d.y.abs()) <= JOG => Run::Plumb,
                    Run::Slant if before == Run::Plumb && after == Run::Plumb && d.x.abs().min(d.y.abs()) <= JOG => Run::Level,
                    run => run,
                }
            })
            .collect();
        if next == kinds {
            break;
        }
        kinds = next;
    }
    kinds
}

/// `v` rounded to the grid of `step` from `from` (unchanged when `step` is 0).
fn snap(v: f64, from: f64, step: f64) -> f64 {
    if step > 0.0 { from + ((v - from) / step).round() * step } else { v }
}

/// Where each level and plumb side of `rings` goes: keyed by (ring, side), the height of a level
/// side or the place of a plumb one. Sides of one kind within [`LINE_UP_WITHIN`] of each other are
/// put on one line, at their mean weighted by length, rounded to the grid.
fn lines_up(rings: &[Vec<Vec2>], low: Vec2, step: f64) -> HashMap<(usize, usize), f64> {
    let mut out = HashMap::new();
    for kind in [Run::Level, Run::Plumb] {
        let mut sides: Vec<(f64, f64, (usize, usize))> = Vec::new();
        for (k, ring) in rings.iter().enumerate() {
            let kinds = runs(ring);
            for (i, (a, b)) in ring.iter().zip(ring.iter().cycle().skip(1)).enumerate() {
                if kinds[i] == kind {
                    let at = if kind == Run::Level { (a.y + b.y) / 2.0 } else { (a.x + b.x) / 2.0 };
                    sides.push((at, a.dist(*b), (k, i)));
                }
            }
        }
        sides.sort_by(|a, b| a.0.total_cmp(&b.0));
        let from = if kind == Run::Level { low.y } else { low.x };
        let mut start = 0;
        while start < sides.len() {
            let mut end = start + 1;
            while end < sides.len() && sides[end].0 - sides[end - 1].0 <= LINE_UP_WITHIN && sides[end].0 - sides[start].0 <= 2.0 * LINE_UP_WITHIN {
                end += 1;
            }
            let group = &sides[start..end];
            let weight: f64 = group.iter().map(|s| s.1).sum();
            let mean = if weight > 0.0 { group.iter().map(|s| s.0 * s.1).sum::<f64>() / weight } else { group[0].0 };
            let at = snap(mean, from, step);
            for s in group {
                out.insert(s.2, at);
            }
            start = end;
        }
    }
    out
}

/// Ring `k` squared and rounded: each level side at its line's height, each plumb side at its
/// line's place, every other corner rounded to the grid; a corner between two level (or two plumb)
/// sides on different lines becomes a square step. `None` when the result folds or crosses itself.
fn square_up(ring: &[Vec2], k: usize, lines: &HashMap<(usize, usize), f64>, low: Vec2, step: f64) -> Option<Vec<Vec2>> {
    let n = ring.len();
    if n < 3 {
        return None;
    }
    let kinds = runs(ring);
    let side = |i: usize| -> (Run, Option<f64>) { (kinds[i % n], lines.get(&(k, i % n)).copied()) };
    let mut out: Vec<Vec2> = Vec::with_capacity(n + 4);
    for (i, &p) in ring.iter().enumerate() {
        let (sx, sy) = (snap(p.x, low.x, step), snap(p.y, low.y, step));
        let (before, after) = (side(i + n - 1), side(i));
        match (before, after) {
            ((Run::Level, Some(y0)), (Run::Level, Some(y1))) => {
                if y0 != y1 {
                    out.push(Vec2::new(sx, y0));
                    out.push(Vec2::new(sx, y1));
                }
            }
            ((Run::Plumb, Some(x0)), (Run::Plumb, Some(x1))) => {
                if x0 != x1 {
                    out.push(Vec2::new(x0, sy));
                    out.push(Vec2::new(x1, sy));
                }
            }
            ((Run::Level, Some(y)), (Run::Plumb, Some(x))) | ((Run::Plumb, Some(x)), (Run::Level, Some(y))) => out.push(Vec2::new(x, y)),
            ((Run::Level, Some(y)), _) | (_, (Run::Level, Some(y))) => out.push(Vec2::new(sx, y)),
            ((Run::Plumb, Some(x)), _) | (_, (Run::Plumb, Some(x))) => out.push(Vec2::new(x, sy)),
            _ => out.push(Vec2::new(sx, sy)),
        }
    }
    // No repeated corners, and none in the middle of a straight side.
    out.dedup();
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    let mut changed = true;
    while changed && out.len() >= 3 {
        changed = false;
        let m = out.len();
        for i in 0..m {
            let (a, b, c) = (out[(i + m - 1) % m], out[i], out[(i + 1) % m]);
            if a == b || (b - a).cross(c - b).abs() <= 1e-9 * (1.0 + a.dist(c) * a.dist(c)) && (b - a).dot(c - b) >= 0.0 {
                out.remove(i);
                changed = true;
                break;
            }
        }
    }
    let kept = out.len() >= 3 && !crosses_itself(&out) && twice_area(&out).signum() == twice_area(ring).signum();
    kept.then_some(out)
}

/// The most a corner of either ring lies from the other ring's sides (mm).
fn apart(a: &[Vec2], b: &[Vec2]) -> f64 {
    let from = |p: Vec2, ring: &[Vec2]| ring.iter().zip(ring.iter().cycle().skip(1)).map(|(s, t)| line_distance(p, *s, *t)).fold(f64::MAX, f64::min);
    let one = a.iter().map(|p| from(*p, b)).fold(0.0, f64::max);
    let two = b.iter().map(|p| from(*p, a)).fold(0.0, f64::max);
    one.max(two)
}

/// The angle (radians, within ±45°) the sides of `ring` run at, square or straight, each side
/// counted by its length: turning the frame by it lines the template's long straight sides up
/// with its axes.
fn squaring_angle(ring: &[Vec2]) -> f64 {
    let (mut sin4, mut cos4) = (0.0, 0.0);
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let d = *b - *a;
        let len = d.x.hypot(d.y);
        if !(len > 0.0) {
            continue;
        }
        let angle = d.y.atan2(d.x);
        sin4 += len * (4.0 * angle).sin();
        cos4 += len * (4.0 * angle).cos();
    }
    if sin4 == 0.0 && cos4 == 0.0 { 0.0 } else { sin4.atan2(cos4) / 4.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall in the XY plane `wx` × `wy` mm, a square every `d` mm, `z = f(x, y)`.
    fn wall(wx: f64, wy: f64, d: f64, f: impl Fn(f64, f64) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (nx, ny) = ((wx / d) as u32, (wy / d) as u32);
        let mut pos = Vec::new();
        for j in 0..=ny {
            for i in 0..=nx {
                let (x, y) = (i as f64 * d, j as f64 * d);
                pos.push(Vec3::new(x, y, f(x, y)));
            }
        }
        let at = |i: u32, j: u32| j * (nx + 1) + i;
        let mut tri = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                tri.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                tri.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        (pos, tri)
    }

    /// A 1200 × 800 wall: a frame at z = 0, panels recessed 60 mm behind it, a hump standing 150
    /// mm out at the middle of its bottom edge (a wheel well) and a box 40 mm out inside it.
    fn van_wall(x: f64, y: f64) -> f64 {
        let hump = ((x - 600.0) / 200.0).powi(2) + (y / 150.0).powi(2) < 1.0;
        let boxed = (900.0..1000.0).contains(&x) && (500.0..600.0).contains(&y);
        let panel = (100.0..550.0).contains(&x) && (300.0..700.0).contains(&y);
        if hump {
            150.0
        } else if boxed {
            40.0
        } else if panel {
            -60.0
        } else {
            0.0
        }
    }

    #[test]
    fn the_whole_wall_rests_on_its_frame_with_the_wheel_well_a_notch() {
        let (pos, tri) = wall(1200.0, 800.0, 5.0, van_wall);
        let scan = Scan::new(&pos, &tri);
        let r = rest(&scan).expect("a plane");
        let n = r.frame.x.cross(r.frame.y);
        assert!(n.z > 0.99, "the normal points out, toward the hump: {n:?}");
        assert!(r.frame.origin.z.abs() < 2.0, "the plane rests on the frame, not between it and the panels: {:?}", r.frame.origin);
        assert!(r.frame.x.x.abs() > 0.99, "x runs along the wall's length");

        let (t, made) = template(&scan, None, Some(10.0), Side::Out, Options { smooth: 0.0, round: 0.0, ..Options::default() }).expect("a template");
        assert!(made.fitted && made.cell == STEP);
        // The outline covers the wall less the notch (half the hump's ellipse: π·200·150/2 ≈ 47 000).
        let whole = 1200.0 * 800.0;
        assert!((t.area - (whole - 47_124.0)).abs() < 0.02 * whole, "outline area {}", t.area);
        // The notch is in the outline: a point in the hump is outside it, one beside it inside.
        let at = |x: f64, y: f64| {
            let d = Vec3::new(x, y, 0.0) - t.frame.origin;
            Vec2::new(d.dot(t.frame.x), d.dot(t.frame.y))
        };
        assert!(!contains(&t.outline, at(600.0, 50.0)), "the wheel well is notched out");
        assert!(contains(&t.outline, at(300.0, 50.0)) && contains(&t.outline, at(300.0, 500.0)), "the recessed panel is spanned");
        // The box is the one cut-out, about 100 × 100.
        assert_eq!(t.cutouts.len(), 1, "{:?}", t.cutouts.iter().map(|c| c.area).collect::<Vec<_>>());
        assert!((t.cutouts[0].area - 10_000.0).abs() < 1_500.0, "box area {}", t.cutouts[0].area);
        assert!((t.cutouts[0].height - 40.0).abs() < 1.0);
    }

    /// Smoothing at 150 mm: a cable 20 mm wide standing 15 mm out across the wall and the 100 mm box
    /// are no longer cut out, while the wheel well, 400 mm wide, is still notched, and so is a
    /// pillar's bracket 60 mm wide at the edge and a ridge only 20 mm wide reaching the edge: what
    /// stands out at the edge is cut round however narrow, and the strip of wall 60 mm wide between
    /// two brackets stays.
    #[test]
    fn smoothing_drops_thin_parts_and_keeps_the_wheel_well() {
        let cabled = |x: f64, y: f64| {
            if (700.0..720.0).contains(&y) && (150.0..1050.0).contains(&x) {
                15.0
            } else if ((1100.0..1160.0).contains(&x) || (1220.0..1280.0).contains(&x)) && y < 200.0 {
                50.0
            } else if (250.0..270.0).contains(&x) && y < 300.0 {
                20.0
            } else {
                van_wall(x, y)
            }
        };
        let (pos, tri) = wall(1200.0, 800.0, 5.0, cabled);
        let scan = Scan::new(&pos, &tri);
        let (raw, _) = template(&scan, None, Some(10.0), Side::Out, Options { smooth: 0.0, round: 0.0, ..Options::default() }).expect("a template");
        assert!(raw.cutouts.len() >= 2, "unsmoothed, the cable and the box are cut: {}", raw.cutouts.len());
        let (clean, _) =
            template(&scan, None, Some(10.0), Side::Out, Options { smooth: 150.0, finger: 0.0, ..Options::default() }).expect("a template");
        assert!(clean.cutouts.is_empty(), "{:?}", clean.cutouts.iter().map(|c| c.area).collect::<Vec<_>>());
        assert!(clean.area < 1200.0 * 800.0 - 30_000.0, "smoothing never grows the template past the scan: {}", clean.area);
        let at = |x: f64, y: f64| {
            let d = Vec3::new(x, y, 0.0) - clean.frame.origin;
            Vec2::new(d.dot(clean.frame.x), d.dot(clean.frame.y))
        };
        assert!(!contains(&clean.outline, at(600.0, 50.0)), "the wheel well is still notched");
        assert!(!contains(&clean.outline, at(1130.0, 100.0)), "the bracket at the edge is still notched");
        assert!(!contains(&clean.outline, at(260.0, 100.0)), "a ridge 20 mm wide reaching the edge is still notched");
        assert!(contains(&clean.outline, at(1190.0, 100.0)), "a strip of wall 60 mm wide between two brackets is kept");
        assert!(contains(&clean.outline, at(950.0, 550.0)) && contains(&clean.outline, at(500.0, 710.0)), "the box and the cable are covered");
        assert!(clean.outline.len() < raw.outline.len(), "fewer points: {} vs {}", clean.outline.len(), raw.outline.len());
    }

    /// A template squares to its sides: a 4000 × 2000 outline with a notch, turned 0.05 rad, is
    /// found turned 0.05 rad; one with no length is left as it is.
    #[test]
    fn the_template_squares_to_its_long_sides() {
        let shape = [(0.0, 0.0), (1200.0, 0.0), (1300.0, 300.0), (1900.0, 300.0), (2000.0, 0.0), (4000.0, 0.0), (4000.0, 2000.0), (0.0, 2000.0)];
        let (c, s) = (0.05_f64.cos(), 0.05_f64.sin());
        let turned: Vec<Vec2> = shape.iter().map(|&(x, y)| Vec2::new(x * c - y * s, x * s + y * c)).collect();
        assert!((squaring_angle(&turned) - 0.05).abs() < 1e-3, "{}", squaring_angle(&turned));
        assert_eq!(squaring_angle(&[Vec2::new(1.0, 1.0); 3]), 0.0);
    }

    /// Tidied: a ring whose sides are a degree off level and plumb, with a 30 mm slot in its top
    /// and corners off the grid, comes out square, its slot kept, every corner on the 5 mm grid
    /// from its low corner, and no corner moved further than the squaring needs.
    #[test]
    fn tidying_squares_rounds_and_keeps_the_slots() {
        let tilt = |x: f64, y: f64| Vec2::new(x + 0.008 * y + 0.3, y + 0.008 * x + 1.7);
        // The slot's walls lean 4 mm, as a scan's do: they come out plumb.
        let ring: Vec<Vec2> =
            [(0.0, 0.0), (1000.0, 0.0), (1000.0, 500.0), (620.0, 500.0), (616.0, 470.0), (584.0, 470.0), (580.0, 500.0), (0.0, 500.0)]
                .iter()
                .map(|&(x, y)| tilt(x, y))
                .collect();
        let low = ring.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
        let rings = vec![ring.clone()];
        let lines = lines_up(&rings, low, 5.0);
        let tidy = square_up(&ring, 0, &lines, low, 5.0).expect("tidied");
        assert_eq!(tidy.len(), 8, "every side kept, the slot too: {tidy:?}");
        for (a, b) in tidy.iter().zip(tidy.iter().cycle().skip(1)) {
            assert!(a.x == b.x || a.y == b.y, "square: {a:?} {b:?}");
        }
        for q in &tidy {
            for v in [q.x - low.x, q.y - low.y] {
                assert!((v / 5.0 - (v / 5.0).round()).abs() < 1e-9, "on the 5 mm grid: {q:?}");
            }
        }
        // The two halves of the top, either side of the slot, are on one line.
        let tops: Vec<f64> = tidy.iter().map(|q| q.y).filter(|y| *y > low.y + 480.0).collect();
        assert!(tops.windows(2).all(|w| w[0] == w[1]), "{tops:?}");
        assert!(apart(&ring, &tidy) < 16.0, "{}", apart(&ring, &tidy));
    }

    /// Fingers: a pocket 140 mm wide and 250 mm deep inside a raised U-shaped frame (open below,
    /// toward the wall) is cut off with the frame, the template stopping below it; the wall between
    /// two slots 20 mm deep in the top edge is a wide, shallow part and stays.
    #[test]
    fn fingers_between_raised_parts_are_cut_off() {
        let raised = |x: f64, y: f64| {
            let frame = (800.0..1100.0).contains(&x) && (450.0..800.0).contains(&y);
            let pocket = (880.0..1020.0).contains(&x) && (450.0..700.0).contains(&y);
            let slot = ((100.0..170.0).contains(&x) || (400.0..470.0).contains(&x)) && y >= 780.0;
            if (frame && !pocket) || slot { 30.0 } else { 0.0 }
        };
        let (pos, tri) = wall(1200.0, 800.0, 5.0, raised);
        let scan = Scan::new(&pos, &tri);
        let raw = Options { smooth: 0.0, round: 0.0, finger: 0.0, ..Options::default() };
        // Whether a point of the wall is in the template: inside its outline and no cut-out.
        let at = |t: &Template, x: f64, y: f64| {
            let d = Vec3::new(x, y, 0.0) - t.frame.origin;
            Vec2::new(d.dot(t.frame.x), d.dot(t.frame.y))
        };
        let panel = |t: &Template, x: f64, y: f64| {
            let q = at(t, x, y);
            contains(&t.outline, q) && !t.cutouts.iter().any(|c| contains(&c.ring, q))
        };
        let (with, _) = template(&scan, None, Some(10.0), Side::Out, raw).expect("a template");
        assert!(panel(&with, 950.0, 600.0), "unfingered, the pocket is a lobe of the template");
        let (clean, _) = template(&scan, None, Some(10.0), Side::Out, Options { finger: FINGER, ..raw }).expect("a template");
        assert!(!panel(&clean, 950.0, 600.0), "the lobe into the pocket is cut off");
        assert!(panel(&clean, 950.0, 400.0), "the wall below the frame stays");
        assert!(panel(&clean, 300.0, 790.0), "the wall between the slots stays");
        assert!(!panel(&clean, 135.0, 795.0) && !panel(&clean, 435.0, 795.0), "the slots stay");
        assert!(template(&scan, None, None, Side::Out, Options { finger: f64::NAN, ..raw }).is_err());
    }

    /// The outline lies within half a cell of the scan's edge, however the grid falls on it (the
    /// finger width moves the grid): a 400 × 300 wall comes out 400 × 300 to within a cell, never
    /// a cell larger all round.
    #[test]
    fn the_outline_is_within_half_a_cell_of_the_scan() {
        let (pos, tri) =
            wall(400.0, 300.0, 400.0 / 120.0, |x, y| if (100.0..=160.0).contains(&x) && (100.0..=180.0).contains(&y) { 25.0 } else { 0.0 });
        let scan = Scan::new(&pos, &tri);
        for finger in [0.0, 120.0, FINGER] {
            for step in [2.0, 3.0] {
                let (t, made) = template(&scan, None, None, Side::Out, Options { smooth: 0.0, round: 0.0, finger, step, ..Options::default() })
                    .expect("a template");
                let (lo, hi) = t.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
                    (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
                });
                let (a, b) = ((hi.x - lo.x).max(hi.y - lo.y), (hi.x - lo.x).min(hi.y - lo.y));
                assert!((a - 400.0).abs() <= made.cell + 1e-9 && (b - 300.0).abs() <= made.cell + 1e-9, "finger {finger}, step {step}: {a} × {b}");
            }
        }
    }

    /// Covering: a plateau 20 mm proud beside a block 150 mm proud is cut out with it, until a
    /// click on the plateau covers it; the block stays cut. A click on the flat wall changes
    /// nothing, and too many or bad places are refused.
    #[test]
    fn a_click_covers_a_plateau_and_not_the_block_beside_it() {
        let raised = |x: f64, y: f64| {
            if (1000.0..1200.0).contains(&x) && y >= 400.0 {
                150.0
            } else if (700.0..1000.0).contains(&x) && y >= 500.0 {
                20.0
            } else {
                0.0
            }
        };
        let (pos, tri) = wall(1200.0, 800.0, 5.0, raised);
        let scan = Scan::new(&pos, &tri);
        let panel = |t: &Template, x: f64, y: f64| {
            let d = Vec3::new(x, y, 0.0) - t.frame.origin;
            let q = Vec2::new(d.dot(t.frame.x), d.dot(t.frame.y));
            contains(&t.outline, q) && !t.cutouts.iter().any(|c| contains(&c.ring, q))
        };
        let o = Options { round: 0.0, ..Options::default() };
        let (plain, _) = template(&scan, None, Some(10.0), Side::Out, o).expect("a template");
        assert!(!panel(&plain, 850.0, 650.0) && !panel(&plain, 1100.0, 600.0), "both are cut unasked");
        let (covered, _) = template_covering(&scan, None, Some(10.0), Side::Out, o, &[Vec3::new(850.0, 650.0, 20.0)]).expect("a template");
        assert!(panel(&covered, 850.0, 650.0) && panel(&covered, 850.0, 780.0), "the plateau is covered");
        assert!(!panel(&covered, 1100.0, 600.0), "the block stays cut");
        let (flat, _) = template_covering(&scan, None, Some(10.0), Side::Out, o, &[Vec3::new(300.0, 300.0, 0.0)]).expect("a template");
        assert!((flat.area - plain.area).abs() < 1e-6, "a click on the flat wall changes nothing");
        assert!(template_covering(&scan, None, None, Side::Out, o, &vec![Vec3::new(1.0, 1.0, 0.0); MOST_COVER + 1]).is_err());
        assert!(template_covering(&scan, None, None, Side::Out, o, &[Vec3::new(f64::NAN, 0.0, 0.0)]).is_err());
    }

    #[test]
    fn tears_narrower_than_the_gap_are_bridged() {
        // A wall with a 20 mm slit cut into it from its top edge, 300 mm deep.
        let (pos, tri) = wall(600.0, 400.0, 5.0, |_, _| 0.0);
        let tri: Vec<[u32; 3]> = tri
            .into_iter()
            .filter(|t| {
                let c = t.iter().fold(Vec3::new(0.0, 0.0, 0.0), |s, v| s + pos[*v as usize] * (1.0 / 3.0));
                !((290.0..310.0).contains(&c.x) && c.y > 100.0)
            })
            .collect();
        let scan = Scan::new(&pos, &tri);
        let (bridged, _) =
            template(&scan, None, Some(5.0), Side::Out, Options { smooth: 0.0, round: 0.0, ..Options::default() }).expect("a template");
        assert!((bridged.area - 600.0 * 400.0).abs() < 0.01 * 240_000.0, "the slit is bridged: {}", bridged.area);
        let (kept, _) =
            template(&scan, None, Some(5.0), Side::Out, Options { gap: 0.0, smooth: 0.0, round: 0.0, ..Options::default() }).expect("a template");
        assert!(bridged.area - kept.area > 4_000.0, "with no gap the slit stays: {} against {}", kept.area, bridged.area);
    }

    #[test]
    fn hostile_input_is_refused_not_a_crash() {
        let (pos, tri) = wall(100.0, 100.0, 10.0, |_, _| 0.0);
        let scan = Scan::new(&pos, &tri);
        assert!(template(&scan, None, None, Side::Out, Options { smooth: f64::NAN, ..Options::default() }).is_err());
        for (gap, step, area) in [(f64::NAN, 2.0, 25.0), (50.0, 0.0, 25.0), (50.0, f64::INFINITY, 25.0), (50.0, 2.0, -1.0)] {
            assert!(template(&scan, None, None, Side::Out, Options { gap, step, least_area: area, ..Options::default() }).is_err());
        }
        assert!(template(&scan, None, Some(f64::NAN), Side::Out, Options::default()).is_err());
        assert!(template(&Scan::new(&pos, &[]), None, None, Side::Out, Options::default()).is_err());
        let bad = [Vec3::new(f64::NAN, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
        assert!(template(&Scan::new(&bad, &[[0, 1, 2], [0, 1, 9]]), None, None, Side::Out, Options::default()).is_err());
        assert!(template(&scan, Some((Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0))), None, Side::Out, Options::default()).is_err());
        // A tiny step on a large scan gets larger cells, not a grid too large to hold.
        let (_, made) = template(&scan, None, None, Side::Out, Options { step: 1e-6, ..Options::default() }).expect("a template");
        assert!(made.cell > 1e-6);
    }
}
