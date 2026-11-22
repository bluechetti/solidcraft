//! A cage fitted to a scan: the cage's points moved, all at once, so that its patches lie on the
//! scan in least squares (after SurfaceCraft's Auto Freeform fit).
//!
//! Each round looks at places spread over every patch. A place is a blend of its patch's points,
//! and each of those a blend of cage points ([`crate::cage`]), so it is a blend of cage points
//! too. A row says: this place should go by its gap to the scan, the gap being to the nearest
//! point of the scan. The cage's points are the unknowns, so whatever the rows ask for, the
//! patches still meet one another in curvature: the fit cannot pull them apart.
//!
//! Two more terms keep the answer sound. Fairness holds down the second differences of each
//! patch's net, so a patch follows the scan's shape and not its noise. A light pull to stay put
//! settles what no row reaches. The system is sparse and positive definite, and is solved by
//! conjugate gradients on the normal equations, matrix-free, with their diagonal as
//! preconditioner. After a round the places are measured again on the moved cage, and the rounds
//! stop when the fit no longer improves.
//!
//! The scan is met at its nearest point, so the cage must start near the scan (a layout puts it
//! there). Past an open rim of the scan the nearest point is on the rim: rims are a later step.

use solidcore_record::Vec3;
use solidcraft_geom::compare::{TriangleTree, closest_on_triangle};

use crate::Scan;
use crate::cage::{Cage, Patches};

/// How much a second difference weighs, as a share of what a point's rows weigh on average.
const FAIRNESS: f64 = 5e-4;
/// How much staying put weighs, in the same terms.
const STAY: f64 = 1e-4;
/// The conjugate gradient's limit of steps, and the share of the residual it stops at.
const STEPS: usize = 80;
const CLOSE: f64 = 1e-6;
/// A round that brings the fit less than this share closer is the last.
const GAIN: f64 = 0.01;
/// Gaps under this share of the scan's size are rounding: the patches are on the scan.
const ON_IT: f64 = 1e-12;

/// How a cage is fitted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitSettings {
    /// The most rounds: each measures the gaps again and solves once.
    pub rounds: usize,
    /// The places looked at along each side of a patch: that many squared on each.
    pub places: usize,
}

impl Default for FitSettings {
    fn default() -> Self {
        FitSettings { rounds: 12, places: 6 }
    }
}

/// Why a cage could not be fitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FitError {
    /// The scan has no triangles.
    NoScan,
    /// The cage has no patches to fit: every face was left out.
    NoPatches,
    /// The settings ask for no rounds or no places.
    BadSettings,
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FitError::NoScan => f.write_str("the scan has no triangles to fit to"),
            FitError::NoPatches => f.write_str("the cage has no patches to fit: every face is at the rim or between irregular corners"),
            FitError::BadSettings => f.write_str("a fit needs at least one round and one place a side"),
        }
    }
}

impl std::error::Error for FitError {}

/// A cage after its fit, and how near the scan its patches lie.
#[derive(Clone, Debug)]
pub struct CageFit {
    /// The cage's points, moved. Its faces are as they were.
    pub points: Vec<Vec3>,
    /// The root mean square of the gaps from the places to the scan, after the last round.
    pub rms: f64,
    /// The largest of those gaps.
    pub worst: f64,
    /// The same root mean square before the first round.
    pub rms_before: f64,
    /// How many places were measured.
    pub places: usize,
    /// How many rounds were run.
    pub rounds: usize,
}

/// One patch's part in the fit: where its points start among all the patches' points, how many
/// it has a side, and which table of shares its places read.
struct Part {
    start: usize,
    side: usize,
    table: usize,
}

/// The shares of a patch's points in each of its places, for one degree: place after place, each
/// `(degree + 1)²` shares, u fastest.
struct Shares {
    degree: usize,
    shares: Vec<f64>,
}

/// The Bernstein polynomials of degree `n` at `t`.
fn bernstein(n: usize, t: f64) -> Vec<f64> {
    let mut b = vec![1.0];
    for _ in 0..n {
        let mut next = vec![0.0; b.len() + 1];
        for (i, x) in b.iter().enumerate() {
            next[i] += x * (1.0 - t);
            next[i + 1] += x * t;
        }
        b = next;
    }
    b
}

impl Shares {
    fn of(degree: usize, places: usize) -> Shares {
        let along: Vec<Vec<f64>> = (0..places).map(|k| bernstein(degree, (k as f64 + 0.5) / places as f64)).collect();
        let mut shares = Vec::with_capacity(places * places * (degree + 1) * (degree + 1));
        for v in &along {
            for u in &along {
                for wv in v {
                    shares.extend(u.iter().map(|wu| wu * wv));
                }
            }
        }
        Shares { degree, shares }
    }
}

/// What a solve is to satisfy: each row a place of a patch (its shares of the patch's points)
/// and how far it should go.
struct Rows<'a> {
    parts: &'a [Part],
    tables: &'a [Shares],
    /// For each row: its patch, and its place on it.
    of: Vec<(u32, u32)>,
    gaps: Vec<Vec3>,
}

impl Rows<'_> {
    /// Each row: where its patch's points start, and its shares of them.
    fn blends(&self) -> impl Iterator<Item = (usize, &[f64])> {
        self.of.iter().filter_map(|&(part, place)| {
            let part = self.parts.get(part as usize)?;
            let count = part.side * part.side;
            let shares = self.tables.get(part.table)?.shares.get(place as usize * count..(place as usize + 1) * count)?;
            Some((part.start, shares))
        })
    }
}

/// The second differences of a net of `side` × `side` points starting at `start`, along its rows
/// and its columns.
fn second_differences(start: usize, side: usize, lines: &mut Vec<[u32; 3]>) {
    let at = |i: usize, j: usize| (start + j * side + i) as u32;
    for j in 0..side {
        for i in 0..side.saturating_sub(2) {
            lines.push([at(i, j), at(i + 1, j), at(i + 2, j)]);
            lines.push([at(j, i), at(j, i + 1), at(j, i + 2)]);
        }
    }
}

/// `A x` over the patches' points: the rows, the fairness and the pull to stay.
fn apply(rows: &Rows, lines: &[[u32; 3]], (fair, stay): (f64, f64), x: &[f64], y: &mut [f64]) {
    for (out, v) in y.iter_mut().zip(x) {
        *out = stay * v;
    }
    for (start, shares) in rows.blends() {
        let (Some(from), Some(to)) = (x.get(start..start + shares.len()), y.get_mut(start..start + shares.len())) else { continue };
        let at: f64 = shares.iter().zip(from).map(|(w, v)| w * v).sum();
        for (out, w) in to.iter_mut().zip(shares) {
            *out += w * at;
        }
    }
    for &[a, b, c] in lines {
        let (a, b, c) = (a as usize, b as usize, c as usize);
        let bend = fair * (x[a] - 2.0 * x[b] + x[c]);
        y[a] += bend;
        y[b] -= 2.0 * bend;
        y[c] += bend;
    }
}

/// The patches' points as their blends make them of the cage's (one coordinate).
fn spread(blends: &[&[(u32, f64)]], cage: &[f64], points: &mut [f64]) {
    for (out, blend) in points.iter_mut().zip(blends) {
        *out = blend.iter().map(|&(k, w)| w * cage.get(k as usize).copied().unwrap_or(0.0)).sum();
    }
}

/// What the patches' points ask of the cage's, each through its blend (one coordinate).
fn gather(blends: &[&[(u32, f64)]], points: &[f64], cage: &mut [f64]) {
    cage.fill(0.0);
    for (v, blend) in points.iter().zip(blends) {
        for &(k, w) in *blend {
            if let Some(out) = cage.get_mut(k as usize) {
                *out += w * v;
            }
        }
    }
}

/// How far each of the cage's `unknowns` points should go to meet `rows` in least squares, with
/// fairness on `lines` (the second differences of the patches' nets, which `now` holds) and the
/// pull to stay. The rows, the fairness and the pull are the patches' points'; the solve is the
/// cage points', so what the blends tie stays tied.
fn moves(rows: &Rows, lines: &[[u32; 3]], now: &[Vec3], blends: &[&[(u32, f64)]], unknowns: usize) -> Vec<Vec3> {
    let count = now.len();
    let mut diagonal = vec![0.0_f64; count];
    for (start, shares) in rows.blends() {
        if let Some(d) = diagonal.get_mut(start..start + shares.len()) {
            for (d, w) in d.iter_mut().zip(shares) {
                *d += w * w;
            }
        }
    }
    let reached = diagonal.iter().filter(|d| **d > 0.0).count().max(1);
    let weight = diagonal.iter().sum::<f64>() / reached as f64;
    // Nothing measured: nothing to fit to, and nothing for the rest of the system to weigh against.
    if !(weight > 0.0) || lines.iter().flatten().any(|&k| k as usize >= count) {
        return vec![Vec3::new(0.0, 0.0, 0.0); unknowns];
    }
    let (fair, stay) = (FAIRNESS * weight, STAY * weight);
    for d in &mut diagonal {
        *d += stay;
    }
    for &[a, b, c] in lines {
        diagonal[a as usize] += fair;
        diagonal[b as usize] += 4.0 * fair;
        diagonal[c as usize] += fair;
    }
    // The cage points' diagonal, as a blend carries the patches' to them (what the patches' points
    // couple is left out: it only conditions).
    let mut reduced = vec![0.0_f64; unknowns];
    for (d, blend) in diagonal.iter().zip(blends) {
        for &(k, w) in *blend {
            if let Some(r) = reduced.get_mut(k as usize) {
                *r += w * w * d;
            }
        }
    }
    for d in &mut reduced {
        if !(*d > 0.0) {
            *d = 1.0;
        }
    }
    let mut answer = vec![Vec3::new(0.0, 0.0, 0.0); unknowns];
    let (mut over, mut product_over) = (vec![0.0; count], vec![0.0; count]);
    for axis in 0..3 {
        let of = |p: &Vec3| [p.x, p.y, p.z][axis];
        let mut right_over = vec![0.0; count];
        for ((start, shares), gap) in rows.blends().zip(&rows.gaps) {
            if let Some(r) = right_over.get_mut(start..start + shares.len()) {
                for (r, w) in r.iter_mut().zip(shares) {
                    *r += w * of(gap);
                }
            }
        }
        for &[a, b, c] in lines {
            let (a, b, c) = (a as usize, b as usize, c as usize);
            let bend = fair * (of(&now[a]) - 2.0 * of(&now[b]) + of(&now[c]));
            right_over[a] -= bend;
            right_over[b] += 2.0 * bend;
            right_over[c] -= bend;
        }
        let mut residual = vec![0.0; unknowns];
        gather(blends, &right_over, &mut residual);
        let mut y = vec![0.0; unknowns];
        let mut direction: Vec<f64> = residual.iter().zip(&reduced).map(|(r, d)| r / d).collect();
        let mut scaled: f64 = residual.iter().zip(&direction).map(|(r, z)| r * z).sum();
        let start = scaled;
        let mut product = vec![0.0; unknowns];
        for _ in 0..STEPS {
            if !(start > 0.0) || scaled <= CLOSE * CLOSE * start {
                break;
            }
            spread(blends, &direction, &mut over);
            apply(rows, lines, (fair, stay), &over, &mut product_over);
            gather(blends, &product_over, &mut product);
            let along: f64 = direction.iter().zip(&product).map(|(p, q)| p * q).sum();
            if !(along > 0.0) {
                break;
            }
            let step = scaled / along;
            for ((y, r), (d, p)) in y.iter_mut().zip(&mut residual).zip(direction.iter().zip(&product)) {
                *y += step * d;
                *r -= step * p;
            }
            let next: f64 = residual.iter().zip(&reduced).map(|(r, d)| r * r / d).sum();
            let turn = next / scaled;
            for ((d, r), m) in direction.iter_mut().zip(&residual).zip(&reduced) {
                *d = r / m + turn * *d;
            }
            scaled = next;
        }
        for (a, v) in answer.iter_mut().zip(&y) {
            match axis {
                0 => a.x = *v,
                1 => a.y = *v,
                _ => a.z = *v,
            }
        }
    }
    answer
}

/// The gap from each place of the patches to the nearest point of the scan, for these cage
/// points; with the patches' points as the cage makes them.
fn measured<'a>(
    parts: &'a [Part],
    tables: &'a [Shares],
    blends: &[&[(u32, f64)]],
    cage: &[Vec3],
    scan: &Scan,
    tree: &TriangleTree,
) -> (Rows<'a>, Vec<Vec3>) {
    let now: Vec<Vec3> = blends
        .iter()
        .map(|blend| {
            let mut p = Vec3::new(0.0, 0.0, 0.0);
            for &(k, w) in *blend {
                if let Some(q) = cage.get(k as usize) {
                    (p.x, p.y, p.z) = (p.x + w * q.x, p.y + w * q.y, p.z + w * q.z);
                }
            }
            p
        })
        .collect();
    let mut rows = Rows { parts, tables, of: Vec::new(), gaps: Vec::new() };
    for (i, part) in parts.iter().enumerate() {
        let count = part.side * part.side;
        let (Some(table), Some(points)) = (tables.get(part.table), now.get(part.start..part.start + count)) else { continue };
        for (place, shares) in table.shares.chunks_exact(count).enumerate() {
            let mut p = Vec3::new(0.0, 0.0, 0.0);
            for (w, q) in shares.iter().zip(points) {
                (p.x, p.y, p.z) = (p.x + w * q.x, p.y + w * q.y, p.z + w * q.z);
            }
            let Some([a, b, c]) = tree.nearest(p).and_then(|(_, t)| scan.corners(t as usize)) else { continue };
            let q = closest_on_triangle(p, a, b, c);
            rows.of.push((i as u32, place as u32));
            rows.gaps.push(Vec3::new(q.x - p.x, q.y - p.y, q.z - p.z));
        }
    }
    (rows, now)
}

/// The root mean square of the gaps, and the largest.
fn spread_of(gaps: &[Vec3]) -> (f64, f64) {
    let squares: Vec<f64> = gaps.iter().map(|g| g.x * g.x + g.y * g.y + g.z * g.z).collect();
    let mean = squares.iter().sum::<f64>() / squares.len().max(1) as f64;
    (mean.sqrt(), squares.iter().fold(0.0_f64, |m, s| m.max(*s)).sqrt())
}

/// `cage` fitted to `scan`: its points moved so that `patches` (the cage's own, from
/// [`Cage::patches`]) lie on the scan in least squares. See the module.
pub fn fit(cage: &Cage, patches: &Patches, scan: &Scan, settings: &FitSettings) -> Result<CageFit, FitError> {
    if settings.rounds == 0 || settings.places == 0 {
        return Err(FitError::BadSettings);
    }
    if scan.triangle_count() == 0 {
        return Err(FitError::NoScan);
    }
    if patches.patches.is_empty() {
        return Err(FitError::NoPatches);
    }
    // The patches' points one after another, each a blend of cage points; the tables of shares,
    // one for each degree met; and the second differences of every net.
    let mut tables: Vec<Shares> = Vec::new();
    let (mut parts, mut blends, mut lines) = (Vec::with_capacity(patches.patches.len()), Vec::new(), Vec::new());
    for patch in &patches.patches {
        let table = match tables.iter().position(|t| t.degree == patch.degree) {
            Some(found) => found,
            None => {
                tables.push(Shares::of(patch.degree, settings.places));
                tables.len() - 1
            }
        };
        let (start, side) = (blends.len(), patch.degree + 1);
        if patch.blends.len() != side * side {
            continue;
        }
        blends.extend(patch.blends.iter().map(Vec::as_slice));
        second_differences(start, side, &mut lines);
        parts.push(Part { start, side, table });
    }
    let tree = TriangleTree::of(scan.positions, scan.triangles);
    let mut points = cage.points().to_vec();
    let (mut rows, mut now) = measured(&parts, &tables, &blends, &points, scan, &tree);
    let (rms_before, _) = spread_of(&rows.gaps);
    let (mut rms, mut worst) = spread_of(&rows.gaps);
    let mut rounds = 0;
    for _ in 0..settings.rounds {
        // Nothing measured, or the patches already on the scan to rounding: nothing to gain.
        if rows.gaps.is_empty() || !(rms > ON_IT * scan.extent()) {
            break;
        }
        let step = moves(&rows, &lines, &now, &blends, points.len());
        let moved: Vec<Vec3> = points.iter().zip(&step).map(|(p, d)| Vec3::new(p.x + d.x, p.y + d.y, p.z + d.z)).collect();
        let (next_rows, next_now) = measured(&parts, &tables, &blends, &moved, scan, &tree);
        let (next_rms, next_worst) = spread_of(&next_rows.gaps);
        // A round that leaves the patches farther from the scan is not taken.
        if !(next_rms < rms) {
            break;
        }
        let gain = (rms - next_rms) / rms;
        (points, rows, now, rms, worst) = (moved, next_rows, next_now, next_rms, next_worst);
        rounds += 1;
        if gain < GAIN {
            break;
        }
    }
    Ok(CageFit { points, rms, worst, rms_before, places: rows.gaps.len(), rounds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cage::kit::{ball, icosahedral_ball, length, shape, torus, worst_join};
    use crate::cage::{Cage, CornerBases};
    use std::collections::HashMap;

    /// The shape bench's bounds: the corner construction gave 0.165 at the three-faced corners
    /// and 0.144 at the five-faced ones (rms 6.1e-5, regular faces 0.058) on 2026-11-19; with the
    /// corner point set to the Catmull and Clark limit point from five faces (2026-11-20), 0.165
    /// and 0.060 (regular 0.054). A better construction lowers these; the plan's bar is 0.05.
    const BENCH_THREE: f64 = 0.18;
    const BENCH_FIVE: f64 = 0.07;

    /// The triangles of a cage's faces, each cut in two: a scan of whatever the cage's points
    /// lie on, as fine as the cage.
    fn as_scan(cage: &Cage) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        (cage.points().to_vec(), cage.faces().iter().flat_map(|f| [[f[0], f[1], f[2]], [f[0], f[2], f[3]]]).collect())
    }

    /// The two principal curvatures at a place, from its curvature tensor.
    fn principal(tensor: &[[f64; 3]; 3]) -> (f64, f64) {
        let trace = tensor[0][0] + tensor[1][1] + tensor[2][2];
        // The tensor is symmetric, so the trace of its square is the sum of its squares.
        let squares: f64 = tensor.iter().flatten().map(|x| x * x).sum();
        let product = 0.5 * (trace * trace - squares);
        let root = (trace * trace - 4.0 * product).max(0.0).sqrt();
        (0.5 * (trace - root), 0.5 * (trace + root))
    }

    /// A ball's cage starts well inside a fine scan of the unit sphere (a B-spline lies inside
    /// its control points). The fit brings its patches onto the sphere, and they still meet in
    /// curvature across every edge, the three-faced corners included. Measured when written:
    /// 0.044 off before, 0.0007 after two rounds, the worst place 0.002 off.
    #[test]
    fn a_ball_cage_is_brought_onto_a_sphere_and_still_meets_in_curvature() {
        let cage = ball(4);
        let (positions, triangles) = as_scan(&ball(48));
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        let fit = fit(&cage, &patches, &scan, &FitSettings::default()).unwrap();
        assert_eq!(fit.places, 96 * 36);
        assert!(fit.rms_before > 0.02, "the cage starts well inside: {}", fit.rms_before);
        assert!(fit.rms < 2e-3 && fit.worst < 5e-3, "rms {:e}, worst {:e} after {} rounds", fit.rms, fit.worst, fit.rounds);
        // The same cage, moved: its patches are the same blends of its points.
        let moved = Cage::new(fit.points.clone(), cage.faces().to_vec()).unwrap();
        let (gap, facing, curving, _) = worst_join(&moved, &patches);
        assert!(gap < 1e-9 && facing < 1e-9 && curving < 1e-7, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
        // On the true sphere within half a hundredth, and round everywhere: no place dents in.
        for patch in &patches.patches {
            let points = patch.points(moved.points());
            for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
                for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let (place, _, tensor) = shape(&points, patch.degree, (u, v));
                    assert!((length(place) - 1.0).abs() < 5e-3, "face {}: {} from the centre", patch.face, length(place));
                    // The faces look outward, so a ball curves below nothing both ways.
                    let (least, most) = principal(&tensor);
                    assert!(least < 0.0 && most < 0.0, "face {} at ({u}, {v}): curves {least} and {most}", patch.face);
                }
            }
        }
    }

    /// How truly a fitted ball curves, a fifth in from each face's edges, against a unit
    /// sphere's curvature of one. Measured when written, on a cage of 384 faces: the bicubic
    /// faces within 5.4%, the faces at the three-faced corners within 13.2%.
    ///
    /// The corners' figure does not come down with a finer cage or a longer fit (it was 13.3%
    /// on 216 faces and 15.8% on 864): it is this corner construction's own shape where three
    /// faces meet, which its authors name. A corner construction of better shape should bring
    /// the second bound here down; the surfacing plan's bar for it is 5%.
    #[test]
    fn a_fitted_ball_curves_nearly_as_a_sphere_less_truly_at_its_three_faced_corners() {
        let cage = ball(8);
        let (positions, triangles) = as_scan(&ball(64));
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        let fit = fit(&cage, &patches, &scan, &FitSettings::default()).unwrap();
        assert!(fit.rms < 1e-4, "rms {:e}", fit.rms);
        let moved = Cage::new(fit.points, cage.faces().to_vec()).unwrap();
        let mut off = [0.0_f64; 2];
        for patch in &patches.patches {
            let points = patch.points(moved.points());
            for i in 2..=8 {
                for j in 2..=8 {
                    let (_, _, tensor) = shape(&points, patch.degree, (f64::from(i) / 10.0, f64::from(j) / 10.0));
                    let (least, most) = principal(&tensor);
                    let kind = usize::from(patch.degree == 7);
                    off[kind] = off[kind].max((least + 1.0).abs()).max((most + 1.0).abs());
                }
            }
        }
        assert!(off[0] < 0.08, "the bicubic faces curve {} off a unit sphere's", off[0]);
        assert!(off[1] < 0.2, "the corners' faces curve {} off a unit sphere's", off[1]);
    }

    /// The shape bench of the surfacing plan (item 1 of its next steps): a ball with both kinds
    /// of irregular corner, five faces meeting at 12 and three at 20, fitted to a fine scan of
    /// the unit sphere, and how far each kind of face curves off the sphere's curvature of one, a
    /// fifth in from its edges. It prints the three figures (regular, three-faced, five-faced)
    /// for a corner construction to be judged on; the plan's bar for both corners is 5 %. Today's
    /// construction meets in curvature everywhere (the bounds on the joins hold whatever the
    /// shape), and the figures are held to what it gave when written, so a worse one shows.
    #[test]
    fn the_shape_bench_a_ball_with_three_and_five_faced_corners() {
        let cage = icosahedral_ball(4);
        let (positions, triangles) = as_scan(&icosahedral_ball(16));
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert!(patches.left_out.is_empty(), "every face has a patch: {:?}", patches.left_out);
        let (gap, facing, curving, _) = worst_join(&cage, &patches);
        assert!(gap < 1e-9 && facing < 1e-9 && curving < 1e-7, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
        let fit = fit(&cage, &patches, &scan, &FitSettings::default()).unwrap();
        let moved = Cage::new(fit.points, cage.faces().to_vec()).unwrap();
        let mut valence: HashMap<u32, usize> = HashMap::new();
        for face in cage.faces() {
            for &v in face {
                *valence.entry(v).or_default() += 1;
            }
        }
        // off[0] the regular faces, off[1] those at a three-faced corner, off[2] at a five-faced one.
        let mut off = [0.0_f64; 3];
        let mut seen = [0_usize; 3];
        for patch in &patches.patches {
            let corner = cage.faces()[patch.face].iter().map(|v| valence[v]).find(|&n| n != 4);
            let kind = match corner {
                None => 0,
                Some(3) => 1,
                Some(_) => 2,
            };
            seen[kind] += 1;
            let points = patch.points(moved.points());
            for i in 2..=8 {
                for j in 2..=8 {
                    let (_, _, tensor) = shape(&points, patch.degree, (f64::from(i) / 10.0, f64::from(j) / 10.0));
                    let (least, most) = principal(&tensor);
                    off[kind] = off[kind].max((least + 1.0).abs()).max((most + 1.0).abs());
                }
            }
        }
        eprintln!(
            "shape bench: rms {:e}; curvature off a sphere's: regular {:.4}, three-faced {:.4}, five-faced {:.4} ({seen:?} faces)",
            fit.rms, off[0], off[1], off[2]
        );
        assert_eq!(seen[1..], [20 * 3, 12 * 5], "a patch at each face of each corner");
        assert!(fit.rms < 1e-3, "rms {:e}", fit.rms);
        assert!(off[0] < 0.08, "the bicubic faces curve {} off", off[0]);
        assert!(off[1] < BENCH_THREE && off[2] < BENCH_FIVE, "three-faced {}, five-faced {}", off[1], off[2]);
    }

    /// A torus has no irregular corner: its cage is one B-spline surface, and the fit brings it
    /// onto a fine scan of the torus.
    #[test]
    fn a_torus_cage_is_brought_onto_a_torus() {
        let cage = torus(16, 8);
        let (positions, triangles) = as_scan(&torus(96, 48));
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        let fit = fit(&cage, &patches, &scan, &FitSettings::default()).unwrap();
        // Measured when written: 0.119 off before, 0.001 after five rounds.
        assert!(fit.rms < fit.rms_before / 20.0 && fit.rms < 3e-3, "rms {:e} from {:e} after {} rounds", fit.rms, fit.rms_before, fit.rounds);
        let moved = Cage::new(fit.points, cage.faces().to_vec()).unwrap();
        let (gap, facing, curving, _) = worst_join(&moved, &patches);
        assert!(gap < 1e-11 && facing < 1e-11 && curving < 1e-9, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
    }

    /// A cage already on its scan stays there: nothing asks it to move.
    #[test]
    fn a_cage_on_a_flat_scan_stays_put() {
        let n = 8_u32;
        let grid = |n: u32| -> (Vec<Vec3>, Vec<[u32; 4]>) {
            let points = (0..n * n).map(|k| Vec3::new(f64::from(k % n), f64::from(k / n), 0.0)).collect();
            let faces = (0..n - 1).flat_map(|j| (0..n - 1).map(move |i| [j * n + i, j * n + i + 1, (j + 1) * n + i + 1, (j + 1) * n + i])).collect();
            (points, faces)
        };
        let (points, faces) = grid(n);
        let cage = Cage::new(points.clone(), faces).unwrap();
        let (positions, triangles) = as_scan(&cage);
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        let fit = fit(&cage, &patches, &scan, &FitSettings::default()).unwrap();
        assert!(fit.rms_before < 1e-12 && fit.rounds == 0, "{:e} off before, {} rounds", fit.rms_before, fit.rounds);
        for (was, is) in points.iter().zip(&fit.points) {
            assert!(length([is.x - was.x, is.y - was.y, is.z - was.z]) < 1e-12);
        }
    }

    #[test]
    fn what_cannot_be_fitted_is_refused() {
        let cage = ball(3);
        let (positions, triangles) = as_scan(&cage);
        let scan = Scan::new(&positions, &triangles);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        let none = Scan::new(&positions, &[]);
        assert_eq!(fit(&cage, &patches, &none, &FitSettings::default()).unwrap_err(), FitError::NoScan);
        assert_eq!(fit(&cage, &Patches::default(), &scan, &FitSettings::default()).unwrap_err(), FitError::NoPatches);
        assert_eq!(fit(&cage, &patches, &scan, &FitSettings { rounds: 0, places: 6 }).unwrap_err(), FitError::BadSettings);
    }
}
