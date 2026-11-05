//! Smooth patches fitted over a region of the scan, after SurfaceCraft's layouts: the region's
//! points placed on a square by the region's own flattening (so a region that curves round, which
//! no plane can see as a height field, fits too), and a bicubic B-spline's control points fitted
//! to them in all three directions by least squares, held smooth where the scan says nothing.
//!
//! - Grid: one patch over the flattened region's box.
//! - Blocks: that patch cut into rows and columns of patches at its knots, exactly, so they meet
//!   as smoothly as one surface.
//! - Quads: the region mapped onto the square with its outline on the square's sides (corners at
//!   the four outline points nearest the corners of its pattern), so the patch's edges follow the
//!   region's edge.

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::bspline::{Dir, NurbsSurface};
use solidcraft_geom::prelude::*;

use crate::flatten::{self, FlattenError};
use crate::surface::{basis_row, cholesky_solve, clamped_knots};
use crate::{Scan, thinned};

/// The most scan points the patches are fitted to; a larger region is sampled evenly.
const POINT_LIMIT: usize = 40_000;

/// How the patches are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Grid,
    Blocks { rows: usize, columns: usize },
    Quads,
}

/// Why no patches could be fitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    Flatten(FlattenError),
    /// Too few points for the control points asked for.
    TooFew,
    /// The region's outline is not one loop (Quads needs one, round the outside).
    NotOneLoop,
    /// More blocks than spans of the surface.
    TooManyBlocks,
    /// The fit had no answer.
    Failed,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::Flatten(e) => write!(f, "{e}"),
            PatchError::TooFew => f.write_str("too few scan points in the region for that many control points: take a larger region or fewer points"),
            PatchError::NotOneLoop => {
                f.write_str("Quads lays the patch's edges along the region's edge, so the region must have one edge round it, and no holes")
            }
            PatchError::TooManyBlocks => f.write_str("more blocks than the surface has spans: ask for more control points, or fewer blocks"),
            PatchError::Failed => f.write_str("the patches could not be fitted"),
        }
    }
}

impl std::error::Error for PatchError {}

/// Patches fitted to a region, and how well.
#[derive(Clone, Debug)]
pub struct Patches {
    /// Row by row, each row's patches along u.
    pub surfaces: Vec<NurbsSurface>,
    pub rows: usize,
    pub columns: usize,
    /// The points' distance from the surface at their own place on it: root mean square, most.
    pub rms: f64,
    pub max: f64,
    pub points: usize,
}

/// Patches of `layout` over the triangles `region` of the scan, `controls` control points each
/// way over the whole (4 to 30), held smooth by `smoothing` (0 to 1).
pub fn fit(scan: &Scan, region: &[usize], layout: Layout, controls: [usize; 2], smoothing: f64) -> Result<Patches, PatchError> {
    let [nu, nv] = controls.map(|c| c.clamp(4, 30));
    let flat = flatten::flatten(scan, region, true).map_err(PatchError::Flatten)?;
    let uv = match layout {
        Layout::Quads => square_map(&flat)?,
        _ => boxed(&flat.points),
    };
    // The points, each at its place on the square.
    let all: Vec<(Vec2, Vec3)> = flat.vertices.iter().zip(&uv).map(|(&v, &q)| (q, scan.positions[v as usize])).collect();
    let samples = thinned(&all, POINT_LIMIT);
    if samples.len() < nu * nv {
        return Err(PatchError::TooFew);
    }
    let surface = least_squares(&samples, [nu, nv], smoothing.clamp(0.0, 1.0))?;
    let errors: Vec<f64> = samples.iter().filter_map(|(q, p)| surface.eval(q.x, q.y).ok().map(|s| s.dist(*p))).collect();
    let rms = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len().max(1) as f64).sqrt();
    let max = errors.iter().copied().fold(0.0, f64::max);
    let (rows, columns) = match layout {
        Layout::Blocks { rows, columns } => (rows.max(1), columns.max(1)),
        _ => (1, 1),
    };
    let surfaces = if rows * columns > 1 { blocks(&surface, rows, columns)? } else { vec![surface] };
    Ok(Patches { surfaces, rows, columns, rms, max, points: samples.len() })
}

/// The pattern's points turned to its own axes and scaled into the unit square.
fn boxed(points: &[Vec2]) -> Vec<Vec2> {
    let n = points.len().max(1) as f64;
    let middle = points.iter().fold(Vec2::ZERO, |s, p| s + *p) / n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in points {
        let d = *p - middle;
        sxx += d.x * d.x;
        sxy += d.x * d.y;
        syy += d.y * d.y;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (a, b) = (Vec2::from_angle(angle), Vec2::from_angle(angle + std::f64::consts::FRAC_PI_2));
    let turned: Vec<Vec2> = points.iter().map(|p| Vec2::new((*p - middle).dot(a), (*p - middle).dot(b))).collect();
    let (lo, hi) = turned.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), p| {
        (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
    });
    let size = Vec2::new((hi.x - lo.x).max(1e-12), (hi.y - lo.y).max(1e-12));
    turned.iter().map(|p| Vec2::new((p.x - lo.x) / size.x, (p.y - lo.y) / size.y)).collect()
}

/// The region mapped onto the unit square, its outline on the square's sides: the four corners at
/// the outline points nearest the corners of the pattern's box, the outline between them laid
/// along each side by its length, and the inside placed where each point is the weighted middle of
/// its neighbours (a harmonic map, which keeps every triangle turned the right way).
fn square_map(flat: &flatten::Flat) -> Result<Vec<Vec2>, PatchError> {
    let [outline] = flat.outline.as_slice() else { return Err(PatchError::NotOneLoop) };
    // The outline as vertex numbers, in order.
    let index: std::collections::HashMap<[u64; 2], u32> =
        flat.points.iter().enumerate().map(|(i, p)| ([p.x.to_bits(), p.y.to_bits()], i as u32)).collect();
    let ring: Vec<u32> = outline.iter().filter_map(|p| index.get(&[p.x.to_bits(), p.y.to_bits()]).copied()).collect();
    if ring.len() < 4 {
        return Err(PatchError::NotOneLoop);
    }
    let unit = boxed(&flat.points);
    let corners_at = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)];
    let mut corners: Vec<usize> = corners_at
        .iter()
        .map(|c| (0..ring.len()).min_by(|&a, &b| unit[ring[a] as usize].dist(*c).total_cmp(&unit[ring[b] as usize].dist(*c))).unwrap_or(0))
        .collect();
    // In the ring's own order, starting from the first, and four apart.
    let first = corners[0];
    corners.sort_by_key(|&k| (k + ring.len() - first) % ring.len());
    corners.dedup();
    if corners.len() < 4 {
        return Err(PatchError::NotOneLoop);
    }
    // Which way round the ring runs against the square's corners: it may run the other way.
    let order: Vec<usize> = corners
        .iter()
        .map(|&k| {
            (0..4).min_by(|&a, &b| unit[ring[k] as usize].dist(corners_at[a]).total_cmp(&unit[ring[k] as usize].dist(corners_at[b]))).unwrap_or(0)
        })
        .collect();
    let mut placed = vec![Vec2::ZERO; flat.points.len()];
    let mut fixed = vec![false; flat.points.len()];
    for side in 0..4 {
        let (from, to) = (corners[side], corners[(side + 1) % 4]);
        let steps: Vec<usize> =
            std::iter::successors(Some(from), |&k| Some((k + 1) % ring.len())).take_while(|&k| k != to).chain(std::iter::once(to)).collect();
        let lengths: Vec<f64> = steps.windows(2).map(|w| flat.points[ring[w[0]] as usize].dist(flat.points[ring[w[1]] as usize])).collect();
        let total: f64 = lengths.iter().sum::<f64>().max(1e-300);
        let (a, b) = (corners_at[order[side]], corners_at[order[(side + 1) % 4]]);
        let mut gone = 0.0;
        for (k, &step) in steps.iter().enumerate() {
            if k > 0 {
                gone += lengths[k - 1];
            }
            let v = ring[step] as usize;
            placed[v] = a.lerp(b, gone / total);
            fixed[v] = true;
        }
    }
    // The inside: each point the weighted middle of its neighbours (weights 1 / length on the
    // pattern), by Gauss–Seidel from where the box puts it.
    let n = flat.points.len();
    let mut neighbours: Vec<Vec<(u32, f64)>> = vec![Vec::new(); n];
    for t in &flat.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let w = 1.0 / flat.points[a as usize].dist(flat.points[b as usize]).max(1e-12);
            neighbours[a as usize].push((b, w));
            neighbours[b as usize].push((a, w));
        }
    }
    for (i, p) in placed.iter_mut().enumerate() {
        if !fixed[i] {
            *p = unit[i];
        }
    }
    for _ in 0..4000 {
        let mut moved = 0.0f64;
        for i in 0..n {
            if fixed[i] || neighbours[i].is_empty() {
                continue;
            }
            let (sum, weight) = neighbours[i].iter().fold((Vec2::ZERO, 0.0), |(s, w), &(j, wj)| (s + placed[j as usize] * wj, w + wj));
            let next = sum / weight;
            moved = moved.max(next.dist(placed[i]));
            placed[i] = next;
        }
        if moved < 1e-9 {
            break;
        }
    }
    Ok(placed)
}

/// A bicubic surface `counts` across fitted to `samples` (each a place on the unit square and
/// the point there) by least squares, held smooth by the second differences of its net.
fn least_squares(samples: &[(Vec2, Vec3)], counts: [usize; 2], smoothing: f64) -> Result<NurbsSurface, PatchError> {
    let [nu, nv] = counts;
    let (ku, kv) = (clamped_knots(nu), clamped_knots(nv));
    let k = nu * nv;
    let mut m = vec![0.0; k * k];
    let mut rhs = [vec![0.0; k], vec![0.0; k], vec![0.0; k]];
    for (q, p) in samples {
        let Some((cols, vals)) = row(&ku, &kv, nu, nv, q.x.clamp(0.0, 1.0), q.y.clamp(0.0, 1.0)) else { continue };
        for a in 0..16 {
            for b in 0..16 {
                m[cols[a] * k + cols[b]] += vals[a] * vals[b];
            }
            rhs[0][cols[a]] += vals[a] * p.x;
            rhs[1][cols[a]] += vals[a] * p.y;
            rhs[2][cols[a]] += vals[a] * p.z;
        }
    }
    // Smoothness: second differences of the net along u and v, weighed against the data.
    let scale = (0..k).map(|i| m[i * k + i]).sum::<f64>() / k as f64;
    let lambda = (1e-4 + smoothing * smoothing) * scale.max(1e-12);
    let at = |i: usize, j: usize| j * nu + i;
    let mut second = |a: usize, b: usize, c: usize| {
        let idx = [a, b, c];
        let w = [1.0, -2.0, 1.0];
        for x in 0..3 {
            for y in 0..3 {
                m[idx[x] * k + idx[y]] += lambda * w[x] * w[y];
            }
        }
    };
    for j in 0..nv {
        for i in 1..nu - 1 {
            second(at(i - 1, j), at(i, j), at(i + 1, j));
        }
    }
    for i in 0..nu {
        for j in 1..nv - 1 {
            second(at(i, j - 1), at(i, j), at(i, j + 1));
        }
    }
    let mut coords = Vec::with_capacity(3);
    for r in &rhs {
        let mut copy = m.clone();
        coords.push(cholesky_solve(&mut copy, r, k).ok_or(PatchError::Failed)?);
    }
    let points: Vec<Vec3> = (0..k).map(|i| Vec3::new(coords[0][i], coords[1][i], coords[2][i])).collect();
    NurbsSurface::new([3, 3], [ku, kv], &points, &vec![1.0; k]).map_err(|_| PatchError::Failed)
}

/// The 16 control points a place depends on and their weights, on a net `nu` × `nv` across.
fn row(ku: &[f64], kv: &[f64], nu: usize, nv: usize, s: f64, t: f64) -> Option<([usize; 16], [f64; 16])> {
    if nu == nv && ku == kv {
        return basis_row(ku, nu, s, t);
    }
    let (fu, bu) = solidcraft_geom::bspline::basis_derivatives(ku, 3, nu, s, 0)?;
    let (fv, bv) = solidcraft_geom::bspline::basis_derivatives(kv, 3, nv, t, 0)?;
    let (mut cols, mut vals) = ([0usize; 16], [0.0f64; 16]);
    for a in 0..4 {
        for b in 0..4 {
            cols[b * 4 + a] = (fv + b) * nu + fu + a;
            vals[b * 4 + a] = bu.get(a)?.first()? * bv.get(b)?.first()?;
        }
    }
    Some((cols, vals))
}

/// The surface cut into `rows` × `columns` patches at evenly spread knots, exactly: each cut a
/// knot of full multiplicity, the net then parted there.
fn blocks(surface: &NurbsSurface, rows: usize, columns: usize) -> Result<Vec<NurbsSurface>, PatchError> {
    let spans = |d: usize| surface.count()[d] - surface.degree()[d];
    if columns > spans(0) || rows > spans(1) {
        return Err(PatchError::TooManyBlocks);
    }
    // The cuts, on existing knots where they fall near one, else where asked.
    let cuts = |d: usize, parts: usize| -> Vec<f64> {
        let knots = &surface.knots()[d];
        (1..parts)
            .map(|k| {
                let at = k as f64 / parts as f64;
                knots
                    .iter()
                    .copied()
                    .filter(|x| *x > 0.0 && *x < 1.0)
                    .min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs()))
                    .filter(|x| (x - at).abs() < 0.5 / parts as f64)
                    .unwrap_or(at)
            })
            .collect()
    };
    let (cu, cv) = (cuts(0, columns), cuts(1, rows));
    let mut s = surface.clone();
    for (dir, list) in [(Dir::S, &cu), (Dir::T, &cv)] {
        let d = if dir == Dir::S { 0 } else { 1 };
        for &at in list {
            let have = s.knots()[d].iter().filter(|x| (*x - at).abs() < 1e-12).count();
            for _ in have..s.degree()[d] {
                s = s.insert_knot(dir, at).map_err(|_| PatchError::Failed)?;
            }
        }
    }
    // Part the net at each cut. A cut at a knot of multiplicity p (index k its last) ends one
    // patch at control point k − p and starts the next there; each patch's knots are its ends,
    // each p + 1 times, and the knots between them.
    let edges = |list: &[f64]| -> Vec<(f64, f64)> {
        let mut at = vec![0.0];
        at.extend(list.iter().copied());
        at.push(1.0);
        at.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let p = 3;
    let control = |d: usize, at: f64| -> usize {
        let knots = &s.knots()[d];
        if at <= 0.0 {
            return 0;
        }
        if at >= 1.0 {
            return s.count()[d] - 1;
        }
        knots.iter().rposition(|x| (*x - at).abs() < 1e-12).map_or(0, |k| k - p)
    };
    let piece = |d: usize, lo: f64, hi: f64| -> (Vec<f64>, usize, usize) {
        let mut local = vec![0.0; p + 1];
        local.extend(s.knots()[d].iter().filter(|x| **x > lo + 1e-12 && **x < hi - 1e-12).map(|x| (x - lo) / (hi - lo)));
        local.extend(std::iter::repeat_n(1.0, p + 1));
        let (first, last) = (control(d, lo), control(d, hi));
        (local, first, last + 1 - first)
    };
    let mut out = Vec::new();
    let net = |i: usize, j: usize| s.point(i, j).project().unwrap_or(Vec3::ZERO);
    for (v0, v1) in edges(&cv) {
        for (u0, u1) in edges(&cu) {
            let (ku, fu, nu) = piece(0, u0, u1);
            let (kv, fv, nv) = piece(1, v0, v1);
            if ku.len() != nu + p + 1 || kv.len() != nv + p + 1 {
                return Err(PatchError::Failed);
            }
            let pts: Vec<Vec3> = (0..nv).flat_map(|j| (0..nu).map(move |i| (i, j))).map(|(i, j)| net(fu + i, fv + j)).collect();
            out.push(NurbsSurface::new([p, p], [ku, kv], &pts, &vec![1.0; nu * nv]).map_err(|_| PatchError::Failed)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet of `n` × `n` squares bent a third of the way round a cylinder of radius 20 about Y:
    /// it curves round, so no plane sees it as a height field.
    fn bent(n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let a = f64::from(i) / f64::from(n) * 2.1 - 1.05;
                positions.push(Vec3::new(20.0 * a.sin(), f64::from(j) * 30.0 / f64::from(n), 20.0 * (1.0 - a.cos())));
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

    fn off_cylinder(s: &NurbsSurface) -> f64 {
        let mut worst = 0.0f64;
        for a in 0..=10 {
            for b in 0..=10 {
                let q = s.eval(f64::from(a) / 10.0, f64::from(b) / 10.0).unwrap();
                worst = worst.max(((q.x * q.x + (q.z - 20.0).powi(2)).sqrt() - 20.0).abs());
            }
        }
        worst
    }

    #[test]
    fn a_curving_region_fits_one_patch_and_cuts_into_blocks() {
        let (p, t) = bent(30);
        let all: Vec<usize> = (0..t.len()).collect();
        let scan = Scan::new(&p, &t);
        let grid = fit(&scan, &all, Layout::Grid, [10, 8], 0.0).unwrap();
        assert!(grid.rms < 0.02, "{} {}", grid.rms, grid.max);
        assert_eq!(grid.surfaces.len(), 1);
        assert!(off_cylinder(&grid.surfaces[0]) < 0.1);
        let blocks = fit(&scan, &all, Layout::Blocks { rows: 2, columns: 3 }, [10, 8], 0.0).unwrap();
        assert_eq!(blocks.surfaces.len(), 6);
        // Neighbouring blocks meet: the first's right side is the second's left.
        let (a, b) = (&blocks.surfaces[0], &blocks.surfaces[1]);
        for k in 0..=5 {
            let v = f64::from(k) / 5.0;
            assert!(a.eval(1.0, v).unwrap().dist(b.eval(0.0, v).unwrap()) < 1e-9);
        }
        for s in &blocks.surfaces {
            assert!(off_cylinder(s) < 0.1);
        }
        assert_eq!(fit(&scan, &all, Layout::Blocks { rows: 2, columns: 20 }, [10, 8], 0.0).err(), Some(PatchError::TooManyBlocks));
    }

    #[test]
    fn quads_lay_the_patch_edges_along_the_region_edge() {
        let (p, t) = bent(24);
        let all: Vec<usize> = (0..t.len()).collect();
        let scan = Scan::new(&p, &t);
        let quads = fit(&scan, &all, Layout::Quads, [8, 8], 0.0).unwrap();
        let s = &quads.surfaces[0];
        // Its corners are the sheet's corners.
        let sheet = [p[0], p[24], p[24 * 25 + 24], p[24 * 25]];
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let q = s.eval(u, v).unwrap();
            let nearest = sheet.iter().map(|c| c.dist(q)).fold(f64::INFINITY, f64::min);
            assert!(nearest < 0.5, "a corner on the sheet's corner: {q:?} ({nearest})");
        }
        assert!(quads.rms < 0.05, "{}", quads.rms);
    }
}
