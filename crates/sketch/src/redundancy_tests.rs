//! Frozen ordered dense MGS reference for numerical equivalence tests.
use super::*;

fn compare(rows: &[Vec<f64>], owners: &[usize], blocks: usize) {
    let cols = rows.first().map_or(0, Vec::len);
    let j = Mat { rows: rows.len(), cols, data: rows.iter().flatten().copied().collect() };
    for support in std::iter::once(None).chain((0..blocks).map(Some)) {
        assert_eq!(classify_rows(&j, owners, blocks, support), dense_reference(&j, owners, blocks, support), "{rows:?}, support {support:?}");
    }
}

#[test]
fn exact_zero_projection_preserves_every_nonzero_result_bit() {
    for scale in [f64::from_bits(1), 1e-150, 1.0, 1e150] {
        for q in [vec![0.0, -0.0, 0.6, 0.0, 0.8, 0.0, 0.0, 0.0], vec![0.125; 8]] {
            for sign in [-1.0, 0.0, 1.0] {
                let input: Vec<f64> = (0..8).map(|i| sign * scale * (i as f64 - 3.0)).collect();
                let mut expected = input.clone();
                let d = dot(&q, &expected);
                for (wk, qk) in expected.iter_mut().zip(&q) {
                    *wk -= d * qk;
                }
                let mut actual = input;
                Direction::new(&q, 1.0).remove_from(&mut actual);
                for (a, b) in actual.iter().zip(&expected) {
                    assert!((*a == 0.0 && *b == 0.0) || a.to_bits() == b.to_bits(), "{a} != {b}");
                }
            }
        }
    }
}

#[test]
fn ordered_ids_and_explanations_match_dense_mgs_at_the_threshold() {
    // The repeated target and the partially dependent two-row block must retain their owners.
    for scale in [1e-150, 1e-6, 1.0, 1e6, 1e150] {
        for epsilon in [0.0, DEPENDENT * (1.0 - 1e-10), DEPENDENT, DEPENDENT * (1.0 + 1e-10), 1e-5] {
            let rows = vec![
                vec![scale, 0.0, 0.0, -0.0, 0.0, 0.0],
                vec![scale, scale * epsilon, 0.0, 0.0, 0.0, 0.0],
                vec![0.0, 0.0, scale, 0.0, 0.0, 0.0],
                vec![scale, scale * epsilon, 0.0, 0.0, 0.0, 0.0],
                vec![0.0; 6],
            ];
            compare(&rows, &[0, 1, 1, 2, 3], 5);
        }
    }
    for hostile in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1)] {
        compare(&[vec![1.0, 0.0, 0.0, 0.0], vec![0.0, hostile, 0.0, 0.0], vec![1.0, 1.0, 0.0, 0.0]], &[0, 1, 2], 3);
    }
    compare(&[], &[], 2);
}

#[test]
fn sparse_and_dense_rows_match_over_seeded_mixed_rank_matrices() {
    let mut seed = 99u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    for cols in [2, 3, 8, 17, 32] {
        for density in [0.05, 0.3, 1.0] {
            for _ in 0..20 {
                let mut rows: Vec<Vec<f64>> = Vec::new();
                for k in 0..cols * 2 {
                    let row = if k > 0 && k % 3 == 0 {
                        rows[k / 3].iter().map(|v| v * 2.0).collect()
                    } else {
                        (0..cols).map(|_| if next().abs() < density { next() } else { 0.0 }).collect()
                    };
                    rows.push(row);
                }
                let owners: Vec<usize> = (0..rows.len()).map(|k| k / 2).collect();
                compare(&rows, &owners, cols + 1);
            }
        }
    }
}

#[test]
fn real_sketch_jacobians_keep_redundant_blocks_and_support() {
    use crate::ConstraintKind;
    use solidcore_record::Vec2;

    for seed in 0..8 {
        let mut sk = Sketch::new();
        let mut points = vec![0];
        for i in 1..10 {
            let p = sk.add_point(Vec2::new(i as f64 * 3.0, (i * seed % 7) as f64), None).unwrap();
            let q = points[i - 1];
            points.push(p);
            let l = sk.add_line_pts(q, p, None).unwrap();
            for k in [ConstraintKind::Horizontal { l }, ConstraintKind::Distance { p, q, value: 3.0 }, ConstraintKind::Horizontal { l }] {
                sk.add_constraint(k, None).unwrap();
            }
            if i % 3 == 0 {
                sk.add_constraint(ConstraintKind::Coincident { p, q }, None).unwrap();
            }
        }
        let lay = layout(&sk, &Hold::default());
        let mut x = initial_x(&sk, &lay);
        let (blocks, _) = build_blocks(&sk, &lay, &points_on_curves(&sk));
        let vars: Vec<usize> = (0..lay.n).collect();
        let mut col = vec![usize::MAX; lay.n];
        columns(&mut col, &vars, &vars);
        let (_, j, owner) = local_system(&sk, &lay, &blocks, &mut x, &col, vars.len());
        for support in std::iter::once(None).chain((0..blocks.len()).map(Some)) {
            assert_eq!(classify_rows(&j, &owner, blocks.len(), support), dense_reference(&j, &owner, blocks.len(), support));
        }
    }
}

fn dense_reference(j: &Mat, owner: &[usize], block_count: usize, support: Option<usize>) -> (Vec<bool>, Vec<usize>) {
    #[cfg(feature = "solver-profiling")]
    let classification_timer = profiling::timer(Phase::RedundancyClassification);
    let n = j.cols;
    let row = |k: usize| -> Vec<f64> { j.data.get(k * n..(k + 1) * n).map(<[f64]>::to_vec).unwrap_or_default() };
    // Orthonormal directions of the independent rows so far, and those rows with their block.
    let mut q: Vec<Vec<f64>> = Vec::new();
    let mut basis: Vec<(Vec<f64>, usize)> = Vec::new();
    let mut has_rows = vec![false; block_count];
    let mut adds_rank = vec![false; block_count];
    let mut target_rows: Vec<Vec<f64>> = Vec::new();
    for k in 0..j.rows {
        let Some(&b) = owner.get(k) else { continue };
        let v = row(k);
        let norm = dot(&v, &v).sqrt();
        let mut w = v.clone();
        // Twice, for the orthogonality lost to rounding.
        for _ in 0..2 {
            for qi in &q {
                let d = dot(qi, &w);
                for (wk, qk) in w.iter_mut().zip(qi) {
                    *wk -= d * qk;
                }
            }
        }
        let left = dot(&w, &w).sqrt();
        if let Some(h) = has_rows.get_mut(b) {
            *h = true;
        }
        if norm.is_finite() && left.is_finite() && norm > 0.0 && left > DEPENDENT * norm {
            q.push(w.iter().map(|wk| wk / left).collect());
            basis.push((v, b));
            if let Some(a) = adds_rank.get_mut(b) {
                *a = true;
            }
        } else if Some(b) == support {
            target_rows.push(v);
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

/// `adds_rank` keeps what adding the constraints one at a time and solving after each keeps
/// (AutoConstrain's dimensions, measured where the sketch is): a slot of two lines and two
/// tangent arcs, dimensioned by every size and every point's distances from the origin, many of
/// them implied by the ones before.
#[test]
fn adds_rank_keeps_what_one_solve_per_constraint_keeps() {
    use crate::model::ConstraintKind::{DistanceX, DistanceY, Length, Radius, Tangent};
    use crate::model::CurveKind;
    use crate::solver::{Analysis, solve_with};
    use solidcore_record::Vec2;
    use solidcraft_geom::prelude::*;
    let v = Vec2::new;
    let mut sk = Sketch::new();
    let p: Vec<usize> = [v(10.0, 5.0), v(50.0, 5.0), v(50.0, 25.0), v(10.0, 25.0)].into_iter().map(|q| sk.add_point(q, None).unwrap()).collect();
    let bottom = sk.add_line_pts(p[0], p[1], None).unwrap();
    let right = sk.add_arc(v(50.0, 15.0), v(50.0, 5.0), v(50.0, 25.0), [None, Some(p[1]), Some(p[2])], None).unwrap();
    let top = sk.add_line_pts(p[2], p[3], None).unwrap();
    let left = sk.add_arc(v(10.0, 15.0), v(10.0, 25.0), v(10.0, 5.0), [None, Some(p[3]), Some(p[0])], None).unwrap();
    for (a, b) in [(bottom, right), (right, top), (top, left), (left, bottom)] {
        sk.add_constraint(Tangent { a, b }, None).unwrap();
    }
    let solved = solve_with(&mut sk, &Hold::default(), None, Analysis::Freedom, None);
    assert!(solved.ok() && solved.dof > 0, "{solved:?}");
    let mut sizes = Vec::new();
    for (i, c) in sk.curves.iter().enumerate() {
        sizes.push(match c.kind {
            CurveKind::Line { a, b } => Length { l: i, value: sk.point(a).unwrap().dist(sk.point(b).unwrap()) },
            _ => Radius { c: i, value: sk.radius(i).unwrap() },
        });
    }
    for q in 1..sk.points.len() {
        let at = sk.point(q).unwrap();
        sizes.extend([DistanceX { p: 0, q, value: at.x.abs() }, DistanceY { p: 0, q, value: at.y.abs() }]);
    }
    // One at a time, as AutoConstrain did.
    let mut one = sk.clone();
    let mut dof = solved.dof;
    let mut kept_one = Vec::new();
    for (i, k) in sizes.iter().enumerate() {
        let before = one.clone();
        one.add_constraint(k.clone(), None).unwrap();
        let rep = solve_with(&mut one, &Hold::default(), None, Analysis::Freedom, None);
        if rep.ok() && rep.dof < dof {
            dof = rep.dof;
            kept_one.push(i);
        } else {
            one = before;
        }
    }
    assert_eq!(dof, 0);
    // All at once.
    let mut all = sk.clone();
    let from = all.constraints.len();
    for k in &sizes {
        all.add_constraint(k.clone(), None).unwrap();
    }
    let adds = adds_rank(&all, from).unwrap();
    let kept_all: Vec<usize> = (0..sizes.len()).filter(|i| adds[*i]).collect();
    assert_eq!(kept_all, kept_one);
    assert!(kept_all.len() < sizes.len(), "some are implied: {kept_all:?}");
}

/// `first_order_moves` predicts what a solve does to a well-conditioned sketch: a rectangle held
/// at the origin by its width and height moves its far corners by exactly the change, and a
/// dimension that is not in the sketch moves nothing.
#[test]
fn first_order_moves_match_the_solve_on_a_rectangle() {
    use crate::model::ConstraintKind::{Coincident, Horizontal, Length, Vertical};
    use crate::solver::{Analysis, solve_with};
    use solidcore_record::Vec2;
    use solidcraft_geom::prelude::*;
    let v = Vec2::new;
    let mut sk = Sketch::new();
    let p: Vec<usize> = [v(0.0, 0.0), v(40.0, 0.0), v(40.0, 30.0), v(0.0, 30.0)].into_iter().map(|q| sk.add_point(q, None).unwrap()).collect();
    let l: Vec<usize> = (0..4).map(|i| sk.add_line_pts(p[i], p[(i + 1) % 4], None).unwrap()).collect();
    for k in [Horizontal { l: l[0] }, Horizontal { l: l[2] }, Vertical { l: l[1] }, Vertical { l: l[3] }, Coincident { p: p[0], q: 0 }] {
        sk.add_constraint(k, None).unwrap();
    }
    sk.add_constraint(Length { l: l[0], value: 40.0 }, None).unwrap();
    sk.add_constraint(Length { l: l[1], value: 30.0 }, None).unwrap();
    let width = sk.constraints.len() - 2;
    assert_eq!(solve_with(&mut sk, &Hold::default(), None, Analysis::Freedom, None).dof, 0);
    let moves = first_order_moves(&sk, &[(width, 40.05), (width + 1, 29.98), (99, 1.0)]).unwrap();
    assert!((moves[0] - 0.05).abs() < 1e-6 && (moves[1] - 0.02).abs() < 1e-6 && moves[2] == 0.0, "{moves:?}");
    let was: Vec<Vec2> = sk.points.iter().map(|q| q.pos).collect();
    sk.constraints[width].kind.set_value(40.05);
    assert!(solve_with(&mut sk, &Hold::default(), None, Analysis::StatusOnly, None).ok());
    let far = sk.points.iter().zip(&was).map(|(q, w)| q.pos.dist(*w)).fold(0.0_f64, f64::max);
    assert!((far - moves[0]).abs() < 1e-6, "{far} {moves:?}");
}
