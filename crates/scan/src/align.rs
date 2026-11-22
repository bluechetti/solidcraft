//! Aligning scans beyond one pass onto another, after SurfaceCraft's: by points picked in pairs
//! (the one turn and shift that lays them on each other best, with what is left of each pair),
//! and several passes of one part at once. For the passes, each one not yet placed is laid onto
//! those that are (from wherever it lies, as Align to Scan lays one), then every overlapping pair
//! is fitted again where they lie, and one move for each pass is found that agrees with all those
//! fits as well as it can (a pose graph), so the errors of a chain of passes are shared out
//! rather than piled onto the last.

use std::collections::HashSet;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::place::Rigid;
use crate::register::{self, Settings, Surface};

/// Why scans could not be aligned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlignError {
    /// Fewer than three pairs of points.
    TooFewPairs,
    /// The points lie along one line, so they leave a turn about it free.
    InALine,
    /// Fewer than two passes.
    TooFewPasses,
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AlignError::TooFewPairs => "pick at least three pairs of points",
            AlignError::InALine => "the points lie along one line: pick one well off it",
            AlignError::TooFewPasses => "align needs two passes or more",
        })
    }
}

impl std::error::Error for AlignError {}

/// The move that lays picked points onto the points they correspond to.
#[derive(Clone, Debug, PartialEq)]
pub struct ByPairs {
    pub moved: Rigid,
    /// How far each point lies from its partner after the move, in the order given.
    pub residuals: Vec<f64>,
    pub rms: f64,
}

impl ByPairs {
    /// The pair that fits worst.
    pub fn worst(&self) -> usize {
        (0..self.residuals.len()).max_by(|&a, &b| self.residuals[a].total_cmp(&self.residuals[b])).unwrap_or(0)
    }
}

/// The rigid move (no scale, no mirror) that lays `from` onto `to`, pair by pair, by least
/// squares; the longer list is cut to the shorter.
pub fn by_pairs(from: &[Vec3], to: &[Vec3]) -> Result<ByPairs, AlignError> {
    let n = from.len().min(to.len());
    if n < 3 {
        return Err(AlignError::TooFewPairs);
    }
    let (from, to) = (&from[..n], &to[..n]);
    let (cs, ct) = (centroid(from), centroid(to));
    // A = Σ t sᵀ; the turn is U Vᵀ from A's singular directions, read off AᵀA.
    let mut a = [[0.0f64; 3]; 3];
    for (s, t) in from.iter().zip(to) {
        let (s, t) = (comps(*s - cs), comps(*t - ct));
        for (i, row) in a.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += t[i] * s[j];
            }
        }
    }
    let ata: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[k][i] * a[k][j]).sum()));
    let (values, axes) = solidcraft_geom::inertia::principal(&ata, 0.0).ok_or(AlignError::InALine)?;
    // Largest first.
    let sigma = [values[2].max(0.0).sqrt(), values[1].max(0.0).sqrt()];
    if !(sigma[0] > 1e-12 && sigma[1] > 1e-7 * sigma[0]) {
        return Err(AlignError::InALine);
    }
    let (v1, v2) = (axes[2].normalized().ok_or(AlignError::InALine)?, axes[1].normalized().ok_or(AlignError::InALine)?);
    let times = |m: &[[f64; 3]; 3], v: Vec3| {
        Vec3::new(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
        )
    };
    let (u1, u2) = (times(&a, v1) / sigma[0], times(&a, v2) / sigma[1]);
    let (v3, u3) = (v1.cross(v2), u1.cross(u2));
    let (u, v) = ([u1, u2, u3].map(comps), [v1, v2, v3].map(comps));
    let rotation: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| u[k][i] * v[k][j]).sum()));
    let turn = Rigid { rotation, translation: Vec3::ZERO };
    let moved = Rigid { translation: ct - turn.turned(cs), ..turn };
    let residuals: Vec<f64> = from.iter().zip(to).map(|(s, t)| moved.apply(*s).dist(*t)).collect();
    let rms = (residuals.iter().map(|r| r * r).sum::<f64>() / n as f64).sqrt();
    Ok(ByPairs { moved, residuals, rms })
}

fn centroid(points: &[Vec3]) -> Vec3 {
    points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len().max(1) as f64
}

fn comps(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// How the passes are aligned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllSettings {
    /// Place a pass that does not yet lie on the others from wherever it lies (as Align to Scan
    /// searches), before the passes are fitted together.
    pub search: bool,
    /// The least share of a pass two passes must have in common to be fitted to each other.
    pub least_overlap: f64,
}

impl Default for AllSettings {
    fn default() -> Self {
        AllSettings { search: true, least_overlap: 0.1 }
    }
}

/// A pair of passes and what fitting them together found.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    /// The share of one lying on the other before and after, and the typical gap after.
    pub overlap_before: f64,
    pub overlap_after: f64,
    pub rms_after: f64,
    /// Whether it took part in the solution, and why not when it did not.
    pub accepted: bool,
    pub note: Option<String>,
    /// How far the pair's own fit still is from what the solution does to them (mm, degrees).
    pub disagreement: Option<(f64, f64)>,
}

/// Every pass aligned.
#[derive(Clone, Debug, PartialEq)]
pub struct AllAligned {
    /// The move to make to each pass, where it lies now.
    pub moves: Vec<Rigid>,
    /// Whether each pass is placed: held, or joined to a held one by the fits accepted.
    pub placed: Vec<bool>,
    /// For a pass placed from wherever it lay, where the winning start came from.
    pub searched: Vec<Option<&'static str>>,
    pub links: Vec<Link>,
    pub rounds: usize,
}

/// Points sampled on each pass for its overlaps.
const OVERLAP_SAMPLES: usize = 3_000;
/// Reaches, as shares of the largest pass's size: for what overlaps before the fit, and after.
const LOOSE_REACH: f64 = 0.015;
const TIGHT_REACH: f64 = 0.004;

/// Lays `passes` together. A pass `held` stays where it is (with none held, the first does).
pub fn align_all(passes: &[Scan], held: &[bool], settings: &AllSettings) -> Result<AllAligned, AlignError> {
    let n = passes.len();
    if n < 2 {
        return Err(AlignError::TooFewPasses);
    }
    let mut held: Vec<bool> = (0..n).map(|i| held.get(i).copied().unwrap_or(false)).collect();
    if !held.iter().any(|&h| h) {
        held[0] = true;
    }
    let size = passes.iter().map(Scan::extent).fold(1e-9, f64::max);
    // Where each pass lies as the work goes on, and the move that has put it there.
    let mut moves = vec![Rigid::shift(Vec3::ZERO); n];
    let mut lying: Vec<Vec<Vec3>> = passes.iter().map(|p| p.positions.to_vec()).collect();
    let mut searched: Vec<Option<&'static str>> = vec![None; n];
    let samples: Vec<Vec<Vec3>> = passes.iter().map(|p| register::sample(p, OVERLAP_SAMPLES)).collect();

    // Placing: in the order the passes came (as a scan is made, each overlapping those before), a
    // pass that does not yet lie on those placed is laid onto them, from wherever it lies; one
    // that cannot be is tried again once the others are placed.
    let mut placed: Vec<bool> = held.clone();
    if settings.search {
        let reach = LOOSE_REACH * size;
        let mut progress = true;
        while progress {
            progress = false;
            for i in 0..n {
                if placed[i] {
                    continue;
                }
                let union = joined(passes, &lying, &placed);
                let onto = Scan::new(&union.0, &union.1);
                let surface = Surface::new(onto);
                let mine: Vec<Vec3> = samples[i].iter().map(|p| moves[i].apply(*p)).collect();
                if share_within(&mine, &surface, reach) >= 0.5 {
                    // It lies on them already: the fits below finish it.
                    placed[i] = true;
                    progress = true;
                    continue;
                }
                let scan = Scan::new(&lying[i], passes[i].triangles);
                let Ok(found) = register::register(&scan, &onto, &Settings { search: true, ignored: 0.5 }) else { continue };
                if found.matched < settings.least_overlap {
                    continue;
                }
                moves[i] = moves[i].then(&found.moved);
                lying[i] = passes[i].positions.iter().map(|p| moves[i].apply(*p)).collect();
                searched[i] = Some(found.start);
                placed[i] = true;
                progress = true;
            }
        }
    } else {
        placed = vec![true; n];
    }

    // Fitting: each pair that has enough in common, fitted where they lie.
    let surfaces: Vec<Surface> = (0..n).map(|i| Surface::new(Scan::new(&lying[i], passes[i].triangles))).collect();
    let loose = LOOSE_REACH * size;
    let lie = |i: usize, moved: &Rigid| -> Vec<Vec3> { samples[i].iter().map(|p| moved.apply(moves[i].apply(*p))).collect() };
    let identity = Rigid::shift(Vec3::ZERO);
    let mut links = Vec::new();
    let mut edges: Vec<(usize, usize, Rigid, f64)> = Vec::new();
    for a in 0..n {
        for b in a + 1..n {
            if (held[a] && held[b]) || !(placed[a] && placed[b]) {
                continue;
            }
            let (moving, target) = if held[a] { (b, a) } else { (a, b) };
            let share_mt = share_within(&lie(moving, &identity), &surfaces[target], loose);
            let share = share_mt.max(share_within(&lie(target, &identity), &surfaces[moving], loose));
            let mut link = Link {
                a: moving,
                b: target,
                overlap_before: share,
                overlap_after: 0.0,
                rms_after: 0.0,
                accepted: false,
                note: None,
                disagreement: None,
            };
            if !(share >= settings.least_overlap) {
                link.note = Some(format!("only {:.0} % of one lies on the other", 100.0 * share));
                links.push(link);
                continue;
            }
            let ignored = (1.0 - share_mt).clamp(0.1, 0.6);
            let found = register::register(
                &Scan::new(&lying[moving], passes[moving].triangles),
                &Scan::new(&lying[target], passes[target].triangles),
                &Settings { search: false, ignored },
            );
            match found {
                Err(e) => link.note = Some(e.to_string()),
                Ok(f) if f.matched < settings.least_overlap => link.note = Some(format!("only {:.0} % matched after the fit", 100.0 * f.matched)),
                Ok(f) => {
                    // A pair placed already lies as close as fitting brings it: it belongs where it lies.
                    let (fit, rms) = if f.rms_after > f.rms_before { (identity, f.rms_before) } else { (f.moved, f.rms_after) };
                    let weight = f.matched.clamp(0.05, 1.0) / rms.max(1e-3 * size);
                    edges.push((moving, target, fit, weight));
                    link.accepted = true;
                    link.note = f.note();
                }
            }
            links.push(link);
        }
    }

    // One move for each pass not held, agreeing with every fit accepted.
    let centre = centroid(&samples.iter().flat_map(|s| s.iter().step_by(15).copied()).collect::<Vec<_>>());
    let joined_to_held = connected(n, &held, &edges);
    let free: Vec<usize> = (0..n).filter(|&i| !held[i] && joined_to_held[i]).collect();
    let (corrections, rounds) = pose_graph(n, &free, &edges, centre, size.max(1.0));
    let mut disagreements = edges
        .iter()
        .map(|(m, t, fit, _)| leftover(&corrections[*m], &corrections[*t], fit, centre))
        .map(|(turn, slide)| (slide.len(), turn.len().to_degrees()));
    for link in links.iter_mut().filter(|l| l.accepted) {
        link.disagreement = disagreements.next();
    }
    for i in 0..n {
        moves[i] = moves[i].then(&corrections[i]);
    }

    // How they lie after.
    let after: Vec<Vec<Vec3>> = passes.iter().zip(&moves).map(|(p, m)| p.positions.iter().map(|q| m.apply(*q)).collect()).collect();
    let after_surfaces: Vec<Surface> = (0..n).map(|i| Surface::new(Scan::new(&after[i], passes[i].triangles))).collect();
    let tight = TIGHT_REACH * size;
    for link in &mut links {
        let mine: Vec<Vec3> = samples[link.a].iter().map(|p| moves[link.a].apply(*p)).collect();
        let (share, rms) = share_and_rms(&mine, &after_surfaces[link.b], tight);
        link.overlap_after = share;
        link.rms_after = rms;
    }
    let placed: Vec<bool> = (0..n).map(|i| held[i] || (placed[i] && joined_to_held[i])).collect();
    Ok(AllAligned { moves, placed, searched, links, rounds })
}

/// The passes `which` as one mesh, where they lie.
fn joined(passes: &[Scan], lying: &[Vec<Vec3>], which: &[bool]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for (i, pass) in passes.iter().enumerate().filter(|(i, _)| which[*i]) {
        let base = positions.len() as u32;
        positions.extend_from_slice(&lying[i]);
        triangles.extend(pass.triangles.iter().map(|t| t.map(|v| v + base)));
    }
    (positions, triangles)
}

/// The share of `points` with `surface` within `reach`.
fn share_within(points: &[Vec3], surface: &Surface, reach: f64) -> f64 {
    share_and_rms(points, surface, reach).0
}

/// The share of `points` with `surface` within `reach`, and their typical gap.
fn share_and_rms(points: &[Vec3], surface: &Surface, reach: f64) -> (f64, f64) {
    let near: Vec<f64> =
        crate::par::map(points.len(), |i| surface.nearest(points[i]).map(|h| h.distance)).into_iter().flatten().filter(|d| *d <= reach).collect();
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let rms = if near.is_empty() { 0.0 } else { (near.iter().map(|d| d * d).sum::<f64>() / near.len() as f64).sqrt() };
    (near.len() as f64 / points.len() as f64, rms)
}

/// Which passes the accepted fits join to a held one.
fn connected(n: usize, held: &[bool], edges: &[(usize, usize, Rigid, f64)]) -> Vec<bool> {
    let mut joined: Vec<bool> = held.to_vec();
    let mut changed = true;
    while changed {
        changed = false;
        for &(m, t, _, _) in edges {
            if joined[m] != joined[t] {
                joined[m] = true;
                joined[t] = true;
                changed = true;
            }
        }
    }
    joined.truncate(n);
    joined
}

/// What is left of a fit once the solution's moves are made: the turn (as an axis times its
/// angle) and the slide of the centre, of `fit⁻¹` then the moving pass's correction then the
/// target's undone.
fn leftover(moving: &Rigid, target: &Rigid, fit: &Rigid, centre: Vec3) -> (Vec3, Vec3) {
    let total = fit.inverse().then(moving).then(&target.inverse());
    let turn = total.axis_angle().map_or(Vec3::ZERO, |(axis, angle)| axis * angle);
    (turn, total.apply(centre) - centre)
}

/// The correction a pass's six numbers stand for: a turn about `centre` (as a length along the
/// arc at `length` from it) and a slide.
fn correction(x: &[f64], k: usize, centre: Vec3, length: f64) -> Rigid {
    let r = Vec3::new(x[6 * k], x[6 * k + 1], x[6 * k + 2]) / length;
    let angle = r.len();
    let turn = if angle < 1e-14 { Rigid::shift(Vec3::ZERO) } else { Rigid::turn(r / angle, angle) };
    let slide = Vec3::new(x[6 * k + 3], x[6 * k + 4], x[6 * k + 5]);
    Rigid { translation: centre + slide - turn.turned(centre), ..turn }
}

/// One move for each `free` pass (the rest stay) that agrees with every fit as well as it can:
/// damped Gauss–Newton on six numbers a pass, the Jacobian by forward differences, at most 60
/// rounds. The moves for all passes, and the rounds taken.
fn pose_graph(n: usize, free: &[usize], edges: &[(usize, usize, Rigid, f64)], centre: Vec3, length: f64) -> (Vec<Rigid>, usize) {
    let slot: Vec<Option<usize>> = (0..n).map(|i| free.iter().position(|&f| f == i)).collect();
    let size = 6 * free.len();
    let moves_of =
        |x: &[f64]| -> Vec<Rigid> { (0..n).map(|i| slot[i].map_or(Rigid::shift(Vec3::ZERO), |k| correction(x, k, centre, length))).collect() };
    let residuals = |x: &[f64]| -> Vec<f64> {
        let moves = moves_of(x);
        edges
            .iter()
            .flat_map(|(m, t, fit, w)| {
                let (turn, slide) = leftover(&moves[*m], &moves[*t], fit, centre);
                let w = w.sqrt();
                [turn.x * length * w, turn.y * length * w, turn.z * length * w, slide.x * w, slide.y * w, slide.z * w]
            })
            .collect()
    };
    let squares = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let mut x = vec![0.0; size];
    let mut rounds = 0;
    if size > 0 && !edges.is_empty() {
        let mut r = residuals(&x);
        let mut damping = 1e-3;
        for round in 0..60 {
            rounds = round + 1;
            let jacobian: Vec<Vec<f64>> = (0..size)
                .map(|c| {
                    let mut moved = x.clone();
                    moved[c] += 1e-6;
                    residuals(&moved).iter().zip(&r).map(|(a, b)| (a - b) / 1e-6).collect()
                })
                .collect();
            let normal: Vec<Vec<f64>> =
                (0..size).map(|i| (0..size).map(|j| jacobian[i].iter().zip(&jacobian[j]).map(|(a, b)| a * b).sum()).collect()).collect();
            let gradient: Vec<f64> = (0..size).map(|i| jacobian[i].iter().zip(&r).map(|(a, b)| a * b).sum()).collect();
            let mut improved = false;
            for _ in 0..12 {
                let mut system = normal.clone();
                for (i, row) in system.iter_mut().enumerate() {
                    row[i] += damping * normal[i][i].max(1e-9) + 1e-12;
                }
                let Some(step) = crate::solve_dense(system, gradient.iter().map(|g| -g).collect()) else {
                    damping *= 10.0;
                    continue;
                };
                let candidate: Vec<f64> = x.iter().zip(&step).map(|(a, b)| a + b).collect();
                let next = residuals(&candidate);
                if squares(&next) <= squares(&r) {
                    x = candidate;
                    r = next;
                    damping = (damping / 3.0).max(1e-9);
                    improved = true;
                    break;
                }
                damping *= 10.0;
            }
            if !improved || squares(&r) < 1e-24 {
                break;
            }
        }
    }
    (moves_of(&x), rounds)
}

/// Which of `points` lie within `reach` of a surface, by index.
pub fn near(points: &[Vec3], scan: &Scan, reach: f64) -> HashSet<usize> {
    let surface = Surface::new(*scan);
    (0..points.len()).filter(|&i| surface.nearest(points[i]).is_some_and(|h| h.distance <= reach)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::{fine_cuboid, turned};

    #[test]
    fn pairs_of_points_give_the_move_that_lays_them_on_each_other() {
        let from = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 7.0, 0.0), Vec3::new(3.0, 2.0, 5.0)];
        let axis = Vec3::new(0.3, -0.5, 0.8).normalized().unwrap();
        let to = turned(&from, axis, 1.1, Vec3::new(5.0, -2.0, 9.0));
        let found = by_pairs(&from, &to).unwrap();
        assert!(found.rms < 1e-9, "{found:?}");
        // A point picked a little off: the worst pair is named.
        let mut off = to.clone();
        off[3] += Vec3::new(0.5, 0.0, 0.0);
        let found = by_pairs(&from, &off).unwrap();
        assert_eq!(found.worst(), 3);
        assert!(found.rms > 0.1);
        assert_eq!(by_pairs(&from[..2], &to[..2]).err(), Some(AlignError::TooFewPairs));
        let line = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
        assert_eq!(by_pairs(&line, &line).err(), Some(AlignError::InALine));
    }

    /// Three passes over a bent block, each what one side of it sees, the second and third turned
    /// and moved off: all three are laid back where they belong, the first held.
    #[test]
    fn three_passes_are_laid_together_from_where_they_lie() {
        let (p, t) = fine_cuboid(Vec3::ZERO, Vec3::new(60.0, 30.0, 20.0), 24);
        // Bent a little all over, as a real part is, so no stretch of it looks like another.
        let p: Vec<Vec3> = p
            .iter()
            .map(|&q| {
                q + Vec3::new(
                    0.6 * (0.13 * q.y + 0.4 * q.z).sin(),
                    0.7 * (0.11 * q.x + 0.2 * q.z).sin(),
                    1.6 * (0.19 * q.x).sin() * (0.23 * q.y + 0.5).cos(),
                )
            })
            .collect();
        // Each pass: what one side sees, the triangles facing it (the top in all three).
        let seen = |from: Vec3| -> Vec<[u32; 3]> {
            t.iter()
                .copied()
                .filter(|&[i, j, k]| {
                    (p[j as usize] - p[i as usize]).cross(p[k as usize] - p[i as usize]).normalized().is_some_and(|n| n.dot(from) > 0.2)
                })
                .collect()
        };
        let (a, b, c) = (seen(Vec3::new(0.6, -0.5, 0.6)), seen(Vec3::new(-0.6, -0.4, 0.7)), seen(Vec3::new(0.1, 0.7, 0.7)));
        let axis = Vec3::new(0.2, 0.9, -0.3).normalized().unwrap();
        let moved_b = turned(&p, axis, 0.08, Vec3::new(1.5, -0.8, 0.6));
        let moved_c = turned(&p, axis, -0.06, Vec3::new(-1.0, 1.2, 0.4));
        let passes = [Scan::new(&p, &a), Scan::new(&moved_b, &b), Scan::new(&moved_c, &c)];
        let done = align_all(&passes, &[true, false, false], &AllSettings::default()).unwrap();
        assert!(done.placed.iter().all(|&x| x), "{:?}", done.links);
        // Every point of the block comes back within a little of where it was (the block's
        // facets are 2.5 mm across over a bent surface, which is what the fits settle to).
        for (k, moved) in [(1, &moved_b), (2, &moved_c)] {
            let worst = p.iter().zip(moved.iter()).map(|(q, m)| done.moves[k].apply(*m).dist(*q)).fold(0.0, f64::max);
            assert!(worst < 0.25, "pass {k} lands {worst} mm off: {:?}", done.links);
        }

        // Passes that already lie nearly right are only fitted together, no search.
        let near_b = turned(&p, axis, 0.01, Vec3::new(0.3, -0.2, 0.1));
        let near_c = turned(&p, axis, -0.012, Vec3::new(-0.2, 0.3, 0.2));
        let passes = [Scan::new(&p, &a), Scan::new(&near_b, &b), Scan::new(&near_c, &c)];
        let done = align_all(&passes, &[true, false, false], &AllSettings { search: false, least_overlap: 0.05 }).unwrap();
        assert!(done.searched.iter().all(Option::is_none));
        for (k, moved) in [(1, &near_b), (2, &near_c)] {
            let worst = p.iter().zip(moved.iter()).map(|(q, m)| done.moves[k].apply(*m).dist(*q)).fold(0.0, f64::max);
            assert!(worst < 0.25, "pass {k} lands {worst} mm off: {:?}", done.links);
        }
        assert_eq!(align_all(&passes[..1], &[], &AllSettings::default()).err(), Some(AlignError::TooFewPasses));
    }
}
