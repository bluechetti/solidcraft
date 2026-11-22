//! Flattening part of a scan onto a plane, after SurfaceCraft's Flatten: the least squares
//! conformal map (Lévy et al., 2002), which keeps angles as well as it can, scaled to the
//! region's own area, then, if asked, relaxed toward keeping every edge its length on the scan
//! (stress majorization), never folding the pattern. How far the pattern is from the scan is
//! measured: its area, each edge's stretch, triangles turned over. A point of the pattern lifts
//! back onto the scan through the triangle it lies in, so a drawing on the pattern rolls back
//! onto the scan.

use std::collections::HashMap;

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

use crate::Scan;

/// Why a region could not be flattened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlattenError {
    /// The region has too few triangles.
    TooSmall,
    /// The region has no open edge: it closes round on itself (cut it with Split Mesh first).
    Closed,
    /// The region's triangles meet three or more along an edge.
    NotASheet,
}

impl std::fmt::Display for FlattenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FlattenError::TooSmall => "the region has too few triangles to flatten",
            FlattenError::Closed => "the region closes round on itself, so it has no edge to open out from: cut it with Split Mesh first",
            FlattenError::NotASheet => "the region is not one sheet: three or more of its triangles meet along an edge (Repair Mesh first)",
        })
    }
}

impl std::error::Error for FlattenError {}

/// How far a pattern is from the scan it was flattened from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// The region's area on the scan and in the pattern.
    pub area: f64,
    pub flat_area: f64,
    /// The worst share a triangle's area changed by, either way.
    pub worst_area: f64,
    /// Each edge's stretch, |flat − scan| / scan: root mean square and most.
    pub strain_rms: f64,
    pub strain_max: f64,
    /// Triangles turned over in the pattern.
    pub flipped: usize,
    /// Rounds of relaxation made, and whether the solve of the map settled.
    pub relaxed: usize,
    pub settled: bool,
}

/// A region of the scan flattened.
#[derive(Clone, Debug, PartialEq)]
pub struct Flat {
    /// The scan's vertices in the region (by the first of each set at one place), each one's place
    /// in the pattern, and the region's triangles on them.
    pub vertices: Vec<u32>,
    pub points: Vec<Vec2>,
    pub triangles: Vec<[u32; 3]>,
    /// The pattern's outline: each loop of the region's open edges, in the pattern.
    pub outline: Vec<Vec<Vec2>>,
    pub stats: Stats,
}

/// The rows of the conformal map's least squares, two to a triangle, each over its corners' u
/// and v (columns `2 i` and `2 i + 1`).
struct Rows {
    rows: Vec<[(u32, f64); 6]>,
    columns: usize,
}

impl Rows {
    /// `Mᵀ M x`, `M x` worked out first.
    fn normal(&self, x: &[f64], out: &mut [f64]) {
        out.iter_mut().for_each(|o| *o = 0.0);
        for row in &self.rows {
            let r: f64 = row.iter().map(|&(c, w)| w * x[c as usize]).sum();
            for &(c, w) in row {
                out[c as usize] += w * r;
            }
        }
    }

    /// The diagonal of `Mᵀ M`.
    fn diagonal(&self) -> Vec<f64> {
        let mut d = vec![0.0; self.columns];
        for row in &self.rows {
            for &(c, w) in row {
                d[c as usize] += w * w;
            }
        }
        d
    }
}

/// `A x = b` for symmetric positive (semi)definite `A` (given as its product), by conjugate
/// gradients with the diagonal as preconditioner, from the `x` given; columns `fixed` stay. Whether
/// it settled to `tolerance` (relative) within `iterations`.
fn conjugate_gradients(
    apply: impl Fn(&[f64], &mut [f64]),
    diagonal: &[f64],
    b: &[f64],
    x: &mut [f64],
    fixed: &[bool],
    tolerance: f64,
    iterations: usize,
) -> bool {
    let n = b.len();
    let mut ax = vec![0.0; n];
    apply(x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|i| if fixed[i] { 0.0 } else { b[i] - ax[i] }).collect();
    let pre = |i: usize, v: f64| if diagonal[i] > 1e-300 { v / diagonal[i] } else { v };
    let mut z: Vec<f64> = (0..n).map(|i| pre(i, r[i])).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let norm_b = b.iter().enumerate().filter(|(i, _)| !fixed[*i]).map(|(_, v)| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..iterations {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() <= tolerance * norm_b {
            return true;
        }
        apply(&p, &mut ap);
        for (i, a) in ap.iter_mut().enumerate() {
            if fixed[i] {
                *a = 0.0;
            }
        }
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if !(pap > 0.0) {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        for i in 0..n {
            z[i] = pre(i, r[i]);
        }
        let next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = next / rz.max(1e-300);
        rz = next;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
    }
    r.iter().map(|v| v * v).sum::<f64>().sqrt() <= tolerance * norm_b
}

/// Above this many triangles a region is flattened through a reduced copy of itself (its rim
/// kept), each of its own points then carried onto the pattern through the copy.
const DIRECT: usize = 24_000;

/// Rounds of the map's solve on the whole region after it is carried through the reduced copy.
const SETTLE_ROUNDS: usize = 400;

/// The triangles `region` of the scan flattened; with `relax`, relaxed toward keeping the edges'
/// lengths.
pub fn flatten(scan: &Scan, region: &[usize], relax: bool) -> Result<Flat, FlattenError> {
    let weld = crate::welded(scan.positions);
    // The region's triangles on welded points, and its vertices numbered from 0.
    let mut local: HashMap<u32, u32> = HashMap::new();
    let mut vertices: Vec<u32> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(region.len());
    // In the scan's own order, not the order the region was found in (grown from the triangle
    // nearest a click): the pattern is then laid the same way whatever click found the region, so
    // one flattened again after the scan was moved lines up with what was drawn on it.
    let mut region = region.to_vec();
    region.sort_unstable();
    region.dedup();
    for &t in &region {
        if scan.corners(t).is_none() {
            continue;
        }
        let Some(tri) = scan.triangles.get(t) else { continue };
        let w = tri.map(|v| weld[v as usize]);
        if w[0] == w[1] || w[1] == w[2] || w[0] == w[2] {
            continue;
        }
        triangles.push(w.map(|v| {
            *local.entry(v).or_insert_with(|| {
                vertices.push(v);
                (vertices.len() - 1) as u32
            })
        }));
    }
    if triangles.len() < 2 {
        return Err(FlattenError::TooSmall);
    }
    let at: Vec<Vec3> = vertices.iter().map(|&v| scan.positions[v as usize]).collect();
    let sheet = Sheet::of(&triangles)?;
    let (points, relaxed, settled) =
        if triangles.len() > DIRECT { through_reduced(&at, &triangles, relax)? } else { unrolled(&at, &triangles, &sheet, relax) };
    let stats = measured(&at, &triangles, &sheet, &points, relaxed, settled);
    let outline = loops(&sheet.directed).into_iter().map(|l| l.iter().map(|&v| points[v as usize]).collect()).collect();
    Ok(Flat { vertices, points, triangles, outline, stats })
}

/// A region's edges: how many triangles each has (one: the outline, its edges run the way their
/// triangle runs them).
struct Sheet {
    uses: HashMap<(u32, u32), u32>,
    directed: HashMap<u32, Vec<u32>>,
}

impl Sheet {
    fn of(triangles: &[[u32; 3]]) -> Result<Sheet, FlattenError> {
        let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
        for t in triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        if uses.values().any(|&u| u > 2) {
            return Err(FlattenError::NotASheet);
        }
        let mut directed: HashMap<u32, Vec<u32>> = HashMap::new();
        for t in triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if uses.get(&(a.min(b), a.max(b))) == Some(&1) {
                    directed.entry(a).or_default().push(b);
                }
            }
        }
        if directed.is_empty() {
            return Err(FlattenError::Closed);
        }
        Ok(Sheet { uses, directed })
    }
}

/// A triangle's area on the scan, and in the pattern (negative when turned over).
fn area_of(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    (b - a).cross(c - a).len() / 2.0
}

fn flat_area_of(pts: &[Vec2], t: &[u32; 3]) -> f64 {
    let [a, b, c] = t.map(|v| pts[v as usize]);
    ((b - a).x * (c - a).y - (b - a).y * (c - a).x) / 2.0
}

/// How far the pattern `points` is from the region.
fn measured(at: &[Vec3], triangles: &[[u32; 3]], sheet: &Sheet, points: &[Vec2], relaxed: usize, settled: bool) -> Stats {
    let area: f64 = triangles.iter().map(|t| area_of(at[t[0] as usize], at[t[1] as usize], at[t[2] as usize])).sum();
    let strains: Vec<f64> = sheet
        .uses
        .keys()
        .filter_map(|&(a, b)| {
            let l = at[a as usize].dist(at[b as usize]);
            (l > 0.0).then(|| (points[a as usize].dist(points[b as usize]) - l).abs() / l)
        })
        .collect();
    let worst_area = triangles
        .iter()
        .map(|t| {
            let a = area_of(at[t[0] as usize], at[t[1] as usize], at[t[2] as usize]);
            if a > 0.0 { (flat_area_of(points, t) / a - 1.0).abs() } else { 0.0 }
        })
        .fold(0.0, f64::max);
    Stats {
        area,
        flat_area: triangles.iter().map(|t| flat_area_of(points, t)).sum(),
        worst_area,
        strain_rms: (strains.iter().map(|s| s * s).sum::<f64>() / strains.len().max(1) as f64).sqrt(),
        strain_max: strains.iter().copied().fold(0.0, f64::max),
        flipped: triangles.iter().filter(|t| flat_area_of(points, t) <= 0.0).count(),
        relaxed,
        settled,
    }
}

/// A large region flattened through a reduced copy of itself, its rim kept: the copy unrolled,
/// then each of the region's points carried onto the pattern through the copy's triangle nearest
/// it.
fn through_reduced(at: &[Vec3], triangles: &[[u32; 3]], relax: bool) -> Result<(Vec<Vec2>, usize, bool), FlattenError> {
    let fine = Scan::new(at, triangles);
    let reduced =
        crate::reduce::reduce(&fine, crate::reduce::Settings { target: DIRECT, max_error: 0.0, keep_rim: true }).ok_or(FlattenError::TooSmall)?;
    let sheet = Sheet::of(&reduced.triangles)?;
    let (coarse, relaxed, settled) = unrolled(&reduced.positions, &reduced.triangles, &sheet, relax);
    let mesh = solidcraft_geom::Mesh { positions: reduced.positions.clone(), triangles: reduced.triangles.clone(), ..Default::default() };
    let tree = solidcraft_geom::compare::TriangleTree::new(&mesh);
    let points = crate::par::map(at.len(), |i| {
        let q = at[i];
        let Some((_, t)) = tree.nearest(q) else { return Vec2::ZERO };
        let tri = reduced.triangles[t as usize];
        let [a, b, c] = tri.map(|v| reduced.positions[v as usize]);
        let [l0, l1, l2] = barycentric(solidcraft_geom::compare::closest_on_triangle(q, a, b, c), a, b, c);
        coarse[tri[0] as usize] * l0 + coarse[tri[1] as usize] * l1 + coarse[tri[2] as usize] * l2
    });
    // What the copy cannot say, the folds of small triangles where a point was carried onto the
    // wrong one, settled by a few hundred rounds of the map on the region itself from there.
    let whole = Sheet::of(triangles)?;
    let (points, _, _) = conformal(at, triangles, &whole, false, Some(&points), SETTLE_ROUNDS);
    Ok((points, relaxed, settled))
}

/// The weights of `q` (on the triangle) at its corners.
fn barycentric(q: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f64; 3] {
    let (e0, e1, d) = (b - a, c - a, q - a);
    let (d00, d01, d11, d20, d21) = (e0.dot(e0), e0.dot(e1), e1.dot(e1), d.dot(e0), d.dot(e1));
    let denom = d00 * d11 - d01 * d01;
    if !(denom.abs() > 1e-300) {
        return [1.0, 0.0, 0.0];
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    [1.0 - v - w, v, w]
}

/// The region `at`, `triangles` unrolled: the conformal map, scaled to its area, then relaxed if
/// asked. The pattern, the rounds of relaxation, and whether the map's solve settled.
fn unrolled(at: &[Vec3], triangles: &[[u32; 3]], sheet: &Sheet, relax: bool) -> (Vec<Vec2>, usize, bool) {
    conformal(at, triangles, sheet, relax, None, usize::MAX)
}

/// The conformal map of the region, from `start` when given (the pins kept where it has them,
/// the rest settled for at most `rounds` rounds of the solve), else from the region seen square
/// to its best plane; scaled to its area, then relaxed if asked.
fn conformal(at: &[Vec3], triangles: &[[u32; 3]], sheet: &Sheet, relax: bool, start: Option<&[Vec2]>, rounds: usize) -> (Vec<Vec2>, usize, bool) {
    let rim: Vec<u32> = {
        let mut r: Vec<u32> = sheet.directed.keys().copied().collect();
        r.sort_unstable();
        r
    };
    // Pins: two rim points far apart, laid along X at their distance on the scan.
    // Of points as far as the farthest within a hair (a rectangle's two corners across from one,
    // equal but for rounding), the first: rounding is not to choose, or the same region moved or
    // turned is laid out turned over and what was drawn on its pattern no longer lines up.
    let far_from = |from: u32| {
        let reach = |a: u32| at[a as usize].dist(at[from as usize]);
        let most = rim.iter().map(|&a| reach(a)).fold(0.0, f64::max);
        *rim.iter().find(|&&a| reach(a) >= most * (1.0 - 1e-9)).unwrap_or(&from)
    };
    let pa = far_from(rim[0]);
    let pb = far_from(pa);
    let n = at.len();
    let mut rows = Rows { rows: Vec::with_capacity(2 * triangles.len()), columns: 2 * n };
    for t in triangles {
        let [p0, p1, p2] = t.map(|v| at[v as usize]);
        let (Some(e1), Some(normal)) = ((p1 - p0).normalized(), (p1 - p0).cross(p2 - p0).normalized()) else { continue };
        let e2 = normal.cross(e1);
        let x = [Vec2::ZERO, Vec2::new((p1 - p0).len(), 0.0), Vec2::new((p2 - p0).dot(e1), (p2 - p0).dot(e2))];
        let area = (x[1].x * x[2].y - x[1].y * x[2].x) / 2.0;
        // A sliver says nothing of the shape, and only stiffens the solve.
        let longest = (p1 - p0).len().max((p2 - p1).len()).max((p0 - p2).len());
        if !(area > 1e-8 * longest * longest) {
            continue;
        }
        // The edge across from each corner, counter-clockwise; the residual of ∇v = J ∇u.
        let d = [x[2] - x[1], x[0] - x[2], x[1] - x[0]];
        let s = 1.0 / (2.0 * area.sqrt());
        let u = |j: usize| 2 * t[j];
        let v = |j: usize| 2 * t[j] + 1;
        rows.rows.push(std::array::from_fn(|k| if k < 3 { (u(k), d[k].x * s) } else { (v(k - 3), -d[k - 3].y * s) }));
        rows.rows.push(std::array::from_fn(|k| if k < 3 { (u(k), d[k].y * s) } else { (v(k - 3), d[k - 3].x * s) }));
    }
    // A start: the region seen square to its best plane, turned and moved onto the pins.
    let centre = at.iter().fold(Vec3::ZERO, |s, p| s + *p) / n as f64;
    let mut spread = [[0.0f64; 3]; 3];
    for p in at {
        let d = [p.x - centre.x, p.y - centre.y, p.z - centre.z];
        for (row, di) in spread.iter_mut().zip(d) {
            for (cell, dj) in row.iter_mut().zip(d) {
                *cell += di * dj;
            }
        }
    }
    let (ax, ay) = match solidcraft_geom::inertia::principal(&spread, 0.0) {
        Some((_, axes)) => (axes[2].normalized().unwrap_or(Vec3::X), axes[1].normalized().unwrap_or(Vec3::Y)),
        None => (Vec3::X, Vec3::Y),
    };
    let seen: Vec<Vec2> = at.iter().map(|p| Vec2::new((*p - centre).dot(ax), (*p - centre).dot(ay))).collect();
    let length = at[pa as usize].dist(at[pb as usize]).max(1e-9);
    let (sa, sb) = (seen[pa as usize], seen[pb as usize]);
    let along = (sb - sa).normalized().unwrap_or(Vec2::X);
    let scale = length / (sb - sa).len().max(1e-12);
    let place = |q: Vec2| {
        let d = (q - sa) * scale;
        Vec2::new(d.dot(along), along.x * d.y - along.y * d.x)
    };
    let mut x = vec![0.0; 2 * n];
    for (i, q) in seen.iter().enumerate() {
        let p = start.and_then(|s| s.get(i).copied()).unwrap_or_else(|| place(*q));
        x[2 * i] = p.x;
        x[2 * i + 1] = p.y;
    }
    let mut fixed = vec![false; 2 * n];
    let pinned_at = |pin: u32, otherwise: Vec2| start.and_then(|s| s.get(pin as usize).copied()).unwrap_or(otherwise);
    for (pin, value) in [(pa, pinned_at(pa, Vec2::ZERO)), (pb, pinned_at(pb, Vec2::new(length, 0.0)))] {
        fixed[2 * pin as usize] = true;
        fixed[2 * pin as usize + 1] = true;
        x[2 * pin as usize] = value.x;
        x[2 * pin as usize + 1] = value.y;
    }
    // Mᵀ M x = Mᵀ b with the pins moved over: b is −M x_pinned on the free columns.
    let pinned: Vec<f64> = (0..2 * n).map(|c| if fixed[c] { x[c] } else { 0.0 }).collect();
    let mut rhs = vec![0.0; 2 * n];
    rows.normal(&pinned, &mut rhs);
    rhs.iter_mut().for_each(|v| *v = -*v);
    let diagonal = rows.diagonal();
    let free_part = |input: &[f64], out: &mut [f64]| {
        let masked: Vec<f64> = input.iter().enumerate().map(|(c, v)| if fixed[c] { 0.0 } else { *v }).collect();
        rows.normal(&masked, out);
    };
    let mut free: Vec<f64> = (0..2 * n).map(|c| if fixed[c] { 0.0 } else { x[c] }).collect();
    let settled = conjugate_gradients(free_part, &diagonal, &rhs, &mut free, &fixed, 1e-8, (20 * n.max(100)).min(rounds));
    let mut points: Vec<Vec2> = (0..n)
        .map(|i| Vec2::new(if fixed[2 * i] { x[2 * i] } else { free[2 * i] }, if fixed[2 * i + 1] { x[2 * i + 1] } else { free[2 * i + 1] }))
        .collect();

    // Scaled about its middle to the region's own area.
    let area: f64 = triangles.iter().map(|t| area_of(at[t[0] as usize], at[t[1] as usize], at[t[2] as usize])).sum();
    let flat: f64 = triangles.iter().map(|t| flat_area_of(&points, t)).sum();
    if flat.abs() > 1e-300 {
        let k = (area / flat.abs()).sqrt();
        let middle = points.iter().fold(Vec2::ZERO, |s, p| s + *p) / n as f64;
        for p in &mut points {
            *p = middle + (*p - middle) * k;
        }
        // Turned over as a whole (the scan wound the other way): mirrored back.
        if flat < 0.0 {
            for p in &mut points {
                p.y = 2.0 * middle.y - p.y;
            }
        }
    }
    let mut relaxed = 0;
    if relax {
        let edges: Vec<(u32, u32, f64)> =
            sheet.uses.keys().map(|&(a, b)| (a, b, at[a as usize].dist(at[b as usize]))).filter(|e| e.2 > 0.0).collect();
        relaxed = relax_edges(&mut points, &edges, triangles);
    }
    (points, relaxed, settled)
}

/// The loops of the open edges, each followed the way its triangle runs it.
fn loops(directed: &HashMap<u32, Vec<u32>>) -> Vec<Vec<u32>> {
    let mut left: HashMap<u32, Vec<u32>> = directed.clone();
    let mut starts: Vec<u32> = left.keys().copied().collect();
    starts.sort_unstable();
    let mut out = Vec::new();
    for start in starts {
        while left.get(&start).is_some_and(|v| !v.is_empty()) {
            let mut lp = vec![start];
            let mut here = start;
            while let Some(next) = left.get_mut(&here).and_then(Vec::pop) {
                if next == start {
                    break;
                }
                lp.push(next);
                here = next;
                if lp.len() > directed.len() + 1 {
                    break;
                }
            }
            if lp.len() >= 3 {
                out.push(lp);
            }
        }
    }
    out
}

/// Stress majorization toward each edge's length on the scan (weights 1 / length²): each round one
/// weighted Laplacian solve for each axis, the first point held; at most 12 rounds. Stops before
/// a round that would turn a triangle over or not lower the stress, and after one that gains less
/// than half a percent; the rounds made.
fn relax_edges(points: &mut [Vec2], edges: &[(u32, u32, f64)], triangles: &[[u32; 3]]) -> usize {
    let n = points.len();
    let stress = |p: &[Vec2]| edges.iter().map(|&(a, b, l)| ((p[a as usize].dist(p[b as usize]) - l) / l).powi(2)).sum::<f64>();
    let flipped = |p: &[Vec2]| triangles.iter().filter(|t| flat_area_of(p, t) <= 0.0).count();
    let mut weights = vec![0.0; n];
    for &(a, b, l) in edges {
        let w = 1.0 / (l * l);
        weights[a as usize] += w;
        weights[b as usize] += w;
    }
    let laplacian = |x: &[f64], out: &mut [f64]| {
        for (i, o) in out.iter_mut().enumerate() {
            *o = weights[i] * x[i];
        }
        for &(a, b, l) in edges {
            let w = 1.0 / (l * l);
            out[a as usize] -= w * x[b as usize];
            out[b as usize] -= w * x[a as usize];
        }
    };
    let mut fixed = vec![false; n];
    fixed[0] = true;
    let mut rounds = 0;
    let mut current = stress(points);
    let folds = flipped(points);
    for _ in 0..12 {
        // The right side: L_Z(x) x, the weights w l / |x_a − x_b|.
        let (mut bx, mut by) = (vec![0.0; n], vec![0.0; n]);
        for &(a, b, l) in edges {
            let (pa, pb) = (points[a as usize], points[b as usize]);
            let d = pa.dist(pb);
            if !(d > 1e-15) {
                continue;
            }
            let w = (1.0 / (l * l)) * l / d;
            let diff = pa - pb;
            bx[a as usize] += w * diff.x;
            by[a as usize] += w * diff.y;
            bx[b as usize] -= w * diff.x;
            by[b as usize] -= w * diff.y;
        }
        let mut x: Vec<f64> = points.iter().map(|p| p.x).collect();
        let mut y: Vec<f64> = points.iter().map(|p| p.y).collect();
        // The held point's column moved over to the right side.
        let shift = |b: &mut [f64], held: f64| {
            for &(a, c, l) in edges {
                let w = 1.0 / (l * l);
                if a == 0 {
                    b[c as usize] += w * held;
                }
                if c == 0 {
                    b[a as usize] += w * held;
                }
            }
        };
        shift(&mut bx, points[0].x);
        shift(&mut by, points[0].y);
        let masked = |input: &[f64], out: &mut [f64]| {
            let m: Vec<f64> = input.iter().enumerate().map(|(i, v)| if fixed[i] { 0.0 } else { *v }).collect();
            laplacian(&m, out);
        };
        conjugate_gradients(masked, &weights, &bx, &mut x, &fixed, 1e-5, 4 * n.max(100));
        conjugate_gradients(masked, &weights, &by, &mut y, &fixed, 1e-5, 4 * n.max(100));
        let next: Vec<Vec2> = (0..n).map(|i| if fixed[i] { points[i] } else { Vec2::new(x[i], y[i]) }).collect();
        let s = stress(&next);
        if flipped(&next) > folds || !(s < current) {
            break;
        }
        let gained = 1.0 - s / current.max(1e-300);
        points.copy_from_slice(&next);
        current = s;
        rounds += 1;
        if gained < 0.005 {
            break;
        }
    }
    rounds
}

impl Flat {
    /// The place on the scan a point of the pattern lies over, through the triangle it is in;
    /// `None` off the pattern.
    pub fn lift(&self, scan: &Scan, q: Vec2) -> Option<Vec3> {
        self.lifter().lift(self, scan, q)
    }

    /// Lifts many points at once, the pattern's triangles found through a grid of cells.
    pub fn lifter(&self) -> Lifter {
        Lifter::new(self)
    }
}

/// The pattern's triangles by cells, for lifting points back onto the scan.
pub struct Lifter {
    low: Vec2,
    cell: f64,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

impl Lifter {
    fn new(flat: &Flat) -> Lifter {
        let (low, high) = flat.points.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
            (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
        });
        let area = ((high.x - low.x) * (high.y - low.y)).max(1e-12);
        let cell = (area / flat.triangles.len().max(1) as f64).sqrt().max(1e-9) * 2.0;
        let key = |p: Vec2| (((p.x - low.x) / cell).floor() as i64, ((p.y - low.y) / cell).floor() as i64);
        let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (t, tri) in flat.triangles.iter().enumerate() {
            let [a, b, c] = tri.map(|v| flat.points[v as usize]);
            let (k0, k1) =
                (key(Vec2::new(a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y))), key(Vec2::new(a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y))));
            for i in k0.0..=k1.0 {
                for j in k0.1..=k1.1 {
                    cells.entry((i, j)).or_default().push(t as u32);
                }
            }
        }
        Lifter { low, cell, cells }
    }

    pub fn lift(&self, flat: &Flat, scan: &Scan, q: Vec2) -> Option<Vec3> {
        let key = (((q.x - self.low.x) / self.cell).floor() as i64, ((q.y - self.low.y) / self.cell).floor() as i64);
        for &t in self.cells.get(&key)? {
            let tri = flat.triangles[t as usize];
            let [a, b, c] = tri.map(|v| flat.points[v as usize]);
            let det = (b - a).x * (c - a).y - (b - a).y * (c - a).x;
            if det.abs() < 1e-300 {
                continue;
            }
            let l1 = ((q - a).x * (c - a).y - (q - a).y * (c - a).x) / det;
            let l2 = ((b - a).x * (q - a).y - (b - a).y * (q - a).x) / det;
            let l0 = 1.0 - l1 - l2;
            let slack = -1e-9;
            if l0 >= slack && l1 >= slack && l2 >= slack {
                let [p0, p1, p2] = tri.map(|v| scan.positions[flat.vertices[v as usize] as usize]);
                return Some(p0 * l0 + p1 * l1 + p2 * l2);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet of `n` × `n` squares of `size`, bent round a cylinder of `radius` about Y (a
    /// developable surface: it flattens with no stretch).
    fn bent_sheet(n: u32, size: f64, radius: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let s = f64::from(i) * size;
                let angle = s / radius;
                positions.push(Vec3::new(radius * angle.sin(), f64::from(j) * size, radius * (1.0 - angle.cos())));
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

    #[test]
    fn a_bent_sheet_flattens_to_its_own_size_with_no_stretch() {
        let (p, t) = bent_sheet(20, 2.0, 30.0);
        let scan = Scan::new(&p, &t);
        let all: Vec<usize> = (0..t.len()).collect();
        let flat = flatten(&scan, &all, false).unwrap();
        assert!(flat.stats.settled, "{:?}", flat.stats);
        assert!((flat.stats.flat_area - 1600.0).abs() < 1.0, "{:?}", flat.stats);
        assert!(flat.stats.strain_max < 0.01, "{:?}", flat.stats);
        assert_eq!(flat.stats.flipped, 0);
        assert_eq!(flat.outline.len(), 1);
        // The outline is the 40 × 40 square: its perimeter 160.
        let perimeter: f64 = flat.outline[0].iter().zip(flat.outline[0].iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
        assert!((perimeter - 160.0).abs() < 0.5, "{perimeter}");
        // A point in the pattern lifts back onto the sheet.
        let middle = flat.points.iter().fold(Vec2::ZERO, |s, q| s + *q) / flat.points.len() as f64;
        let up = flat.lift(&scan, middle).unwrap();
        assert!((up.y - 20.0).abs() < 1.0, "{up:?}");
        assert!((up.x.powi(2) + (up.z - 30.0).powi(2)).sqrt() - 30.0 < 0.05, "on the cylinder: {up:?}");
    }

    #[test]
    fn a_dome_stretches_and_relaxing_lowers_the_stretch() {
        // A sheet pushed up into a dome: not developable.
        let (mut p, t) = bent_sheet(20, 2.0, 1e9);
        for q in &mut p {
            let (dx, dy) = (q.x - 20.0, q.y - 20.0);
            q.z = 8.0 * (-(dx * dx + dy * dy) / 200.0).exp();
        }
        let scan = Scan::new(&p, &t);
        let all: Vec<usize> = (0..t.len()).collect();
        let conformal = flatten(&scan, &all, false).unwrap();
        let relaxed = flatten(&scan, &all, true).unwrap();
        assert!(relaxed.stats.relaxed > 0);
        assert!(relaxed.stats.strain_rms < conformal.stats.strain_rms, "{:?} {:?}", relaxed.stats, conformal.stats);
        assert_eq!(relaxed.stats.flipped, 0);
    }

    #[test]
    fn a_closed_region_is_refused() {
        let (p, t) = crate::test_meshes::fine_cuboid(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0), 4);
        let all: Vec<usize> = (0..t.len()).collect();
        assert_eq!(flatten(&Scan::new(&p, &t), &all, false).err(), Some(FlattenError::Closed));
        assert_eq!(flatten(&Scan::new(&p, &t), &[0], false).err(), Some(FlattenError::TooSmall));
    }
}
