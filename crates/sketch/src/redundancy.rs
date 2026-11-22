//! Redundant constraints: ones whose equations are already implied by the others where the
//! sketch is now (two rectangles sharing a side both say the side is vertical; a coincidence
//! between two corners that equal sizes already put together). They remove no degree of freedom
//! but they do not conflict either: the solver satisfies them along with the rest.
//!
//! The test is the Jacobian's: walking the rows in order (the curves' own relations, then the
//! constraints in the order they were added), a row that adds no rank to the rows before it is
//! dependent on them. A constraint none of whose rows add rank is redundant. Whether a
//! redundant constraint is consistent is a question of its residual (see
//! [`constraint_residual`]): zero where the other constraints hold means it is implied, not in
//! conflict.

use super::{Block, Hold, Layout, State, block_residuals, build_blocks, columns, initial_x, layout, local_system, max_abs, points_on_curves};
use crate::linalg::{Mat, cholesky, cholesky_solve, solve as lin_solve};
use crate::model::Sketch;
#[cfg(feature = "solver-profiling")]
use crate::profiling::{self, Phase};
use crate::union_find::{find, union};

#[cfg(test)]
#[path = "redundancy_tests.rs"]
mod tests;

/// A row is dependent on the rows before it when what is left of it, once their directions are
/// taken out, is below this share of its length (the rank analysis' pivot tolerance is 1e-7 on
/// rows of a comparable scale).
const DEPENDENT: f64 = 1e-6;
/// Variables in one component past which its constraints are not checked for redundancy: the
/// worst case is still dense, rows × rank × variables.
pub(super) const MAX_REDUNDANCY_VARS: usize = 400;
/// A constraint's residual (mm, or radians for angles) below which it holds.
pub const CONSISTENT: f64 = 1e-6;

/// The constraint a block stands for.
pub(super) fn block_constraint(b: &Block) -> Option<usize> {
    match b {
        Block::User(ci, _) | Block::TangentAt(ci, _, _) => Some(*ci),
        Block::Closure { .. } | Block::Arc { .. } => None,
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Keep exact nonzeros in column order; never drop a small coefficient. Dense directions
/// retain the contiguous loop when indexing a sparse representation would cost more.
enum Direction {
    Dense(Vec<f64>),
    Sparse(Vec<(usize, f64)>),
}

impl Direction {
    fn new(w: &[f64], length: f64) -> Self {
        let dense: Vec<f64> = w.iter().map(|v| v / length).collect();
        let nonzero = dense.iter().filter(|v| **v != 0.0).count();
        if nonzero < dense.len() / 2 { Self::Sparse(dense.into_iter().enumerate().filter(|(_, v)| *v != 0.0).collect()) } else { Self::Dense(dense) }
    }

    fn remove_from(&self, w: &mut [f64]) {
        match self {
            Self::Dense(q) => {
                let d = dot(q, w);
                for (wk, qk) in w.iter_mut().zip(q) {
                    *wk -= d * qk;
                }
            }
            Self::Sparse(q) => {
                // Same reduction order as the dense dot product, omitting only exact zeros.
                let d: f64 = q.iter().filter_map(|(i, v)| w.get(*i).map(|wk| v * wk)).sum();
                if d != 0.0 {
                    for &(i, v) in q {
                        if let Some(wk) = w.get_mut(i) {
                            *wk -= d * v;
                        }
                    }
                }
            }
        }
    }
}

/// Per block of `blocks` (in order, over the variables `vars`): whether every one of its rows
/// is a combination of the rows before it. With `support`, also for the block at that index
/// (when redundant) the earlier blocks its rows combine.
pub(super) fn dependent_blocks(
    sk: &Sketch,
    lay: &Layout,
    blocks: &[(Block, Vec<usize>)],
    vars: &[usize],
    x: &mut [f64],
    support: Option<usize>,
) -> (Vec<bool>, Vec<usize>) {
    #[cfg(feature = "solver-profiling")]
    let _timer = {
        profiling::count(|c| c.redundancy_checks += 1);
        profiling::timer(Phase::Redundancy)
    };
    let mut col = vec![usize::MAX; lay.n];
    columns(&mut col, vars, vars);
    let (_, j, owner) = {
        #[cfg(feature = "solver-profiling")]
        let _timer = profiling::timer(Phase::RedundancyJacobian);
        local_system(sk, lay, blocks, x, &col, vars.len())
    };
    classify_rows(&j, &owner, blocks.len(), support)
}

fn classify_rows(j: &Mat, owner: &[usize], block_count: usize, support: Option<usize>) -> (Vec<bool>, Vec<usize>) {
    #[cfg(feature = "solver-profiling")]
    let classification_timer = profiling::timer(Phase::RedundancyClassification);
    let n = j.cols;
    let row = |k: usize| -> &[f64] { j.data.get(k * n..(k + 1) * n).unwrap_or_default() };
    // Orthonormal directions of the independent rows so far, and those rows with their block.
    let mut q: Vec<Direction> = Vec::new();
    let mut basis: Vec<(Vec<f64>, usize)> = Vec::new();
    let mut has_rows = vec![false; block_count];
    let mut adds_rank = vec![false; block_count];
    let mut target_rows: Vec<Vec<f64>> = Vec::new();
    for k in 0..j.rows {
        let Some(&b) = owner.get(k) else { continue };
        let v = row(k);
        let norm = dot(v, v).sqrt();
        let mut w = v.to_vec();
        // A non-finite or zero norm cannot add rank, regardless of its projections. Skipping
        // those rows also keeps non-finite inputs out of the exact-zero sparse fast path.
        if norm.is_finite() && norm > 0.0 {
            // Twice, for the orthogonality lost to rounding. Preserve row/direction order.
            for _ in 0..2 {
                for qi in &q {
                    qi.remove_from(&mut w);
                }
            }
        }
        let left = dot(&w, &w).sqrt();
        if let Some(h) = has_rows.get_mut(b) {
            *h = true;
        }
        if norm.is_finite() && left.is_finite() && norm > 0.0 && left > DEPENDENT * norm {
            q.push(Direction::new(&w, left));
            // Original rows are used only by explanation queries, never ordinary reports.
            if support.is_some() {
                basis.push((v.to_vec(), b));
            }
            if let Some(a) = adds_rank.get_mut(b) {
                *a = true;
            }
        } else if Some(b) == support {
            target_rows.push(v.to_vec());
        }
    }
    let dependent: Vec<bool> = has_rows.iter().zip(&adds_rank).map(|(h, a)| *h && !*a).collect();
    #[cfg(feature = "solver-profiling")]
    drop(classification_timer);
    #[cfg(feature = "solver-profiling")]
    let _support_timer = profiling::timer(Phase::RedundancySupport);
    let mut by = Vec::new();
    if let Some(t) = support
        && dependent.get(t).copied().unwrap_or(false)
        && !basis.is_empty()
    {
        // The combination of the independent rows that gives each of the target's rows: the
        // least-squares solution of Bᵀ a = v, from (B Bᵀ) a = B v (B has full row rank).
        let m = basis.len();
        let mut g = Mat::zeros(m, m);
        for (r, (br, _)) in basis.iter().enumerate() {
            for (c, (bc, _)) in basis.iter().enumerate() {
                g.set(r, c, dot(br, bc));
            }
        }
        for v in &target_rows {
            let rhs: Vec<f64> = basis.iter().map(|(b, _)| dot(b, v)).collect();
            let Some(a) = lin_solve(g.clone(), rhs) else { continue };
            let big = a.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
            for (ak, (_, b)) in a.iter().zip(&basis) {
                if ak.abs() > 1e-6 * big && !by.contains(b) {
                    by.push(*b);
                }
            }
        }
        by.sort_unstable();
    }
    (dependent, by)
}

/// How far the constraint at index `ci` is from holding where the sketch is now: its largest
/// residual (mm, or radians for angles). Zero for a constraint that holds, so a constraint that
/// removes no degree of freedom but holds is redundant, not conflicting. `None` for a driven
/// dimension (it constrains nothing) or an unknown index.
pub fn constraint_residual(sk: &Sketch, ci: usize) -> Option<f64> {
    let c = sk.constraints.get(ci).filter(|c| !c.driven)?;
    let fixed =
        Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new(), heavy: Vec::new() };
    let on_curve = points_on_curves(sk);
    let b = match super::touch_point(sk, &c.kind, &on_curve) {
        Some(p) => Block::TangentAt(ci, &c.kind, p),
        None => Block::User(ci, &c.kind),
    };
    let state = State { sk, lay: &fixed, x: &[] };
    let mut r = Vec::new();
    block_residuals(&state, &b, &mut r);
    // A tangency read at its touch point holds there when the curves only touch at a point that
    // is not on both: the touch-free form (distance from the centre equals the radius) shows a
    // violated tying constraint as well, so the larger of the two is the constraint's distance.
    if matches!(b, Block::TangentAt(..)) {
        super::residuals(&state, &c.kind, &mut r);
    }
    Some(max_abs(&r))
}

/// The constraints (indices) that already determine what the constraint at index `ci` asks
/// for, where the sketch is now: the ones whose equations its own are a combination of. Empty
/// when it is not redundant (it removes a degree of freedom), or when its component is too
/// large to check.
pub fn determined_by(sk: &Sketch, ci: usize) -> Vec<usize> {
    let lay = layout(sk, &Hold::default());
    let mut x = initial_x(sk, &lay);
    let on_curve = points_on_curves(sk);
    let (blocks, _) = build_blocks(sk, &lay, &on_curve);
    let Some(t) = blocks.iter().position(|(b, _)| block_constraint(b) == Some(ci)) else { return Vec::new() };
    let mut parent: Vec<usize> = (0..lay.n).collect();
    for (_, vars) in &blocks {
        if let Some(&first) = vars.first() {
            for &v in vars.iter().skip(1) {
                union(&mut parent, first, v);
            }
        }
    }
    let Some(root) = blocks.get(t).and_then(|(_, v)| v.first().copied()).map(|v| find(&mut parent, v)) else { return Vec::new() };
    let vars: Vec<usize> = (0..lay.n).filter(|v| find(&mut parent, *v) == root).collect();
    if vars.len() > MAX_REDUNDANCY_VARS {
        return Vec::new();
    }
    // The component's other blocks in order, then the constraint asked about.
    let mut order: Vec<usize> = (0..blocks.len())
        .filter(|bi| *bi != t && blocks.get(*bi).and_then(|(_, v)| v.first()).is_some_and(|v| find(&mut parent, *v) == root))
        .collect();
    order.push(t);
    let sub: Vec<(Block, Vec<usize>)> = order.iter().filter_map(|bi| blocks.get(*bi).map(|(b, v)| (super::copy_block(b), v.clone()))).collect();
    let (_, by) = dependent_blocks(sk, &lay, &sub, &vars, &mut x, Some(sub.len().saturating_sub(1)));
    let mut out: Vec<usize> = by.iter().filter_map(|k| sub.get(*k).and_then(|(b, _)| block_constraint(b))).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Variables in one component past which [`adds_rank`] gives up: its walk is dense in the worst
/// case, rows × rank × variables.
pub const MAX_ADDS_RANK_VARS: usize = 6000;

/// For each constraint from index `from` on, in order: whether it removes a degree of freedom
/// from the sketch made of everything before it, where the sketch is now (its equations add
/// rank to the curves' own relations and the constraints before it). This is what adding the
/// constraints one at a time and solving after each would find for constraints that already
/// hold (dimensions measured where the sketch is), in one walk over the Jacobian's rows instead
/// of a solve per constraint. A driven dimension, or one reading only fixed geometry, is
/// `false`. `None` when a component has more than [`MAX_ADDS_RANK_VARS`] variables.
pub fn adds_rank(sk: &Sketch, from: usize) -> Option<Vec<bool>> {
    let lay = layout(sk, &Hold::default());
    let mut x = initial_x(sk, &lay);
    let on_curve = points_on_curves(sk);
    let (blocks, _) = build_blocks(sk, &lay, &on_curve);
    let mut parent: Vec<usize> = (0..lay.n).collect();
    for (_, vars) in &blocks {
        if let Some(&first) = vars.first() {
            for &v in vars.iter().skip(1) {
                union(&mut parent, first, v);
            }
        }
    }
    // The blocks of each component, in order.
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (bi, (_, vars)) in blocks.iter().enumerate() {
        let Some(&first) = vars.first() else { continue };
        let root = find(&mut parent, first);
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, members)) => members.push(bi),
            None => groups.push((root, vec![bi])),
        }
    }
    let mut out = vec![false; sk.constraints.len().saturating_sub(from)];
    for (root, members) in groups {
        // Only components that hold a constraint asked about.
        if !members.iter().any(|bi| blocks.get(*bi).and_then(|(b, _)| block_constraint(b)).is_some_and(|ci| ci >= from)) {
            continue;
        }
        let vars: Vec<usize> = (0..lay.n).filter(|v| find(&mut parent, *v) == root).collect();
        if vars.len() > MAX_ADDS_RANK_VARS {
            return None;
        }
        let sub: Vec<(Block, Vec<usize>)> = members.iter().filter_map(|bi| blocks.get(*bi).map(|(b, v)| (super::copy_block(b), v.clone()))).collect();
        let mut col = vec![usize::MAX; lay.n];
        columns(&mut col, &vars, &vars);
        let (_, j, owner) = local_system(sk, &lay, &sub, &mut x, &col, vars.len());
        let added = rank_adding_blocks(&j, &owner, sub.len());
        for ((b, _), adds) in sub.iter().zip(added) {
            if let Some(slot) = block_constraint(b).and_then(|ci| ci.checked_sub(from)).and_then(|k| out.get_mut(k)) {
                *slot = adds;
            }
        }
    }
    Some(out)
}

/// Per block: whether one of its rows adds rank to the rows before it (rows in order, `owner`
/// naming each row's block).
fn rank_adding_blocks(j: &Mat, owner: &[usize], block_count: usize) -> Vec<bool> {
    let n = j.cols;
    let mut q: Vec<Direction> = Vec::new();
    let mut adds = vec![false; block_count];
    for k in 0..j.rows {
        let Some(&b) = owner.get(k) else { continue };
        let v = j.data.get(k * n..(k + 1) * n).unwrap_or_default();
        let norm = dot(v, v).sqrt();
        if !(norm.is_finite() && norm > 0.0) {
            continue;
        }
        let mut w = v.to_vec();
        for _ in 0..2 {
            for qi in &q {
                qi.remove_from(&mut w);
            }
        }
        let left = dot(&w, &w).sqrt();
        if left.is_finite() && left > DEPENDENT * norm {
            q.push(Direction::new(&w, left));
            if let Some(a) = adds.get_mut(b) {
                *a = true;
            }
        }
    }
    adds
}

/// Variables past which [`first_order_moves`] gives up (its normal matrix is dense).
pub const MAX_MOVES_VARS: usize = 4000;

/// For each change (constraint index, new value) on its own: how far the farthest point of the
/// sketch would move, to first order, if that constraint took that value and the sketch were
/// solved again from where it is now (one Gauss–Newton step, the least change that meets the
/// new value). A change no small move can meet (a shallow arc whose centre must swing far to
/// follow a hundredth of a millimetre at its end) reads as a large move, so a caller can skip
/// it without a solve that would crawl to its iteration limit. One factorisation serves every
/// change. `None` when the sketch has more than [`MAX_MOVES_VARS`] variables or its normal
/// matrix cannot be factored.
pub fn first_order_moves(sk: &Sketch, changes: &[(usize, f64)]) -> Option<Vec<f64>> {
    let lay = layout(sk, &Hold::default());
    let n = lay.n;
    if n == 0 {
        return Some(vec![0.0; changes.len()]);
    }
    if n > MAX_MOVES_VARS {
        return None;
    }
    let mut x = initial_x(sk, &lay);
    let on_curve = points_on_curves(sk);
    let (blocks, _) = build_blocks(sk, &lay, &on_curve);
    let all: Vec<usize> = (0..n).collect();
    let mut col = vec![usize::MAX; n];
    columns(&mut col, &all, &all);
    let (r0, j, owner) = local_system(sk, &lay, &blocks, &mut x, &col, n);
    // Jᵀ J from the rows' few nonzeros, damped by a hair so a direction no row fixes stays put.
    let mut a = Mat::zeros(n, n);
    let mut nz: Vec<(usize, f64)> = Vec::new();
    for k in 0..j.rows {
        nz.clear();
        nz.extend((0..n).filter_map(|c| Some(j.get(k, c)).filter(|v| *v != 0.0 && v.is_finite()).map(|v| (c, v))));
        for &(c1, v1) in &nz {
            for &(c2, v2) in &nz {
                a.add(c1, c2, v1 * v2);
            }
        }
    }
    let scale = (0..n).map(|i| a.get(i, i)).fold(0.0_f64, f64::max).max(1.0);
    for i in 0..n {
        a.add(i, i, 1e-10 * scale);
    }
    let l = cholesky(a)?;
    // Each block's first row.
    let mut first = vec![usize::MAX; blocks.len()];
    for (row, b) in owner.iter().enumerate() {
        if let Some(f) = first.get_mut(*b)
            && *f == usize::MAX
        {
            *f = row;
        }
    }
    let mut out = Vec::with_capacity(changes.len());
    for &(ci, value) in changes {
        let Some(bi) = blocks.iter().position(|(b, _)| block_constraint(b) == Some(ci)) else {
            out.push(0.0);
            continue;
        };
        let mut changed = sk.clone();
        if let Some(c) = changed.constraints.get_mut(ci) {
            c.kind.set_value(value);
        }
        let (moved_blocks, _) = build_blocks(&changed, &lay, &on_curve);
        let Some((b, _)) = moved_blocks.get(bi) else {
            out.push(f64::INFINITY);
            continue;
        };
        let mut r1 = Vec::new();
        block_residuals(&State { sk: &changed, lay: &lay, x: &x }, b, &mut r1);
        // g = Jᵀ (r1 − r0) over the block's rows; the step solves (JᵀJ) dx = −g.
        let mut g = vec![0.0; n];
        for (k, v) in r1.iter().enumerate() {
            let row = first[bi] + k;
            let d = v - r0.get(row).copied().unwrap_or(0.0);
            if d == 0.0 {
                continue;
            }
            for (c, gc) in g.iter_mut().enumerate() {
                *gc -= j.get(row, c) * d;
            }
        }
        cholesky_solve(&l, &mut g);
        let far = sk
            .points
            .iter()
            .enumerate()
            .filter_map(|(i, _)| lay.pvar.get(i).copied().flatten())
            .map(|v| g.get(v).copied().unwrap_or(0.0).hypot(g.get(v + 1).copied().unwrap_or(0.0)))
            .fold(0.0_f64, |m, d| if d.is_finite() { m.max(d) } else { f64::INFINITY });
        out.push(far);
    }
    Some(out)
}
