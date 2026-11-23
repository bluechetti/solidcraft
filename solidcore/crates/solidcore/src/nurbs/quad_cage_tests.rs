//! Tests of the quad-cage surface. Every expectation is derived, not taken from a run: the uniform
//! bicubic B-spline on regular cages, the Catmull-Clark limit points, the cubic B-spline rim and
//! pinned corners, affine invariance, G1 where the paper's construction holds unchanged, and face
//! counts worked out by hand from the layout rule.

use super::*;

type P = Point3;

fn p(x: f64, y: f64, z: f64) -> P {
    Point3::new(x, y, z)
}

/// The cubic B-spline basis functions and their derivatives at `t` for `knots`.
fn basis(knots: &[f64], t: f64) -> (Vec<f64>, Vec<f64>) {
    let n = knots.len() - 4;
    let last = knots[knots.len() - 1];
    let t = t.clamp(knots[0], last);
    // degree 0
    let mut b: Vec<f64> = (0..knots.len() - 1)
        .map(|i| {
            let inside = knots[i] <= t && t < knots[i + 1];
            let at_end = t == last && knots[i] < knots[i + 1] && knots[i + 1] == last;
            if inside || at_end {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let mut prev = b.clone();
    for d in 1..=3 {
        prev = b.clone();
        b = (0..knots.len() - 1 - d)
            .map(|i| {
                let mut v = 0.0;
                if knots[i + d] > knots[i] {
                    v += (t - knots[i]) / (knots[i + d] - knots[i]) * prev[i];
                }
                if knots[i + d + 1] > knots[i + 1] {
                    v += (knots[i + d + 1] - t) / (knots[i + d + 1] - knots[i + 1]) * prev[i + 1];
                }
                v
            })
            .collect();
    }
    // derivative from the degree-2 functions
    let db: Vec<f64> = (0..n)
        .map(|i| {
            let mut v = 0.0;
            if knots[i + 3] > knots[i] {
                v += 3.0 / (knots[i + 3] - knots[i]) * prev[i];
            }
            if knots[i + 4] > knots[i + 1] {
                v -= 3.0 / (knots[i + 4] - knots[i + 1]) * prev[i + 1];
            }
            v
        })
        .collect();
    (b, db)
}

/// The face's point and partial derivatives at (x, y).
fn eval(face: &CageFace, poles: &[P], x: f64, y: f64) -> (P, P, P) {
    let (bu, du) = basis(&face.knots_u, x);
    let (bv, dv) = basis(&face.knots_v, y);
    let nu = bu.len();
    assert_eq!(nu * bv.len(), poles.len());
    let mut s = p(0.0, 0.0, 0.0);
    let mut su = s;
    let mut sv = s;
    for (j, (&wv, &dwv)) in bv.iter().zip(&dv).enumerate() {
        for (i, (&wu, &dwu)) in bu.iter().zip(&du).enumerate() {
            let q = poles[i + nu * j];
            s += q * (wu * wv);
            su += q * (dwu * wv);
            sv += q * (wu * dwv);
        }
    }
    (s, su, sv)
}

/// Where quad `q`'s own (s, t) lies: (face, x, y).
fn locate(surf: &CageSurface, q: usize, s: f64, t: f64) -> (usize, f64, f64) {
    for (fi, face) in surf.faces.iter().enumerate() {
        for (k, &(cq, rot)) in face.quads.iter().enumerate() {
            if cq != q {
                continue;
            }
            let (iu, iv) = ((k % face.size[0]) as f64, (k / face.size[0]) as f64);
            let (x, y) = match rot {
                0 => (s, t),
                1 => (t, 1.0 - s),
                2 => (1.0 - s, 1.0 - t),
                _ => (1.0 - t, s),
            };
            return (fi, iu + x, iv + y);
        }
    }
    panic!("quad {q} is in no face");
}

fn at(surf: &CageSurface, pts: &[P], q: usize, s: f64, t: f64) -> (P, P, P) {
    let (fi, x, y) = locate(surf, q, s, t);
    let face = &surf.faces[fi];
    let (pt, mut su, mut sv) = eval(face, &face.points(pts).unwrap(), x, y);
    // the derivatives in the quad's own frame
    let rot = face.quads.iter().find(|c| c.0 == q).unwrap().1;
    (su, sv) = match rot {
        0 => (su, sv),
        1 => (sv * -1.0, su),
        2 => (su * -1.0, sv * -1.0),
        _ => (sv, su * -1.0),
    };
    (pt, su, sv)
}

fn dist(a: P, b: P) -> f64 {
    let d = a - b;
    (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
}

fn cross(a: P, b: P) -> P {
    p(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

fn norm(a: P) -> f64 {
    (a.x * a.x + a.y * a.y + a.z * a.z).sqrt()
}

/// A torus of `nu` x `nv` quads, all vertices of four, with a wobble so nothing is symmetric.
fn torus(nu: usize, nv: usize) -> (Vec<P>, Vec<[u32; 4]>) {
    let mut v = Vec::new();
    for i in 0..nu {
        for j in 0..nv {
            let a = std::f64::consts::TAU * i as f64 / nu as f64;
            let b = std::f64::consts::TAU * j as f64 / nv as f64;
            let r = 10.0 + 1.5 * (3.0 * a + 2.0 * b).sin();
            v.push(p(
                (40.0 + r * b.cos()) * a.cos(),
                (40.0 + r * b.cos()) * a.sin(),
                r * b.sin() + (2.0 * a).sin(),
            ));
        }
    }
    let id = |i: usize, j: usize| ((i % nu) * nv + (j % nv)) as u32;
    let mut f = Vec::new();
    for i in 0..nu {
        for j in 0..nv {
            f.push([id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    (v, f)
}

/// An open grid of `n` x `n` quads over a bump.
fn grid(n: usize) -> (Vec<P>, Vec<[u32; 4]>) {
    let mut v = Vec::new();
    for i in 0..=n {
        for j in 0..=n {
            let (x, y) = (6.0 * i as f64, 6.0 * j as f64);
            v.push(p(
                x + 0.4 * (0.7 * j as f64).sin(),
                y,
                5.0 * (0.21 * x).sin() * (0.13 * y).cos(),
            ));
        }
    }
    let id = |i: usize, j: usize| (i * (n + 1) + j) as u32;
    let mut f = Vec::new();
    for i in 0..n {
        for j in 0..n {
            f.push([id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    (v, f)
}

/// Turn the edge a-b inside the hexagon of its two quads by one step: two vertices lose a quad,
/// two gain one.
fn flip(f: &mut Vec<[u32; 4]>, a: u32, b: u32) {
    let k: Vec<usize> = (0..f.len())
        .filter(|&k| f[k].contains(&a) && f[k].contains(&b))
        .collect();
    assert_eq!(k.len(), 2);
    let (mut q1, mut q2) = (f[k[0]], f[k[1]]);
    let i1 = q1.iter().position(|&x| x == a).unwrap();
    if q1[(i1 + 1) % 4] != b {
        std::mem::swap(&mut q1, &mut q2);
    }
    let i = q1.iter().position(|&x| x == a).unwrap();
    let j = q2.iter().position(|&x| x == b).unwrap();
    let h = [
        a,
        q2[(j + 2) % 4],
        q2[(j + 3) % 4],
        b,
        q1[(i + 2) % 4],
        q1[(i + 3) % 4],
    ];
    let n1 = [h[1], h[2], h[3], h[4]];
    let n2 = [h[4], h[5], h[0], h[1]];
    f.retain(|q| *q != q1 && *q != q2);
    f.push(n1);
    f.push(n2);
}

/// The cube, its corners of three quads.
fn cube() -> (Vec<P>, Vec<[u32; 4]>) {
    let v: Vec<P> = (0..8)
        .map(|i| {
            p(
                f64::from(i & 1) * 20.0,
                f64::from((i >> 1) & 1) * 21.0,
                f64::from(i >> 2) * 19.0 + f64::from(i & 1),
            )
        })
        .collect();
    let f = vec![
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    (v, f)
}

/// The cube with each face split in four (edge and face middle points added): corners of three
/// quads two edges apart, everything else of four.
fn cube1() -> (Vec<P>, Vec<[u32; 4]>) {
    let (cv, cf) = cube();
    let mut v = cv.clone();
    let mut mid: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut f = Vec::new();
    for q in &cf {
        let centre = v.len() as u32;
        let c = q
            .iter()
            .fold(p(0.0, 0.0, 0.0), |s, &i| s + cv[i as usize] * 0.25);
        v.push(c + p(0.3, -0.2, 0.1));
        let mut m = [0u32; 4];
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            let key = (a.min(b), a.max(b));
            m[k] = *mid.entry(key).or_insert_with(|| {
                v.push((cv[a as usize] + cv[b as usize]) * 0.5);
                (v.len() - 1) as u32
            });
        }
        for k in 0..4 {
            f.push([q[k], m[k], centre, m[(k + 3) % 4]]);
        }
    }
    (v, f)
}

/// Each vertex's limit point (Catmull-Clark): `(n^2 v + 4 sum(edge ends) + sum(diagonals)) /
/// (n (n + 5))`, and the quad and corner it is seen from.
fn limit_points(v: &[P], f: &[[u32; 4]]) -> Vec<(usize, usize, P)> {
    let mut out = Vec::new();
    for vert in 0..v.len() as u32 {
        let around: Vec<(usize, usize)> = f
            .iter()
            .enumerate()
            .filter_map(|(k, q)| q.iter().position(|&x| x == vert).map(|c| (k, c)))
            .collect();
        let n = around.len() as f64;
        // closed: every quad's edge on to its next corner is another quad's edge back
        let mut e = BTreeSet::new();
        let mut d = Vec::new();
        for &(k, c) in &around {
            e.insert(f[k][(c + 1) % 4]);
            e.insert(f[k][(c + 3) % 4]);
            d.push(f[k][(c + 2) % 4]);
        }
        if e.len() != around.len() {
            continue; // on the rim
        }
        let mut s = v[vert as usize] * (n * n);
        for &x in &e {
            s += v[x as usize] * 4.0;
        }
        for &x in &d {
            s += v[x as usize];
        }
        let (k, c) = around[0];
        out.push((k, c, s * (1.0 / (n * (n + 5.0)))));
    }
    out
}

/// Quad `k`'s own (s, t) of its corner `c`.
fn corner_st(c: usize) -> (f64, f64) {
    [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)][c]
}

/// The uniform bicubic B-spline weights of the four points along one way at `s`.
fn uniform(s: f64) -> [f64; 4] {
    let r = 1.0 - s;
    [
        r * r * r / 6.0,
        (3.0 * s * s * s - 6.0 * s * s + 4.0) / 6.0,
        (-3.0 * s * s * s + 3.0 * s * s + 3.0 * s + 1.0) / 6.0,
        s * s * s / 6.0,
    ]
}

/// The largest sine between the normals of neighbouring quads at places along every shared edge.
fn worst_crease(v: &[P], f: &[[u32; 4]], surf: &CageSurface) -> f64 {
    let mut worst: f64 = 0.0;
    for (k, q) in f.iter().enumerate() {
        for side in 0..4 {
            let (a, b) = (q[side], q[(side + 1) % 4]);
            let Some(g) = f
                .iter()
                .position(|h| (0..4).any(|i| h[i] == b && h[(i + 1) % 4] == a))
            else {
                continue;
            };
            let kg = (0..4)
                .find(|&i| f[g][i] == b && f[g][(i + 1) % 4] == a)
                .unwrap();
            for w in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let st = |side: usize, w: f64| {
                    [(w, 0.0), (1.0, w), (1.0 - w, 1.0), (0.0, 1.0 - w)][side]
                };
                let (s1, t1) = st(side, w);
                let (s2, t2) = st(kg, 1.0 - w);
                let (p1, u1, v1) = at(surf, v, k, s1, t1);
                let (p2, u2, v2) = at(surf, v, g, s2, t2);
                assert!(
                    dist(p1, p2) < 1e-9,
                    "the quads part along an edge: {}",
                    dist(p1, p2)
                );
                let (n1, n2) = (cross(u1, v1), cross(u2, v2));
                worst = worst.max(norm(cross(n1, n2)) / (norm(n1) * norm(n2)));
            }
        }
    }
    worst
}

#[test]
fn a_regular_cage_is_the_uniform_bicubic_b_spline() {
    let (nu, nv) = (12, 8);
    let (v, f) = torus(nu, nv);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    let id = |i: usize, j: usize| (i % nu) * nv + (j % nv);
    let mut worst: f64 = 0.0;
    for i in 0..nu {
        for j in 0..nv {
            let q = i * nv + j;
            for (s, t) in [(0.0, 0.0), (0.3, 0.7), (0.5, 0.5), (0.9, 0.2), (1.0, 0.4)] {
                let (wu, wv) = (uniform(s), uniform(t));
                let mut want = p(0.0, 0.0, 0.0);
                for (a, &x) in wu.iter().enumerate() {
                    for (b, &y) in wv.iter().enumerate() {
                        want += v[id(i + nu + a - 1, j + nv + b - 1)] * (x * y);
                    }
                }
                worst = worst.max(dist(at(&surf, &v, q, s, t).0, want));
            }
        }
    }
    assert!(worst < 1e-10, "off the uniform B-spline by {worst}");
}

#[test]
fn every_inner_vertex_is_on_its_limit_point() {
    let mut cases = vec![cube(), cube1(), torus(12, 8)];
    let (v, mut f) = torus(16, 12);
    flip(&mut f, 5 * 12 + 4, 5 * 12 + 5);
    cases.push((v, f));
    let (v, mut f) = grid(12);
    flip(&mut f, 6 * 13 + 6, 6 * 13 + 7);
    cases.push((v, f));
    for (v, f) in cases {
        let surf = CageSurface::of(v.len(), &f).unwrap();
        for (k, c, want) in limit_points(&v, &f) {
            let (s, t) = corner_st(c);
            let got = at(&surf, &v, k, s, t).0;
            assert!(
                dist(got, want) < 1e-9,
                "a vertex is {} off its limit point",
                dist(got, want)
            );
        }
    }
}

#[test]
fn the_rim_is_the_cubic_b_spline_of_its_points_and_corners_stay() {
    let n = 6;
    let (v, f) = grid(n);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    let id = |i: usize, j: usize| i * (n + 1) + j;
    // corners: the quad at (0, 0) has its corner 0 there
    assert!(dist(at(&surf, &v, 0, 0.0, 0.0).0, v[0]) < 1e-12);
    // rim points along j = 0: limit (prev + 4 v + next) / 6, and between them the cubic B-spline of
    // the rim points, with the end points doubled back (a clamped end: 2 v0 - v1 beyond it)
    for i in 1..n {
        let want = (v[id(i - 1, 0)] + v[id(i, 0)] * 4.0 + v[id(i + 1, 0)]) * (1.0 / 6.0);
        let q = i * n; // quad (i, 0), corner 0 at (i, 0)
        assert!(dist(at(&surf, &v, q, 0.0, 0.0).0, want) < 1e-10);
    }
    let rim = |i: isize| -> P {
        if i < 0 {
            v[id(0, 0)] * 2.0 - v[id(1, 0)]
        } else if i as usize > n {
            v[id(n, 0)] * 2.0 - v[id(n - 1, 0)]
        } else {
            v[id(i as usize, 0)]
        }
    };
    for i in 0..n {
        for s in [0.25, 0.5, 0.8] {
            let w = uniform(s);
            let mut want = p(0.0, 0.0, 0.0);
            for (a, &x) in w.iter().enumerate() {
                want += rim(i as isize + a as isize - 1) * x;
            }
            let got = at(&surf, &v, i * n, s, 0.0).0;
            assert!(
                dist(got, want) < 1e-10,
                "the rim is {} off its B-spline",
                dist(got, want)
            );
        }
    }
}

#[test]
fn every_pole_is_an_affine_blend() {
    for (v, f) in [cube(), cube1(), torus(12, 8), grid(5)] {
        for face in CageSurface::of(v.len(), &f).unwrap().faces {
            for b in &face.poles {
                let s: f64 = b.iter().map(|&(_, w)| w).sum();
                assert!((s - 1.0).abs() < 1e-12, "a pole's weights add up to {s}");
            }
        }
    }
}

#[test]
fn the_cube_and_the_split_cube_are_tangent_continuous() {
    // Corners of three quads, two edges apart: the paper's construction holds unchanged there.
    for (v, f) in [cube(), cube1()] {
        let surf = CageSurface::of(v.len(), &f).unwrap();
        let w = worst_crease(&v, &f, &surf);
        assert!(w < 1e-10, "a crease of sine {w}");
    }
}

#[test]
fn faces_follow_the_layout() {
    // The cube: every edge joins two irregular corners, so every quad is a face.
    let (v, f) = cube();
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 6);
    assert!(surf
        .faces
        .iter()
        .all(|x| x.size == [1, 1] && x.poles.len() == 64));
    // The split cube: each line runs from a corner through an edge's middle point to the next
    // corner (a link of two edges), along the cube's edges; nothing else is a line, so each of the
    // cube's faces is one face of 2 x 2 quads, 13 x 13 poles.
    let (v, f) = cube1();
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 6);
    assert!(surf
        .faces
        .iter()
        .all(|x| x.size == [2, 2] && x.poles.len() == 13 * 13));
    // A regular open grid: one face of 5 x 5 quads, 28 x 28 poles.
    let (v, f) = grid(5);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 1);
    assert_eq!(surf.faces[0].size, [5, 5]);
    assert_eq!(surf.faces[0].poles.len(), 28 * 28);
    // A closed regular cage can be no one face (a face would meet itself): it is cut into
    // rectangles, each a face, the quads all accounted for once.
    let (v, f) = torus(12, 8);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert!(surf.faces.len() > 1);
    let mut seen: Vec<usize> = surf
        .faces
        .iter()
        .flat_map(|x| x.quads.iter().map(|c| c.0))
        .collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..f.len()).collect::<Vec<_>>());
}

#[test]
fn faces_inside_are_c2_across_their_joints() {
    // One knot between quads inside a face: checked on the poles, the second differences across
    // a joint agree on both sides when the face is evaluated just either side of it.
    let (v, mut f) = grid(10);
    flip(&mut f, 5 * 11 + 5, 5 * 11 + 6);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert!(surf.faces.iter().any(|x| x.size[0] > 1 || x.size[1] > 1));
    for face in &surf.faces {
        assert_eq!(
            face.knots_u.len(),
            face.poles.len() / face.knots_v.len().saturating_sub(4) + 4
        );
        let poles = face.points(&v).unwrap();
        for iu in 1..face.size[0] {
            for y in [0.2, 0.5, 0.7] {
                let y = y * face.size[1] as f64;
                let h = 1e-4;
                let x = iu as f64;
                let second = |x0: f64| {
                    let a = eval(face, &poles, x0 - h, y).0;
                    let b = eval(face, &poles, x0, y).0;
                    let c = eval(face, &poles, x0 + h, y).0;
                    (a + c - b * 2.0) * (1.0 / (h * h))
                };
                let left = second(x - 2.0 * h);
                let right = second(x + 2.0 * h);
                assert!(
                    dist(left, right) < 1e-3 * (1.0 + norm(left)),
                    "a joint is not C2: {}",
                    dist(left, right)
                );
            }
        }
    }
}

#[test]
fn bad_cages_are_refused() {
    assert!(CageSurface::of(4, &[]).is_err());
    assert!(CageSurface::of(3, &[[0, 1, 2, 3]]).is_err());
    assert!(CageSurface::of(4, &[[0, 1, 1, 3]]).is_err());
    // two quads running the same way along an edge
    assert!(CageSurface::of(6, &[[0, 1, 2, 3], [0, 1, 4, 5]]).is_err());
    // one quad alone is a cage: its four corners kept, its rim the B-spline of four points
    let surf = CageSurface::of(4, &[[0, 1, 2, 3]]).unwrap();
    assert_eq!(surf.faces.len(), 1);
    // a pole naming a point the caller does not give
    assert!(surf.faces[0].points(&[p(0.0, 0.0, 0.0)]).is_err());
}
