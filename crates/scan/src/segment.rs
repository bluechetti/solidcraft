//! Find All: the scan cut into regions, each a simple shape (a plane, cylinder, sphere, cone or
//! torus) or freeform, so that every triangle is in one. After SurfaceCraft's Find All:
//!
//! - The scan's noise is measured first (a quadric fitted to small patches all over it, the
//!   lower middle of how far their points stray), and sets how close a shape must lie to it.
//! - Shapes are taken from the inside out: the triangles furthest from any crease or open edge
//!   are tried first. Each seed's patch is grown as every kind of shape, fitted again as it grows,
//!   and the kind that takes the most of the scan is kept, a simpler kind where it takes nearly as
//!   much (a small patch of a wide shaft is flat too, so the kind is told by what it grows into).
//! - Shapes that meet and are one are joined, and what no shape takes is freeform, in connected
//!   regions; a sliver of it goes to the neighbour it shares most edges with.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::base::edge_neighbours;
use crate::fit::{self, Kind, Shape};

/// How Find All reads a scan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// A shape must lie within this many times the scan's noise of every corner it takes.
    pub noise_multiple: f64,
    /// ... and within at least this share of the scan's size, however clean the scan.
    pub form_share: f64,
    /// A seed's patch reaches this share of the scan's size, with at least `seed_triangles`
    /// triangles and at most `patch_limit`.
    pub seed_radius_share: f64,
    pub seed_triangles: usize,
    pub patch_limit: usize,
    /// A shape needs this many triangles and this share of the scan's area.
    pub minimum_triangles: usize,
    pub minimum_area_share: f64,
    /// The most seeds tried.
    pub maximum_seeds: usize,
    /// How many places the noise is measured at, and each patch's reach as a share of the size.
    pub noise_samples: usize,
    pub noise_radius_share: f64,
    /// A simpler kind is kept while it takes at least this share of what the best kind takes.
    pub simpler_share: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            noise_multiple: 3.0,
            form_share: 0.001,
            seed_radius_share: 0.04,
            seed_triangles: 48,
            patch_limit: 20_000,
            minimum_triangles: 24,
            minimum_area_share: 0.001,
            maximum_seeds: 600,
            noise_samples: 200,
            noise_radius_share: 0.015,
            simpler_share: 0.9,
        }
    }
}

/// A region of the scan: a shape's, or freeform (`shape` none). A plane's normal faces out of
/// the material, as the scan is wound.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub triangles: Vec<usize>,
    pub shape: Option<Shape>,
    /// In mm².
    pub area: f64,
    /// The root mean square of its corners' distance from the shape (0 for freeform).
    pub rms: f64,
}

/// What Find All found.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Shapes first, largest first, then freeform regions, largest first.
    pub segments: Vec<Segment>,
    /// The scan's noise, and how close a shape had to lie, in mm.
    pub noise: f64,
    pub tolerance: f64,
    /// The share of the scan's area in shapes.
    pub in_shapes: f64,
    /// Whether it stopped because it was asked to (the segments are what it had).
    pub cancelled: bool,
}

/// The kinds tried, simplest first.
const KINDS: [Kind; 5] = [Kind::Plane, Kind::Cylinder, Kind::Sphere, Kind::Cone, Kind::Torus];

/// The points a seed's patch is fitted to, and a growing region each time it is fitted again.
const PATCH_POINTS: usize = 3_000;
const GROWING_POINTS: usize = 6_000;

/// No triangle across an edge.
const NONE: u32 = u32::MAX;

/// A triangle in no segment.
const OPEN: u32 = u32::MAX;

/// Neighbours whose directions differ by more than this meet at a crease.
const CREASE: f64 = 30.0 * std::f64::consts::PI / 180.0;

/// A triangle's direction may lean this far from the shape's and still be on it.
const LEAN: f64 = 20.0 * std::f64::consts::PI / 180.0;

/// A round shape this many times wider than the region it covers is flat (a plane takes it).
const FLAT: f64 = 20.0;

/// The triangles of a scan as Find All reads them, and what it has taken so far.
struct Reading<'a> {
    scan: Scan<'a>,
    /// The triangle across each edge (the edge from corner k to the next), or [`NONE`].
    across: Vec<[u32; 3]>,
    middles: Vec<Vec3>,
    normals: Vec<Option<Vec3>>,
    areas: Vec<f64>,
    /// Each triangle's segment, or [`OPEN`].
    label: Vec<u32>,
    /// Visits, by stamp: a triangle is seen in this visit when its stamp is `now`.
    stamp: Vec<u32>,
    now: u32,
}

impl<'a> Reading<'a> {
    fn new(scan: Scan<'a>) -> Self {
        let n = scan.triangle_count();
        let neighbours = edge_neighbours(&scan);
        let mut across = vec![[NONE; 3]; n];
        for (t, edges) in across.iter_mut().enumerate() {
            let Some(&[a, b, c]) = scan.triangles.get(t) else { continue };
            for n in neighbours.of(&scan, t) {
                let Some(other) = scan.triangles.get(n) else { continue };
                for (k, (p, q)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
                    if edges[k] == NONE && other.contains(&p) && other.contains(&q) {
                        edges[k] = n as u32;
                        break;
                    }
                }
            }
        }
        let (mut middles, mut normals, mut areas) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for t in 0..n {
            match scan.corners(t) {
                Some([a, b, c]) => {
                    let cross = (b - a).cross(c - a);
                    middles.push((a + b + c) / 3.0);
                    normals.push(cross.normalized());
                    areas.push(cross.len() / 2.0);
                }
                None => {
                    middles.push(Vec3::ZERO);
                    normals.push(None);
                    areas.push(0.0);
                }
            }
        }
        Reading { scan, across, middles, normals, areas, label: vec![OPEN; n], stamp: vec![0; n], now: 0 }
    }

    fn count(&self) -> usize {
        self.middles.len()
    }

    /// The triangles across `t`'s edges.
    fn beside(&self, t: usize) -> impl Iterator<Item = usize> + '_ {
        self.across.get(t).into_iter().flatten().filter(|&&n| n != NONE).map(|&n| n as usize)
    }

    /// A new visit: no triangle is seen in it yet.
    fn visit(&mut self) -> u32 {
        self.now = self.now.wrapping_add(1);
        if self.now == 0 {
            self.stamp.fill(0);
            self.now = 1;
        }
        self.now
    }

    fn seen(&mut self, t: usize, visit: u32) -> bool {
        match self.stamp.get_mut(t) {
            Some(s) if *s == visit => true,
            Some(s) => {
                *s = visit;
                false
            }
            None => true,
        }
    }

    fn is_open(&self, t: usize) -> bool {
        self.label.get(t).is_some_and(|&l| l == OPEN)
    }

    /// The open triangles reached from `seed` across edges whose middles lie within `reach` of
    /// the seed's, at most `limit` of them.
    fn patch(&mut self, seed: usize, reach: f64, limit: usize) -> Vec<usize> {
        let visit = self.visit();
        let at = self.middles[seed];
        self.seen(seed, visit);
        let mut found = vec![seed];
        let mut head = 0;
        while let Some(&t) = found.get(head) {
            head += 1;
            for n in self.beside(t).collect::<Vec<_>>() {
                if found.len() >= limit {
                    return found;
                }
                if self.is_open(n) && self.middles[n].dist(at) <= reach && !self.seen(n, visit) {
                    found.push(n);
                }
            }
        }
        found
    }

    /// The open triangles reached from `seeds` that lie on `shape`: every corner within
    /// `tolerance`, its direction within [`LEAN`] of the shape's, and the turn from the triangle
    /// it is reached from within `bend` of the shape's turn there.
    fn grow(&mut self, seeds: &[usize], shape: &Shape, tolerance: f64, bend: f64) -> Vec<usize> {
        let visit = self.visit();
        let cos = LEAN.cos();
        let mut found = Vec::new();
        for &s in seeds {
            if self.is_open(s) && !self.seen(s, visit) {
                found.push(s);
            }
        }
        let mut head = 0;
        while let Some(&t) = found.get(head) {
            head += 1;
            let (from, a) = (self.middles[t], self.normals[t]);
            for k in 0..3 {
                let n = self.across[t][k];
                if n == NONE {
                    continue;
                }
                let n = n as usize;
                if !self.is_open(n) || self.stamp[n] == visit {
                    continue;
                }
                let Some(corners) = self.scan.corners(n) else { continue };
                if !corners.iter().all(|p| shape.distance(*p).abs() <= tolerance) {
                    continue;
                }
                let (to, b) = (self.middles[n], self.normals[n]);
                if let (Some(b), Some(m)) = (b, shape.normal_at(to))
                    && b.dot(m).abs() < cos
                {
                    continue;
                }
                if let (Some(a), Some(b), Some(u), Some(v)) = (a, b, shape.normal_at(from), shape.normal_at(to)) {
                    let turn = |x: Vec3, y: Vec3| x.dot(y).abs().min(1.0).acos();
                    if (turn(a, b) - turn(u, v)).abs() > bend {
                        continue;
                    }
                }
                self.stamp[n] = visit;
                found.push(n);
            }
        }
        found
    }

    /// The turn allowed between neighbours on `shape`, read from how far it is off in `region`:
    /// half as much again as the most it is off between nearly every pair (the 995th in a
    /// thousand), and at least 2°. Read so, a faceted mesh's turns at its facets' edges, few and
    /// all alike, are allowed, as a noisy scan's are.
    fn bend_slack(&mut self, region: &[usize], shape: &Shape) -> f64 {
        let visit = self.visit();
        for &t in region {
            if let Some(s) = self.stamp.get_mut(t) {
                *s = visit;
            }
        }
        let turn = |x: Vec3, y: Vec3| x.dot(y).abs().min(1.0).acos();
        let mut offs = Vec::new();
        for &t in region {
            for n in self.beside(t).filter(|&n| n > t && self.stamp[n] == visit) {
                let (Some(a), Some(b)) = (self.normals[t], self.normals[n]) else { continue };
                let (Some(u), Some(v)) = (shape.normal_at(self.middles[t]), shape.normal_at(self.middles[n])) else { continue };
                offs.push((turn(a, b) - turn(u, v)).abs());
            }
        }
        (1.5 * crate::fit::high_share(&mut offs, 0.995)).max(2f64.to_radians())
    }

    fn area_of(&self, region: &[usize]) -> f64 {
        region.iter().map(|&t| self.areas[t]).sum()
    }

    /// The root mean square of the region's corners' distance from `shape`.
    fn rms(&self, region: &[usize], shape: &Shape) -> f64 {
        let (mut squares, mut count) = (0.0, 0usize);
        for &t in region {
            for p in self.scan.corners(t).into_iter().flatten() {
                let d = shape.distance(p);
                squares += d * d;
                count += 1;
            }
        }
        if count > 0 { (squares / count as f64).sqrt() } else { 0.0 }
    }

    /// How far across the region is, the diagonal of its box.
    fn across_of(&self, region: &[usize]) -> f64 {
        let mut corners = region.iter().filter_map(|&t| self.scan.corners(t)).flatten();
        let Some(first) = corners.next() else { return 0.0 };
        let (lo, hi) = corners.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        (hi - lo).len()
    }
}

/// Whether `shape`, fitted to a region `across` wide about `at` on a scan `size` across, is no more than a
/// flatter or simpler kind bent by noise or a scan's warp: a round far wider than the region, or
/// wider than the part, a cone as steep as a plane or as narrow as a cylinder.
fn degenerate(shape: &Shape, at: Vec3, across: f64, size: f64) -> bool {
    let wide = |r: f64| r > FLAT * across || r > size;
    match *shape {
        Shape::Plane { .. } => false,
        Shape::Cylinder { radius, .. } | Shape::Sphere { radius, .. } => wide(radius),
        // Its bend across at the region: the radius there over the cosine.
        Shape::Cone { apex, axis, half_angle } => {
            let v = at - apex;
            !(1.0f64.to_radians()..=89.0f64.to_radians()).contains(&half_angle) || wide((v - axis * v.dot(axis)).len() / half_angle.cos())
        }
        Shape::Torus { major, minor, .. } => wide(minor) || major > size,
    }
}

/// The scan's noise: a quadric fitted to the patch round each of `samples` triangles spread over
/// the scan, and the lower middle (the 30th in a hundred) of how far their points stray from it,
/// so that patches across a crease, which stray far, do not count.
fn noise(reading: &mut Reading, options: &Options, size: f64) -> f64 {
    let n = reading.count();
    if n == 0 {
        return 0.0;
    }
    let reach = options.noise_radius_share * size;
    let step = (n / options.noise_samples.max(1)).max(1);
    let mut spreads = Vec::new();
    for t in (0..n).step_by(step) {
        if reading.areas[t] > 0.0 {
            let patch = reading.patch(t, reach, 400);
            if let Some(spread) = (patch.len() >= 8).then(|| quadric_spread(&reading.scan, &patch)).flatten() {
                spreads.push(spread);
            }
        }
    }
    if spreads.is_empty() {
        return 0.0;
    }
    spreads.sort_by(f64::total_cmp);
    spreads[(spreads.len() * 3) / 10]
}

/// How far the patch's corners stray from the quadric `z = a x² + b xy + c y² + d x + e y + f`
/// fitted to them in the frame of their plane: the root mean square.
fn quadric_spread(scan: &Scan, patch: &[usize]) -> Option<f64> {
    let points: Vec<Vec3> = patch.iter().filter_map(|&t| scan.corners(t)).flatten().collect();
    let middle = points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len().max(1) as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in &points {
        let d = *p - middle;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    let normal = axes[0].normalized()?;
    let (u, v) = crate::in_plane(normal);
    let (mut a, mut b) = ([[0.0f64; 6]; 6], [0.0f64; 6]);
    for p in &points {
        let d = *p - middle;
        let (x, y, z) = (d.dot(u), d.dot(v), d.dot(normal));
        let row = [x * x, x * y, y * y, x, y, 1.0];
        for i in 0..6 {
            b[i] += row[i] * z;
            for j in 0..6 {
                a[i][j] += row[i] * row[j];
            }
        }
    }
    let q = solve6(a, b)?;
    let squares: f64 = points
        .iter()
        .map(|p| {
            let d = *p - middle;
            let (x, y, z) = (d.dot(u), d.dot(v), d.dot(normal));
            let off = z - (q[0] * x * x + q[1] * x * y + q[2] * y * y + q[3] * x + q[4] * y + q[5]);
            off * off
        })
        .sum();
    Some((squares / points.len() as f64).sqrt())
}

/// The 6 × 6 system `a x = b` by Gaussian elimination with partial pivoting; `None` if singular.
fn solve6(mut a: [[f64; 6]; 6], mut b: [f64; 6]) -> Option<[f64; 6]> {
    for col in 0..6 {
        let pivot = (col..6).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if !(a[pivot][col].abs() > 1e-300) {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (pivot_row, pivot_b) = (a[col], b[col]);
        for (row, rhs) in a.iter_mut().zip(b.iter_mut()).skip(col + 1) {
            let f = row[col] / pivot_row[col];
            for (cell, p) in row.iter_mut().zip(pivot_row).skip(col) {
                *cell -= f * p;
            }
            *rhs -= f * pivot_b;
        }
    }
    let mut x = [0.0; 6];
    for row in (0..6).rev() {
        let sum: f64 = (row + 1..6).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// How deep each triangle lies inside the smooth part of the scan: its count of edges from the
/// nearest crease or open edge.
fn depths(reading: &Reading) -> Vec<u32> {
    let n = reading.count();
    let mut depth = vec![u32::MAX; n];
    let mut queue = Vec::new();
    let cos = CREASE.cos();
    for (t, slot) in depth.iter_mut().enumerate() {
        let edge = reading.across[t].iter().any(|&a| {
            a == NONE
                || match (reading.normals[t], reading.normals[a as usize]) {
                    (Some(x), Some(y)) => x.dot(y) < cos,
                    _ => true,
                }
        });
        if edge {
            *slot = 0;
            queue.push(t);
        }
    }
    let mut head = 0;
    while let Some(&t) = queue.get(head) {
        head += 1;
        let next = depth[t].saturating_add(1);
        for n in reading.beside(t) {
            if depth[n] == u32::MAX {
                depth[n] = next;
                queue.push(n);
            }
        }
    }
    depth
}

/// A shape grown from a seed: the shape fitted again as its region grows, while it grows by a
/// tenth, then once more against the final shape. `None` when no shape of the kind fits the patch.
/// Grown by distance and direction alone: a shape that runs on over a neighbour in tangency gives
/// it back when the neighbour's own shape retakes it ([`retaken`]).
fn grow_kind(reading: &mut Reading, seed: usize, patch: &[usize], kind: Kind, tolerance: f64, size: f64) -> Option<(Shape, Vec<usize>)> {
    let mut shape = fit::shape_of_sampled(&reading.scan, patch, kind, PATCH_POINTS)?;
    // The patch must be the shape before it is grown: a kind that cannot follow it strays.
    if reading.rms(patch, &shape) > tolerance || degenerate(&shape, reading.middles[seed], reading.across_of(patch), size) && kind != Kind::Plane {
        return None;
    }
    let mut region = patch.to_vec();
    for _ in 0..16 {
        let grown = reading.grow(&[seed], &shape, tolerance, f64::INFINITY);
        let more = grown.len() * 10 >= region.len() * 11;
        let refit = fit::refined(&reading.scan, &grown, &shape, GROWING_POINTS);
        region = grown;
        if let Some(next) = refit {
            shape = next;
        }
        if !more {
            break;
        }
    }
    let region = reading.grow(&[seed], &shape, tolerance, f64::INFINITY);
    let shape = fit::refined(&reading.scan, &region, &shape, GROWING_POINTS).unwrap_or(shape);
    if degenerate(&shape, reading.middles[seed], reading.across_of(&region), size) {
        return None;
    }
    Some((shape, region))
}

/// Find All on `scan`. `cancelled` is asked between seeds; once it says so, the segments are what
/// it had, every triangle still in one.
pub fn find_all(scan: &Scan, options: &Options, cancelled: &dyn Fn() -> bool) -> Outcome {
    let mut reading = Reading::new(*scan);
    let n = reading.count();
    let size = scan.extent();
    let total: f64 = reading.areas.iter().sum();
    let noise = noise(&mut reading, options, size);
    let tolerance = (options.noise_multiple * noise).max(options.form_share * size).max(1e-9);
    let least_area = options.minimum_area_share * total;
    let reach = options.seed_radius_share * size;

    // Seeds, deepest first.
    let depth = depths(&reading);
    let mut order: Vec<usize> = (0..n).filter(|&t| reading.areas[t] > 0.0).collect();
    order.sort_by_key(|&t| (std::cmp::Reverse(depth[t]), t));
    let mut tried = vec![false; n];
    let mut shapes: Vec<(Shape, Vec<usize>)> = Vec::new();
    let mut seeds = 0usize;
    let mut stopped = false;
    for &seed in &order {
        if seeds >= options.maximum_seeds {
            break;
        }
        if tried[seed] || !reading.is_open(seed) {
            continue;
        }
        if cancelled() {
            stopped = true;
            break;
        }
        seeds += 1;
        let mut patch = reading.patch(seed, reach, options.patch_limit);
        let mut wider = reach;
        while patch.len() < options.seed_triangles && wider < 4.0 * reach {
            wider *= 2.0;
            patch = reading.patch(seed, wider, options.patch_limit);
        }
        for &t in &patch {
            tried[t] = true;
        }
        if patch.len() < options.seed_triangles / 2 {
            continue;
        }
        // Each kind grown; the one that takes most, or a simpler one taking nearly as much.
        let mut grown: Vec<(Shape, Vec<usize>, f64)> = Vec::new();
        for kind in KINDS {
            if let Some((shape, region)) = grow_kind(&mut reading, seed, &patch, kind, tolerance, size) {
                let area = reading.area_of(&region);
                grown.push((shape, region, area));
            }
        }
        let Some(most) = grown.iter().map(|g| g.2).max_by(f64::total_cmp) else { continue };
        let Some((shape, region, area)) = grown.into_iter().find(|g| g.2 >= options.simpler_share * most) else { continue };
        if region.len() < options.minimum_triangles || area < least_area {
            continue;
        }
        let id = shapes.len() as u32;
        for &t in &region {
            reading.label[t] = id;
        }
        shapes.push((shape, region));
    }

    let shapes = retaken(&mut reading, shapes, tolerance, options.minimum_triangles, least_area);
    let shapes = joined(&mut reading, shapes, tolerance);
    let mut segments: Vec<Segment> = shapes
        .into_iter()
        .map(|(shape, triangles)| {
            let (area, rms) = (reading.area_of(&triangles), reading.rms(&triangles, &shape));
            Segment { triangles, shape: Some(shape), area, rms }
        })
        .collect();
    leftovers(&mut reading, &mut segments, options.minimum_triangles, least_area);
    // Each plane faces out of the material, as the scan is wound.
    let outward = crate::reconstruct::outward(scan);
    for segment in &mut segments {
        if let Some(Shape::Plane { point, normal }) = segment.shape {
            let facing: f64 = segment.triangles.iter().filter_map(|&t| reading.normals[t].map(|n| n.dot(normal) * reading.areas[t])).sum();
            if facing * outward < 0.0 {
                segment.shape = Some(Shape::Plane { point, normal: -normal });
            }
        }
    }
    let in_shapes = segments.iter().filter(|s| s.shape.is_some()).map(|s| s.area).sum::<f64>() / total.max(1e-300);
    segments.sort_by(|a, b| b.shape.is_some().cmp(&a.shape.is_some()).then(b.area.total_cmp(&a.area)));
    Outcome { segments, noise, tolerance, in_shapes, cancelled: stopped }
}

/// Each shape, largest first, takes back the triangles beside it that it fits more closely than
/// the shape that took them (every corner nearer it, and on it by the rules a region grows by): a
/// shape grown from a seed on the edge between two faces, which straddles them, gives each back
/// to its own. Shapes changed are fitted again; one left too small gives its triangles up.
fn retaken(
    reading: &mut Reading,
    mut shapes: Vec<(Shape, Vec<usize>)>,
    tolerance: f64,
    least_triangles: usize,
    least_area: f64,
) -> Vec<(Shape, Vec<usize>)> {
    let cos = LEAN.cos();
    let worst = |reading: &Reading, t: usize, shape: &Shape| {
        reading.scan.corners(t).map_or(f64::INFINITY, |c| c.iter().map(|p| shape.distance(*p).abs()).fold(0.0, f64::max))
    };
    for _ in 0..3 {
        let mut order: Vec<usize> = (0..shapes.len()).collect();
        let areas: Vec<f64> = shapes.iter().map(|(_, r)| reading.area_of(r)).collect();
        order.sort_by(|&a, &b| areas[b].total_cmp(&areas[a]).then(a.cmp(&b)));
        let mut changed = vec![false; shapes.len()];
        for &id in &order {
            let shape = shapes[id].0;
            let bend = reading.bend_slack(&shapes[id].1, &shape);
            let mut front: Vec<usize> = shapes[id].1.clone();
            let mut taken = Vec::new();
            while let Some(t) = front.pop() {
                let (from, a) = (reading.middles[t], reading.normals[t]);
                for k in 0..3 {
                    let n = reading.across[t][k];
                    if n == NONE {
                        continue;
                    }
                    let n = n as usize;
                    let owner = reading.label[n];
                    if owner == OPEN || owner as usize == id {
                        continue;
                    }
                    let here = worst(reading, n, &shape);
                    if !(here <= tolerance && here < worst(reading, n, &shapes[owner as usize].0)) {
                        continue;
                    }
                    let (to, b) = (reading.middles[n], reading.normals[n]);
                    if let (Some(b), Some(m)) = (b, shape.normal_at(to))
                        && b.dot(m).abs() < cos
                    {
                        continue;
                    }
                    if let (Some(a), Some(b), Some(u), Some(v)) = (a, b, shape.normal_at(from), shape.normal_at(to)) {
                        let turn = |x: Vec3, y: Vec3| x.dot(y).abs().min(1.0).acos();
                        if (turn(a, b) - turn(u, v)).abs() > bend {
                            continue;
                        }
                    }
                    changed[owner as usize] = true;
                    changed[id] = true;
                    reading.label[n] = id as u32;
                    taken.push(n);
                    front.push(n);
                }
            }
            shapes[id].1.extend(taken);
        }
        if !changed.iter().any(|c| *c) {
            break;
        }
        // Each shape's own triangles again, and those changed fitted again.
        for (_, region) in shapes.iter_mut() {
            region.clear();
        }
        for t in 0..reading.count() {
            let owner = reading.label[t];
            if owner != OPEN {
                shapes[owner as usize].1.push(t);
            }
        }
        for (id, (shape, region)) in shapes.iter_mut().enumerate() {
            if changed[id]
                && !region.is_empty()
                && let Some(next) = fit::refined(&reading.scan, region, shape, GROWING_POINTS)
            {
                *shape = next;
            }
        }
    }
    // Those left too small give their triangles up.
    for (shape, region) in shapes.iter_mut() {
        if region.len() < least_triangles
            || reading.area_of(region) < least_area
            || degenerate(shape, reading.middles[region.first().copied().unwrap_or(0)], reading.across_of(region), f64::INFINITY)
        {
            for &t in region.iter() {
                reading.label[t] = OPEN;
            }
            region.clear();
        }
    }
    let kept: Vec<(Shape, Vec<usize>)> = shapes.into_iter().filter(|(_, r)| !r.is_empty()).collect();
    for (id, (_, region)) in kept.iter().enumerate() {
        for &t in region {
            reading.label[t] = id as u32;
        }
    }
    kept
}

/// Whether two shapes of one kind are one: the same plane, round or ball, within `tolerance`.
fn same_shape(a: &Shape, b: &Shape, tolerance: f64) -> bool {
    let parallel = |x: Vec3, y: Vec3| x.dot(y).abs() >= 2f64.to_radians().cos();
    let off_line = |p: Vec3, (q, d): (Vec3, Vec3)| {
        let v = p - q;
        (v - d * v.dot(d)).len()
    };
    match (*a, *b) {
        (Shape::Plane { point: p, normal: n }, Shape::Plane { point: q, normal: m }) => parallel(n, m) && (q - p).dot(n).abs() <= tolerance,
        (Shape::Cylinder { point: p, axis: d, radius: r }, Shape::Cylinder { point: q, axis: e, radius: s }) => {
            parallel(d, e) && off_line(q, (p, d)) <= 2.0 * tolerance && (r - s).abs() <= 2.0 * tolerance
        }
        (Shape::Sphere { center: c, radius: r }, Shape::Sphere { center: e, radius: s }) => {
            c.dist(e) <= 2.0 * tolerance && (r - s).abs() <= 2.0 * tolerance
        }
        (Shape::Cone { apex: p, axis: d, half_angle: x }, Shape::Cone { apex: q, axis: e, half_angle: y }) => {
            d.dot(e) >= 2f64.to_radians().cos() && p.dist(q) <= 4.0 * tolerance && (x - y).abs() <= 1f64.to_radians()
        }
        (Shape::Torus { center: c, axis: d, major: r, minor: s }, Shape::Torus { center: e, axis: f, major: t, minor: u }) => {
            parallel(d, f) && c.dist(e) <= 2.0 * tolerance && (r - t).abs() <= 2.0 * tolerance && (s - u).abs() <= 2.0 * tolerance
        }
        _ => false,
    }
}

/// Shapes that meet across edges and are one, joined (fitted again over both, kept where every
/// corner still lies within `tolerance`).
fn joined(reading: &mut Reading, mut shapes: Vec<(Shape, Vec<usize>)>, tolerance: f64) -> Vec<(Shape, Vec<usize>)> {
    loop {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (id, (_, region)) in shapes.iter().enumerate() {
            for &t in region {
                for n in reading.beside(t) {
                    let other = reading.label[n];
                    if other != OPEN && (other as usize) > id && !pairs.contains(&(id, other as usize)) {
                        pairs.push((id, other as usize));
                    }
                }
            }
        }
        let mut merged = false;
        for (a, b) in pairs {
            if shapes[a].1.is_empty() || shapes[b].1.is_empty() || !same_shape(&shapes[a].0, &shapes[b].0, tolerance) {
                continue;
            }
            let both: Vec<usize> = shapes[a].1.iter().chain(&shapes[b].1).copied().collect();
            let Some(shape) = fit::shape_of(&reading.scan, &both, shapes[a].0.kind()) else { continue };
            let holds = both.iter().all(|&t| reading.scan.corners(t).is_some_and(|c| c.iter().all(|p| shape.distance(*p).abs() <= tolerance)));
            if !holds {
                continue;
            }
            for &t in &shapes[b].1 {
                reading.label[t] = a as u32;
            }
            shapes[a] = (shape, both);
            shapes[b].1.clear();
            merged = true;
        }
        if !merged {
            break;
        }
    }
    // Numbered again without the emptied ones.
    let kept: Vec<(Shape, Vec<usize>)> = shapes.into_iter().filter(|(_, r)| !r.is_empty()).collect();
    for (id, (_, region)) in kept.iter().enumerate() {
        for &t in region {
            reading.label[t] = id as u32;
        }
    }
    kept
}

/// What no shape took: connected freeform regions, and each sliver of them (fewer than
/// `least_triangles` or less than `least_area`) given to the segment it shares most edges with.
fn leftovers(reading: &mut Reading, segments: &mut Vec<Segment>, least_triangles: usize, least_area: f64) {
    let n = reading.count();
    let mut pieces: Vec<Vec<usize>> = Vec::new();
    let visit = reading.visit();
    for start in 0..n {
        if !reading.is_open(start) || reading.seen(start, visit) {
            continue;
        }
        let mut piece = vec![start];
        let mut head = 0;
        while let Some(&t) = piece.get(head) {
            head += 1;
            for k in 0..3 {
                let a = reading.across[t][k];
                if a != NONE && reading.is_open(a as usize) && !reading.seen(a as usize, visit) {
                    piece.push(a as usize);
                }
            }
        }
        pieces.push(piece);
    }
    let mut slivers = Vec::new();
    for piece in pieces {
        let area = reading.area_of(&piece);
        if piece.len() < least_triangles || area < least_area {
            slivers.push(piece);
            continue;
        }
        let id = segments.len() as u32;
        for &t in &piece {
            reading.label[t] = id;
        }
        segments.push(Segment { triangles: piece, shape: None, area, rms: 0.0 });
    }
    for sliver in slivers {
        let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
        for &t in &sliver {
            for b in reading.beside(t) {
                let l = reading.label[b];
                if l != OPEN {
                    *counts.entry(l).or_default() += 1;
                }
            }
        }
        let area = reading.area_of(&sliver);
        match counts.into_iter().max_by_key(|&(l, c)| (c, std::cmp::Reverse(l))) {
            Some((l, _)) if (l as usize) < segments.len() => {
                for &t in &sliver {
                    reading.label[t] = l;
                }
                let segment = &mut segments[l as usize];
                segment.triangles.extend(&sliver);
                segment.area += area;
            }
            _ => {
                let id = segments.len() as u32;
                for &t in &sliver {
                    reading.label[t] = id;
                }
                segments.push(Segment { triangles: sliver, shape: None, area, rms: 0.0 });
            }
        }
    }
    for segment in segments.iter_mut() {
        if let Some(shape) = segment.shape {
            segment.rms = reading.rms(&segment.triangles, &shape);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, revolved, steps, turned};

    fn kinds(outcome: &Outcome) -> Vec<(Kind, f64)> {
        outcome.segments.iter().filter_map(|s| s.shape.map(|sh| (sh.kind(), s.area))).collect()
    }

    /// A box 60 × 40 × 20, finely cut, turned and moved anywhere: six planes and nothing else.
    #[test]
    fn a_box_is_six_planes() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 40.0, 20.0), 24);
        let axis = Vec3::new(0.2, -0.7, 0.4).normalized().unwrap();
        let moved = turned(&positions, axis, 0.8, Vec3::new(5.0, 9.0, -30.0));
        let scan = Scan::new(&moved, &triangles);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let found = kinds(&outcome);
        assert_eq!(found.len(), 6, "{found:?}");
        assert!(found.iter().all(|f| f.0 == Kind::Plane), "{found:?}");
        assert!((found[0].1 - 2400.0).abs() < 1.0, "{found:?}");
        assert!(outcome.in_shapes > 0.999, "{}", outcome.in_shapes);
        assert_eq!(outcome.segments.iter().map(|s| s.triangles.len()).sum::<usize>(), triangles.len());
    }

    /// The turned part: a shaft, a chamfer, two ends and a round between them. Each face is its own
    /// kind, the round included, though it runs into the shaft and an end in tangency.
    #[test]
    fn a_turned_part_is_its_faces() {
        let mut profile = vec![[-4.0, 0.0]];
        profile.extend(steps([-4.0, 0.5], [-4.0, 15.5], 30));
        profile.extend((0..=24).map(|i| f64::from(i) / 24.0 * std::f64::consts::FRAC_PI_2).map(|a| [-4.0 * a.cos(), 16.0 + 4.0 * a.sin()]));
        profile.extend(steps([0.0, 20.0], [20.0, 20.0], 40).into_iter().skip(1));
        profile.extend(steps([20.0, 20.0], [25.0, 15.0], 14).into_iter().skip(1));
        profile.extend(steps([25.0, 15.0], [25.0, 0.5], 29).into_iter().skip(1));
        profile.push([25.0, 0.0]);
        let (positions, triangles) = revolved(&profile, 180, false);
        let scan = Scan::new(&positions, &triangles);
        let outcome = find_all(&scan, &Options::default(), &|| false);
        let found = kinds(&outcome);
        let count = |k: Kind| found.iter().filter(|f| f.0 == k).count();
        assert_eq!((count(Kind::Plane), count(Kind::Cylinder), count(Kind::Cone), count(Kind::Torus)), (2, 1, 1, 1), "{found:?}");
        for segment in outcome.segments.iter().filter(|s| s.shape.is_some()) {
            match segment.shape {
                Some(Shape::Cylinder { radius, .. }) => assert!((radius - 20.0).abs() < 0.01, "{radius}"),
                Some(Shape::Cone { half_angle, .. }) => assert!((half_angle.to_degrees() - 45.0).abs() < 0.1, "{half_angle}"),
                Some(Shape::Torus { major, minor, .. }) => assert!((major - 16.0).abs() < 0.05 && (minor - 4.0).abs() < 0.05, "{major} {minor}"),
                _ => {}
            }
        }
    }
}
