//! Smooth patches fitted over a region of the scan, after SurfaceCraft's layouts: the region's
//! points placed on a square by the region's own flattening (so a region that curves round, which
//! no plane can see as a height field, fits too), and a bicubic B-spline's control points fitted
//! to them in all three directions by least squares, held smooth where the scan says nothing, each
//! point's place then moved to its foot on the surface and the net fitted again
//! ([`crate::net_fit`]).
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
use crate::net_fit::{self, Hold};
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
    /// The points' distance from the surface at their feet on it: root mean square, most.
    pub rms: f64,
    pub max: f64,
    pub points: usize,
    /// Where the surface is wrong ([`crate::surface_check`]): its folds, pinches and crossings. A
    /// region too intricate for one surface (deep crevices, a part curling over itself) folds.
    pub trouble: [usize; 3],
}

/// Patches of `layout` over the triangles `region` of the scan, `controls` control points each
/// way over the whole (4 to 30), held smooth by `smoothing` (0 to 1).
/// [`fit`] with as many control points each way as bring it within `aim` of the scan (root mean
/// square), as [`crate::surface::fit_surface_close`] does: from 6 up to 30, stopping once it is
/// that close or where more points no longer cut the distance by a quarter (the scan's noise is
/// reached). The patches and their control points each way.
pub fn fit_close(scan: &Scan, region: &[usize], layout: Layout, smoothing: f64, aim: f64) -> Result<(Patches, usize), PatchError> {
    let mut best: Option<(Patches, usize)> = None;
    for &n in crate::surface::CLOSE_STEPS {
        let found = match fit(scan, region, layout, [n, n], smoothing) {
            Ok(found) => found,
            // Too few points for more control points: the last that fitted is the closest.
            Err(PatchError::TooFew) if best.is_some() => break,
            Err(e) => return Err(e),
        };
        if best.as_ref().is_some_and(|(b, _)| found.rms >= 0.75 * b.rms) {
            break;
        }
        let close = found.rms <= aim;
        best = Some((found, n));
        if close {
            break;
        }
    }
    best.ok_or(PatchError::TooFew)
}

pub fn fit(scan: &Scan, region: &[usize], layout: Layout, controls: [usize; 2], smoothing: f64) -> Result<Patches, PatchError> {
    let [nu, nv] = controls.map(|c| c.clamp(4, 30));
    let smoothing = smoothing.clamp(0.0, 1.0);
    let weight = smoothing_weight(smoothing);
    // The points, each at its place on the square. Quads lays the outline on the square's sides;
    // Grid and Blocks start from the base surface the region fits best (its plane, a cylinder
    // round it, or its flattening), as each point's place there is near its place on the surface.
    let all: Vec<(Vec2, Vec3)> = if layout == Layout::Quads {
        let flat = flatten::flatten(scan, region, true).map_err(PatchError::Flatten)?;
        // The pattern laid over the region's plane where it faces one way: nearer the surface's
        // own parameters than the flattening, so the square's inside starts nearer them too.
        let (_, facing) = region_points(scan, region);
        let positions: Vec<Vec3> = flat.vertices.iter().map(|&v| scan.positions[v as usize]).collect();
        let flat = match net_fit::over_plane(&positions, &facing, false) {
            Some(places) => replanted(flat, places),
            None => flat,
        };
        let uv = square_map(&flat)?;
        flat.vertices.iter().zip(&uv).map(|(&v, &q)| (q, scan.positions[v as usize])).collect()
    } else {
        best_start(scan, region, [nu, nv], weight)?
    };
    let samples = thinned(&all, POINT_LIMIT);
    if samples.len() < nu * nv {
        return Err(PatchError::TooFew);
    }
    // Quads keeps the outline on the square's sides: a point there slides along its side only.
    let hold: Option<Vec<Hold>> = (layout == Layout::Quads).then(|| {
        samples
            .iter()
            .map(|(q, _)| match (q.x == 0.0 || q.x == 1.0, q.y == 0.0 || q.y == 1.0) {
                (true, true) => Hold::Both,
                (true, false) => Hold::U,
                (false, true) => Hold::V,
                _ => Hold::Free,
            })
            .collect()
    });
    let fitted = net_fit::fit(&samples, hold.as_deref(), [nu, nv], weight, net_fit::ROUNDS).ok_or(PatchError::Failed)?;
    let (surface, rms, max) = (fitted.surface, fitted.rms, fitted.max);
    let (rows, columns) = match layout {
        Layout::Blocks { rows, columns } => (rows.max(1), columns.max(1)),
        _ => (1, 1),
    };
    let trouble = crate::surface_check::trouble(&surface);
    let surfaces = if rows * columns > 1 { blocks(&surface, rows, columns)? } else { vec![surface] };
    Ok(Patches { surfaces, rows, columns, rms, max, points: samples.len(), trouble })
}

/// How hard the smoothing holds the net against the points, from the 0 to 1 a person sets: at the
/// default (0.1) about a thousandth of the data's own weight, as SurfaceCraft's patch fit holds it,
/// rising to a tenth at 1.
pub(crate) fn smoothing_weight(smoothing: f64) -> f64 {
    let s = if smoothing.is_finite() { smoothing.clamp(0.0, 1.0) } else { 0.0 };
    1e-4 + 0.1 * s * s
}

/// Points each candidate start is tried on before the best is fitted on all of them.
const TRIAL_POINTS: usize = 4_000;

/// The region's points placed on the square by the base surface that fits them best: each start
/// (the region's plane, a cylinder round it, its flattening) is fitted on a sample of the points,
/// and the one whose surface comes closest without folding is kept (a parametrization from a base
/// surface the points are projected onto, after Ma and Kruth, 1995).
fn best_start(scan: &Scan, region: &[usize], counts: [usize; 2], weight: f64) -> Result<Vec<(Vec2, Vec3)>, PatchError> {
    let (points, facing) = region_points(scan, region);
    let mut starts: Vec<Vec<(Vec2, Vec3)>> = Vec::new();
    if let Some(places) = net_fit::over_plane(&points, &facing, false) {
        starts.push(places.into_iter().zip(points.iter().copied()).collect());
    }
    if let Some(places) = round_cylinder(scan, region, &points) {
        starts.push(places.into_iter().zip(points.iter().copied()).collect());
    }
    let flat_error = match flatten::flatten(scan, region, true) {
        Ok(flat) => {
            let uv = boxed(&flat.points);
            starts.push(flat.vertices.iter().zip(&uv).map(|(&v, &q)| (q, scan.positions[v as usize])).collect());
            None
        }
        Err(e) => Some(e),
    };
    if starts.len() <= 1 {
        return starts.pop().ok_or_else(|| flat_error.map_or(PatchError::Failed, PatchError::Flatten));
    }
    // Each tried on the same few points; the closest that does not fold kept.
    let score = |start: &Vec<(Vec2, Vec3)>| -> Option<(bool, f64)> {
        let trial = thinned(start, TRIAL_POINTS);
        let f = net_fit::fit(&trial, None, counts, weight, net_fit::ROUNDS)?;
        let clean = crate::surface_check::check_whole(&f.surface, 24).is_some_and(|c| c.is_clean());
        Some((clean, f.rms))
    };
    let scored: Vec<Option<(bool, f64)>> = starts.iter().map(score).collect();
    let best = scored
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .min_by(|(_, a), (_, b)| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)))
        .map(|(i, _)| i)
        .ok_or(PatchError::Failed)?;
    Ok(starts.swap_remove(best))
}

/// The region's points (each scan vertex once) and its triangles' normals and areas.
fn region_points(scan: &Scan, region: &[usize]) -> (Vec<Vec3>, Vec<(Vec3, f64)>) {
    let mut seen = vec![false; scan.positions.len()];
    let (mut points, mut facing) = (Vec::new(), Vec::with_capacity(region.len()));
    for &t in region {
        let Some([a, b, c]) = scan.corners(t) else { continue };
        let cross = (b - a).cross(c - a);
        if let Some(n) = cross.normalized() {
            facing.push((n, cross.len() / 2.0));
        }
        for &v in scan.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), scan.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                points.push(*p);
            }
        }
    }
    (points, facing)
}

/// Places on the square for `points` round a cylinder fitted to the region: the angle round its
/// axis and the distance along it, scaled to their box. `None` when no cylinder fits, or the region
/// goes the whole way round (one patch cannot close on itself).
fn round_cylinder(scan: &Scan, region: &[usize], points: &[Vec3]) -> Option<Vec<Vec2>> {
    let crate::fit::Shape::Cylinder { point, axis, radius } = crate::fit::shape_of_sampled(scan, region, crate::fit::Kind::Cylinder, TRIAL_POINTS)?
    else {
        return None;
    };
    let axis = axis.normalized()?;
    if !(radius.is_finite() && radius > 0.0) || points.is_empty() {
        return None;
    }
    // Angles measured from the way the region lies from the axis, so they do not jump across ±π.
    let across = |p: Vec3| {
        let d = p - point;
        d - axis * d.dot(axis)
    };
    let middle = points.iter().fold(Vec3::ZERO, |s, p| s + across(*p)).normalized()?;
    let side = axis.cross(middle);
    let placed: Vec<(f64, f64)> = points
        .iter()
        .map(|p| {
            let r = across(*p);
            (r.dot(side).atan2(r.dot(middle)), (*p - point).dot(axis))
        })
        .collect();
    let (lo, hi) = placed
        .iter()
        .fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(lo, hi), (a, h)| ((lo.0.min(*a), lo.1.min(*h)), (hi.0.max(*a), hi.1.max(*h))));
    let (turn, length) = (hi.0 - lo.0, hi.1 - lo.1);
    if !(turn > 0.0 && turn < 1.9 * std::f64::consts::PI && length > 0.0) {
        return None;
    }
    Some(placed.into_iter().map(|(a, h)| Vec2::new((a - lo.0) / turn, (h - lo.1) / length)).collect())
}

/// `flat` with its points at `places` (one each, in order), its outline moved with them.
fn replanted(flat: flatten::Flat, places: Vec<Vec2>) -> flatten::Flat {
    if places.len() != flat.points.len() {
        return flat;
    }
    let index: std::collections::HashMap<[u64; 2], usize> =
        flat.points.iter().enumerate().map(|(i, p)| ([p.x.to_bits(), p.y.to_bits()], i)).collect();
    let outline = flat
        .outline
        .iter()
        .map(|ring| ring.iter().filter_map(|p| index.get(&[p.x.to_bits(), p.y.to_bits()]).map(|&i| places[i])).collect())
        .collect();
    flatten::Flat { points: places, outline, ..flat }
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

    /// A sheet of `n` × `n` squares bent `turn` radians round a cylinder of radius 20 about Y.
    fn bent_by(n: u32, turn: f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        for j in 0..=n {
            for i in 0..=n {
                let a = (f64::from(i) / f64::from(n) - 0.5) * turn;
                positions.push(Vec3::new(20.0 * a.sin(), f64::from(j) * 30.0 / f64::from(n), 20.0 * (1.0 - a.cos())));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        (positions, triangles)
    }

    /// A gentle dome fits from its own plane: within a hundredth of a millimetre at the default
    /// smoothing, as SurfaceCraft's patch fit does, and without folds.
    #[test]
    fn a_dome_fits_closely_at_the_default_smoothing() {
        let n = 40u32;
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        for j in 0..=n {
            for i in 0..=n {
                let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
                positions.push(Vec3::new(x, y, 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2))));
            }
        }
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        let scan = Scan::new(&positions, &triangles);
        let all: Vec<usize> = (0..triangles.len()).collect();
        let grid = fit(&scan, &all, Layout::Grid, [8, 8], 0.1).unwrap();
        assert!(grid.rms < 0.01, "{} {}", grid.rms, grid.max);
        assert_eq!(grid.trouble, [0, 0, 0]);
        assert!(crate::surface_check::check_whole(&grid.surfaces[0], 40).is_some_and(|c| c.is_clean()));
    }

    /// A sheet bent three quarters of the way round, which no plane sees without folding it over,
    /// fits round a cylinder: close to it, without folds.
    #[test]
    fn a_sheet_bent_most_of_the_way_round_fits_without_folding() {
        let (p, t) = bent_by(36, 1.5 * std::f64::consts::PI);
        let all: Vec<usize> = (0..t.len()).collect();
        let scan = Scan::new(&p, &t);
        let grid = fit(&scan, &all, Layout::Grid, [12, 6], 0.1).unwrap();
        assert!(grid.rms < 0.05, "{} {}", grid.rms, grid.max);
        assert!(off_cylinder(&grid.surfaces[0]) < 0.2, "{}", off_cylinder(&grid.surfaces[0]));
        assert!(crate::surface_check::check_whole(&grid.surfaces[0], 40).is_some_and(|c| c.is_clean()));
        assert!(round_cylinder(&scan, &all, &region_points(&scan, &all).0).is_some(), "a cylinder fits round it");
    }
}
