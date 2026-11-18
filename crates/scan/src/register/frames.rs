//! Starts for two passes that overlap only along a seam (a van's walls and its ceiling), found
//! from their flat faces. A part made of a few flat faces has a frame: the three directions most
//! of its surface faces. Laying one pass's frame on the other's takes one of 24 turns (which face
//! is which, and which way round); with the turn fixed, the move that lays the faces on each
//! other is found for every shift at once by correlating how the two passes fill a grid of cells,
//! one grid for each way a face can look, with a Fourier transform. The few best shifts of the
//! best turns are starts for the fit.
//!
//! This is the approach of registering structured scenes by their dominant surface directions
//! for the turn and a correlation of the passes for the move, rather than by matching points: a
//! thin overlap has too few distinctive points to match, but its flat faces still agree.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use super::{Surface, frame_move, normals_at, sample};
use crate::par;
use crate::place::Rigid;

/// Points each pass is read at.
const POINTS: usize = 60_000;
/// The most cells of the grid the correlation works on (a power of two on each side).
#[cfg(not(target_arch = "wasm32"))]
const CELLS: usize = 1 << 21;
#[cfg(target_arch = "wasm32")]
const CELLS: usize = 1 << 18;
/// The cosine a normal must reach with a frame direction to count for that face.
const LOOKS: f64 = 0.85;
/// The cosine two directions of one frame must stay under (70°), and the cosine within which
/// normals are one direction when it is refined.
const APART: f64 = 0.35;
const ONE_DIRECTION: f64 = 0.985;
/// The least share of a pass's points each direction of its frame must have.
const LEAST_SHARE: f64 = 0.02;
/// Starts handed back, and the correlation peaks taken from each turn.
const MOST_STARTS: usize = 24;
const PEAKS: usize = 3;

#[derive(Clone, Copy, Default)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    fn mul(self, o: C) -> C {
        C { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

/// The twiddle factors of a transform of `n` points (a power of two).
struct Plan {
    n: usize,
    forward: Vec<C>,
}

impl Plan {
    fn new(n: usize) -> Self {
        let forward = (0..n / 2)
            .map(|k| {
                let (s, c) = (-2.0 * std::f64::consts::PI * k as f64 / n as f64).sin_cos();
                C { re: c, im: s }
            })
            .collect();
        Plan { n, forward }
    }

    /// The transform of `line` in place, unscaled; `inverse` runs it the other way.
    fn run(&self, line: &mut [C], inverse: bool) {
        let n = self.n;
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                line.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let mut w = self.forward[k * step];
                    if inverse {
                        w.im = -w.im;
                    }
                    let (a, b) = (line[start + k], line[start + k + len / 2].mul(w));
                    line[start + k] = C { re: a.re + b.re, im: a.im + b.im };
                    line[start + k + len / 2] = C { re: a.re - b.re, im: a.im - b.im };
                }
            }
            len <<= 1;
        }
    }
}

/// The transform of a `dims`-sized array (z fastest) along all three axes, unscaled.
fn fft3(data: &mut [C], dims: [usize; 3], plans: &[Plan; 3], inverse: bool) {
    for axis in 0..3 {
        let stride = [dims[1] * dims[2], dims[2], 1][axis];
        let n = dims[axis];
        let bases: Vec<usize> = (0..dims[0] * dims[1] * dims[2]).filter(|i| (i / stride).is_multiple_of(n)).collect();
        let done = {
            let read: &[C] = data;
            par::map(bases.len(), |l| {
                let mut line: Vec<C> = (0..n).map(|k| read[bases[l] + k * stride]).collect();
                plans[axis].run(&mut line, inverse);
                line
            })
        };
        for (base, line) in bases.iter().zip(done) {
            for (k, c) in line.into_iter().enumerate() {
                data[base + k * stride] = c;
            }
        }
    }
}

/// The three directions most of the surface faces, as a right-handed frame, or `None` when the
/// surface has fewer than three (a turned part, a ball).
fn dominant_frame(normals: &[Vec3]) -> Option<[Vec3; 3]> {
    let usable: Vec<Vec3> = normals.iter().copied().filter(|n| n.len2() > 0.5).collect();
    if usable.len() < 200 {
        return None;
    }
    let mut found: Vec<Vec3> = Vec::new();
    for _ in 0..3 {
        let rest: Vec<Vec3> = usable.iter().copied().filter(|n| found.iter().all(|f| n.dot(*f).abs() < APART)).collect();
        // The direction, either way round, as a cell of a grid of sixths.
        let cell = |n: Vec3| -> [i32; 3] {
            let n = if [n.x, n.y, n.z].iter().copied().fold(0.0f64, |m, c| if c.abs() > m.abs() { c } else { m }) < 0.0 { -n } else { n };
            [(n.x * 6.0).round() as i32, (n.y * 6.0).round() as i32, (n.z * 6.0).round() as i32]
        };
        let mut votes: HashMap<[i32; 3], u32> = HashMap::new();
        for n in &rest {
            *votes.entry(cell(*n)).or_default() += 1;
        }
        let around = |k: &[i32; 3]| -> u32 {
            (-1..=1)
                .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| [k[0] + x, k[1] + y, k[2] + z])))
                .filter_map(|c| votes.get(&c))
                .sum()
        };
        let (key, _) = votes.keys().map(|k| (*k, around(k))).max_by_key(|&(k, v)| (v, std::cmp::Reverse(k)))?;
        let mut d = Vec3::new(f64::from(key[0]), f64::from(key[1]), f64::from(key[2])).normalized()?;
        let mut members = 0;
        for _ in 0..4 {
            let mut sum = Vec3::ZERO;
            members = 0;
            for n in &rest {
                let along = n.dot(d);
                if along.abs() > ONE_DIRECTION {
                    sum += if along < 0.0 { -*n } else { *n };
                    members += 1;
                }
            }
            d = sum.normalized()?;
        }
        if (members as f64) < LEAST_SHARE * usable.len() as f64 {
            return None;
        }
        found.push(d);
    }
    let e1 = found[0];
    let e2 = (found[1] - e1 * found[1].dot(e1)).normalized()?;
    Some([e1, e2, e1.cross(e2)])
}

/// The channel of a normal in a frame's axes: which axis it looks along and which way, or `None`
/// for a face that looks along none.
fn channel(n: [f64; 3]) -> Option<usize> {
    let k = (0..3).max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))?;
    (n[k].abs() >= LOOKS).then_some(k * 2 + usize::from(n[k] < 0.0))
}

/// The 24 ways to lay one right-handed frame on another: each axis of the first on an axis of
/// the second, one way or the other.
fn turns() -> Vec<([usize; 3], [f64; 3])> {
    let mut all = Vec::new();
    for (p, parity) in [([0, 1, 2], 1.0), ([0, 2, 1], -1.0), ([1, 0, 2], -1.0), ([1, 2, 0], 1.0), ([2, 0, 1], 1.0), ([2, 1, 0], -1.0)] {
        for s in [
            [1.0, 1.0, 1.0],
            [1.0, 1.0, -1.0],
            [1.0, -1.0, 1.0],
            [1.0, -1.0, -1.0],
            [-1.0, 1.0, 1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [-1.0, -1.0, -1.0],
        ] {
            if s[0] * s[1] * s[2] * parity > 0.0 {
                all.push((p, s));
            }
        }
    }
    all
}

/// A point in the frame `f`.
fn in_frame(p: Vec3, f: &[Vec3; 3]) -> [f64; 3] {
    [p.dot(f[0]), p.dot(f[1]), p.dot(f[2])]
}

/// The starts that lay `moving` on `reference` by their flat faces, best first, with the width of
/// a cell of the grid they were found on (a start is that near).
pub(super) fn starts(moving: &Surface, reference: &Surface) -> (Vec<Rigid>, f64) {
    let (pm, pr) = (sample(&moving.scan, POINTS), sample(&reference.scan, POINTS));
    let (nm, nr) = (normals_at(&pm, moving), normals_at(&pr, reference));
    let (Some(fm), Some(fr)) = (dominant_frame(&nm), dominant_frame(&nr)) else { return (Vec::new(), 0.0) };
    // The reference in its own frame's axes.
    let reference_laid: Vec<([f64; 3], usize)> =
        pr.iter().zip(&nr).filter_map(|(p, n)| Some((in_frame(*p, &fr), channel(in_frame(*n, &fr))?))).collect();
    let moving_in: Vec<([f64; 3], [f64; 3])> = pm.iter().zip(&nm).map(|(p, n)| (in_frame(*p, &fm), in_frame(*n, &fm))).collect();
    if reference_laid.len() < 100 || moving_in.len() < 100 {
        return (Vec::new(), 0.0);
    }
    let range = |pts: &mut dyn Iterator<Item = [f64; 3]>| -> ([f64; 3], [f64; 3]) {
        pts.fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k]))))
    };
    let (lo_r, hi_r) = range(&mut reference_laid.iter().map(|(p, _)| *p));
    let (lo_m, hi_m) = range(&mut moving_in.iter().map(|(p, _)| *p));
    let reach_r: [f64; 3] = std::array::from_fn(|k| hi_r[k] - lo_r[k]);
    let reach_m: f64 = (0..3).map(|k| hi_m[k] - lo_m[k]).fold(0.0, f64::max);
    // The cell width: the finest at which the grid, with room for either pass shifted over the
    // other, stays within `CELLS`.
    let sides = |cell: f64| -> [usize; 3] { std::array::from_fn(|k| (((reach_r[k] + reach_m) / cell).ceil() as usize + 3).next_power_of_two()) };
    let mut cell = (reach_r.iter().sum::<f64>() + 3.0 * reach_m) / 3.0 / 64.0;
    if !(cell > 0.0 && cell.is_finite()) {
        return (Vec::new(), 0.0);
    }
    while sides(cell).iter().product::<usize>() > CELLS {
        cell *= 1.1;
    }
    let dims = sides(cell);
    let plans = [Plan::new(dims[0]), Plan::new(dims[1]), Plan::new(dims[2])];
    let total = dims[0] * dims[1] * dims[2];
    let at = |idx: [usize; 3]| (idx[0] * dims[1] + idx[1]) * dims[2] + idx[2];
    let cell_of =
        |p: [f64; 3], lo: [f64; 3]| -> [usize; 3] { std::array::from_fn(|k| (((p[k] - lo[k]) / cell).floor().max(0.0) as usize).min(dims[k] - 1)) };

    // The reference's spectra, one for each way a face looks.
    let mut filled: Vec<Vec<usize>> = vec![Vec::new(); 6];
    for (p, ch) in &reference_laid {
        filled[*ch].push(at(cell_of(*p, lo_r)));
    }
    let spectra: Vec<Option<Vec<C>>> = filled
        .iter()
        .map(|cells| {
            if cells.is_empty() {
                return None;
            }
            let mut grid = vec![C::default(); total];
            for &c in cells {
                grid[c].re = 1.0;
            }
            fft3(&mut grid, dims, &plans, false);
            Some(grid)
        })
        .collect();

    let mut peaks: Vec<(f64, usize, usize)> = Vec::new();
    let mut shifts: Vec<[f64; 3]> = Vec::new();
    for (turn, (perm, signs)) in turns().into_iter().enumerate() {
        // The moving pass in the reference's axes: its axis i is the reference's axis perm[i], and
        // `signs[i]` times it.
        let to_axes = |v: [f64; 3]| -> [f64; 3] {
            let mut out = [0.0; 3];
            for i in 0..3 {
                out[perm[i]] = signs[i] * v[i];
            }
            out
        };
        let laid: Vec<([f64; 3], usize)> = moving_in.iter().filter_map(|(p, n)| Some((to_axes(*p), channel(to_axes(*n))?))).collect();
        if laid.len() < 100 {
            continue;
        }
        let (lo, _) = range(&mut laid.iter().map(|(p, _)| *p));
        let mut cells: Vec<std::collections::BTreeSet<usize>> = vec![Default::default(); 6];
        for (p, ch) in &laid {
            cells[*ch].insert(at(cell_of(*p, lo)));
        }
        let occupied: usize = cells.iter().map(|c| c.len()).sum();
        let mut sum = vec![C::default(); total];
        for (ch, set) in cells.iter().enumerate() {
            let (Some(spectrum), false) = (&spectra[ch], set.is_empty()) else { continue };
            let mut grid = vec![C::default(); total];
            for &c in set {
                grid[c].re = 1.0;
            }
            fft3(&mut grid, dims, &plans, false);
            for ((s, a), b) in sum.iter_mut().zip(spectrum).zip(&grid) {
                let p = a.mul(C { re: b.re, im: -b.im });
                s.re += p.re;
                s.im += p.im;
            }
        }
        fft3(&mut sum, dims, &plans, true);
        // The best shifts that put the moving pass inside the grid, apart from each other.
        let valid = |i: usize| -> Option<[i64; 3]> {
            let idx = [i / (dims[1] * dims[2]), (i / dims[2]) % dims[1], i % dims[2]];
            let mut t = [0i64; 3];
            for k in 0..3 {
                let reach = ((reach_r[k] / cell).floor() as usize) + 1;
                t[k] = if idx[k] < reach { idx[k] as i64 } else { idx[k] as i64 - dims[k] as i64 };
                let moving_cells = (((reach_m) / cell).ceil() as i64) + 1;
                if t[k] < -moving_cells {
                    return None;
                }
            }
            Some(t)
        };
        let mut best: Vec<usize> = (0..total).collect();
        let keep = 400.min(total);
        best.select_nth_unstable_by(keep - 1, |a, b| sum[*b].re.total_cmp(&sum[*a].re));
        best.truncate(keep);
        best.sort_by(|a, b| sum[*b].re.total_cmp(&sum[*a].re));
        let mut taken: Vec<[i64; 3]> = Vec::new();
        for i in best {
            let Some(t) = valid(i) else { continue };
            if taken.iter().any(|q| (0..3).all(|k| (q[k] - t[k]).abs() <= 3)) {
                continue;
            }
            taken.push(t);
            let score = sum[i].re / total as f64 / occupied.max(1) as f64;
            let shift: [f64; 3] = std::array::from_fn(|k| lo_r[k] - lo[k] + t[k] as f64 * cell);
            peaks.push((score, turn, shifts.len()));
            shifts.push(shift);
            if taken.len() >= PEAKS {
                break;
            }
        }
    }
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    let all_turns = turns();
    let mut out = Vec::new();
    for (_, turn, shift) in peaks.into_iter().take(MOST_STARTS) {
        let (perm, signs) = all_turns[turn];
        // Each axis of the moving frame goes where the turn puts it, in the reference's axes.
        let f: [Vec3; 3] = std::array::from_fn(|i| fr[perm[i]] * signs[i]);
        let d = shifts[shift];
        let rotation = frame_move(fm, f, Vec3::ZERO, Vec3::ZERO);
        let translation = fr[0] * d[0] + fr[1] * d[1] + fr[2] * d[2];
        out.push(Rigid { translation, ..rotation });
    }
    (out, cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_of_a_line_is_the_discrete_transform() {
        let n = 8;
        let plan = Plan::new(n);
        let input: Vec<C> = (0..n).map(|i| C { re: (i * i % 5) as f64, im: 0.5 * i as f64 }).collect();
        let mut line = input.clone();
        plan.run(&mut line, false);
        for (k, got) in line.iter().enumerate() {
            let (mut re, mut im) = (0.0, 0.0);
            for (j, x) in input.iter().enumerate() {
                let (s, c) = (-2.0 * std::f64::consts::PI * (j * k) as f64 / n as f64).sin_cos();
                re += x.re * c - x.im * s;
                im += x.re * s + x.im * c;
            }
            assert!((got.re - re).abs() < 1e-9 && (got.im - im).abs() < 1e-9, "bin {k}: {got:?} vs {re} {im}", got = (got.re, got.im));
        }
        plan.run(&mut line, true);
        for (a, b) in line.iter().zip(&input) {
            assert!((a.re / n as f64 - b.re).abs() < 1e-9 && (a.im / n as f64 - b.im).abs() < 1e-9);
        }
    }

    #[test]
    fn a_frame_is_read_from_the_faces_of_a_block() {
        let mut normals = Vec::new();
        for (n, count) in
            [(Vec3::new(0.0, 0.0, 1.0), 500), (Vec3::new(1.0, 0.0, 0.0), 300), (Vec3::new(0.0, 1.0, 0.0), 200), (Vec3::new(0.0, 0.0, -1.0), 400)]
        {
            normals.extend(std::iter::repeat_n(n, count));
        }
        let frame = dominant_frame(&normals).expect("a frame");
        assert!(frame[0].dot(Vec3::Z).abs() > 0.999 && frame[1].dot(Vec3::X).abs() > 0.999 && frame[2].dot(Vec3::Y).abs() > 0.999, "{frame:?}");
        assert!(frame[0].cross(frame[1]).dot(frame[2]) > 0.999, "right-handed");
    }

    #[test]
    fn a_surface_with_one_direction_has_no_frame() {
        assert!(dominant_frame(&vec![Vec3::Z; 1000]).is_none());
    }
}
