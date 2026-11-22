//! The plane a scan is a mirror image about ([`find`]), or the one near a line drawn along it
//! ([`find_guided`]), and how well the scan matches its own reflection in it.
//!
//! The search reflects a sample of the points in a trial plane, finds where each reflection lands
//! on the scan, and moves the plane to the bisector of each point and what it landed on, over the
//! better-matched four fifths; it starts from the directions the scan is longest and shortest in.
//! The answer is then polished against the surface itself: each reflection's distance along the
//! surface's direction, by a few steps of Tukey-weighted Gauss–Newton on the plane's two turns and
//! its place.

use std::f64::consts::PI;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::place::Rigid;
use crate::{Scan, thinned};

/// A mirror plane of a scan and how well the scan agrees with its reflection in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    /// A point of the plane, over the middle of the scan's box.
    pub point: Vec3,
    /// A unit vector square to the plane (either way).
    pub normal: Vec3,
    /// The typical distance (mm) from a point's reflection to the scan, over the best four fifths.
    pub rms: f64,
    /// The share of the scan whose reflection lands within a few point spacings of the scan.
    pub coverage: f64,
    /// The diagonal of the scan's box and the spacing of its points (mm).
    pub size: f64,
    pub spacing: f64,
    /// How far out from the plane the part that matches reaches, as a share of how far the scan
    /// itself reaches, on the side where that is less (0–1): near 1 when both sides of the thing
    /// are there and match out to their ends; low for a scan of one side and a little past the
    /// middle, whose only match is the strip about the plane. 0 when not judged.
    pub both_sides: f64,
}

impl Mirror {
    /// Whether the scan is a mirror image about the plane: most of it (85 %) lands on itself, and
    /// typically within 2 % of its size or two point spacings (a coarse mesh's spacing counted as
    /// at most [`SPACING_SHARE`] of its size). A thing with parts that are no mirror image (a
    /// car's underbody and exhaust: 76 % about its true plane) still counts from
    /// [`MOSTLY_MIRRORED`] when both its sides are there and match out to their ends
    /// ([`Mirror::both_sides`]); a scan of one side does not.
    pub fn is_symmetric(&self) -> bool {
        let enough = self.coverage >= MIRRORED || (self.coverage >= MOSTLY_MIRRORED && self.both_sides >= BOTH_SIDES);
        enough && self.rms <= (0.02 * self.size).max(2.0 * judged_spacing(self.spacing, self.size))
    }

    /// For a mirror image only most of which is one ([`MOSTLY_MIRRORED`], under [`MIRRORED`]): the
    /// share of the scan that has no match. That part is the thing's own (pipes down one side),
    /// not noise, and whoever halves or mirrors the scan has to be told.
    pub fn unmatched(&self) -> Option<f64> {
        (self.is_symmetric() && self.coverage < MIRRORED).then_some(1.0 - self.coverage)
    }

    /// Whether the whole scan is a mirror image about the plane, with no part that is on one side
    /// only ([`Mirror::unmatched`]): what completing a scan by mirroring asks, since the reflection
    /// of a one-sided part (a post on one corner, the pipes down one side) is not the thing.
    pub fn is_whole(&self) -> bool {
        self.is_symmetric() && self.coverage >= MIRRORED
    }

    /// Lower is better: how far off, weighted by how much of the scan has no match.
    fn score(&self) -> f64 {
        self.rms * (2.0 - self.coverage)
    }

    /// `p` reflected in the plane.
    pub fn reflect(&self, p: Vec3) -> Vec3 {
        reflect(p, self.normal, self.point)
    }
}

/// The share of a scan that must match its reflection for it to count as a mirror image outright.
const MIRRORED: f64 = 0.85;
/// The share of a scan that must match its reflection for it to count as a mirror image when both
/// its sides are there ([`Mirror::is_symmetric`]).
const MOSTLY_MIRRORED: f64 = 0.6;
/// The matching part reaches this share of the way out the scan does, on both sides, for both
/// sides to count as there.
const BOTH_SIDES: f64 = 0.9;

/// A coarse mesh's points lie far apart, a sizeable share of the part (948 triangles of the
/// Stanford bunny lie 11.5 mm apart, and "within three spacings" of them matched 95 % of a part
/// that is no mirror image). A match is judged as if they were at most this share of the size
/// apart, against points spread over its surface ([`surface_points`]), not its corners alone.
const SPACING_SHARE: f64 = 0.01;

/// The spacing a match is judged by: the points', or [`SPACING_SHARE`] of the size if less.
fn judged_spacing(spacing: f64, size: f64) -> f64 {
    spacing.min(SPACING_SHARE * size)
}

/// A triangle more than this many squares of the judged spacing in area gets points of its own
/// inside it, so a reflection is measured to the surface, not to the nearest of its corners.
const BIG_TRIANGLE: f64 = 4.0;

/// The most points added inside big triangles; past it they are spread more thinly.
const ADDED_LIMIT: usize = 4 * TABLE_LIMIT;

/// The scan's points, and points spread inside its big triangles about `spacing` apart: what a
/// coarse mesh or a reduced scan (big triangles, their corners on the edges) is matched against.
fn surface_points(scan: &Scan, vertices: &[Vec3], spacing: f64) -> Vec<Vec3> {
    let cell = spacing * spacing;
    let big = |area: f64| area > BIG_TRIANGLE * cell;
    let areas: Vec<(usize, f64)> = (0..scan.triangle_count())
        .filter_map(|t| scan.corners(t).map(|[a, b, c]| (t, (b - a).cross(c - a).len() / 2.0)))
        .filter(|&(_, area)| area.is_finite() && big(area))
        .collect();
    let wanted: f64 = areas.iter().map(|&(_, area)| area / cell).sum();
    let thin = (wanted / ADDED_LIMIT as f64).max(1.0);
    let mut points = vertices.to_vec();
    for (t, area) in areas {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        // A triangular grid of about `area / cell` points, each in the middle of its little triangle.
        let n = (area / cell / thin).floor().max(1.0);
        let m = (2.0 * n).sqrt().ceil().clamp(1.0, 400.0) as usize;
        for i in 0..m {
            for j in 0..m - i {
                let (u, v) = ((i as f64 + 1.0 / 3.0) / m as f64, (j as f64 + 1.0 / 3.0) / m as f64);
                points.push(a + (b - a) * u + (c - a) * v);
            }
        }
    }
    points
}

fn reflect(p: Vec3, normal: Vec3, point: Vec3) -> Vec3 {
    p - normal * (2.0 * (p - point).dot(normal))
}

/// A line drawn along the rough mirror plane, and the way the person was looking when they drew it
/// (from the scan toward the eye). The plane is searched near the one through the line and that
/// direction, turned about the line by up to 40° and tilted by up to 15°.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub from: Vec3,
    pub to: Vec3,
    pub toward: Vec3,
}

/// The most sample points the search reflects, and the most points it looks them up among.
const SAMPLE_LIMIT: usize = 3_000;
const TABLE_LIMIT: usize = 150_000;

/// Points in a grid of cells, for the nearest point to a place. The cells are one flat array over
/// the points' box, each cell's points stored together, so a lookup is arithmetic, not hashing.
struct Table {
    cell: f64,
    points: Vec<Vec3>,
    lo: Vec3,
    dims: [i64; 3],
    /// Where each cell's points start in `items` (one more entry than cells).
    starts: Vec<u32>,
    items: Vec<u32>,
}

/// The most cells a table makes; a finer grid over a larger box grows its cells to stay within it.
const MAX_CELLS: f64 = 4.0e6;

impl Table {
    fn new(points: Vec<Vec3>, cell: f64) -> Self {
        let mut cell = if cell.is_finite() && cell > 0.0 { cell } else { 1.0 };
        let finite = points.iter().filter(|p| p.is_finite());
        let (lo, hi) =
            finite.fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let (lo, hi) = if lo.x <= hi.x { (lo, hi) } else { (Vec3::ZERO, Vec3::ZERO) };
        let size = hi - lo;
        let count = |c: f64| ((size.x / c).floor() + 1.0) * ((size.y / c).floor() + 1.0) * ((size.z / c).floor() + 1.0);
        while count(cell) > MAX_CELLS {
            cell *= 1.5;
        }
        let dims = [(size.x / cell) as i64 + 1, (size.y / cell) as i64 + 1, (size.z / cell) as i64 + 1];
        let cells = (dims[0] * dims[1] * dims[2]) as usize;
        let mut table = Table { cell, points, lo, dims, starts: vec![0; cells + 1], items: Vec::new() };
        let slots: Vec<Option<usize>> = table.points.iter().map(|p| table.slot(table.key(*p))).collect();
        for s in slots.iter().flatten() {
            table.starts[s + 1] += 1;
        }
        for i in 0..cells {
            table.starts[i + 1] += table.starts[i];
        }
        let mut fill = table.starts.clone();
        table.items = vec![0; table.starts[cells] as usize];
        for (i, s) in slots.iter().enumerate() {
            if let Some(s) = *s {
                table.items[fill[s] as usize] = i as u32;
                fill[s] += 1;
            }
        }
        table
    }

    fn key(&self, p: Vec3) -> [i64; 3] {
        let d = p - self.lo;
        [(d.x / self.cell).floor() as i64, (d.y / self.cell).floor() as i64, (d.z / self.cell).floor() as i64]
    }

    /// The cell's place in the flat array, `None` outside the grid.
    fn slot(&self, [x, y, z]: [i64; 3]) -> Option<usize> {
        let [nx, ny, nz] = self.dims;
        ((0..nx).contains(&x) && (0..ny).contains(&y) && (0..nz).contains(&z)).then(|| ((z * ny + y) * nx + x) as usize)
    }

    /// The index of the point nearest `place` within `reach`, looking `span` cells either way. The
    /// rings of cells round the place's own are searched outward, and the search stops once the
    /// best so far is nearer than any point of the next ring can be.
    fn nearest_within(&self, place: Vec3, span: i64, reach: f64) -> Option<usize> {
        if !place.is_finite() {
            return None;
        }
        let [x, y, z] = self.key(place);
        let [nx, ny, nz] = self.dims;
        let mut best: Option<(usize, f64)> = None;
        let limit = reach * reach;
        for ring in 0..=span {
            for cz in (z - ring).max(0)..=(z + ring).min(nz - 1) {
                for cy in (y - ring).max(0)..=(y + ring).min(ny - 1) {
                    let inside = (cz - z).abs() < ring && (cy - y).abs() < ring;
                    // Inside the ring's faces only its two ends along X are new; on them, the whole row.
                    let (ends, whole) = ([(x - ring, x - ring), (x + ring, x + ring)], [(x - ring, x + ring)]);
                    let xs: &[(i64, i64)] = if inside { &ends } else { &whole };
                    for &(from, to) in xs {
                        let (from, to) = (from.max(0), to.min(nx - 1));
                        if from > to {
                            continue;
                        }
                        let row = (cz * ny + cy) * nx;
                        let (a, b) = (self.starts[(row + from) as usize] as usize, self.starts[(row + to + 1) as usize] as usize);
                        for &i in &self.items[a..b] {
                            let d = (self.points[i as usize] - place).len2();
                            if d <= limit && best.is_none_or(|(_, b)| d < b) {
                                best = Some((i as usize, d));
                            }
                        }
                    }
                }
            }
            // Every point of the next ring is at least `ring` cells from the place.
            let next = ring as f64 * self.cell;
            if best.is_some_and(|(_, d)| d <= next * next) {
                break;
            }
        }
        best.map(|(i, _)| i)
    }

    /// The point nearest `place` within `reach` (looking at most three cells either way).
    fn nearest(&self, place: Vec3, reach: f64) -> Option<Vec3> {
        let span = (reach / self.cell).ceil().clamp(1.0, 3.0) as i64;
        self.nearest_within(place, span, reach).and_then(|i| self.points.get(i).copied())
    }
}

/// The scan as the search sees it.
struct Scene {
    vertices: Vec<Vec3>,
    /// The surface's direction at each vertex (area-weighted, unit).
    normals: Vec<Vec3>,
    samples: Vec<Vec3>,
    table: Table,
    /// What a plane found is judged by ([`judge`]): points over the surface ([`surface_points`])
    /// and a sample of them, and how far apart they are (the vertices' spacing, or less on a
    /// coarse mesh, whose surface is filled in between its points).
    surface_table: Table,
    surface_samples: Vec<Vec3>,
    judged: f64,
    centroid: Vec3,
    /// The middle of the scan's box, and the box.
    middle: Vec3,
    bounds: Bounds,
    size: f64,
    spacing: f64,
    /// The vertices where the surface turns sharply (edges, rims, corners): what says where a plane
    /// is when most of a part is flat and lands on itself however the plane slides along it.
    features: Vec<Vec3>,
}

/// The scan's box. A reflection landing beyond it says nothing about a plane: that may be the side
/// of the part that was never scanned, so a part scanned from one side is not pulled towards a
/// plane that keeps its reflection on what there is. A reflection inside the box that lands on
/// nothing still counts against the plane: the scanner would have seen something there.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    lo: Vec3,
    hi: Vec3,
    pad: f64,
}

impl Bounds {
    fn holds(&self, p: Vec3) -> bool {
        let (lo, hi) = (self.lo - Vec3::new(self.pad, self.pad, self.pad), self.hi + Vec3::new(self.pad, self.pad, self.pad));
        (lo.x..=hi.x).contains(&p.x) && (lo.y..=hi.y).contains(&p.y) && (lo.z..=hi.z).contains(&p.z)
    }
}

/// A plane judged on fewer than this share of the points (the rest reflected out of the scan's
/// box) is judged as matching nothing: it has thrown the scan aside rather than onto itself.
const JUDGED_LEAST: f64 = 1.0 / 3.0;

/// The mean of `distances` of the reflections inside `bounds`, each at most `reach`; `reach` when
/// too few are inside to judge by.
fn mean_inside(reflections: impl Iterator<Item = (Vec3, f64)>, bounds: &Bounds, reach: f64) -> f64 {
    let (mut total, mut inside, mut all) = (0.0, 0usize, 0usize);
    for (m, d) in reflections {
        all += 1;
        if bounds.holds(m) {
            total += d;
            inside += 1;
        }
    }
    if inside == 0 || (inside as f64) < JUDGED_LEAST * all as f64 { reach } else { total / inside as f64 }
}

/// A vertex is on an edge when a triangle round it faces more than this far (cosine, about 30°)
/// from the way the surface faces there.
const EDGE_TURN: f64 = 0.87;

/// The most edge points the plane is settled on.
const FEATURE_LIMIT: usize = 20_000;

/// `None` for fewer than 50 vertices in use, or no area.
fn scene(scan: &Scan) -> Option<Scene> {
    let n = scan.positions.len();
    let mut used = vec![false; n];
    let mut sums = vec![Vec3::ZERO; n];
    let (mut area, mut centroid) = (0.0, Vec3::ZERO);
    for (t, tri) in scan.triangles.iter().enumerate() {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let cross = (b - a).cross(c - a);
        let half = cross.len() / 2.0;
        if !half.is_finite() {
            continue;
        }
        area += half;
        centroid += (a + b + c) * (half / 3.0);
        for &v in tri {
            if let (Some(u), Some(s)) = (used.get_mut(v as usize), sums.get_mut(v as usize)) {
                *u = true;
                *s += cross;
            }
        }
    }
    let members: Vec<usize> = (0..n).filter(|&i| used[i]).collect();
    if !(members.len() >= 50 && area > 0.0) {
        return None;
    }
    let vertices: Vec<Vec3> = members.iter().filter_map(|&i| scan.positions.get(i).copied()).collect();
    let normals: Vec<Vec3> = members.iter().map(|&i| sums[i].normalized().unwrap_or(Vec3::Z)).collect();
    // How far the triangles round each point turn from the way the surface faces there, points at
    // one place counted as one (a file may write each face's corners apart).
    let weld = crate::welded(scan.positions);
    let mut joined = vec![Vec3::ZERO; n];
    for (i, &rep) in weld.iter().enumerate() {
        joined[rep as usize] += sums[i];
    }
    let mut least = vec![1.0f64; n];
    for (t, tri) in scan.triangles.iter().enumerate() {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let Some(face) = (b - a).cross(c - a).normalized() else { continue };
        for &v in tri {
            let rep = weld[v as usize] as usize;
            if let Some(normal) = joined[rep].normalized() {
                least[rep] = least[rep].min(face.dot(normal));
            }
        }
    }
    let features: Vec<Vec3> = thinned(
        &members
            .iter()
            .filter(|&&i| weld[i] as usize == i && least[i] < EDGE_TURN)
            .filter_map(|&i| scan.positions.get(i).copied())
            .collect::<Vec<_>>(),
        FEATURE_LIMIT,
    );
    let centroid = centroid / area;
    let (lo, hi) = vertices.iter().fold((vertices[0], vertices[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let size = (hi - lo).len();
    let spacing = (area / vertices.len() as f64).sqrt();
    let factor = if vertices.len() > TABLE_LIMIT { (vertices.len() as f64 / TABLE_LIMIT as f64).sqrt() } else { 1.0 };
    let table = Table::new(thinned(&vertices, TABLE_LIMIT), 2.5 * spacing * factor);
    let samples = thinned(&vertices, SAMPLE_LIMIT);
    let judged = judged_spacing(spacing, size);
    let surface = surface_points(scan, &vertices, judged);
    let factor = if surface.len() > TABLE_LIMIT { (surface.len() as f64 / TABLE_LIMIT as f64).sqrt() } else { 1.0 };
    let surface_table = Table::new(thinned(&surface, TABLE_LIMIT), 2.5 * judged * factor);
    let surface_samples = thinned(&surface, SAMPLE_LIMIT);
    Some(Scene {
        vertices,
        normals,
        samples,
        table,
        surface_table,
        surface_samples,
        judged,
        centroid,
        middle: (lo + hi) / 2.0,
        bounds: Bounds { lo, hi, pad: 2.0 * spacing },
        size,
        spacing,
        features,
    })
}

/// The best mirror plane of the scan, `None` for a scan too small to search.
pub fn find(scan: &Scan) -> Option<Mirror> {
    let scene = scene(scan)?;
    let mut spread = [[0.0f64; 3]; 3];
    for v in &scene.samples {
        let d = *v - scene.centroid;
        let d = [d.x, d.y, d.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (_, axes) = solidcraft_geom::inertia::principal(&spread, 0.0)?;
    let coarse = Table::new(thinned(&scene.vertices, TABLE_LIMIT), (scene.size / 24.0).max(3.0 * scene.spacing));
    let probes = thinned(&scene.samples, 500);
    let mut starts: Vec<(Vec3, Vec3, f64)> = axes
        .iter()
        .filter_map(|axis| axis.normalized())
        .map(|axis| {
            let (normal, point) = refine(axis, scene.centroid, &probes, &coarse, 0.5 * coarse.cell, scene.size, 6);
            (normal, point, assess(normal, point, &scene).score())
        })
        .collect();
    starts.sort_by(|a, b| a.2.total_cmp(&b.2));
    let best = starts
        .iter()
        .take(2)
        .map(|s| {
            let (normal, point) = refine(s.0, s.1, &scene.samples, &scene.table, 2.0 * scene.spacing, scene.size, 8);
            assess(normal, point, &scene)
        })
        .min_by(|a, b| a.score().total_cmp(&b.score()))?;
    let (normal, point) = match Edges::new(&scene) {
        Some(e) => {
            let (n, p, _) = e.settle(best.normal, best.point, Stage::Coarse, &scene);
            let (n, p, _) = e.settle(n, p, Stage::Fine, &scene);
            (n, p)
        }
        None => (best.normal, best.point),
    };
    let settled = assess(normal, point, &scene);
    let best = if settled.score() <= best.score() * 1.05 { settled } else { best };
    Some(judge(polish(&best, &scene, 12), &scene))
}

/// How far a guided plane may turn from the guide's own direction.
const GUIDE_TURN: f64 = 25.0 * PI / 180.0;

/// The mirror plane near the one `guide` marks: the turns about the line and the places along the
/// plane's normal ranked on a very coarse picture of the scan, the best three a good way apart and
/// their tilts settled on a coarse one, the best of those finished on the scan's own spacing and
/// polished. `None` for a guide with no length, one seen end-on, or a scan too small to search.
pub fn find_guided(scan: &Scan, guide: &Guide) -> Option<Mirror> {
    let toward = guide.toward.normalized()?;
    let line = guide.to - guide.from;
    let along = (line - toward * line.dot(toward)).normalized()?;
    let seed_normal = along.cross(toward).normalized()?;
    let seed_point = (guide.from + guide.to) / 2.0;
    let scene = scene(scan)?;
    let ranking = Table::new(thinned(&scene.vertices, 4_000), (scene.size / 12.0).max(3.0 * scene.spacing));
    let coarse = Table::new(thinned(&scene.vertices, 15_000), (scene.size / 24.0).max(3.0 * scene.spacing));
    let probes = thinned(&scene.samples, 250);
    let finer = thinned(&scene.samples, 800);
    let reach = ranking.cell;
    let bounds = scene.bounds;
    let rough = |normal: Vec3, point: Vec3, table: &Table, reach: f64| roughness(normal, point, &probes, table, reach, &bounds);

    // The plane stays near the guide: within GUIDE_TURN of its direction and a quarter of the scan
    // of where it was drawn, whatever matches better further off (on a scan of one side, a plane
    // turned well away can match more of what is there).
    let near = |n: Vec3, p: Vec3| n.dot(seed_normal).abs() >= GUIDE_TURN.cos() && (p - seed_point).dot(n).abs() <= scene.size / 4.0;
    // The turns about the line (up to GUIDE_TURN either way, 5° apart) and the places along the
    // normal (to a quarter of the scan either way).
    let step = 5.0 * PI / 180.0;
    let turns = (GUIDE_TURN / step).round() as i32;
    let mut trials: Vec<(Vec3, Vec3, f64)> = Vec::new();
    for k in -turns..=turns {
        let normal = turned(seed_normal, f64::from(k) * step, along);
        for j in -3..=3 {
            let point = seed_point + normal * (f64::from(j) * scene.size / 12.0);
            trials.push((normal, point, rough(normal, point, &ranking, reach)));
        }
    }
    trials.sort_by(|a, b| a.2.total_cmp(&b.2));
    let apart = |kept: &[(Vec3, Vec3)], n: Vec3, p: Vec3| {
        kept.iter().all(|k| k.0.dot(n) < (8.0 * PI / 180.0).cos() || (k.1 - p).dot(n).abs() > scene.size / 10.0)
    };
    let mut seeds: Vec<(Vec3, Vec3)> = Vec::new();
    for &(n, p, _) in &trials {
        if seeds.len() < 3 && apart(&seeds, n, p) {
            seeds.push((n, p));
        }
    }
    // Each is tilted about the line across it, and a tilt that does better is tried too.
    let mut starts = seeds.clone();
    for &(n, p) in &seeds {
        let Some(across) = along.cross(n).normalized() else { continue };
        let base = rough(n, p, &ranking, reach);
        let tilt = [-15.0f64, -7.5, 7.5, 15.0]
            .iter()
            .map(|a| turned(n, a * PI / 180.0, across))
            .map(|t| (t, rough(t, p, &ranking, reach)))
            .filter(|(_, r)| *r < base)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((t, _)) = tilt {
            starts.push((t, p));
        }
    }
    // Each start settled where the scan's edges land on its edges, coarsely; the best of them
    // finished on the scan's own spacing.
    let edges = Edges::new(&scene);
    let (normal, point, _) = starts
        .iter()
        .map(|&(n, p)| {
            let (n, p) = refine(n, p, &probes, &coarse, 0.5 * coarse.cell, scene.size, 5);
            let settled = match &edges {
                Some(e) => e.settle(n, p, Stage::Coarse, &scene),
                None => (n, p, rough(n, p, &coarse, 3.0 * coarse.cell)),
            };
            if near(settled.0, settled.1) { settled } else { (n, p, rough(n, p, &coarse, 3.0 * coarse.cell)) }
        })
        .filter(|s| near(s.0, s.1))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .unwrap_or((seed_normal, seed_point, 0.0));
    let finished = match &edges {
        Some(e) => {
            let (n, p, _) = e.settle(normal, point, Stage::Fine, &scene);
            (n, p)
        }
        None => refine(normal, point, &finer, &scene.table, 2.0 * scene.spacing, scene.size, 8),
    };
    let (normal, point) = if near(finished.0, finished.1) { finished } else { (normal, point) };
    let polished = polish(&assess(normal, point, &scene), &scene, 12);
    let kept = if near(polished.normal, polished.point) { polished } else { assess(normal, point, &scene) };
    Some(judge(kept, &scene))
}

/// `v` turned by `angle` radians about the unit `axis`.
fn turned(v: Vec3, angle: f64, axis: Vec3) -> Vec3 {
    Rigid::turn(axis, angle).turned(v)
}

/// How far, on the whole, the probes' reflections inside the scan's box are from the scan (`reach`
/// for one that lands on nothing). Lower is better.
fn roughness(normal: Vec3, point: Vec3, probes: &[Vec3], table: &Table, reach: f64, bounds: &Bounds) -> f64 {
    let reflections = probes.iter().map(|p| {
        let m = reflect(*p, normal, point);
        (m, table.nearest(m, reach).map_or(reach, |q| q.dist(m)))
    });
    mean_inside(reflections, bounds, reach)
}

/// A point, what its reflection landed nearest, and how far the reflection was from that.
struct Match {
    difference: Vec3,
    middle: Vec3,
    distance: f64,
}

/// Turns and moves a plane, from where it is, until the scan lands on its own reflection as well
/// as it can: each step the plane becomes the bisector of the points and what their reflections
/// landed on, over the better four fifths. Points within `exclusion` of the plane say nothing.
fn refine(start: Vec3, start_point: Vec3, samples: &[Vec3], table: &Table, exclusion: f64, size: f64, iterations: usize) -> (Vec3, Vec3) {
    let (mut normal, mut point) = (start, start_point);
    let reach = 3.0 * table.cell;
    for _ in 0..iterations {
        let mut matches: Vec<Match> = samples
            .iter()
            .filter_map(|&p| {
                let side = (p - point).dot(normal);
                let mirrored = p - normal * (2.0 * side);
                let q = table.nearest(mirrored, reach)?;
                let difference = p - q;
                (side.abs() > exclusion && difference.len() > 1e-9).then(|| Match { difference, middle: (p + q) / 2.0, distance: q.dist(mirrored) })
            })
            .collect();
        if matches.len() < 20 {
            break;
        }
        let keep = matches.len() * 4 / 5;
        if keep == 0 {
            break;
        }
        matches.select_nth_unstable_by(keep - 1, |a, b| a.distance.total_cmp(&b.distance));
        let (mut sum, mut offset) = (Vec3::ZERO, 0.0);
        for m in &matches[..keep] {
            sum += if m.difference.dot(normal) >= 0.0 { m.difference } else { -m.difference };
            offset += (m.middle - point).dot(normal);
        }
        let Some(next) = sum.normalized() else { break };
        let moved = point + normal * (offset / keep as f64);
        let change = (next - normal).len() + moved.dist(point) / size.max(1e-9);
        normal = next;
        point = moved;
        if change < 1e-7 {
            break;
        }
    }
    (normal, point)
}

/// How far, on the whole, the probes' reflections land from the points of `table`, each counted at
/// most `reach` (one that lands on nothing counts `reach`). Capped, a part on one side only (a
/// handle, a fixture) counts the same wherever the plane is, so it does not pull the plane, while
/// the ends and corners that land on nothing at a wrong place still count against it.
fn edge_mismatch(normal: Vec3, point: Vec3, probes: &[Vec3], table: &Table, reach: f64, bounds: &Bounds) -> f64 {
    let reflections = probes.iter().map(|p| {
        let m = reflect(*p, normal, point);
        (m, table.nearest(m, reach).map_or(reach, |q| q.dist(m)))
    });
    mean_inside(reflections, bounds, reach)
}

/// How far a settle searches and how finely.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Coarse,
    Fine,
}

/// The scan's edge points looked up at two scales, to settle planes on.
struct Edges {
    coarse: Table,
    fine: Table,
    few: Vec<Vec3>,
    many: Vec<Vec3>,
}

impl Edges {
    /// `None` when the scan has too few edges to place a plane by.
    fn new(scene: &Scene) -> Option<Edges> {
        (scene.features.len() >= 30).then(|| Edges {
            coarse: Table::new(thinned(&scene.features, 4_000), (scene.size / 60.0).max(2.0 * scene.spacing)),
            fine: Table::new(scene.features.clone(), 2.0 * scene.spacing),
            few: thinned(&scene.features, 600),
            many: thinned(&scene.features, 2_500),
        })
    }

    /// The plane settled where the scan's edges land on its edges: a pattern search on its two
    /// turns and its place, and how far the edges then land on the whole. On a part of broad flat
    /// faces the faces land on themselves however the plane is turned or slid along them, so the
    /// edges, rims and corners are what place it; each counts at most a reach, so a part on one
    /// side only does not pull it.
    fn settle(&self, normal: Vec3, point: Vec3, stage: Stage, scene: &Scene) -> (Vec3, Vec3, f64) {
        let (table, probes, angle, offset, done) = match stage {
            Stage::Coarse => (&self.coarse, &self.few, 4.0f64.to_radians(), scene.size / 30.0, (0.05f64.to_radians(), scene.spacing / 4.0)),
            Stage::Fine => (&self.fine, &self.many, 0.5f64.to_radians(), scene.size / 200.0, (0.002f64.to_radians(), scene.spacing / 200.0)),
        };
        let reach = 3.0 * table.cell;
        let cost = |n: Vec3, p: Vec3| edge_mismatch(n, p, probes, table, reach, &scene.bounds);
        let (mut n, mut p) = (normal, point);
        let (mut step_a, mut step_d) = (angle, offset);
        let mut best = cost(n, p);
        for _ in 0..400 {
            let (e1, e2) = crate::in_plane(n);
            let tries = [
                ((n + e1 * step_a.tan()).normalized(), p),
                ((n - e1 * step_a.tan()).normalized(), p),
                ((n + e2 * step_a.tan()).normalized(), p),
                ((n - e2 * step_a.tan()).normalized(), p),
                (Some(n), p + n * step_d),
                (Some(n), p - n * step_d),
            ];
            let found = tries
                .into_iter()
                .filter_map(|(tn, tp)| tn.map(|tn| (tn, tp, cost(tn, tp))))
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .filter(|t| t.2 < best - 1e-12);
            match found {
                Some((tn, tp, c)) => {
                    n = tn;
                    p = tp;
                    best = c;
                }
                None => {
                    step_a /= 2.0;
                    step_d /= 2.0;
                    if step_a < done.0 && step_d < done.1 {
                        break;
                    }
                }
            }
        }
        (n, p, best)
    }
}

/// How far each point's reflection lands from the scan (at most `reach`), for the reflections that
/// land inside the scan's box; when too few do to judge by, every one, counted `reach` beyond it
/// (the plane has thrown the scan aside).
fn inside_distances(points: &[Vec3], mirror: impl Fn(Vec3) -> Vec3, table: &Table, reach: f64, bounds: &Bounds) -> Vec<f64> {
    let all: Vec<(Vec3, f64)> = points
        .iter()
        .map(|&p| {
            let m = mirror(p);
            (m, table.nearest(m, reach).map_or(reach, |q| q.dist(m)))
        })
        .collect();
    let inside: Vec<f64> = all.iter().filter(|(m, _)| bounds.holds(*m)).map(|(_, d)| *d).collect();
    if !inside.is_empty() && inside.len() as f64 >= JUDGED_LEAST * all.len() as f64 {
        inside
    } else {
        all.iter().map(|(m, d)| if bounds.holds(*m) { *d } else { reach }).collect()
    }
}

/// How well the scan matches itself in the plane through `point` square to `normal`.
fn assess(normal: Vec3, point: Vec3, scene: &Scene) -> Mirror {
    let reach = 3.0 * scene.table.cell;
    let mut distances = inside_distances(&scene.samples, |p| reflect(p, normal, point), &scene.table, reach, &scene.bounds);
    distances.sort_by(f64::total_cmp);
    let matched = distances.iter().filter(|&&d| d <= 3.0 * scene.spacing).count();
    let core = &distances[..(distances.len() * 4 / 5).clamp(1, distances.len().max(1))];
    let rms = (core.iter().map(|d| d * d).sum::<f64>() / core.len().max(1) as f64).sqrt();
    Mirror {
        point: scene.middle - normal * (scene.middle - point).dot(normal),
        normal,
        rms,
        coverage: matched as f64 / distances.len().max(1) as f64,
        size: scene.size,
        spacing: scene.spacing,
        both_sides: 0.0,
    }
}

/// The plane found, judged against the scan's surface rather than its corners: how far each of
/// the surface's points lands from it when reflected, and how much lands within three of its
/// judged spacings. On a fine scan this is close to [`assess`]; on a coarse or reduced mesh its
/// big triangles count by their area, and a reflection landing on one is on the scan.
fn judge(found: Mirror, scene: &Scene) -> Mirror {
    let reach = 3.0 * scene.surface_table.cell;
    // Judged on every point, those reflected out of the scan's box too: the verdict says how much
    // of the scan is a mirror image, whatever the search left aside.
    let each: Vec<f64> = scene
        .surface_samples
        .iter()
        .map(|p| {
            let m = found.reflect(*p);
            scene.surface_table.nearest(m, reach).map_or(reach, |q| q.dist(m))
        })
        .collect();
    // How far out from the plane the scan reaches on each side, and how far what matches does:
    // all but the outermost fiftieth of each, so that a stray point is no side.
    let out = |p: &Vec3| (*p - found.point).dot(found.normal);
    let reach_of = |side: f64, only_matched: bool| {
        let mut far: Vec<f64> = scene
            .surface_samples
            .iter()
            .zip(&each)
            .filter(|(p, d)| side * out(p) > 0.0 && (!only_matched || **d <= 3.0 * scene.judged))
            .map(|(p, _)| side * out(p))
            .collect();
        far.sort_by(f64::total_cmp);
        far.get(far.len().saturating_sub(1) * 49 / 50).copied().unwrap_or(0.0)
    };
    let both_sides = [1.0, -1.0]
        .into_iter()
        .map(|side| {
            let all = reach_of(side, false);
            if all > 0.0 { reach_of(side, true) / all } else { 0.0 }
        })
        .fold(f64::INFINITY, f64::min);
    let mut distances = each;
    distances.sort_by(f64::total_cmp);
    let matched = distances.iter().filter(|&&d| d <= 3.0 * scene.judged).count();
    let core = &distances[..(distances.len() * 4 / 5).clamp(1, distances.len().max(1))];
    let rms = (core.iter().map(|d| d * d).sum::<f64>() / core.len().max(1) as f64).sqrt();
    Mirror { rms, coverage: matched as f64 / distances.len().max(1) as f64, both_sides: both_sides.clamp(0.0, 1.0), ..found }
}

/// Settles a plane against the surface: each sample's reflection measured along the surface's
/// direction at the nearest point, by Tukey-weighted Gauss–Newton steps on the plane's two turns
/// and its place. The plane as it was if that does not match its reflection better.
fn polish(plane: &Mirror, scene: &Scene, iterations: usize) -> Mirror {
    let samples = thinned(&scene.vertices, 6_000);
    if samples.len() < 50 {
        return *plane;
    }
    let big = scene.vertices.len() > TABLE_LIMIT;
    let spacing = scene.spacing * if big { (scene.vertices.len() as f64 / TABLE_LIMIT as f64).sqrt() } else { 1.0 };
    let indices: Vec<usize> = thinned(&(0..scene.vertices.len()).collect::<Vec<_>>(), TABLE_LIMIT);
    let grid = Table::new(indices.iter().map(|&i| scene.vertices[i]).collect(), 2.5 * spacing);
    let grid_normals: Vec<Vec3> = indices.iter().map(|&i| scene.normals[i]).collect();
    let mut n = plane.normal;
    let mut d = plane.point.dot(n);
    for _ in 0..iterations {
        let (e1, e2) = crate::in_plane(n);
        let rows: Vec<(f64, [f64; 3])> = samples
            .iter()
            .filter_map(|&x| {
                let s = x.dot(n) - d;
                if !(s.abs() > 2.0 * spacing) {
                    return None;
                }
                let reflected = x - n * (2.0 * s);
                let i = grid.nearest_within(reflected, 1, f64::INFINITY)?;
                let (q, nu) = (*grid.points.get(i)?, *grid_normals.get(i)?);
                if !(q.dist(reflected) <= 3.0 * spacing) {
                    return None;
                }
                let r = nu.dot(reflected - q);
                let j1 = nu.dot(n * (-2.0 * e1.dot(x)) - e1 * (2.0 * s));
                let j2 = nu.dot(n * (-2.0 * e2.dot(x)) - e2 * (2.0 * s));
                let jd = 2.0 * nu.dot(n);
                r.is_finite().then_some((r, [j1, j2, jd]))
            })
            .collect();
        if rows.len() < 30 {
            break;
        }
        let mut magnitudes: Vec<f64> = rows.iter().map(|r| r.0.abs()).collect();
        let mid = magnitudes.len() / 2;
        magnitudes.select_nth_unstable_by(mid, f64::total_cmp);
        // 1.4826 × the median absolute value: the noise, as a standard deviation.
        let scale = (1.4826 * magnitudes[mid]).max(1e-9 * scene.size.max(1.0));
        let (mut a, mut b) = ([[0.0f64; 3]; 3], [0.0f64; 3]);
        for (r, j) in &rows {
            let u = r / (4.685 * scale);
            if !(u.abs() < 1.0) {
                continue;
            }
            let w = (1.0 - u * u) * (1.0 - u * u);
            for p in 0..3 {
                for q in 0..3 {
                    a[p][q] += w * j[p] * j[q];
                }
                b[p] -= w * j[p] * r;
            }
        }
        let damp = 1e-6 * (a[0][0] + a[1][1] + a[2][2]) + 1e-12;
        for (i, row) in a.iter_mut().enumerate() {
            row[i] += damp;
        }
        let Some(step) = solve3(&a, b) else { break };
        if !(step[0].abs() < 0.1 && step[1].abs() < 0.1) {
            break;
        }
        let Some(next) = (n + e1 * step[0] + e2 * step[1]).normalized() else { break };
        n = next;
        d += step[2];
        if step[0].abs() + step[1].abs() < 1e-10 && step[2].abs() < 1e-9 * scene.size.max(1.0) {
            break;
        }
    }
    let polished = assess(n, plane.point - n * (plane.point.dot(n) - d), scene);
    if polished.score() <= plane.score() { polished } else { *plane }
}

/// How well the scan matches itself in the plane through `point` square to `normal`, without
/// moving the plane: a plane given rather than found. `None` for a scan too small to measure or a
/// plane with no direction.
pub fn measure(scan: &Scan, point: Vec3, normal: Vec3) -> Option<Mirror> {
    let normal = normal.normalized()?;
    point.is_finite().then_some(())?;
    let scene = scene(scan)?;
    Some(judge(assess(normal, point, &scene), &scene))
}

/// How far each point's reflection lands from the scan, for colouring the scan by where its two
/// sides differ.
#[derive(Clone, Debug, PartialEq)]
pub struct Mismatch {
    /// One per position: the distance (mm) from its reflection to the nearest point of the scan,
    /// at most `reach`; NaN for a position no triangle uses.
    pub values: Vec<f32>,
    /// Where the colours should end: a few noise widths.
    pub limit: f64,
    /// The farthest a reflection is looked for; one that lands on nothing reads this.
    pub reach: f64,
}

/// The most points the mismatch looks reflections up among; a larger scan is thinned evenly.
const MISMATCH_LIMIT: usize = 300_000;

/// How far each point of the scan lands from the scan when reflected in `mirror`'s plane.
pub fn mismatch(scan: &Scan, mirror: &Mirror) -> Mismatch {
    let mut used = vec![false; scan.positions.len()];
    for t in 0..scan.triangle_count() {
        if scan.corners(t).is_some()
            && let Some(tri) = scan.triangles.get(t)
        {
            for &v in tri {
                if let Some(u) = used.get_mut(v as usize) {
                    *u = true;
                }
            }
        }
    }
    let members: Vec<Vec3> = scan.positions.iter().zip(&used).filter(|(_, u)| **u).map(|(p, _)| *p).collect();
    let points = thinned(&members, MISMATCH_LIMIT);
    let factor = if points.is_empty() { 1.0 } else { (members.len() as f64 / points.len() as f64).sqrt() };
    let table = Table::new(points, 2.5 * mirror.spacing.max(1e-9) * factor);
    let reach = 3.0 * table.cell;
    let values = scan
        .positions
        .iter()
        .zip(&used)
        .map(|(p, u)| {
            if !u {
                return f32::NAN;
            }
            let m = mirror.reflect(*p);
            table.nearest(m, reach).map_or(reach, |q| q.dist(m)) as f32
        })
        .collect();
    Mismatch { values, limit: (3.0 * mirror.rms).max(3.0 * mirror.spacing).max(1e-6), reach }
}

/// The 3 × 3 system `a x = b` by Cramer's rule, `None` if it is (nearly) singular.
pub(crate) fn solve3(a: &[[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let whole = det(a);
    if !(whole.abs() > 1e-30) {
        return None;
    }
    let mut x = [0.0; 3];
    for (k, out) in x.iter_mut().enumerate() {
        let mut m = *a;
        for (row, bi) in m.iter_mut().zip(b) {
            row[k] = bi;
        }
        *out = det(&m) / whole;
    }
    Some(x)
}

/// Squares the scan to its mirror plane: the smallest turn that makes the plane's normal lie along
/// X (whichever way it is nearer), then sideways along X until the plane is the YZ plane (X = 0).
/// Height and the place along Y are kept.
pub fn square_to(mirror: &Mirror) -> Option<Rigid> {
    square_onto(mirror, Vec3::X)
}

/// [`square_to`] onto the origin plane square to `axis` (X: the YZ plane; Y: the XZ plane, as a
/// vehicle is drawn): the smallest turn that makes the mirror plane's normal lie along `axis`
/// (whichever way is nearer), then along `axis` until the plane passes through the origin.
pub fn square_onto(mirror: &Mirror, axis: Vec3) -> Option<Rigid> {
    let n = mirror.normal.normalized()?;
    let axis = axis.normalized()?;
    let target = if n.dot(axis) >= 0.0 { axis } else { -axis };
    let mut by = crate::place::turn_onto(n, target);
    let on = by.apply(mirror.point);
    by.translation -= axis * on.dot(axis);
    Some(by)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, turned as turned_points};

    /// An L-shaped block: one mirror plane (across its long side), none other.
    fn l_block() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        // Two boxes sharing a face: 60 × 20 × 10 and, on top of its middle, 20 × 20 × 15.
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 20.0, 10.0), 12);
        let (top, top_triangles) = fine_cuboid(Vec3::new(20.0, 0.0, 10.0), Vec3::new(40.0, 20.0, 25.0), 8);
        let base = positions.len() as u32;
        positions.extend(top);
        triangles.extend(top_triangles.iter().map(|t| t.map(|i| i + base)));
        (positions, triangles)
    }

    fn plane_error(m: &Mirror, normal: Vec3, point: Vec3) -> (f64, f64) {
        let angle = m.normal.dot(normal).abs().clamp(-1.0, 1.0).acos();
        let offset = (m.point - point).dot(normal).abs();
        (angle, offset)
    }

    /// The block reduced to 200 big triangles (from 2,496), their corners left on its edges where the
    /// collapses stopped (as Reduce Mesh leaves a scan): its corners are no mirror image of one
    /// another, but its surface is, and it is judged by its surface, typically within a fraction
    /// of a millimetre (T-LESS 05 reduced to 2 % read 3.55 mm off by its corners, 0.99 by its
    /// surface).
    #[test]
    fn a_reduced_part_is_judged_by_its_surface() {
        let (positions, triangles) = l_block();
        let full = Scan::new(&positions, &triangles);
        let settings = crate::reduce::Settings { target: 200, max_error: 0.0, keep_rim: true };
        let reduced = crate::reduce::reduce(&full, settings).unwrap();
        assert!(reduced.triangles.len() <= 200, "{}", reduced.triangles.len());
        let scan = Scan::new(&reduced.positions, &reduced.triangles);
        let mirror = find(&scan).unwrap();
        assert!(mirror.is_symmetric(), "{mirror:?}");
        assert!(mirror.rms < 0.005 * mirror.size && mirror.coverage > 0.95, "{mirror:?}");
    }

    /// A body 100 × 40 × 20 with a long box slung under one side of it (as a car's exhaust is): a
    /// quarter of it is no mirror image about the body's middle, but both its sides are there and
    /// match out to their ends, so it is a mirror image. The same body scanned from one side and a
    /// little past the middle matches about as much of itself, all of it in the strip about the
    /// plane: it is not (Complete by Mirroring fills its other side).
    #[test]
    fn a_thing_with_both_sides_there_is_a_mirror_image_though_part_of_it_is_not() {
        let (mut positions, mut triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(100.0, 40.0, 20.0), 40);
        let (under, more) = fine_cuboid(Vec3::new(5.0, 2.0, -14.0), Vec3::new(70.0, 16.0, 0.0), 26);
        let base = positions.len() as u32;
        positions.extend(under);
        triangles.extend(more.iter().map(|t| t.map(|i| i + base)));
        let (point, normal) = (Vec3::new(50.0, 20.0, 10.0), Vec3::Y);
        let whole = measure(&Scan::new(&positions, &triangles), point, normal).unwrap();
        assert!(whole.coverage > 0.6 && whole.coverage < 0.85, "part of it is no mirror image: {whole:?}");
        assert!(whole.both_sides > 0.9 && whole.is_symmetric(), "{whole:?}");
        // What does not match is said, as a share: the box under one side.
        assert!(!whole.is_whole());
        assert!(whole.unmatched().is_some_and(|share| (share - (1.0 - whole.coverage)).abs() < 1e-12 && share > 0.15), "{whole:?}");
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(100.0, 40.0, 20.0), 40);
        assert_eq!(measure(&Scan::new(&positions, &triangles), point, normal).unwrap().unmatched(), None);
        // One side and a little past the middle (to Y = 30 of 40), with no box under it.
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(100.0, 40.0, 20.0), 40);
        let one_side: Vec<[u32; 3]> =
            triangles.iter().copied().filter(|t| t.iter().map(|&i| positions[i as usize].y).sum::<f64>() / 3.0 < 30.0).collect();
        let part = measure(&Scan::new(&positions, &one_side), point, normal).unwrap();
        assert!(part.coverage > 0.5 && part.both_sides < 0.7 && !part.is_symmetric() && part.unmatched().is_none(), "{part:?}");
    }

    /// The block is symmetric about X = 30 and about Y = 10: found wherever it lies, to within a
    /// thousandth of a degree and a micron.
    #[test]
    fn the_mirror_plane_of_a_block_is_found_wherever_it_lies() {
        let (positions, triangles) = l_block();
        let axis = Vec3::new(0.2, 0.9, -0.4).normalized().unwrap();
        let (angle, shift) = (0.8, Vec3::new(120.0, -35.0, 60.0));
        let moved = turned_points(&positions, axis, angle, shift);
        let scan = Scan::new(&moved, &triangles);
        let mirror = find(&scan).unwrap();
        assert!(mirror.is_symmetric(), "{mirror:?}");
        let truth = |n: Vec3, p: Vec3| (Rigid::turn(axis, angle).turned(n), Rigid::turn(axis, angle).turned(p) + shift);
        let (nx, px) = truth(Vec3::X, Vec3::new(30.0, 10.0, 10.0));
        let (ny, py) = truth(Vec3::Y, Vec3::new(30.0, 10.0, 10.0));
        let ex = plane_error(&mirror, nx, px);
        let ey = plane_error(&mirror, ny, py);
        let (a, o) = if ex.0 < ey.0 { ex } else { ey };
        assert!(a < 1e-3_f64.to_radians() && o < 1e-3, "off by {}° and {o} mm", a.to_degrees());
    }

    /// A guide drawn roughly along X = 30 (20° off, 6 mm aside) finds that plane, not the other.
    #[test]
    fn a_guide_finds_the_plane_it_marks() {
        let (positions, triangles) = l_block();
        let scan = Scan::new(&positions, &triangles);
        let tilt = 20.0f64.to_radians();
        let guide = Guide {
            from: Vec3::new(36.0 - 15.0 * tilt.sin(), 10.0 - 15.0 * tilt.cos(), 30.0),
            to: Vec3::new(36.0 + 15.0 * tilt.sin(), 10.0 + 15.0 * tilt.cos(), 30.0),
            toward: Vec3::Z,
        };
        let mirror = find_guided(&scan, &guide).unwrap();
        let (a, o) = plane_error(&mirror, Vec3::X, Vec3::new(30.0, 10.0, 10.0));
        assert!(a < 1e-3_f64.to_radians() && o < 1e-3, "off by {}° and {o} mm: {mirror:?}", a.to_degrees());
        assert!(mirror.is_symmetric());
    }

    /// The block with its end past X = 45 never scanned (as one side of a part is when it was
    /// scanned from the other): a guide along X = 30 finds X = 30, though about that plane the
    /// missing end's mirror image lands on nothing and a plane tilted to overlap more would match
    /// more of what is there.
    #[test]
    fn a_guide_finds_its_plane_on_a_part_missing_one_end() {
        let (positions, triangles) = l_block();
        let kept: Vec<[u32; 3]> = triangles.into_iter().filter(|t| t.iter().all(|&i| positions[i as usize].x <= 45.0)).collect();
        let scan = Scan::new(&positions, &kept);
        let guide = Guide { from: Vec3::new(30.0, -5.0, 30.0), to: Vec3::new(30.0, 25.0, 30.0), toward: Vec3::Z };
        let mirror = find_guided(&scan, &guide).unwrap();
        let (a, o) = plane_error(&mirror, Vec3::X, Vec3::new(30.0, 10.0, 10.0));
        assert!(a < 0.1_f64.to_radians() && o < 0.1, "off by {}° and {o} mm: {mirror:?}", a.to_degrees());
    }

    /// Squared to its mirror plane, the block lies evenly either side of X = 0 (whichever of its
    /// two planes was found).
    #[test]
    fn squaring_puts_the_plane_on_x_0() {
        let (positions, triangles) = l_block();
        let axis = Vec3::new(0.0, 0.3, 1.0).normalized().unwrap();
        let moved = turned_points(&positions, axis, 0.5, Vec3::new(10.0, 20.0, 0.0));
        let mirror = find(&Scan::new(&moved, &triangles)).unwrap();
        let by = square_to(&mirror).unwrap();
        let placed: Vec<Vec3> = moved.iter().map(|p| by.apply(*p)).collect();
        let (lo, hi) = placed.iter().fold((placed[0], placed[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        assert!((lo.x + hi.x).abs() < 1e-3 && hi.x > 5.0, "{lo:?} {hi:?}");
        assert!(by.turned(mirror.normal).x.abs() > 1.0 - 1e-12, "the plane's normal along X");
    }

    /// About its mirror plane the block lands on itself everywhere; about a plane off to one side,
    /// the far end of the block lands on nothing and reads the reach.
    #[test]
    fn the_mismatch_shows_where_the_two_sides_differ() {
        let (positions, triangles) = l_block();
        let scan = Scan::new(&positions, &triangles);
        let mirror = find_guided(&scan, &Guide { from: Vec3::new(30.0, -5.0, 30.0), to: Vec3::new(30.0, 25.0, 30.0), toward: Vec3::Z }).unwrap();
        let on = mismatch(&scan, &mirror);
        assert_eq!(on.values.len(), positions.len());
        assert!(on.values.iter().all(|v| f64::from(*v) < 1e-3), "every point lands on the scan");
        let off = mismatch(&scan, &Mirror { point: Vec3::new(45.0, 10.0, 10.0), normal: Vec3::X, ..mirror });
        let far = positions.iter().zip(&off.values).filter(|(p, v)| p.x < 5.0 && f64::from(**v) >= off.limit).count();
        let near_end = positions.iter().filter(|p| p.x < 5.0).count();
        assert_eq!(far, near_end, "the end beyond the reflection's reach differs");
    }

    /// A long thin plate with a tab on each end (a mirror image about X = 100) and a handle on one
    /// end only. Mirrored, the plate lands on itself wherever the plane is turned or slid along it,
    /// so only its ends, edges and tabs say where the plane is, and the handle matches nothing: a
    /// guide drawn 8° and 15 mm off still finds X = 100.
    #[test]
    fn a_handle_on_one_end_does_not_pull_a_guided_plane() {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for (lo, hi, n) in [
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(200.0, 80.0, 4.0), 24),
            (Vec3::new(5.0, 30.0, 4.0), Vec3::new(20.0, 50.0, 14.0), 6),
            (Vec3::new(180.0, 30.0, 4.0), Vec3::new(195.0, 50.0, 14.0), 6),
            (Vec3::new(200.0, 25.0, 0.0), Vec3::new(260.0, 55.0, 4.0), 12),
        ] {
            let (p, t) = fine_cuboid(lo, hi, n);
            let base = positions.len() as u32;
            positions.extend(p);
            triangles.extend(t.iter().map(|tri| tri.map(|i| i + base)));
        }
        let scan = Scan::new(&positions, &triangles);
        let tilt = 8.0f64.to_radians();
        let c = Vec3::new(115.0, 40.0, 30.0);
        let along = Vec3::new(tilt.sin(), tilt.cos(), 0.0);
        let guide = Guide { from: c - along * 50.0, to: c + along * 50.0, toward: Vec3::Z };
        let mirror = find_guided(&scan, &guide).unwrap();
        let (a, o) = plane_error(&mirror, Vec3::X, Vec3::new(100.0, 40.0, 2.0));
        assert!(a < 0.05_f64.to_radians() && o < 0.05, "off by {}° and {o} mm: {mirror:?}", a.to_degrees());
    }

    #[test]
    fn a_scan_too_small_has_no_plane() {
        assert!(find(&Scan::new(&[], &[])).is_none());
        let guide = Guide { from: Vec3::ZERO, to: Vec3::ZERO, toward: Vec3::Z };
        let (positions, triangles) = l_block();
        assert!(find_guided(&Scan::new(&positions, &triangles), &guide).is_none(), "a guide with no length says nothing");
    }
}
