//! The openings in a wall's inner metal behind a straight panel: where the scan lies far behind the
//! panel's plane over an area (a van's wall bays, the outer skin seen through the inner sheet), each
//! traced at its lip so a piece (insulation board, a filler) can be cut to drop into it.

use std::collections::VecDeque;

use solidcore_record::Vec2;
use solidcraft_geom::prelude::*;

use crate::panel::{Backing, Edges, Seam, grown};
use crate::profile::{area as twice_area, contains};
use crate::template::Template;
use crate::wall_outline::{boundaries, close, crosses_itself, filled, simplify_ring};

/// An opening's back lies at least this far behind the panel (mm): what keeps two openings apart.
pub const DEEP: f64 = 60.0;
/// The opening reaches out to where the metal is this far behind the panel (mm), at most
/// [`GROW`] (mm) past its deep part: its lip.
pub const LIP: f64 = 20.0;
const GROW: f64 = 40.0;
/// Two openings joined by a channel narrower than this (mm) are two (a groove along the metal, the
/// gap round its edge); a corner rounder than half of it is kept as it is.
const NECK: f64 = 50.0;
/// Openings smaller than this (mm²) are not proposed.
pub const LEAST: f64 = 50_000.0;
/// The most openings proposed, and the most cells looked at.
pub const MOST: usize = 100;
const MOST_CELLS: usize = 8_000_000;
/// Of an opening's cells, at least this share lies deep behind the panel (the rest is its lip and
/// what the scan missed in it).
const DEEP_SHARE: f64 = 0.4;
/// Openings are numbered in rows this tall (mm) from the top, left to right in each.
const ROW: f64 = 300.0;

/// An opening behind the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Insert {
    /// Traced at its lip, counter-clockwise, in the panel's coordinates.
    pub ring: Vec<Vec2>,
    /// Its area (mm²) and how far its back lies behind the panel (the median, mm).
    pub area: f64,
    pub depth: f64,
    /// The box it fits in.
    pub lo: Vec2,
    pub hi: Vec2,
    /// It runs out to the panel's edge and is cut off there.
    pub at_edge: bool,
}

impl Insert {
    /// Its box (width, height, mm).
    pub fn size(&self) -> [f64; 2] {
        [self.hi.x - self.lo.x, self.hi.y - self.lo.y]
    }
    /// The middle of its box.
    pub fn center(&self) -> Vec2 {
        (self.lo + self.hi) * 0.5
    }
    /// The piece cut to drop into it: its ring `gap` in all round, in the panel's coordinates.
    pub fn piece(&self, gap: f64) -> Vec<Vec2> {
        shrunk(&self.ring, gap)
    }
}

/// The rounded box nearest an outline: its box, the corner radius that fits it best, and the
/// farthest the outline strays from it (mm): whether the piece can be cut as a box with rounded
/// corners rather than along the trace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedBox {
    pub size: [f64; 2],
    pub radius: f64,
    pub off: f64,
}

/// [`RoundedBox`] for `ring`; none for a ring with no area.
pub fn rounded_box(ring: &[Vec2]) -> Option<RoundedBox> {
    let first = *ring.first()?;
    let (lo, hi) = ring.iter().fold((first, first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    if !(w > 0.0 && h > 0.0 && twice_area(ring).abs() > 0.0 && w.is_finite() && h.is_finite()) {
        return None;
    }
    // Points along the outline every few mm, and the corner radius that keeps them nearest the
    // box's sides and corners (least squares, in 1 mm steps).
    let mut along = Vec::new();
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let steps = (a.dist(b) / 5.0).ceil().clamp(1.0, 10_000.0) as usize;
        along.extend((0..steps).map(|t| a + (b - a) * (t as f64 / steps as f64)));
    }
    let c = (lo + hi) * 0.5;
    let off_at = |radius: f64, p: Vec2| {
        let q = Vec2::new((p.x - c.x).abs() - (w / 2.0 - radius), (p.y - c.y).abs() - (h / 2.0 - radius));
        let outside = Vec2::new(q.x.max(0.0), q.y.max(0.0));
        ((outside.x * outside.x + outside.y * outside.y).sqrt() + q.x.max(q.y).min(0.0) - radius).abs()
    };
    let most = (w.min(h) / 2.0).floor().min(5_000.0) as usize;
    let radius = (0..=most)
        .map(|r| r as f64)
        .min_by(|a, b| {
            let sq = |r: f64| along.iter().map(|p| off_at(r, *p).powi(2)).sum::<f64>();
            sq(*a).total_cmp(&sq(*b))
        })
        .unwrap_or(0.0);
    let off = along.iter().map(|p| off_at(radius, *p)).fold(0.0, f64::max);
    Some(RoundedBox { size: [w, h], radius, off })
}

/// `ring` (counter-clockwise) taken in by `by` all round; as it is when that would cross itself.
pub fn shrunk(ring: &[Vec2], by: f64) -> Vec<Vec2> {
    if !(by.is_finite() && by > 0.0) || ring.len() < 3 {
        return ring.to_vec();
    }
    // Grown the other way round: each corner moved in along its bisector.
    let back: Vec<Vec2> = ring.iter().rev().copied().collect();
    let mut out = grown(&back, by);
    out.reverse();
    let out = untangled(out);
    if out.len() >= 3 && out.iter().all(|p| p.is_finite()) && twice_area(&out) > 0.0 && !crosses_itself(&out) { out } else { ring.to_vec() }
}

/// Where segments `a`–`b` and `c`–`d` cross, if they do (not where they only touch).
fn crossing(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<Vec2> {
    let (r, s) = (b - a, d - c);
    let den = r.x * s.y - r.y * s.x;
    if den.abs() < 1e-12 {
        return None;
    }
    let t = ((c - a).x * s.y - (c - a).y * s.x) / den;
    let u = ((c - a).x * r.y - (c - a).y * r.x) / den;
    (t > 1e-9 && t < 1.0 - 1e-9 && u > 1e-9 && u < 1.0 - 1e-9).then(|| a + r * t)
}

/// `ring` with the loops an inset makes where it runs back over itself (round a dent narrower than
/// twice the inset) cut out, the shorter way round each.
fn untangled(mut r: Vec<Vec2>) -> Vec<Vec2> {
    for _ in 0..r.len() {
        let n = r.len();
        if n < 4 {
            break;
        }
        let mut cut = None;
        'find: for i in 0..n {
            for j in i + 2..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                if let Some(x) = crossing(r[i], r[(i + 1) % n], r[j], r[(j + 1) % n]) {
                    cut = Some((i, j, x));
                    break 'find;
                }
            }
        }
        let Some((i, j, x)) = cut else { break };
        if j - i <= n / 2 {
            r.splice(i + 1..=j, [x]);
        } else {
            let mut kept = vec![x];
            kept.extend_from_slice(&r[i + 1..=j]);
            r = kept;
        }
    }
    r
}

/// How far (in cells, along rows, columns and diagonals) each cell of a `w` × `h` grid is from the
/// nearest of `from`; off the grid counts as one when `edge` says so.
fn distance(from: &[bool], w: usize, h: usize, edge: bool) -> Vec<f32> {
    const FAR: f32 = f32::MAX / 4.0;
    let mut d: Vec<f32> = from.iter().map(|f| if *f { 0.0 } else { FAR }).collect();
    let diag = std::f32::consts::SQRT_2;
    let get = |d: &[f32], i: isize, j: isize| -> f32 {
        if i < 0 || j < 0 || i as usize >= w || j as usize >= h { if edge { 0.0 } else { FAR } } else { d[j as usize * w + i as usize] }
    };
    for j in 0..h as isize {
        for i in 0..w as isize {
            let k = j as usize * w + i as usize;
            let v =
                d[k].min(get(&d, i - 1, j) + 1.0).min(get(&d, i, j - 1) + 1.0).min(get(&d, i - 1, j - 1) + diag).min(get(&d, i + 1, j - 1) + diag);
            d[k] = v;
        }
    }
    for j in (0..h as isize).rev() {
        for i in (0..w as isize).rev() {
            let k = j as usize * w + i as usize;
            let v =
                d[k].min(get(&d, i + 1, j) + 1.0).min(get(&d, i, j + 1) + 1.0).min(get(&d, i + 1, j + 1) + diag).min(get(&d, i - 1, j + 1) + diag);
            d[k] = v;
        }
    }
    d
}

/// `m` opened by a disc of radius `r` cells: what a disc that size, kept inside `m`, can reach.
fn opened(m: &[bool], w: usize, h: usize, r: f64) -> Vec<bool> {
    if !(r.is_finite() && r > 0.0) {
        return m.to_vec();
    }
    let r = r as f32;
    let outside: Vec<bool> = m.iter().map(|b| !b).collect();
    let inner: Vec<bool> = distance(&outside, w, h, true).into_iter().map(|d| d >= r).collect();
    distance(&inner, w, h, false).into_iter().zip(m).map(|(d, b)| *b && d <= r).collect()
}

fn neighbours(k: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (i, j) = (k % w, k / w);
    [(i > 0).then(|| k - 1), (i + 1 < w).then(|| k + 1), (j > 0).then(|| k - w), (j + 1 < h).then(|| k + w)].into_iter().flatten()
}

/// The openings behind the panel `panel` (its outline) within the straight edges `e` of `made`,
/// from `backing`: those of `least` mm² or more, numbered in rows from the top. An opening that runs
/// past one of the `seams` across the panel (they lie along the frame's members) is one each side.
pub fn find(made: &Template, backing: &Backing, e: &Edges, panel: &[Vec2], seams: &[Seam], least: f64) -> Vec<Insert> {
    let cell = backing.cell();
    if !(cell.is_finite() && cell > 0.0 && least.is_finite() && least > 0.0) {
        return Vec::new();
    }
    let (wf, hf) = ((e.right - e.left) / cell, (e.top - e.bottom) / cell);
    if !(wf.is_finite() && hf.is_finite() && wf >= 3.0 && hf >= 3.0 && wf * hf <= MOST_CELLS as f64) {
        return Vec::new();
    }
    let (w, h) = (wf.ceil() as usize, hf.ceil() as usize);
    let at = |i: usize, j: usize| Vec2::new(e.left + (i as f64 + 0.5) * cell, e.bottom + (j as f64 + 0.5) * cell);
    let scanned: Vec<Option<f64>> = (0..w * h).map(|k| backing.front_at(made.at(at(k % w, k / w)))).collect();
    // What the scan missed inside what it saw takes the nearest it saw: a gap in a bay is the bay, a
    // crack along a frame member the member. Beyond the scan's edge stays missed.
    let seen: Vec<bool> = scanned.iter().map(Option::is_some).collect();
    let within = filled(&seen, w, h);
    let mut front = scanned;
    let mut queue: VecDeque<usize> = (0..w * h).filter(|k| seen[*k]).collect();
    while let Some(k) = queue.pop_front() {
        for n in neighbours(k, w, h) {
            if front[n].is_none() && within[n] {
                front[n] = front[k];
                queue.push_back(n);
            }
        }
    }
    // The seams across the panel: a band of cells along each nothing crosses.
    let mut wall = vec![false; w * h];
    for sm in seams.iter().filter(|sm| !sm.upright) {
        let (a, b) = if sm.from.x <= sm.to.x { (sm.from, sm.to) } else { (sm.to, sm.from) };
        for i in 0..w {
            let x = e.left + (i as f64 + 0.5) * cell;
            if x < a.x || x > b.x || b.x - a.x < 1e-9 {
                continue;
            }
            let y = a.y + (b.y - a.y) * (x - a.x) / (b.x - a.x);
            let j = ((y - e.bottom) / cell).floor();
            for dj in -1..=1 {
                let jj = j + f64::from(dj);
                if jj >= 0.0 && (jj as usize) < h {
                    wall[jj as usize * w + i] = true;
                }
            }
        }
    }
    let behind = |k: usize, by: f64| !wall[k] && front[k].is_some_and(|f| f < -by);
    let deep: Vec<bool> = (0..w * h).map(|k| behind(k, DEEP)).collect();
    // Channels between two openings (a groove in the metal) do not join them.
    let core = opened(&deep, w, h, NECK / 2.0 / cell);

    let mut label = vec![0u32; w * h];
    let mut deep_cells: Vec<usize> = Vec::new();
    let mut queue = VecDeque::new();
    let mut count = 0u32;
    for start in 0..w * h {
        if !core[start] || label[start] != 0 {
            continue;
        }
        count += 1;
        label[start] = count;
        deep_cells.push(0);
        let mut todo = VecDeque::from([start]);
        while let Some(k) = todo.pop_front() {
            queue.push_back((k, 0u32));
            for n in neighbours(k, w, h) {
                if core[n] && label[n] == 0 {
                    label[n] = count;
                    todo.push_back(n);
                }
            }
        }
    }
    // Each grown out to its lip, the nearest opening first.
    let reach = (GROW / cell).round().max(1.0) as u32;
    while let Some((k, d)) = queue.pop_front() {
        if d >= reach {
            continue;
        }
        for n in neighbours(k, w, h) {
            // Out to the lip, not along a channel the opening cut off.
            if label[n] == 0 && !deep[n] && behind(n, LIP) {
                label[n] = label[k];
                queue.push_back((n, d + 1));
            }
        }
    }
    let mut cells: Vec<Vec<usize>> = vec![Vec::new(); count as usize];
    for (k, l) in label.iter().enumerate() {
        if *l > 0
            && let Some(c) = cells.get_mut(*l as usize - 1)
        {
            c.push(k);
        }
    }
    for (l, c) in cells.iter().enumerate() {
        if let Some(d) = deep_cells.get_mut(l) {
            *d = c.iter().filter(|k| front[**k].is_some_and(|f| f < -DEEP)).count();
        }
    }

    let mut out = Vec::new();
    for (l, c) in cells.iter().enumerate() {
        let area = c.len() as f64 * cell * cell;
        let deep = deep_cells.get(l).copied().unwrap_or(0);
        if area < least || (deep as f64) < DEEP_SHARE * c.len() as f64 {
            continue;
        }
        let Some(insert) = traced(c, &front, w, h, cell, e) else { continue };
        if insert.area < least || !contains(panel, insert.center()) {
            continue;
        }
        out.push(insert);
    }
    out.sort_by(|a, b| {
        let row = |x: &Insert| ((e.top - x.hi.y) / ROW).floor();
        row(a).total_cmp(&row(b)).then(a.lo.x.total_cmp(&b.lo.x))
    });
    out.truncate(MOST);
    out
}

/// The opening made of `cells` (of a `w` × `h` grid of `cell` mm over `e`), traced round its outside.
fn traced(cells: &[usize], front: &[Option<f64>], w: usize, h: usize, cell: f64, e: &Edges) -> Option<Insert> {
    let (mut i0, mut j0, mut i1, mut j1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for k in cells {
        let (i, j) = (k % w, k / w);
        (i0, j0, i1, j1) = (i0.min(i), j0.min(j), i1.max(i), j1.max(j));
    }
    if i0 > i1 || j0 > j1 {
        return None;
    }
    let at_edge = i0 == 0 || j0 == 0 || i1 + 1 == w || j1 + 1 == h;
    // Its own grid, two cells clear all round.
    const PAD: usize = 2;
    let (sw, sh) = (i1 - i0 + 1 + 2 * PAD, j1 - j0 + 1 + 2 * PAD);
    let mut m = vec![false; sw * sh];
    for k in cells {
        let (i, j) = (k % w - i0 + PAD, k / w - j0 + PAD);
        if let Some(c) = m.get_mut(j * sw + i) {
            *c = true;
        }
    }
    let m = filled(&close(&m, sw, sh, 1), sw, sh);
    let ring = boundaries(&m, sw, sh)
        .into_iter()
        .map(|r| {
            r.into_iter()
                .map(|(x, y)| Vec2::new(e.left + (x as f64 + i0 as f64 - PAD as f64) * cell, e.bottom + (y as f64 + j0 as f64 - PAD as f64) * cell))
                .collect::<Vec<_>>()
        })
        .max_by(|a, b| twice_area(a).abs().total_cmp(&twice_area(b).abs()))?;
    let mut ring = simplify_ring(&ring, 0.75 * cell);
    if ring.len() < 3 {
        return None;
    }
    if twice_area(&ring) < 0.0 {
        ring.reverse();
    }
    // Kept inside the panel's edges.
    for p in &mut ring {
        *p = Vec2::new(p.x.clamp(e.left, e.right), p.y.clamp(e.bottom, e.top));
    }
    let area = twice_area(&ring).abs() / 2.0;
    let mut depths: Vec<f64> = cells.iter().filter_map(|k| front.get(*k).copied().flatten()).filter(|f| *f < -DEEP).map(|f| -f).collect();
    depths.sort_by(f64::total_cmp);
    let depth = depths.get(depths.len() / 2).copied().unwrap_or(0.0);
    let (lo, hi) = ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    Some(Insert { ring, area, depth, lo, hi, at_edge })
}

#[cfg(test)]
mod tests {
    use solidcore_record::{Plane, Vec3};

    use super::*;
    use crate::Scan;

    /// A wall 1200 × 800 sampled every 2 mm whose face is 0, with two rounded bays 120 mm deep (a
    /// groove 4 mm wide and 70 deep between them is not one), and a small pocket under the least
    /// area.
    fn wall() -> Vec<Vec3> {
        let bay = |x: f64, y: f64, lo: Vec2, hi: Vec2, r: f64| {
            let cx = x.clamp(lo.x + r, hi.x - r);
            let cy = y.clamp(lo.y + r, hi.y - r);
            (x - cx).hypot(y - cy) <= r && x >= lo.x && x <= hi.x && y >= lo.y && y <= hi.y
        };
        let depth = |x: f64, y: f64| {
            if bay(x, y, Vec2::new(100.0, 200.0), Vec2::new(500.0, 650.0), 60.0) || bay(x, y, Vec2::new(620.0, 200.0), Vec2::new(1100.0, 650.0), 60.0)
            {
                -120.0
            } else if (558.0..562.0).contains(&x) && (250.0..600.0).contains(&y) {
                -70.0
            } else if bay(x, y, Vec2::new(200.0, 60.0), Vec2::new(300.0, 140.0), 10.0) {
                -100.0
            } else {
                0.0
            }
        };
        let mut points = Vec::new();
        for j in 0..400 {
            for i in 0..600 {
                let (x, y) = (f64::from(i) * 2.0 + 1.0, f64::from(j) * 2.0 + 1.0);
                points.push(Vec3::new(x, y, depth(x, y)));
            }
        }
        points
    }

    fn made() -> Template {
        Template {
            frame: Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y },
            outline: Vec::new(),
            cutouts: Vec::new(),
            scan_edge: Vec::new(),
            area: 0.0,
            cut_area: 0.0,
            found: 0,
            specks: 0,
            outside: 0,
        }
    }

    #[test]
    fn the_bays_behind_the_panel_are_traced_at_their_lips() {
        let points = wall();
        let scan = Scan::new(&points, &[]);
        let made = made();
        let backing = crate::panel::backing(&scan, &made.frame).expect("a backing");
        let e = Edges { left: 0.0, right: 1200.0, bottom: 0.0, top: 800.0 };
        let panel = [Vec2::new(0.0, 0.0), Vec2::new(1200.0, 0.0), Vec2::new(1200.0, 800.0), Vec2::new(0.0, 800.0)];
        let found = find(&made, &backing, &e, &panel, &[], LEAST);
        assert_eq!(found.len(), 2, "the two bays, not the groove nor the small pocket: {found:?}");
        let (a, b) = (&found[0], &found[1]);
        assert!(a.lo.x < b.lo.x, "left to right");
        for (got, want) in [(a, [100.0, 200.0, 500.0, 650.0]), (b, [620.0, 200.0, 1100.0, 650.0])] {
            for (v, w) in [got.lo.x, got.lo.y, got.hi.x, got.hi.y].into_iter().zip(want) {
                assert!((v - w).abs() <= 8.0, "{v} for {w}: {got:?}");
            }
            assert!((got.depth - 120.0).abs() < 1.0 && !got.at_edge, "{got:?}");
            assert!(twice_area(&got.ring) > 0.0, "counter-clockwise");
        }
        // The rounded corners are traced, not boxed: less area than the box.
        assert!(a.area < 400.0 * 450.0 - 1000.0 && a.area > 400.0 * 450.0 - 6000.0, "{}", a.area);
        let piece = a.piece(3.0);
        let lo = piece.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
        assert!((lo.x - a.lo.x - 3.0).abs() < 0.5 && (lo.y - a.lo.y - 3.0).abs() < 0.5, "{lo:?}");
        // Smaller than the least area asked: the pocket too.
        assert_eq!(find(&made, &backing, &e, &panel, &[], 5_000.0).len(), 3);
        // A seam across the panel through both bays: four, the upper ones first.
        let seam = Seam { from: Vec2::new(0.0, 425.0), to: Vec2::new(1200.0, 425.0), upright: false, between_rows: true };
        let split = find(&made, &backing, &e, &panel, &[seam], LEAST);
        assert_eq!(split.len(), 4, "{split:?}");
        assert!(split[0].lo.y > 420.0 && split[1].lo.y > 420.0 && split[2].hi.y < 430.0, "{split:?}");
    }

    #[test]
    fn hostile_edges_and_areas_find_nothing() {
        let points = wall();
        let scan = Scan::new(&points, &[]);
        let made = made();
        let backing = crate::panel::backing(&scan, &made.frame).expect("a backing");
        let panel = [Vec2::new(0.0, 0.0), Vec2::new(1200.0, 0.0), Vec2::new(1200.0, 800.0)];
        for (e, least) in [
            (Edges { left: 0.0, right: 1e12, bottom: 0.0, top: 1e12 }, LEAST),
            (Edges { left: f64::NAN, right: 10.0, bottom: 0.0, top: 10.0 }, LEAST),
            (Edges { left: 0.0, right: 1200.0, bottom: 0.0, top: 800.0 }, f64::NAN),
            (Edges { left: 0.0, right: 1200.0, bottom: 0.0, top: 800.0 }, -1.0),
        ] {
            assert!(find(&made, &backing, &e, &panel, &[], least).is_empty());
        }
        assert_eq!(shrunk(&[Vec2::new(0.0, 0.0)], 3.0).len(), 1);
    }

    /// A box with corners of 50 mm radius is found to be one, within the trace's chords; a slanted
    /// corner is not.
    #[test]
    fn a_rounded_box_is_recognised() {
        let mut ring = Vec::new();
        for (cx, cy, a0) in [(450.0, 50.0, -90.0f64), (450.0, 250.0, 0.0), (50.0, 250.0, 90.0), (50.0, 50.0, 180.0)] {
            for k in 0..=9 {
                let a = (a0 + 10.0 * f64::from(k)).to_radians();
                ring.push(Vec2::new(cx + 50.0 * a.cos(), cy + 50.0 * a.sin()));
            }
        }
        let b = rounded_box(&ring).expect("a box");
        assert!((b.size[0] - 500.0).abs() < 1e-6 && (b.size[1] - 300.0).abs() < 1e-6, "{b:?}");
        assert!((b.radius - 50.0).abs() < 1.5 && b.off < 1.5, "{b:?}");
        let slanted = [Vec2::new(0.0, 0.0), Vec2::new(500.0, 0.0), Vec2::new(500.0, 300.0), Vec2::new(150.0, 300.0), Vec2::new(0.0, 150.0)];
        assert!(rounded_box(&slanted).expect("a box").off > 20.0);
        assert!(rounded_box(&[]).is_none() && rounded_box(&[Vec2::new(1.0, 1.0)]).is_none());
    }

    /// A slot 4 mm wide into a square, taken in by 3 mm: the slot closes and the rest is 3 mm in.
    #[test]
    fn a_narrow_dent_closes_when_taken_in() {
        let ring = [
            Vec2::new(0.0, 0.0),
            Vec2::new(48.0, 0.0),
            Vec2::new(48.0, 60.0),
            Vec2::new(52.0, 60.0),
            Vec2::new(52.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        ];
        let piece = shrunk(&ring, 3.0);
        assert!(!crosses_itself(&piece) && twice_area(&piece) > 0.0, "{piece:?}");
        let (lo, hi) = piece.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
            (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
        });
        assert!(lo.dist(Vec2::new(3.0, 3.0)) < 1e-6 && hi.dist(Vec2::new(97.0, 97.0)) < 1e-6, "{lo:?} {hi:?}");
    }
}
