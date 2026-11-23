//! Tests of the quad-cage surface. Every expectation is derived, not taken from a run: the uniform
//! bicubic B-spline on regular cages, the Catmull-Clark limit points, the cubic B-spline rim and
//! pinned corners, the paper's Theorem 1 (tangent planes shared along every side, curvature too
//! where both ends of a side are regular), the faces being their quads' patches, and face and
//! pole counts worked out by hand from the layout rule and the knots.

use super::*;

type P = Point3;

fn p(x: f64, y: f64, z: f64) -> P {
    Point3::new(x, y, z)
}

fn dot(a: P, b: P) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross(a: P, b: P) -> P {
    p(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}

fn dist(a: P, b: P) -> f64 {
    norm(a - b)
}

/// The cubic B-spline functions for `knots` at `t` with their first and second derivatives;
/// `left` takes the limit from below at a knot.
fn basis(knots: &[f64], t: f64, left: bool) -> [Vec<f64>; 3] {
    let m = knots.len();
    let n = m - 4;
    let spans: Vec<usize> = (0..m - 1).filter(|&i| knots[i] < knots[i + 1]).collect();
    let inside = |i: usize| {
        if left {
            knots[i] < t && t <= knots[i + 1]
        } else {
            knots[i] <= t && t < knots[i + 1]
        }
    };
    let ends = if t <= knots[0] {
        spans[0]
    } else {
        spans[spans.len() - 1]
    };
    let j = spans.iter().copied().find(|&i| inside(i)).unwrap_or(ends);
    let mut b = vec![vec![0.0; m - 1]; 4];
    b[0][j] = 1.0;
    for d in 1..=3 {
        for i in 0..m - 1 - d {
            let mut v = 0.0;
            if knots[i + d] > knots[i] {
                v += (t - knots[i]) / (knots[i + d] - knots[i]) * b[d - 1][i];
            }
            if knots[i + d + 1] > knots[i + 1] {
                v += (knots[i + d + 1] - t) / (knots[i + d + 1] - knots[i + 1]) * b[d - 1][i + 1];
            }
            b[d][i] = v;
        }
    }
    // the derivative of the degree-`d` functions from the values `lower` one degree down
    let derived = |lower: &[f64], d: usize, count: usize| -> Vec<f64> {
        (0..count)
            .map(|i| {
                let mut v = 0.0;
                if knots[i + d] > knots[i] {
                    v += d as f64 / (knots[i + d] - knots[i]) * lower[i];
                }
                if knots[i + d + 1] > knots[i + 1] {
                    v -= d as f64 / (knots[i + d + 1] - knots[i + 1]) * lower[i + 1];
                }
                v
            })
            .collect()
    };
    let first = derived(&b[2], 3, n);
    let second = derived(&derived(&b[1], 2, n + 1), 3, n);
    [b[3][..n].to_vec(), first, second]
}

/// A bicubic patch at (x, y): the point, then its derivatives u, v, uu, uv, vv.
fn patch(knots: [&[f64]; 2], poles: &[P], x: f64, y: f64, left: [bool; 2]) -> [P; 6] {
    let [bu, du, ddu] = basis(knots[0], x, left[0]);
    let [bv, dv, ddv] = basis(knots[1], y, left[1]);
    let nu = bu.len();
    assert_eq!(nu * bv.len(), poles.len());
    let mut out = [p(0.0, 0.0, 0.0); 6];
    for j in 0..bv.len() {
        for i in 0..nu {
            let q = poles[i + nu * j];
            out[0] += q * (bu[i] * bv[j]);
            out[1] += q * (du[i] * bv[j]);
            out[2] += q * (bu[i] * dv[j]);
            out[3] += q * (ddu[i] * bv[j]);
            out[4] += q * (du[i] * dv[j]);
            out[5] += q * (bu[i] * ddv[j]);
        }
    }
    out
}

/// A cage's surface with its poles worked out and each quad's place among the faces.
struct Laid {
    surf: CageSurface,
    poles: Vec<Vec<P>>,
    place: BTreeMap<usize, (usize, usize, usize, u8)>,
}

impl Laid {
    fn of(v: &[P], f: &[[u32; 4]]) -> Laid {
        let surf = CageSurface::of(v.len(), f).unwrap();
        let poles = surf.faces.iter().map(|x| x.points(v).unwrap()).collect();
        let mut place = BTreeMap::new();
        for (fi, face) in surf.faces.iter().enumerate() {
            assert_eq!(face.quads.len(), face.size[0] * face.size[1]);
            for (k, &(q, rot)) in face.quads.iter().enumerate() {
                let was = place.insert(q, (fi, k % face.size[0], k / face.size[0], rot));
                assert!(was.is_none(), "quad {q} is in two faces");
            }
        }
        assert_eq!(place.len(), f.len(), "every quad is in a face");
        Laid { surf, poles, place }
    }

    /// Quad `q` at its own (s, t), from inside the quad: the point, then its derivatives s, t,
    /// ss, st, tt.
    fn at(&self, q: usize, s: f64, t: f64) -> [P; 6] {
        let (fi, iu, iv, rot) = self.place[&q];
        let face = &self.surf.faces[fi];
        let (x, y) = match rot {
            0 => (s, t),
            1 => (t, 1.0 - s),
            2 => (1.0 - s, 1.0 - t),
            _ => (1.0 - t, s),
        };
        let e = patch(
            [&face.knots_u, &face.knots_v],
            &self.poles[fi],
            iu as f64 + x,
            iv as f64 + y,
            [x > 0.5, y > 0.5],
        );
        let neg = |a: P| a * -1.0;
        match rot {
            0 => e,
            1 => [e[0], neg(e[2]), e[1], e[5], neg(e[4]), e[3]],
            2 => [e[0], neg(e[1]), neg(e[2]), e[3], e[4], e[5]],
            _ => [e[0], e[2], neg(e[1]), e[5], neg(e[4]), e[3]],
        }
    }
}

// ---- cages --------------------------------------------------------------------------------------

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

/// The grid of 4 x 4 quads less one corner's 2 x 2: its inside corner is a rim point of three
/// quads.
fn ell() -> (Vec<P>, Vec<[u32; 4]>) {
    let (v, f) = grid(4);
    let keep = |k: usize| !(k / 4 >= 2 && k % 4 >= 2);
    let f = f
        .into_iter()
        .enumerate()
        .filter_map(|(k, q)| keep(k).then_some(q))
        .collect();
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

/// Every quad split in four (edge and face middle points added, each nudged so nothing is
/// symmetric): the corners keep their quads, every new point has four.
fn split(v: &[P], f: &[[u32; 4]], nudge: f64) -> (Vec<P>, Vec<[u32; 4]>) {
    let off = |i: usize| {
        let a = i as f64;
        p(
            (1.7 * a).sin(),
            (2.3 * a + 0.4).sin(),
            (3.1 * a + 1.1).sin(),
        ) * nudge
    };
    let mut out = v.to_vec();
    let mut mid: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut g = Vec::new();
    for q in f {
        let centre = out.len() as u32;
        let c = q
            .iter()
            .fold(p(0.0, 0.0, 0.0), |s, &i| s + v[i as usize] * 0.25);
        out.push(c + off(out.len()));
        let mut m = [0u32; 4];
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            m[k] = *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let w = off(out.len());
                out.push((v[a as usize] + v[b as usize]) * 0.5 + w);
                (out.len() - 1) as u32
            });
        }
        for k in 0..4 {
            g.push([q[k], m[k], centre, m[(k + 3) % 4]]);
        }
    }
    (out, g)
}

/// One point of `n` quads in the middle of an open patch, `k` quads out to the rim along each of
/// its `n` edges, over a bump. Sector `i`'s quads come first to last, `(a, b)` at `i k^2 + a k +
/// b`, the quad at the middle point first.
fn fan(n: usize, k: usize) -> (Vec<P>, Vec<[u32; 4]>) {
    let mut v = vec![p(0.0, 0.0, 0.0)];
    let mut id: BTreeMap<(usize, usize, usize), u32> = BTreeMap::new();
    let dir = |i: usize| {
        let a = std::f64::consts::TAU * (i % n) as f64 / n as f64;
        (a.cos(), a.sin())
    };
    let mut get = |v: &mut Vec<P>, i: usize, a: usize, b: usize| -> u32 {
        if a == 0 && b == 0 {
            return 0;
        }
        // sector i's (a, 0) is the sector before's (0, a)
        let key = if b == 0 {
            ((i + n - 1) % n, 0, a)
        } else {
            (i % n, a, b)
        };
        *id.entry(key).or_insert_with(|| {
            let (i, a, b) = key;
            let (d0, d1) = (dir(i), dir(i + 1));
            let x = 5.0 * (a as f64 * d0.0 + b as f64 * d1.0);
            let y = 5.0 * (a as f64 * d0.1 + b as f64 * d1.1);
            v.push(p(
                x,
                y,
                4.0 * (0.11 * x + 0.3).sin() * (0.09 * y - 0.2).cos() + 0.004 * x * y,
            ));
            (v.len() - 1) as u32
        })
    };
    let mut f = Vec::new();
    for i in 0..n {
        for a in 0..k {
            for b in 0..k {
                f.push([
                    get(&mut v, i, a, b),
                    get(&mut v, i, a + 1, b),
                    get(&mut v, i, a + 1, b + 1),
                    get(&mut v, i, a, b + 1),
                ]);
            }
        }
    }
    (v, f)
}

/// A cage by name: its points and its quads.
type Named = (&'static str, Vec<P>, Vec<[u32; 4]>);

/// Cages with every kind of point and side: regular, one irregular point of 3, 5, 6, 7 and 8
/// quads, irregular points side by side (flips), every corner of a quad irregular (the cube), an
/// irregular point on the rim, open and closed.
fn cages() -> Vec<Named> {
    let mut out = Vec::new();
    let (v, f) = grid(6);
    out.push(("an open grid", v, f));
    let (v, f) = torus(12, 8);
    out.push(("a torus", v, f));
    for (name, n) in [
        ("a point of three", 3),
        ("a point of five", 5),
        ("a point of six", 6),
        ("a point of seven", 7),
        ("a point of eight", 8),
    ] {
        let (v, f) = fan(n, 3);
        out.push((name, v, f));
    }
    let (v, mut f) = grid(10);
    flip(&mut f, 5 * 11 + 5, 5 * 11 + 6);
    out.push(("a grid with a flip", v, f));
    let (v, mut f) = torus(16, 12);
    flip(&mut f, 5 * 12 + 4, 5 * 12 + 5);
    flip(&mut f, 11 * 12 + 9, 11 * 12 + 10);
    out.push(("a torus with two flips", v, f));
    let (v, f) = cube();
    out.push(("the cube", v.clone(), f.clone()));
    let (v1, f1) = split(&v, &f, 0.3);
    out.push(("the cube split once", v1.clone(), f1.clone()));
    let (v2, f2) = split(&v1, &f1, 0.1);
    out.push(("the cube split twice", v2, f2));
    let (v, f) = ell();
    out.push(("an L", v, f));
    out
}

/// The quad and side across each quad's side.
fn across(f: &[[u32; 4]]) -> BTreeMap<(usize, usize), (usize, usize)> {
    let mut along: BTreeMap<(u32, u32), (usize, usize)> = BTreeMap::new();
    for (k, q) in f.iter().enumerate() {
        for s in 0..4 {
            along.insert((q[s], q[(s + 1) % 4]), (k, s));
        }
    }
    let mut out = BTreeMap::new();
    for (&(a, b), &ks) in &along {
        if let Some(&gs) = along.get(&(b, a)) {
            out.insert(ks, gs);
        }
    }
    out
}

/// Whether each point is regular: four quads inside the cage, two on its rim, one at a corner.
fn regular(n: usize, f: &[[u32; 4]]) -> Vec<bool> {
    let beside = across(f);
    let mut count = vec![0usize; n];
    let mut rim = vec![false; n];
    for (k, q) in f.iter().enumerate() {
        for s in 0..4 {
            count[q[s] as usize] += 1;
            if !beside.contains_key(&(k, s)) {
                rim[q[s] as usize] = true;
                rim[q[(s + 1) % 4] as usize] = true;
            }
        }
    }
    (0..n)
        .map(|i| if rim[i] { count[i] <= 2 } else { count[i] == 4 })
        .collect()
}

/// A quad's own (s, t) a share `w` along its side `side`.
fn on_side(side: usize, w: f64) -> (f64, f64) {
    [(w, 0.0), (1.0, w), (1.0 - w, 1.0), (0.0, 1.0 - w)][side]
}

/// The derivative into the quad across its side `side`, and the second derivative across it.
fn into(side: usize, e: &[P; 6]) -> (P, P) {
    match side {
        0 => (e[2], e[5]),
        1 => (e[1] * -1.0, e[3]),
        2 => (e[2] * -1.0, e[5]),
        _ => (e[1], e[3]),
    }
}

const ALONG: [f64; 7] = [0.02, 0.2, 0.4, 0.5, 0.6, 0.8, 0.98];

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
        if around.is_empty() || e.len() != around.len() {
            continue; // on the rim, or in no quad
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

// ---- the patches --------------------------------------------------------------------------------

#[test]
fn a_regular_cage_is_the_uniform_bicubic_b_spline() {
    let (nu, nv) = (12, 8);
    let (v, f) = torus(nu, nv);
    let laid = Laid::of(&v, &f);
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
                worst = worst.max(dist(laid.at(q, s, t)[0], want));
            }
        }
    }
    assert!(worst < 1e-10, "off the uniform B-spline by {worst}");
}

#[test]
fn every_inner_vertex_is_on_its_limit_point() {
    for (name, v, f) in cages() {
        let laid = Laid::of(&v, &f);
        for (k, c, want) in limit_points(&v, &f) {
            let (s, t) = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)][c];
            let got = laid.at(k, s, t)[0];
            assert!(
                dist(got, want) < 1e-9,
                "{name}: a vertex is {} off its limit point",
                dist(got, want)
            );
        }
    }
}

#[test]
fn the_rim_is_the_cubic_b_spline_of_its_points_and_corners_stay() {
    let n = 6;
    let (v, f) = grid(n);
    let laid = Laid::of(&v, &f);
    let id = |i: usize, j: usize| i * (n + 1) + j;
    // corners: the quad at (0, 0) has its corner 0 there
    assert!(dist(laid.at(0, 0.0, 0.0)[0], v[0]) < 1e-12);
    // rim points along j = 0: limit (prev + 4 v + next) / 6, and between them the cubic B-spline of
    // the rim points, with the end points doubled back (a clamped end: 2 v0 - v1 beyond it)
    for i in 1..n {
        let want = (v[id(i - 1, 0)] + v[id(i, 0)] * 4.0 + v[id(i + 1, 0)]) * (1.0 / 6.0);
        let q = i * n; // quad (i, 0), corner 0 at (i, 0)
        assert!(dist(laid.at(q, 0.0, 0.0)[0], want) < 1e-10);
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
            let got = laid.at(i * n, s, 0.0)[0];
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
    for (name, v, f) in cages() {
        for face in CageSurface::of(v.len(), &f).unwrap().faces {
            for b in &face.poles {
                let s: f64 = b.iter().map(|&(_, w)| w).sum();
                assert!(
                    (s - 1.0).abs() < 1e-11,
                    "{name}: a pole's weights add up to {s}"
                );
            }
        }
    }
}

#[test]
fn the_patches_share_their_tangent_planes_along_every_side() {
    // The paper's Theorem 1, whatever the points at a side's ends: irregular ones alone, side by
    // side, at every corner of a quad, on the rim.
    for (name, v, f) in cages() {
        let laid = Laid::of(&v, &f);
        for (&(k, side), &(g, gside)) in &across(&f) {
            for w in ALONG {
                let (s1, t1) = on_side(side, w);
                let (s2, t2) = on_side(gside, 1.0 - w);
                let (a, b) = (laid.at(k, s1, t1), laid.at(g, s2, t2));
                assert!(
                    dist(a[0], b[0]) < 1e-9,
                    "{name}: quads {k} and {g} part by {}",
                    dist(a[0], b[0])
                );
                let (n1, n2) = (cross(a[1], a[2]), cross(b[1], b[2]));
                let sine = norm(cross(n1, n2)) / (norm(n1) * norm(n2));
                assert!(
                    sine < 1e-9 && dot(n1, n2) > 0.0,
                    "{name}: a crease of sine {sine} between quads {k} and {g}"
                );
            }
        }
    }
}

#[test]
fn where_both_ends_of_a_side_are_regular_the_patches_share_their_curvature() {
    // The paper's Lemma 1: there the two patches are one twice-differentiable map, a unit a quad
    // each way, so their derivatives across the side agree to the second.
    let mut sides = 0;
    for (name, v, f) in cages() {
        let laid = Laid::of(&v, &f);
        let regular = regular(v.len(), &f);
        for (&(k, side), &(g, gside)) in &across(&f) {
            if !regular[f[k][side] as usize] || !regular[f[k][(side + 1) % 4] as usize] {
                continue;
            }
            sides += 1;
            for w in ALONG {
                let (s1, t1) = on_side(side, w);
                let (s2, t2) = on_side(gside, 1.0 - w);
                let (a, b) = (
                    into(side, &laid.at(k, s1, t1)),
                    into(gside, &laid.at(g, s2, t2)),
                );
                let size = 1.0 + norm(a.0) + norm(a.1);
                assert!(
                    norm(a.0 + b.0) < 1e-9 * size,
                    "{name}: first derivatives across the side of quads {k} and {g} differ by {}",
                    norm(a.0 + b.0)
                );
                assert!(
                    dist(a.1, b.1) < 1e-8 * size,
                    "{name}: second derivatives across the side of quads {k} and {g} differ by {}",
                    dist(a.1, b.1)
                );
            }
        }
    }
    assert!(sides > 1000);
}

#[test]
fn the_surface_does_not_depend_on_how_the_cage_is_numbered() {
    // The same cage with its points and quads in another order and every quad begun at another
    // corner is the same surface: only the faces it is packed in may differ.
    for (name, v, f) in cages() {
        let laid = Laid::of(&v, &f);
        for seed in [1usize, 2] {
            let n = v.len();
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&a| ((a * 7919 + seed * 104_729) % 9973, a));
            let mut new_of = vec![0u32; n];
            for (new, &old) in order.iter().enumerate() {
                new_of[old] = new as u32;
            }
            let v2: Vec<P> = order.iter().map(|&o| v[o]).collect();
            let mut quads: Vec<usize> = (0..f.len()).collect();
            quads.sort_by_key(|&a| ((a * 6151 + seed * 389) % 7919, a));
            let turn = |new: usize, old: usize| (old * 3 + new + seed) % 4;
            let f2: Vec<[u32; 4]> = quads
                .iter()
                .enumerate()
                .map(|(new, &old)| {
                    let r = turn(new, old);
                    [0, 1, 2, 3].map(|c| new_of[f[old][(r + c) % 4] as usize])
                })
                .collect();
            let other = Laid::of(&v2, &f2);
            for (new, &old) in quads.iter().enumerate() {
                for (s, t) in [(0.2, 0.3), (0.5, 0.5), (0.85, 0.1), (0.0, 0.6), (1.0, 1.0)] {
                    // the new quad begins at the old one's corner `turn`
                    let (s2, t2) = match turn(new, old) {
                        0 => (s, t),
                        1 => (t, 1.0 - s),
                        2 => (1.0 - s, 1.0 - t),
                        _ => (1.0 - t, s),
                    };
                    let d = dist(laid.at(old, s, t)[0], other.at(new, s2, t2)[0]);
                    assert!(
                        d < 1e-9,
                        "{name}: numbered another way, the surface moves {d}"
                    );
                }
            }
        }
    }
}

#[test]
fn at_an_irregular_point_the_surface_has_its_point_tangents_and_twist() {
    // Tangent planes are shared whatever the tangents' lengths and the twists, so these are read
    // off the surface and held to the paper's step 2 worked out here from the cage: with a unit a
    // quad, and coefficients a ninth apart at a corner, the first derivative there is
    // 9 (b10 - b00) and the mixed one 81 (b11 - b10 - b01 + b00). The tangents' length is the
    // paper's at three and five quads, and from there on as many steps of the cage as at five
    // (the module doc).
    for n in [3usize, 5, 6, 7, 8] {
        let k = 3;
        let (v, f) = fan(n, k);
        let laid = Laid::of(&v, &f);
        let nf = n as f64;
        let turn = std::f64::consts::TAU / nf;
        // the middle point's neighbours: sector l's quad at the middle is quad l k^2, its corner
        // 1 the end of edge l and its corner 2 the diagonal between edges l and l + 1
        let spoke = |l: usize| v[f[(l % n) * k * k][1] as usize];
        let diagonal = |l: usize| v[f[(l % n) * k * k][2] as usize];
        let mut b00 = v[0] * (nf / (nf + 5.0));
        for l in 0..n {
            b00 += spoke(l) * (4.0 / (nf * (nf + 5.0))) + diagonal(l) * (1.0 / (nf * (nf + 5.0)));
        }
        let c = turn.cos();
        let lambda = (c + 5.0 + ((c + 9.0) * (c + 1.0)).sqrt()) / 16.0;
        let omega = 16.0 * lambda - 4.0;
        let sigma = if n == 3 {
            0.53
        } else {
            steps_at_five() / steps_per_sigma(n)
        };
        let (mut e1, mut e2) = (p(0.0, 0.0, 0.0), p(0.0, 0.0, 0.0));
        for l in 0..n {
            let (a, b) = (turn * l as f64, turn * (l as f64 + 1.0));
            e1 += spoke(l) * (omega * a.cos()) + diagonal(l) * (a.cos() + b.cos());
            e2 += spoke(l) * (omega * a.sin()) + diagonal(l) * (a.sin() + b.sin());
        }
        let scale = sigma / (3.0 * (2.0 + omega));
        let b10 = |l: usize| {
            let a = turn * l as f64;
            b00 + (e1 * a.cos() + e2 * a.sin()) * (scale / 3.0)
        };
        for l in 0..n {
            let got = laid.at(l * k * k, 0.0, 0.0);
            let q11 = (v[0] * 4.0 + (spoke(l) + spoke(l + 1)) * 2.0 + diagonal(l)) * (1.0 / 9.0);
            let b11 = ((b10(l) + b10(l + 1)) * 6.0 - b00 * 4.0 + q11) * (1.0 / 9.0);
            let want = [
                b00,
                (b10(l) - b00) * 9.0,
                (b10(l + 1) - b00) * 9.0,
                (b11 - b10(l) - b10(l + 1) + b00) * 81.0,
            ];
            for (which, (got, want)) in [got[0], got[1], got[2], got[4]]
                .iter()
                .zip(want)
                .enumerate()
            {
                assert!(
                    dist(*got, want) < 1e-9 * (1.0 + norm(want)),
                    "{n} quads, quad {l}, derivative {which}: {} off",
                    dist(*got, want)
                );
            }
        }
    }
}

/// Catmull-Clark's subdominant eigenvalue at a point of `n` quads and `cos(2 pi / n)`.
fn eigen(n: usize) -> (f64, f64) {
    let c = (std::f64::consts::TAU / n as f64).cos();
    ((c + 5.0 + ((c + 9.0) * (c + 1.0)).sqrt()) / 16.0, c)
}

/// The steps of the cage step 2's tangent is long for each unit of `sigma`, the point's ring laid
/// out as the tangent rule reads it: summing the rule's weights over the edges' ends `d_l` and the
/// far corners `(d_l + d_l+1) / (4 lambda - 1)` gives `n (omega / 2 + (1 + c) / (4 lambda - 1)) /
/// (2 + omega)`.
fn steps_per_sigma(n: usize) -> f64 {
    let (lambda, c) = eigen(n);
    let omega = 16.0 * lambda - 4.0;
    n as f64 * (omega / 2.0 + (1.0 + c) / (4.0 * lambda - 1.0)) / (2.0 + omega)
}

/// The paper's tangent at five quads in steps of the cage: its `sigma` there is `1 / (4 lambda)`.
fn steps_at_five() -> f64 {
    steps_per_sigma(5) / (4.0 * eigen(5).0)
}

#[test]
fn from_five_quads_on_the_tangent_is_as_many_steps_of_the_cage_as_at_five() {
    // A flat cage whose ring round the point lies as the tangent rule reads it (the edges' ends a
    // step out at the turns 2 pi k / n, each quad's far corner at (d_k + d_k+1) / (4 lambda - 1)):
    // the surface's first derivative along edge k, a unit a quad, is that step times the paper's
    // length at five, 1.17, whatever the number of quads.
    assert!((steps_at_five() - 1.1668).abs() < 1e-4);
    for n in [5usize, 6, 7, 8, 10, 12] {
        let k = 3;
        let (mut v, f) = fan(n, k);
        let turn = std::f64::consts::TAU / n as f64;
        let far = 1.0 / (4.0 * eigen(n).0 - 1.0);
        let step = 7.0;
        let d = |l: usize| {
            let a = turn * l as f64;
            p(step * a.cos(), step * a.sin(), 0.0)
        };
        for pt in &mut v {
            *pt = p(pt.x, pt.y, 0.0);
        }
        v[0] = p(0.0, 0.0, 0.0);
        for l in 0..n {
            let quad = f[l * k * k];
            v[quad[1] as usize] = d(l);
            v[quad[2] as usize] = (d(l) + d(l + 1)) * far;
        }
        let laid = Laid::of(&v, &f);
        for l in 0..n {
            let (got, want) = (laid.at(l * k * k, 0.0, 0.0)[1], d(l) * steps_at_five());
            assert!(
                dist(got, want) < 1e-9,
                "{n} quads: the tangent along edge {l} is {} off",
                dist(got, want)
            );
        }
    }
}

#[test]
fn a_side_from_an_irregular_point_is_twice_differentiable_at_its_joints() {
    // Along a side with an irregular end, in the paper's subscripts from that end: the first two
    // pieces meet in their second derivative, b50 = 2 b40 - 2 b20 + b10 (pieces a third long, their
    // joint (b20 + b40) / 2). Where the far end is irregular too the same holds from it; where it
    // is regular the middle piece is a quadratic, b50 = b40 + (b70 - b20) / 5, which is what lets
    // the tangent planes be shared there (the paper's eq. 15).
    let mut checked = [0usize; 2];
    for (name, v, f) in cages() {
        let (points, quads) = named(v.len(), &f).unwrap();
        let built = build(points, quads).unwrap();
        let regular = regular(v.len(), &f);
        for (q, quad) in f.iter().enumerate() {
            let grid = built.patches.grid(q).unwrap();
            for c in 0..4 {
                let (here, there) = (quad[c] as usize, quad[(c + 1) % 4] as usize);
                // the side from corner c to c + 1, from either end
                let from = |g: usize| coefficient(&v, grid, c, g, 0);
                let back = |g: usize| coefficient(&v, grid, c, 9 - g, 0);
                let close = |a: P, b: P, what: &str| {
                    assert!(
                        dist(a, b) < 1e-9 * (1.0 + norm(a)),
                        "{name}: quad {q} side {c}: {what} is {} off",
                        dist(a, b)
                    );
                };
                for (near, far, b) in [
                    (here, there, &from as &dyn Fn(usize) -> P),
                    (there, here, &back),
                ] {
                    if regular[near] {
                        continue;
                    }
                    close(
                        b(5),
                        b(4) * 2.0 - b(2) * 2.0 + b(1),
                        "the first joint's second derivative",
                    );
                    checked[0] += 1;
                    if regular[far] {
                        close(
                            b(5),
                            b(4) + (b(7) - b(2)) * 0.2,
                            "the middle piece as a quadratic",
                        );
                        checked[1] += 1;
                    }
                }
            }
        }
    }
    assert!(checked.iter().all(|&n| n > 100), "{checked:?}");
}

/// A built patch's coefficient at the paper's subscripts `(i, j)` (0, 1, 2, 4, 5, 7, 8, 9), seen
/// from corner `c` of its quad with `i` along the side to the next corner.
fn coefficient(v: &[P], grid: &[Blend], c: usize, i: usize, j: usize) -> P {
    let at = |g: usize| [0, 1, 2, 9, 3, 4, 9, 5, 6, 7][g];
    let (i, j) = (at(i), at(j));
    let (a, b) = [(i, j), (7 - j, i), (7 - i, 7 - j), (j, 7 - i)][c];
    grid[a * 8 + b]
        .iter()
        .fold(p(0.0, 0.0, 0.0), |s, &(k, w)| s + v[k as usize] * w)
}

#[test]
fn the_rows_inside_follow_the_papers_steps_4a_and_5() {
    // What tangent planes do not settle, held to the paper's formulas written out here. Step 4a:
    // across a side with an irregular end, the two patches' first rows differ by what the single
    // cubics through each row's ends differ by. Step 5: the four middle points, the row third in
    // from a side with an irregular end, and the point (2, 2) at an irregular corner.
    let mut checked = [0usize; 4];
    for (name, v, f) in cages() {
        let (points, quads) = named(v.len(), &f).unwrap();
        let built = build(points, quads).unwrap();
        let regular = regular(v.len(), &f);
        let close = |a: P, b: P, what: &str| {
            assert!(
                dist(a, b) < 1e-9 * (1.0 + norm(a)),
                "{name}: {what} is {} off",
                dist(a, b)
            );
        };
        let beside = across(&f);
        for (q, quad) in f.iter().enumerate() {
            let grid = built.patches.grid(q).unwrap();
            let at = |c: usize, i: usize, j: usize| coefficient(&v, grid, c % 4, i, j);
            let irregular: Vec<bool> = quad.iter().map(|&x| !regular[x as usize]).collect();
            if !irregular.contains(&true) {
                continue;
            }
            for c in 0..4 {
                // the single cubic through a row's ends, at the row's places 2 and 4
                let plain = |x0: P, x1: P, x8: P, x9: P| {
                    [
                        x0 * (-4.0 / 9.0) + x1 * (4.0 / 3.0) + x8 * (1.0 / 3.0) + x9 * (-2.0 / 9.0),
                        x0 * (-20.0 / 27.0) + x1 * (4.0 / 3.0) + x8 + x9 * (-16.0 / 27.0),
                    ]
                };
                if irregular[c] {
                    if let Some(&(g, gside)) = beside.get(&(q, c)) {
                        // the quad across side c has this side from its corner gside + 1 back
                        let other = built.patches.grid(g).unwrap();
                        let theirs =
                            |i: usize, j: usize| coefficient(&v, other, (gside + 1) % 4, j, i);
                        let mine = plain(at(c, 0, 1), at(c, 1, 1), at(c, 8, 1), at(c, 9, 1));
                        let yours = plain(theirs(0, 1), theirs(1, 1), theirs(8, 1), theirs(9, 1));
                        for (k, place) in [2, 4].into_iter().enumerate() {
                            close(
                                at(c, place, 1) - theirs(place, 1),
                                mine[k] - yours[k],
                                "the first rows' difference (4a)",
                            );
                            checked[0] += 1;
                        }
                    }
                }
                let middle =
                    (at(c, 0, 4) * (-20.0 / 27.0) + at(c, 1, 4) * (4.0 / 3.0) + at(c, 8, 4)
                        - at(c, 9, 4) * (16.0 / 27.0)
                        + at(c, 4, 0) * (-20.0 / 27.0)
                        + at(c, 4, 1) * (4.0 / 3.0)
                        + at(c, 4, 8)
                        - at(c, 4, 9) * (16.0 / 27.0))
                        * 0.5;
                close(at(c, 4, 4), middle, "a middle point (5)");
                checked[1] += 1;
                if irregular[c] || irregular[(c + 1) % 4] {
                    for g in [4, 5] {
                        let want = at(c, g, 1) * 0.5 + at(c, g, 4) - at(c, g, 5) * 0.5;
                        close(at(c, g, 2), want, "the row third in (5)");
                        checked[2] += 1;
                    }
                }
                if irregular[c] {
                    let want = (at(c, 1, 2) * 0.5 + at(c, 4, 2) - at(c, 5, 2) * 0.5) * 0.5
                        + (at(c, 2, 1) * 0.5 + at(c, 2, 4) - at(c, 2, 5) * 0.5) * 0.5;
                    close(
                        at(c, 2, 2),
                        want,
                        "the point (2, 2) at an irregular corner (5)",
                    );
                    checked[3] += 1;
                }
            }
        }
    }
    assert!(checked.iter().all(|&n| n > 100), "{checked:?}");
}

// ---- the faces ----------------------------------------------------------------------------------

#[test]
fn every_face_is_its_quads_own_patches() {
    // A face's poles are read off its quads' patches, so the face is those patches wherever it is
    // read: the layout packs the surface and never changes it.
    let own: Vec<f64> = [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 3.0, 3.0]
        .iter()
        .map(|k| k / 3.0)
        .collect();
    for (name, v, f) in cages() {
        let laid = Laid::of(&v, &f);
        let (points, quads) = named(v.len(), &f).unwrap();
        let built = build(points, quads).unwrap();
        for q in 0..f.len() {
            // the quad's own (a, b) is at a * 8 + b; `patch` wants its first way fastest
            let grid = built.patches.grid(q).unwrap();
            let poles: Vec<P> = (0..64)
                .map(|k| {
                    let blend = &grid[(k % 8) * 8 + k / 8];
                    blend
                        .iter()
                        .fold(p(0.0, 0.0, 0.0), |s, &(i, w)| s + v[i as usize] * w)
                })
                .collect();
            for (s, t) in [
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.15, 0.2),
                (0.5, 0.45),
                (0.7, 0.9),
                (1.0 / 3.0, 0.6),
                (0.4, 2.0 / 3.0),
                (0.95, 0.05),
            ] {
                let want = patch([&own, &own], &poles, s, t, [s > 0.5, t > 0.5]);
                let got = laid.at(q, s, t);
                for (k, (a, b)) in want.iter().zip(&got).take(3).enumerate() {
                    assert!(
                        dist(*a, *b) < 1e-9 * (1.0 + norm(*a)),
                        "{name}: quad {q}'s face is {} off its patch (derivative {k})",
                        dist(*a, *b)
                    );
                }
            }
        }
    }
}

#[test]
fn a_regular_open_grid_is_one_face_whose_poles_are_its_own_points() {
    // No irregular point, so no border: one face. No quad has knots of its own and each joint is
    // one knot, so the knots are 0 0 0 0 1 2 3 4 5 5 5 5 and there are 8 poles each way. They
    // are the uniform B-spline's (the cage's 6 points d0..d5 each way and, outside the rim, the
    // mirror point 2 d0 - d1) with its end knots made four-fold: reading that spline at the new
    // knots, (0, 0, 0) is its end (2 d0 - d1 + 4 d0 + d1) / 6 = d0, (0, 0, 1) is (2 d0 + d1) / 3,
    // (0, 1, 2) is d1, and so on to the far end: d0, (2 d0 + d1) / 3, d1, d2, d3, d4,
    // (d4 + 2 d5) / 3, d5.
    let n = 5;
    let (v, f) = grid(n);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 1);
    let face = &surf.faces[0];
    assert_eq!(face.size, [n, n]);
    let knots = [0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 5.0, 5.0, 5.0];
    assert_eq!(face.knots_u.len(), 12);
    assert_eq!(face.knots_v.len(), 12);
    for (got, want) in face
        .knots_u
        .iter()
        .chain(&face.knots_v)
        .zip(knots.iter().chain(&knots))
    {
        assert!((got - want).abs() < 1e-12);
    }
    assert_eq!(face.poles.len(), 64);
    // the cage point at each place of the face, from the quads laid there
    let at = |x: usize, y: usize| -> P {
        let (iu, iv) = (x.min(n - 1), y.min(n - 1));
        let (q, rot) = face.quads[iu + n * iv];
        let corner = [[0, 3], [1, 2]][x - iu][y - iv];
        v[f[q][(usize::from(rot) + corner) % 4] as usize]
    };
    // each pole one way as weights on the cage's points that way
    let made = |a: usize| -> Vec<(usize, f64)> {
        match a {
            0 => vec![(0, 1.0)],
            1 => vec![(0, 2.0 / 3.0), (1, 1.0 / 3.0)],
            6 => vec![(n - 1, 1.0 / 3.0), (n, 2.0 / 3.0)],
            7 => vec![(n, 1.0)],
            _ => vec![(a - 1, 1.0)],
        }
    };
    let poles = face.points(&v).unwrap();
    for b in 0..8 {
        for a in 0..8 {
            let mut want = p(0.0, 0.0, 0.0);
            for &(x, wx) in &made(a) {
                for &(y, wy) in &made(b) {
                    want += at(x, y) * (wx * wy);
                }
            }
            let d = dist(poles[a + 8 * b], want);
            assert!(d < 1e-10, "pole ({a}, {b}) is {d} off");
            // and a pole that is one cage point is that point alone
            if made(a).len() == 1 && made(b).len() == 1 {
                let big: Vec<f64> = face.poles[a + 8 * b]
                    .iter()
                    .map(|&(_, w)| w)
                    .filter(|w| w.abs() > 1e-12)
                    .collect();
                assert!(big.len() == 1 && (big[0] - 1.0).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn faces_follow_the_layout() {
    let poles_each_way = |face: &CageFace| [face.knots_u.len() - 4, face.knots_v.len() - 4];
    // The cube: every side has an irregular end, so every quad is a face; every quad has knots of
    // its own, 0 0 0 0 1/3 1/3 2/3 2/3 1 1 1 1, so 8 poles each way.
    let (v, f) = cube();
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 6);
    for face in &surf.faces {
        assert_eq!(face.size, [1, 1]);
        assert_eq!(poles_each_way(face), [8, 8]);
        assert_eq!(face.poles.len(), 64);
    }
    // The cube split once: the borders run from each corner through an edge's middle point to
    // the next corner, along the cube's edges, so each of the cube's faces is one face of 2 x 2
    // quads. Each quad has a corner of the cube, so each has knots of its own: 4 + 4 + 1 + 4 + 4
    // knots, 13 poles each way.
    let (v1, f1) = split(&v, &f, 0.3);
    let surf = CageSurface::of(v1.len(), &f1).unwrap();
    assert_eq!(surf.faces.len(), 6);
    for face in &surf.faces {
        assert_eq!(face.size, [2, 2]);
        assert_eq!(poles_each_way(face), [13, 13]);
    }
    // One point of five quads, four quads out to the rim: five borders from it to the rim, five
    // faces of 4 x 4 quads. In each only the quad at the middle point is irregular, so one column
    // and one row have knots of their own: 4 + 4 + 3 + 4 knots, 11 poles each way.
    let (v, f) = fan(5, 4);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 5);
    for face in &surf.faces {
        assert_eq!(face.size, [4, 4]);
        assert_eq!(poles_each_way(face), [11, 11]);
    }
    // A closed regular cage has no border of its own and can be no one face (a face would meet
    // itself): it is cut both ways through its first quad and both ways through the quad farthest
    // from it, (6, 4) of 12 x 8, into four faces of 6 x 4 quads with 9 x 7 poles.
    let (v, f) = torus(12, 8);
    let surf = CageSurface::of(v.len(), &f).unwrap();
    assert_eq!(surf.faces.len(), 4);
    for face in &surf.faces {
        let mut size = face.size;
        size.sort_unstable();
        assert_eq!(size, [4, 6]);
        assert_eq!(face.poles.len(), 9 * 7);
    }
    // whatever the layout, every quad is in one face (Laid checks it)
    for (_, v, f) in cages() {
        Laid::of(&v, &f);
    }
}

#[test]
fn inside_a_face_a_joint_between_quads_is_one_knot() {
    for (name, v, f) in cages() {
        for face in CageSurface::of(v.len(), &f).unwrap().faces {
            for (knots, quads) in [(&face.knots_u, face.size[0]), (&face.knots_v, face.size[1])] {
                assert_eq!(
                    knots.len() - 4,
                    face.poles.len() / (other(&face, knots) - 4)
                );
                for joint in 1..quads {
                    let here = knots
                        .iter()
                        .filter(|&&k| (k - joint as f64).abs() < 1e-12)
                        .count();
                    assert_eq!(here, 1, "{name}: {here} knots at a joint");
                }
                // nothing but the joints, and thirds twice over inside a quad
                for &k in knots {
                    let third = (3.0 * k).round();
                    assert!((3.0 * k - third).abs() < 1e-12);
                    let times = knots.iter().filter(|&&x| (x - k).abs() < 1e-12).count();
                    let whole = (third / 3.0 - (third / 3.0).round()).abs() < 1e-12;
                    let end = k < 1e-12 || (k - quads as f64).abs() < 1e-12;
                    assert_eq!(
                        times,
                        if end {
                            4
                        } else if whole {
                            1
                        } else {
                            2
                        }
                    );
                }
            }
        }
    }
}

/// The length of the face's other knot vector.
fn other(face: &CageFace, knots: &[f64]) -> usize {
    if std::ptr::eq(knots, face.knots_u.as_slice()) {
        face.knots_v.len()
    } else {
        face.knots_u.len()
    }
}

// ---- what is refused ----------------------------------------------------------------------------

#[test]
fn bad_cages_are_refused() {
    let refused = |points: usize, quads: &[[u32; 4]]| {
        matches!(
            CageSurface::of(points, quads),
            Err(SolidError::InvalidInput { .. })
        )
    };
    assert!(refused(4, &[]));
    assert!(refused(3, &[[0, 1, 2, 3]]));
    assert!(refused(4, &[[0, 1, 1, 3]]));
    // two quads running the same way along an edge
    assert!(refused(6, &[[0, 1, 2, 3], [0, 1, 4, 5]]));
    // two quads that touch at one point only: two rims meet there
    assert!(refused(7, &[[0, 1, 2, 3], [0, 4, 5, 6]]));
    // a point inside with two quads round it: the paper's corner needs three, and with two its
    // tangents are nothing (every cosine of the step-2 sums cancels), a surface with no normal
    assert!(refused(5, &[[0, 1, 2, 3], [2, 1, 0, 4]]));
    // one quad alone is a cage: its four corners kept, its rim the B-spline of four points
    let surf = CageSurface::of(4, &[[0, 1, 2, 3]]).unwrap();
    assert_eq!(surf.faces.len(), 1);
    // a pole naming a point the caller does not give
    assert!(surf.faces[0].points(&[p(0.0, 0.0, 0.0)]).is_err());
    // the work is the quads' size, whatever number of points is claimed
    assert!(CageSurface::of(usize::MAX, &[[0, 1, 2, 3]]).is_ok());
}
