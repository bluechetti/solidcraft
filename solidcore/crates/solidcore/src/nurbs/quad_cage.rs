//! The smooth surface of a quad cage: one bicubic B-spline patch a quad, tangent-continuous
//! everywhere, curvature-continuous wherever four quads meet at both ends of an edge, and packed
//! into as few faces as that allows.
//!
//! Upstream: none (solidcore-native). The patches are those of J. Fan and J. Peters, *Smooth
//! bi-3 spline surfaces with fewest knots*, Computer-Aided Design 43(2), 2011, Sections 2.1-2.8:
//! every formula below names the paper's step it is, and two numbers the paper chooses are chosen
//! otherwise (below). The rim of an open cage and the packing of the patches into faces are this
//! file's own.
//!
//! | Rust                        | Upstream |
//! |-----------------------------|----------|
//! | [`CageSurface::of`]         | none     |
//! | [`CageFace`]                | none     |
//! | [`CageFace::points`]        | none     |
//!
//! **The patches.** A point with four quads round it (or, on the rim, two) is *regular*; any
//! other is *irregular*. A quad whose four corners are regular is the uniform bicubic B-spline of
//! the sixteen cage points round it, one polynomial piece. A quad with an irregular corner is
//! three pieces by three (double knots at a third and two thirds, both ways), built in the
//! paper's five steps:
//! 1. its Bézier patch (eq. 4-7), each corner the Catmull-Clark limit point of the cage's point
//!    there (eq. 5), with the knots put in;
//! 2. at an irregular corner the two tangent points, so that all the quads round the corner have
//!    one tangent plane there (their length from six quads on is this file's, below), and the
//!    twist point from them;
//! 3. the side: along a side with an irregular end, the points that make the two patches
//!    meeting there share their tangent planes at that end (3b where both ends are irregular,
//!    3c where the far end is regular: there the far third of the side is left as the regular
//!    patches have it, and `lambda_1` is this file's, below);
//! 4. the first row in from such a side, so the two patches share their tangent planes all
//!    along it (4a, then 4b or 4c);
//! 5. the points inside, by the paper's averages: all of them only at an irregular corner, the
//!    row third in from a side only where that side has an irregular end.
//!
//! The paper proves the result (its Theorem 1): the patches of two quads share their tangent
//! planes along every side, and where both ends of a side are regular they also share their
//! curvature (the three rows of points each side of it are the regular ones, untouched). A corner,
//! a tangent point and a side with an irregular end are worked out once and given to every patch
//! that has them.
//!
//! **Two numbers of the paper's chosen otherwise.** Both are choices the paper makes (its 2.6 "we
//! choose", its 2.9 "set heuristically"); neither touches its proof, which holds for any value of
//! either.
//! - Along a side from an irregular point to a regular one (3c, 4c) the paper's `alpha` falls
//!   from `lambda_0` to `lambda_1 = lambda_0 / 2` over the first third. Here `lambda_1 =
//!   lambda_0 / 3`. For any `lambda_1 = r lambda_0` the step's conditions (the middle piece a
//!   quadratic, `b50 = b40 + (b70 - b20) / 5`, and the two sides of eq. 2 meeting in their
//!   derivative at the first joint) give `b40 = ((153 r - 15) b20 + 12 r b70 - 60 r b10) /
//!   (5 (21 r - 3))` and, in 4c, `h2 = b40 + lambda_1 (b70 - b50) / 12`; a third is the one value
//!   at which the side itself is twice differentiable at that joint (`b40 = 9/5 b20 + 1/5 b70 -
//!   b10`, `b50 = 8/5 b20 + 2/5 b70 - b10`).
//! - The tangents' length at a point of six or more quads. The paper's `sigma = 1 / (4 lambda)`
//!   makes a tangent 1.17 steps of the cage long at five quads, 1.34 at six and 2.49 at twelve,
//!   and the side has to slow by as much before its far end. Here the tangent stays 1.17 steps
//!   long from five quads on ([`sigma`]): the paper's own at five, unchanged there, and at three
//!   (0.72 steps).
//!
//! Why: with the paper's numbers the surface's curvature in the quads at an irregular point is
//! less even, spiking at a side's first joint (twice a dome's own curvature at eight quads) and
//! dipping in its middle third. Measured on balls and on domes, bowls and saddles over fans of
//! three layouts (a harness outside this repository), the typical error of the two curvatures in
//! those quads, as a share of the shape's own curvature, falls from 24 % to 19 % at three quads,
//! 16 % to 10 % at five, 19 % to 17 % at six, 107 % to 55 % at twelve, and is level at seven and
//! eight (23 % and 28 %), where the worst error falls instead (64 % to 54 %, 72 % to 68 %). On
//! shapes the numbers were not chosen on it does the same.
//!
//! **Faces.** The surface is the patches; faces only pack them, exactly. Two patches can be one
//! B-spline face only where they share their curvature, so the faces' borders are the sides with
//! an irregular end. Each is run straight on through regular points to the next irregular point
//! or the rim, which leaves rectangles of quads (a region with no irregular point at all, a ring
//! or a whole torus, is cut along straight lines until its parts are rectangles that do not meet
//! themselves); neighbouring rectangles are then joined while their union is still one. Inside a
//! face a joint between two quads is a single knot, and a row or column of regular quads has no
//! knots of its own: a regular open grid of `m` x `n` quads is one face of `(m + 3)` x `(n + 3)`
//! poles. Each pole is its patch's own polynomial read at the pole's three knots (the blossom),
//! so a face is its quads' patches to rounding, whatever the layout: how the cage's points and
//! quads are numbered can change which faces there are, never the surface.
//!
//! **The rim.** An open cage is first given two rings of points outside its rim, each the
//! mirror of the cage in its rim: across a rim point with one edge into the cage, `2 v - inner`;
//! across a corner of one quad, `2 v - a`, `2 v - b` and (both ways at once) `4 v - 2 a - 2 b +
//! c`; across a rim point with more edges into the cage, `2 v` less their mean. With them a
//! regular rim is the cubic B-spline of the rim's own points, and a corner of one quad stays where
//! it is.
//!
//! Every pole is a fixed blend of the cage's points ([`Blend`]) whose weights add up to one, so a
//! fit can take the cage's points as its unknowns.
//!
//! Deviations: none (no upstream). Every loop is bounded by the cage's size; nothing recurses.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Result, SolidError};
use crate::math::Point3;

const OP: &str = "nurbs::quad_cage";

/// A pole as a blend of cage points: `(point, weight)`, by point, each point once.
pub type Blend = Vec<(u32, f64)>;

fn bad(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

fn fail(reason: &'static str) -> SolidError {
    SolidError::port_invariant(OP, reason)
}

/// `x`, or the failure of an index outside its array (none is by construction).
fn within<T>(x: Option<T>) -> Result<T> {
    x.ok_or_else(|| fail("an index outside its array"))
}

/// Weights under this are dropped from a blend.
const NEGLIGIBLE: f64 = 1e-15;

/// The paper's `sigma` for three quads at a corner (its step 2); see [`sigma`].
const SIGMA3: f64 = 0.53;

/// How many times a region that is no rectangle is cut before its quads are faces of their own.
const MOST_CUTS: usize = 64;

/// The sum of the weighted blends.
fn lin(terms: &[(f64, &Blend)]) -> Blend {
    let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
    for &(w, b) in terms {
        if w == 0.0 {
            continue;
        }
        for &(i, x) in b {
            *acc.entry(i).or_insert(0.0) += w * x;
        }
    }
    acc.into_iter()
        .filter(|&(_, x)| x.abs() > NEGLIGIBLE)
        .collect()
}

/// `i` taken round a ring of `n`.
fn wrap(i: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut i = i;
    while i >= n {
        i -= n;
    }
    i
}

/// Catmull-Clark's numbers at a point of `n` quads, as the paper's step 2 has them: `cos(2 pi /
/// n)`, the subdominant eigenvalue `lambda` and `omega = 16 lambda - 4`.
fn ring(n: usize) -> (f64, f64, f64) {
    let c = (std::f64::consts::TAU / n as f64).cos();
    let lam = (c + 5.0 + ((c + 9.0) * (c + 1.0)).sqrt()) / 16.0;
    (c, lam, 16.0 * lam - 4.0)
}

/// How many steps of the cage long step 2 makes a tangent at a point of `n` quads, for each unit
/// of its `sigma`: with the point's ring laid out as the tangent rule itself reads it (the edges'
/// ends a step out at the turns `2 pi k / n`, each quad's far corner at `(d_k + d_k+1) /
/// (4 lambda - 1)`, Catmull-Clark's own layout), the rule's sums come to `n (omega / 2 +
/// (1 + cos(2 pi / n)) / (4 lambda - 1)) / (2 + omega)`. At four quads that is 2, and the
/// B-spline's `sigma` of a half makes the tangent one step.
fn steps_per_sigma(n: usize) -> f64 {
    let (c, lam, omega) = ring(n);
    n as f64 * (omega / 2.0 + (1.0 + c) / (4.0 * lam - 1.0)) / (2.0 + omega)
}

/// Step 2's `sigma` at an irregular point of `n` quads: the paper's 0.53 at three; from five on,
/// the one that makes the tangent as many steps of the cage long as the paper's `1 / (4 lambda)`
/// makes it at five, 1.17 (the module doc).
fn sigma(n: usize) -> f64 {
    if n == 3 {
        return SIGMA3;
    }
    let at_five = steps_per_sigma(5) / (4.0 * ring(5).1);
    at_five / steps_per_sigma(n)
}

/// `2 cos(2 pi / n)`: the paper's `lambda_0` at a corner of `n` quads (nothing at four).
fn lambda(n: usize) -> f64 {
    2.0 * (std::f64::consts::TAU / n as f64).cos()
}

// ---- the cage -----------------------------------------------------------------------------------

/// The quads round a point, in order: each quad's side back to the point is the next quad's side
/// out of it.
#[derive(Clone, Default)]
struct Fan {
    /// `(quad, the quad's corner at the point)`.
    quads: Vec<(usize, usize)>,
    /// The fan closes on itself: the point is inside the cage.
    closed: bool,
}

/// A quad mesh: its points as blends of the caller's, its quads, and the quads round each point.
struct Cage {
    points: Vec<Blend>,
    quads: Vec<[usize; 4]>,
    /// The quad that runs from `a` to `b` along one of its sides, and that side (side `k` runs
    /// from corner `k` to corner `k + 1`).
    along: BTreeMap<(usize, usize), (usize, usize)>,
    fans: Vec<Fan>,
}

type Edge = (usize, usize);

fn edge(a: usize, b: usize) -> Edge {
    (a.min(b), a.max(b))
}

impl Cage {
    fn new(points: Vec<Blend>, quads: Vec<[usize; 4]>) -> Result<Self> {
        let n = points.len();
        let mut along = BTreeMap::new();
        let mut at: Vec<Vec<(usize, usize)>> = Vec::new();
        at.resize_with(n, Vec::new);
        for (f, q) in quads.iter().enumerate() {
            for (c, &v) in q.iter().enumerate() {
                if q.iter().filter(|&&w| w == v).count() != 1 {
                    return Err(bad("a quad names one point twice"));
                }
                let w = *within(q.get((c + 1) % 4))?;
                within(at.get_mut(v))?.push((f, c));
                if along.insert((v, w), (f, c)).is_some() {
                    return Err(bad(
                        "two quads run the same way along one edge: the quads must all face one way, two to an edge",
                    ));
                }
            }
        }
        let mut cage = Cage {
            points,
            quads,
            along,
            fans: Vec::new(),
        };
        let mut fans = Vec::new();
        for (v, here) in at.iter().enumerate() {
            fans.push(cage.fan_of(v, here)?);
        }
        cage.fans = fans;
        Ok(cage)
    }

    fn vert(&self, f: usize, c: usize) -> Result<usize> {
        within(self.quads.get(f).and_then(|q| q.get(c % 4)).copied())
    }

    /// The quads `here` at `v`, in order round it.
    fn fan_of(&self, v: usize, here: &[(usize, usize)]) -> Result<Fan> {
        let Some(&first) = here.first() else {
            return Ok(Fan::default());
        };
        // a quad nothing comes before: no quad runs back to `v` along its side out of `v`
        let mut starts = Vec::new();
        for &(f, c) in here {
            if !self.along.contains_key(&(self.vert(f, c + 1)?, v)) {
                starts.push((f, c));
            }
        }
        if starts.len() > 1 {
            return Err(bad("two rims of the cage meet at one point"));
        }
        let start = starts.first().copied().unwrap_or(first);
        let mut quads = vec![start];
        let mut closed = false;
        let mut cur = start;
        while quads.len() <= here.len() {
            match self.along.get(&(v, self.vert(cur.0, cur.1 + 3)?)) {
                None => break,
                Some(&next) if next == start => {
                    closed = true;
                    break;
                }
                Some(&next) => {
                    quads.push(next);
                    cur = next;
                }
            }
        }
        if quads.len() != here.len() {
            return Err(bad("two sheets of the cage meet at one point"));
        }
        if closed && quads.len() < 3 {
            return Err(bad(
                "a point inside the cage has fewer than three quads round it",
            ));
        }
        Ok(Fan { quads, closed })
    }

    fn fan(&self, v: usize) -> Result<&Fan> {
        within(self.fans.get(v))
    }

    fn point(&self, v: usize) -> Result<&Blend> {
        within(self.points.get(v))
    }

    fn open(&self) -> bool {
        self.along
            .keys()
            .any(|&(a, b)| !self.along.contains_key(&(b, a)))
    }

    /// The quad across side `side` of quad `f`, and its side there.
    fn across(&self, f: usize, side: usize) -> Result<Option<(usize, usize)>> {
        let (a, b) = (self.vert(f, side)?, self.vert(f, side + 1)?);
        Ok(self.along.get(&(b, a)).copied())
    }

    /// The point straight on from `prev` through `cur`, a point with four quads round it.
    fn straight_on(&self, prev: usize, cur: usize) -> Result<Option<usize>> {
        let fan = self.fan(cur)?;
        if !fan.closed || fan.quads.len() != 4 {
            return Ok(None);
        }
        for (k, &(f, c)) in fan.quads.iter().enumerate() {
            if self.vert(f, c + 1)? == prev {
                let &(g, cg) = within(fan.quads.get((k + 2) % 4))?;
                return Ok(Some(self.vert(g, cg + 1)?));
            }
        }
        Ok(None)
    }

    /// This cage with one ring of points outside its rim, the mirror of the cage in the rim (see
    /// the module doc).
    fn ringed(&self) -> Result<Cage> {
        let mut points = self.points.clone();
        let mut quads = self.quads.clone();
        // per rim point: its mirror point(s); at a corner of one quad `(ga, gb, a, gd)`
        let mut beyond: BTreeMap<usize, usize> = BTreeMap::new();
        let mut corners: BTreeMap<usize, (usize, usize, usize, usize)> = BTreeMap::new();
        for (v, fan) in self.fans.iter().enumerate() {
            if fan.closed || fan.quads.is_empty() {
                continue;
            }
            let pv = self.point(v)?;
            if let [(f, c)] = fan.quads.as_slice() {
                let (a, d, b) = (
                    self.vert(*f, c + 1)?,
                    self.vert(*f, c + 2)?,
                    self.vert(*f, c + 3)?,
                );
                let (pa, pd, pb) = (self.point(a)?, self.point(d)?, self.point(b)?);
                let ga = points.len();
                points.push(lin(&[(2.0, pv), (-1.0, pa)]));
                let gb = points.len();
                points.push(lin(&[(2.0, pv), (-1.0, pb)]));
                let gd = points.len();
                points.push(lin(&[(4.0, pv), (-2.0, pa), (-2.0, pb), (1.0, pd)]));
                corners.insert(v, (ga, gb, a, gd));
                continue;
            }
            // the edges into the cage: every quad's side back to `v` but the last quad's
            let inner = fan.quads.len() - 1;
            let share = -1.0 / inner as f64;
            let mut terms = vec![(2.0, pv)];
            for &(f, c) in fan.quads.iter().take(inner) {
                terms.push((share, self.point(self.vert(f, c + 3)?)?));
            }
            beyond.insert(v, points.len());
            points.push(lin(&terms));
        }
        let mirror = |v: usize, other: usize| -> Result<usize> {
            if let Some(&(ga, gb, a, _)) = corners.get(&v) {
                return Ok(if other == a { gb } else { ga });
            }
            within(beyond.get(&v).copied())
        };
        for &(a, b) in self.along.keys() {
            if !self.along.contains_key(&(b, a)) {
                quads.push([b, a, mirror(a, b)?, mirror(b, a)?]);
            }
        }
        for (&v, &(ga, gb, _, gd)) in &corners {
            quads.push([v, ga, gd, gb]);
        }
        Cage::new(points, quads)
    }
}

// ---- what the construction needs at a point -------------------------------------------------------

/// A point with quads all round it, as the paper's steps 1 and 2 read it.
struct Hub {
    /// How many quads meet here (the paper's `n`).
    n: usize,
    /// The surface's point here: the Catmull-Clark limit point (eq. 5).
    limit: Blend,
    /// Toward each neighbour, in the fan's order, the Bézier point next to the limit point
    /// (eq. 6).
    toward: Vec<Blend>,
    /// Toward each neighbour, the spline's point next to the limit point: by knot insertion at
    /// four quads, the paper's tangent (step 2) otherwise.
    tangent: Vec<Blend>,
    /// At an irregular point, each quad's twist point `b11` (step 2); none at four quads, where
    /// knot insertion has it.
    twist: Vec<Blend>,
}

impl Hub {
    fn of(cage: &Cage, v: usize) -> Result<Option<Hub>> {
        let fan = cage.fan(v)?;
        if !fan.closed {
            return Ok(None);
        }
        let n = fan.quads.len();
        let mut spokes = Vec::new();
        let mut diagonals = Vec::new();
        for &(f, c) in &fan.quads {
            spokes.push(cage.point(cage.vert(f, c + 1)?)?);
            diagonals.push(cage.point(cage.vert(f, c + 2)?)?);
        }
        let spoke = |i: usize| within(spokes.get(wrap(i, n)).copied());
        let diagonal = |i: usize| within(diagonals.get(wrap(i, n)).copied());
        let here = cage.point(v)?;
        let nf = n as f64;
        // eq. 5
        let mut terms = vec![(nf / (nf + 5.0), here)];
        for i in 0..n {
            terms.push((4.0 / (nf * (nf + 5.0)), spoke(i)?));
            terms.push((1.0 / (nf * (nf + 5.0)), diagonal(i)?));
        }
        let limit = lin(&terms);
        // eq. 6
        let mut toward = Vec::new();
        for k in 0..n {
            toward.push(lin(&[
                (8.0 / 18.0, here),
                (4.0 / 18.0, spoke(k)?),
                (2.0 / 18.0, spoke(k + 1)?),
                (2.0 / 18.0, spoke(k + n - 1)?),
                (1.0 / 18.0, diagonal(k)?),
                (1.0 / 18.0, diagonal(k + n - 1)?),
            ]));
        }
        let mut tangent = Vec::new();
        let mut twist = Vec::new();
        if n == 4 {
            for t in &toward {
                tangent.push(lin(&[(2.0 / 3.0, &limit), (1.0 / 3.0, t)]));
            }
        } else {
            // step 2: b10 = b00 + (e1 cos(2 pi k / n) + e2 sin(2 pi k / n)) / 3
            let turn = std::f64::consts::TAU / nf;
            let (_, _, omega) = ring(n);
            let sigma = sigma(n);
            let scale = sigma / (9.0 * (2.0 + omega));
            for k in 0..n {
                let mut terms = vec![(1.0, &limit)];
                for l in 0..n {
                    let a = turn * (l as f64 - k as f64);
                    let b = turn * (l as f64 + 1.0 - k as f64);
                    terms.push((scale * omega * a.cos(), spoke(l)?));
                    terms.push((scale * (a.cos() + b.cos()), diagonal(l)?));
                }
                tangent.push(lin(&terms));
            }
            // step 2's twist points, where knot insertion would put them (eq. 7 for q11)
            for k in 0..n {
                let q11 = lin(&[
                    (4.0 / 9.0, here),
                    (2.0 / 9.0, spoke(k)?),
                    (2.0 / 9.0, spoke(k + 1)?),
                    (1.0 / 9.0, diagonal(k)?),
                ]);
                twist.push(lin(&[
                    (6.0 / 9.0, within(tangent.get(k))?),
                    (6.0 / 9.0, within(tangent.get(wrap(k + 1, n)))?),
                    (-4.0 / 9.0, &limit),
                    (1.0 / 9.0, &q11),
                ]));
            }
        }
        Ok(Some(Hub {
            n,
            limit,
            toward,
            tangent,
            twist,
        }))
    }
}

// ---- the patches ----------------------------------------------------------------------------------

/// The coefficient at the paper's subscript `g`, the Greville place in ninths of the side:
/// knots 0, 0, 0, 0, 3, 3, 6, 6, 9, 9, 9, 9 have their coefficients at 0, 1, 2, 4, 5, 7, 8, 9.
fn place(g: usize) -> Result<usize> {
    match g {
        0 => Ok(0),
        1 => Ok(1),
        2 => Ok(2),
        4 => Ok(3),
        5 => Ok(4),
        7 => Ok(5),
        8 => Ok(6),
        9 => Ok(7),
        _ => Err(fail("no coefficient at that place")),
    }
}

/// A cubic's four Bézier points as its eight coefficients once the knots are put in (exact).
const INSERTED: [[f64; 4]; 8] = [
    [1.0, 0.0, 0.0, 0.0],
    [2.0 / 3.0, 1.0 / 3.0, 0.0, 0.0],
    [4.0 / 9.0, 4.0 / 9.0, 1.0 / 9.0, 0.0],
    [4.0 / 27.0, 4.0 / 9.0, 1.0 / 3.0, 2.0 / 27.0],
    [2.0 / 27.0, 1.0 / 3.0, 4.0 / 9.0, 4.0 / 27.0],
    [0.0, 1.0 / 9.0, 4.0 / 9.0, 4.0 / 9.0],
    [0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Corner `c`'s `(i, j)` on a grid of `last + 1` each way, as the quad's own `(a, b)`: `i` runs
/// along the side to the next corner, `j` along the side back to the corner before.
fn turned(c: usize, i: usize, j: usize, last: usize) -> Result<(usize, usize)> {
    let r = |x: usize| within(last.checked_sub(x));
    Ok(match c % 4 {
        0 => (i, j),
        1 => (r(j)?, i),
        2 => (r(i)?, r(j)?),
        _ => (j, r(i)?),
    })
}

/// A patch seen from one corner of its quad along one of the two sides there. Its places are
/// `(along, into)`: `along` the side from the corner (the paper's first subscript for patch `k`
/// along `b(u, 0)`), `into` the patch.
#[derive(Clone, Copy)]
struct Frame {
    quad: usize,
    corner: usize,
    /// Along the side back to the corner before (the paper's patch `k - 1`, which has the shared
    /// curve as `b(0, u)`); otherwise along the side to the next corner.
    back: bool,
}

impl Frame {
    fn slot(self, along: usize, into: usize) -> Result<usize> {
        let (i, j) = (place(along)?, place(into)?);
        let (i, j) = if self.back { (j, i) } else { (i, j) };
        let (a, b) = turned(self.corner, i, j, 7)?;
        Ok(a * 8 + b)
    }
}

/// The patches of the quads: 8 x 8 blends each, the quad's own `(a, b)` at `a * 8 + b`.
struct Patches {
    grids: Vec<Option<Vec<Blend>>>,
}

impl Patches {
    fn grid(&self, f: usize) -> Result<&Vec<Blend>> {
        within(self.grids.get(f).and_then(Option::as_ref))
    }

    fn built(&self, f: usize) -> bool {
        self.grids.get(f).is_some_and(Option::is_some)
    }

    fn get(&self, fr: Frame, along: usize, into: usize) -> Result<&Blend> {
        within(self.grid(fr.quad)?.get(fr.slot(along, into)?))
    }

    fn set(&mut self, fr: Frame, along: usize, into: usize, val: Blend) -> Result<()> {
        let slot = fr.slot(along, into)?;
        let grid = within(self.grids.get_mut(fr.quad).and_then(Option::as_mut))?;
        *within(grid.get_mut(slot))? = val;
        Ok(())
    }

    /// The two patches along the side from `a` to `b`, both seen from `a`: the quad that runs
    /// `a` to `b`, and the quad that runs `b` to `a`.
    fn seam(&self, cage: &Cage, a: usize, b: usize) -> Option<(Frame, Frame)> {
        let &(f, c) = cage.along.get(&(a, b))?;
        let &(g, cg) = cage.along.get(&(b, a))?;
        if !self.built(f) || !self.built(g) {
            return None;
        }
        Some((
            Frame {
                quad: f,
                corner: c,
                back: false,
            },
            Frame {
                quad: g,
                corner: cg + 1,
                back: true,
            },
        ))
    }

    /// Step 1 for quad `f`, then step 2 at its irregular corners.
    fn begin(&mut self, cage: &Cage, hubs: &[Option<Hub>], f: usize) -> Result<()> {
        let mut bezier: Vec<Blend> = Vec::new();
        bezier.resize_with(16, Vec::new);
        // per corner: the hub and which of its quads this is
        let mut seen = Vec::new();
        for c in 0..4 {
            let v = cage.vert(f, c)?;
            let Some(hub) = hubs.get(v).and_then(Option::as_ref) else {
                return Ok(());
            };
            let fan = cage.fan(v)?;
            let k = within(fan.quads.iter().position(|&q| q == (f, c)))?;
            let (a, d, b) = (
                cage.vert(f, c + 1)?,
                cage.vert(f, c + 2)?,
                cage.vert(f, c + 3)?,
            );
            // eq. 7
            let q11 = lin(&[
                (4.0 / 9.0, cage.point(v)?),
                (2.0 / 9.0, cage.point(a)?),
                (2.0 / 9.0, cage.point(b)?),
                (1.0 / 9.0, cage.point(d)?),
            ]);
            let next = wrap(k + 1, hub.n);
            for ((i, j), val) in [
                ((0, 0), hub.limit.clone()),
                ((1, 0), within(hub.toward.get(k))?.clone()),
                ((0, 1), within(hub.toward.get(next))?.clone()),
                ((1, 1), q11),
            ] {
                let (a, b) = turned(c, i, j, 3)?;
                *within(bezier.get_mut(a * 4 + b))? = val;
            }
            seen.push((hub, k, next));
        }
        let mut grid = Vec::new();
        for ra in &INSERTED {
            for rb in &INSERTED {
                let mut terms = Vec::new();
                for (i, &wa) in ra.iter().enumerate() {
                    for (j, &wb) in rb.iter().enumerate() {
                        if wa * wb != 0.0 {
                            terms.push((wa * wb, within(bezier.get(i * 4 + j))?));
                        }
                    }
                }
                grid.push(lin(&terms));
            }
        }
        *within(self.grids.get_mut(f))? = Some(grid);
        for (c, (hub, k, next)) in seen.into_iter().enumerate() {
            let fr = Frame {
                quad: f,
                corner: c,
                back: false,
            };
            let b10 = within(hub.tangent.get(k))?;
            let b01 = within(hub.tangent.get(next))?;
            // shared with the quads beside this one: theirs is this same blend
            self.set(fr, 1, 0, b10.clone())?;
            self.set(fr, 0, 1, b01.clone())?;
            if let Some(b11) = hub.twist.get(k) {
                self.set(fr, 1, 1, b11.clone())?;
            }
        }
        Ok(())
    }

    /// Step 3's `b20` at an end of `n0` quads: `b10 + (3 (b11 + b11' - 2 b10) - l1 (b10 - b00)) /
    /// (2 l0)`, read `near` the end (places 0, 1) or from the far one (places 9, 8).
    fn second(&self, l: Frame, r: Frame, near: bool, l0: f64, l1: f64) -> Result<Blend> {
        let (g0, g1) = if near { (0, 1) } else { (9, 8) };
        let s = 1.0 / (2.0 * l0);
        Ok(lin(&[
            (1.0 - 6.0 * s - l1 * s, self.get(l, g1, 0)?),
            (3.0 * s, self.get(l, g1, 1)?),
            (3.0 * s, self.get(r, g1, 1)?),
            (l1 * s, self.get(l, g0, 0)?),
        ]))
    }

    /// Step 3 along the side the frames share, from its end of `n0` quads (not four) to its end
    /// of `n1`.
    fn side(&mut self, l: Frame, r: Frame, n0: usize, n1: usize) -> Result<()> {
        let l0 = lambda(n0);
        let (b20, b40, b50, b70);
        if n1 == 4 {
            // 3c, with lambda_1 = l0 / 3 (the module doc): the far end's b70 stays as the knots
            // left it, the middle piece is a quadratic (b5 = b4 + (b7 - b2) / 5) and the first two
            // pieces meet in their second derivative (b5 = 2 b4 - 2 b2 + b1)
            b20 = self.second(l, r, true, l0, l0 / 3.0)?;
            let (b10, old70) = (self.get(l, 1, 0)?, self.get(l, 7, 0)?);
            b40 = lin(&[(9.0 / 5.0, &b20), (1.0 / 5.0, old70), (-1.0, b10)]);
            b50 = lin(&[(8.0 / 5.0, &b20), (2.0 / 5.0, old70), (-1.0, b10)]);
            b70 = None;
        } else {
            // 3b, both ends alike: from the far end lambda_0 is -l3 and lambda_1 is -l2
            let l3 = -lambda(n1);
            let (l1, l2) = ((2.0 * l0 + l3) / 3.0, (l0 + 2.0 * l3) / 3.0);
            b20 = self.second(l, r, true, l0, l1)?;
            let far = self.second(l, r, false, -l3, -l2)?;
            let (b10, b80) = (self.get(l, 1, 0)?, self.get(l, 8, 0)?);
            b40 = lin(&[
                (4.0 / 3.0, &b20),
                (-1.0 / 3.0, b80),
                (2.0 / 3.0, &far),
                (-2.0 / 3.0, b10),
            ]);
            b50 = lin(&[
                (4.0 / 3.0, &far),
                (-1.0 / 3.0, b10),
                (2.0 / 3.0, &b20),
                (-2.0 / 3.0, b80),
            ]);
            b70 = Some(far);
        }
        for (g, val) in [(2, Some(b20)), (4, Some(b40)), (5, Some(b50)), (7, b70)] {
            if let Some(val) = val {
                self.set(l, g, 0, val.clone())?;
                self.set(r, g, 0, val)?;
            }
        }
        Ok(())
    }

    /// Step 4a: the row first in from the frame's side, as the one cubic through its ends' points
    /// has it, at places 2, 4, 5 and 7.
    fn plain_row(&self, fr: Frame) -> Result<[Blend; 4]> {
        let x = [
            self.get(fr, 0, 1)?,
            self.get(fr, 1, 1)?,
            self.get(fr, 8, 1)?,
            self.get(fr, 9, 1)?,
        ];
        let row = |w: [f64; 4]| -> Blend {
            let terms: Vec<(f64, &Blend)> = w.iter().copied().zip(x.iter().copied()).collect();
            lin(&terms)
        };
        Ok([
            row([-4.0 / 9.0, 4.0 / 3.0, 1.0 / 3.0, -2.0 / 9.0]),
            row([-20.0 / 27.0, 4.0 / 3.0, 1.0, -16.0 / 27.0]),
            row([-16.0 / 27.0, 1.0, 4.0 / 3.0, -20.0 / 27.0]),
            row([-2.0 / 9.0, 1.0 / 3.0, 4.0 / 3.0, -4.0 / 9.0]),
        ])
    }

    /// Step 4 along the side the frames share, from its end of `n0` quads (not four) to its end
    /// of `n1`: the first row in on both sides.
    fn first_row(&mut self, l: Frame, r: Frame, n0: usize, n1: usize) -> Result<()> {
        let l0 = lambda(n0);
        let b = |g: usize| self.get(l, g, 0);
        // the middle of the two rows at each place (the paper's h1, h2 and their mirrors)
        let mut mid: Vec<(usize, Blend)> = Vec::new();
        if n1 == 4 {
            // 4c, with lambda_1 = l0 / 3: h1 as in 4b, h2 = b40 + lambda_1 (b70 - b50) / 12
            let l1 = l0 / 3.0;
            mid.push((
                2,
                lin(&[
                    (1.0 - l0 / 12.0 + l1 / 3.0, b(2)?),
                    (l0 / 12.0, b(4)?),
                    (-l1 / 3.0, b(1)?),
                ]),
            ));
            mid.push((
                4,
                lin(&[(1.0, b(4)?), (l1 / 12.0, b(7)?), (-l1 / 12.0, b(5)?)]),
            ));
            mid.push((5, b(5)?.clone()));
        } else {
            // 4b
            let l3 = -lambda(n1);
            let (l1, l2) = ((2.0 * l0 + l3) / 3.0, (l0 + 2.0 * l3) / 3.0);
            mid.push((
                2,
                lin(&[
                    (1.0 - l0 / 12.0 + l1 / 3.0, b(2)?),
                    (l0 / 12.0, b(4)?),
                    (-l1 / 3.0, b(1)?),
                ]),
            ));
            mid.push((
                4,
                lin(&[
                    (1.0 - l1 / 3.0 + l2 / 12.0, b(4)?),
                    (l1 / 3.0, b(5)?),
                    (-l2 / 12.0, b(2)?),
                ]),
            ));
            mid.push((
                5,
                lin(&[
                    (1.0 + l2 / 3.0 - l1 / 12.0, b(5)?),
                    (-l2 / 3.0, b(4)?),
                    (l1 / 12.0, b(7)?),
                ]),
            ));
            mid.push((
                7,
                lin(&[
                    (1.0 + l3 / 12.0 - l2 / 3.0, b(7)?),
                    (-l3 / 12.0, b(5)?),
                    (l2 / 3.0, b(8)?),
                ]),
            ));
        }
        let (mine, theirs) = (self.plain_row(l)?, self.plain_row(r)?);
        for (g, h) in mid {
            let k = match g {
                2 => 0,
                4 => 1,
                5 => 2,
                _ => 3,
            };
            let (m, t) = (within(mine.get(k))?, within(theirs.get(k))?);
            self.set(l, g, 1, lin(&[(1.0, &h), (0.5, m), (-0.5, t)]))?;
            self.set(r, g, 1, lin(&[(1.0, &h), (0.5, t), (-0.5, m)]))?;
        }
        Ok(())
    }

    /// Step 5 for quad `f`, whose corners are `irregular` or not.
    fn inside(&mut self, f: usize, irregular: [bool; 4]) -> Result<()> {
        let frame = |c: usize| Frame {
            quad: f,
            corner: c,
            back: false,
        };
        let mut new = Vec::new();
        for c in 0..4 {
            let fr = frame(c);
            let p = |i: usize, j: usize| self.get(fr, i, j);
            new.push((
                fr,
                lin(&[
                    (-10.0 / 27.0, p(0, 4)?),
                    (2.0 / 3.0, p(1, 4)?),
                    (0.5, p(8, 4)?),
                    (-8.0 / 27.0, p(9, 4)?),
                    (-10.0 / 27.0, p(4, 0)?),
                    (2.0 / 3.0, p(4, 1)?),
                    (0.5, p(4, 8)?),
                    (-8.0 / 27.0, p(4, 9)?),
                ]),
            ));
        }
        for (fr, val) in new {
            self.set(fr, 4, 4, val)?;
        }
        // the row third in from a side with an irregular end
        for (c, &here) in irregular.iter().enumerate() {
            if !here && !*within(irregular.get((c + 1) % 4))? {
                continue;
            }
            let fr = frame(c);
            for g in [4, 5] {
                let val = lin(&[
                    (0.5, self.get(fr, g, 1)?),
                    (1.0, self.get(fr, g, 4)?),
                    (-0.5, self.get(fr, g, 5)?),
                ]);
                self.set(fr, g, 2, val)?;
            }
        }
        for (c, &here) in irregular.iter().enumerate() {
            if !here {
                continue;
            }
            let fr = frame(c);
            let p = |i: usize, j: usize| self.get(fr, i, j);
            let val = lin(&[
                (0.25, p(1, 2)?),
                (0.5, p(4, 2)?),
                (-0.25, p(5, 2)?),
                (0.25, p(2, 1)?),
                (0.5, p(2, 4)?),
                (-0.25, p(2, 5)?),
            ]);
            self.set(fr, 2, 2, val)?;
        }
        Ok(())
    }
}

/// The cage's quads' patches, and whether each of its points is regular.
struct Built {
    cage: Cage,
    patches: Patches,
    regular: Vec<bool>,
}

fn build(points: Vec<Blend>, quads: Vec<[usize; 4]>) -> Result<Built> {
    let cage = Cage::new(points, quads)?;
    let real = cage.quads.len();
    // two rings outside an open cage: one so its rim points have quads all round them, one more
    // so the quads of the first have their corners' quads too
    let mut ringed = None;
    if cage.open() {
        ringed = Some(cage.ringed()?.ringed()?);
    }
    let all = ringed.as_ref().unwrap_or(&cage);
    let mut hubs = Vec::new();
    for v in 0..all.points.len() {
        hubs.push(Hub::of(all, v)?);
    }
    let mut grids = Vec::new();
    grids.resize_with(all.quads.len(), || None);
    let mut patches = Patches { grids };
    for f in 0..all.quads.len() {
        patches.begin(all, &hubs, f)?;
    }
    let valence = |v: usize| hubs.get(v).and_then(Option::as_ref).map(|h| h.n);
    // steps 3 and 4 along every side of a cage quad with an irregular end, once, from an
    // irregular end
    let mut sides = Vec::new();
    for (&(a, b), &(f, _)) in &all.along {
        let Some(&(g, _)) = all.along.get(&(b, a)) else {
            continue;
        };
        if f >= real && g >= real {
            continue;
        }
        let (Some(n0), Some(n1)) = (valence(a), valence(b)) else {
            return Err(fail("a quad of the cage is not surrounded after the rings"));
        };
        if n0 == 4 || (n1 != 4 && b < a) {
            continue;
        }
        let Some((l, r)) = patches.seam(all, a, b) else {
            return Err(fail("a quad of the cage is not surrounded after the rings"));
        };
        sides.push((l, r, n0, n1));
    }
    for &(l, r, n0, n1) in &sides {
        patches.side(l, r, n0, n1)?;
    }
    for &(l, r, n0, n1) in &sides {
        patches.first_row(l, r, n0, n1)?;
    }
    let mut regular = Vec::new();
    for v in 0..cage.points.len() {
        regular.push(cage.fan(v)?.quads.is_empty() || valence(v) == Some(4));
    }
    for f in 0..real {
        let mut irregular = [false; 4];
        for (c, slot) in irregular.iter_mut().enumerate() {
            *slot = !*within(regular.get(cage.vert(f, c)?))?;
        }
        if irregular.contains(&true) {
            patches.inside(f, irregular)?;
        }
    }
    patches.grids.truncate(real);
    Ok(Built {
        cage,
        patches,
        regular,
    })
}

// ---- the layout -----------------------------------------------------------------------------------

/// A rectangle of quads: its size and the `(quad, rot)` at each place, u fastest.
type Rect = ([usize; 2], Vec<(usize, u8)>);

/// Which quads are packed together, and the edges between quads that are borders of faces.
struct Layout<'c> {
    cage: &'c Cage,
    /// Edges two patches meet across without sharing their curvature: borders whatever else.
    hard: BTreeSet<Edge>,
    border: BTreeSet<Edge>,
    /// Each quad's group, named by a quad in it.
    label: Vec<usize>,
}

impl<'c> Layout<'c> {
    /// The borders the surface needs: every side with an irregular end, run straight on to the
    /// next irregular point or the rim.
    fn new(cage: &'c Cage, regular: &[bool]) -> Result<Self> {
        let mut hard = BTreeSet::new();
        let mut border = BTreeSet::new();
        let most = cage.along.len();
        for &(a, b) in cage.along.keys() {
            if *within(regular.get(a))? || !cage.along.contains_key(&(b, a)) {
                continue;
            }
            hard.insert(edge(a, b));
            border.insert(edge(a, b));
            let (mut prev, mut cur) = (a, b);
            for _ in 0..most {
                if !*within(regular.get(cur))? {
                    break;
                }
                let Some(next) = cage.straight_on(prev, cur)? else {
                    break;
                };
                if !border.insert(edge(cur, next)) {
                    break;
                }
                prev = cur;
                cur = next;
            }
        }
        let mut layout = Layout {
            cage,
            hard,
            border,
            label: (0..cage.quads.len()).collect(),
        };
        let every: BTreeSet<usize> = (0..cage.quads.len()).collect();
        for part in layout.parts(&every)? {
            layout.name(&part);
        }
        Ok(layout)
    }

    fn name(&mut self, part: &BTreeSet<usize>) {
        if let Some(&low) = part.iter().next() {
            for &f in part {
                if let Some(l) = self.label.get_mut(f) {
                    *l = low;
                }
            }
        }
    }

    /// The quads of `of` in the parts the borders leave them in.
    fn parts(&self, of: &BTreeSet<usize>) -> Result<Vec<BTreeSet<usize>>> {
        let mut out = Vec::new();
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        for &seed in of {
            if !seen.insert(seed) {
                continue;
            }
            let mut part = BTreeSet::from([seed]);
            let mut stack = vec![seed];
            while let Some(f) = stack.pop() {
                for side in 0..4 {
                    let (a, b) = (self.cage.vert(f, side)?, self.cage.vert(f, side + 1)?);
                    if self.border.contains(&edge(a, b)) {
                        continue;
                    }
                    if let Some((g, _)) = self.cage.across(f, side)? {
                        if of.contains(&g) && seen.insert(g) {
                            part.insert(g);
                            stack.push(g);
                        }
                    }
                }
            }
            out.push(part);
        }
        Ok(out)
    }

    fn groups(&self) -> BTreeMap<usize, BTreeSet<usize>> {
        let mut out: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for (f, &l) in self.label.iter().enumerate() {
            out.entry(l).or_default().insert(f);
        }
        out
    }

    /// `quads` as a rectangle, the edges in `open` no longer borders: every quad at its place,
    /// every two quads of it that share an edge beside each other there with no border between
    /// them. `None` when they are no rectangle, or one that meets itself.
    fn rect(&self, quads: &BTreeSet<usize>, open: &[Edge]) -> Result<Option<Rect>> {
        let Some(&seed) = quads.iter().next() else {
            return Ok(None);
        };
        let step = [(0i64, -1i64), (1, 0), (0, 1), (-1, 0)];
        let mut at: BTreeMap<(i64, i64), (usize, usize)> = BTreeMap::new();
        let mut seen: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
        at.insert((0, 0), (seed, 0));
        seen.insert(seed, (0, 0));
        let mut stack = vec![(0i64, 0i64)];
        while let Some(cell) = stack.pop() {
            let (q, rot) = *within(at.get(&cell))?;
            for (d, &(du, dv)) in step.iter().enumerate() {
                let k = (d + rot) % 4;
                let Some((g, kg)) = self.cage.across(q, k)? else {
                    continue;
                };
                if !quads.contains(&g) {
                    continue;
                }
                let between = edge(self.cage.vert(q, k)?, self.cage.vert(q, k + 1)?);
                let rg = (kg + 6 - d) % 4;
                let nc = (cell.0 + du, cell.1 + dv);
                match (at.get(&nc), seen.get(&g)) {
                    _ if self.border.contains(&between) && !open.contains(&between) => {
                        return Ok(None)
                    }
                    (None, None) => {
                        at.insert(nc, (g, rg));
                        seen.insert(g, nc);
                        stack.push(nc);
                    }
                    (Some(&(h, rh)), _) if h == g && rh == rg => {}
                    _ => return Ok(None),
                }
            }
        }
        if seen.len() != quads.len() {
            return Ok(None);
        }
        let (mut u0, mut v0, mut u1, mut v1) = (0i64, 0i64, 0i64, 0i64);
        for &(u, v) in at.keys() {
            u0 = u0.min(u);
            v0 = v0.min(v);
            u1 = u1.max(u);
            v1 = v1.max(v);
        }
        let mut cells = Vec::new();
        for v in v0..=v1 {
            for u in u0..=u1 {
                let Some(&(q, r)) = at.get(&(u, v)) else {
                    return Ok(None);
                };
                cells.push((q, u8::try_from(r).map_err(|_| fail("a turn past three"))?));
            }
        }
        let size = |lo: i64, hi: i64| {
            usize::try_from(hi - lo + 1).map_err(|_| fail("a rectangle with no quads"))
        };
        Ok(Some(([size(u0, u1)?, size(v0, v1)?], cells)))
    }

    /// Lay a border straight through side `side` of quad `q`, both ways, inside `part`; how many
    /// edges it added.
    fn cut(&mut self, part: &BTreeSet<usize>, q: usize, side: usize) -> Result<usize> {
        let start = (self.cage.vert(q, side)?, self.cage.vert(q, side + 1)?);
        let inside = |a: usize, b: usize| {
            [self.cage.along.get(&(a, b)), self.cage.along.get(&(b, a))]
                .iter()
                .all(|x| x.is_some_and(|&(f, _)| part.contains(&f)))
        };
        if !inside(start.0, start.1) {
            return Ok(0);
        }
        let mut new = Vec::new();
        if !self.border.contains(&edge(start.0, start.1)) {
            new.push(edge(start.0, start.1));
        }
        for (mut prev, mut cur) in [start, (start.1, start.0)] {
            for _ in 0..self.cage.along.len() {
                let Some(next) = self.cage.straight_on(prev, cur)? else {
                    break;
                };
                let e = edge(cur, next);
                if !inside(cur, next) || self.border.contains(&e) || new.contains(&e) {
                    break;
                }
                new.push(e);
                prev = cur;
                cur = next;
            }
        }
        let added = new.len();
        self.border.extend(new);
        Ok(added)
    }

    /// The quads of `part` by how far each is from `from` (farthest first, then by number).
    fn farthest(&self, part: &BTreeSet<usize>, from: usize) -> Result<Vec<usize>> {
        let mut far: BTreeMap<usize, usize> = BTreeMap::from([(from, 0)]);
        let mut front = vec![from];
        let mut d = 0;
        while !front.is_empty() {
            d += 1;
            let mut next = Vec::new();
            for &f in &front {
                for side in 0..4 {
                    if let Some((g, _)) = self.cage.across(f, side)? {
                        if part.contains(&g) && !far.contains_key(&g) {
                            far.insert(g, d);
                            next.push(g);
                        }
                    }
                }
            }
            front = next;
        }
        let mut out: Vec<usize> = part.iter().copied().collect();
        out.sort_by_key(|f| (std::cmp::Reverse(far.get(f).copied().unwrap_or(0)), *f));
        Ok(out)
    }

    /// Cut every group that is no rectangle (a ring, a torus) along straight lines until each
    /// part is one: both ways through the part's lowest quad, else through the quad farthest
    /// from it. A part still none after [`MOST_CUTS`] cuts has each quad its own face.
    fn cut_rings(&mut self) -> Result<()> {
        for (_, quads) in self.groups() {
            let mut todo = vec![quads];
            let mut cuts = 0;
            while let Some(part) = todo.pop() {
                if self.rect(&part, &[])?.is_some() {
                    self.name(&part);
                    continue;
                }
                let mut added = 0;
                if cuts < MOST_CUTS {
                    let low = *within(part.iter().next())?;
                    for q in std::iter::once(low).chain(self.farthest(&part, low)?) {
                        added = self.cut(&part, q, 0)? + self.cut(&part, q, 1)?;
                        if added > 0 {
                            break;
                        }
                    }
                    cuts += 1;
                }
                if added == 0 {
                    for &f in &part {
                        self.name(&BTreeSet::from([f]));
                        for side in 0..4 {
                            let e = edge(self.cage.vert(f, side)?, self.cage.vert(f, side + 1)?);
                            self.border.insert(e);
                        }
                    }
                    continue;
                }
                todo.extend(self.parts(&part)?);
            }
        }
        Ok(())
    }

    /// Join neighbouring groups while their union is a rectangle with no hard edge inside.
    fn merge(&mut self) -> Result<()> {
        for _ in 0..self.label.len() {
            let groups = self.groups();
            let mut joined = false;
            'groups: for (&l, quads) in &groups {
                // the groups beside this one, and the edges between
                let mut beside: BTreeMap<usize, Vec<Edge>> = BTreeMap::new();
                for &f in quads {
                    for side in 0..4 {
                        let Some((g, _)) = self.cage.across(f, side)? else {
                            continue;
                        };
                        let lg = *within(self.label.get(g))?;
                        if lg != l {
                            let e = edge(self.cage.vert(f, side)?, self.cage.vert(f, side + 1)?);
                            beside.entry(lg).or_default().push(e);
                        }
                    }
                }
                for (other, between) in beside {
                    if between.iter().any(|e| self.hard.contains(e)) {
                        continue;
                    }
                    let both: BTreeSet<usize> =
                        quads.union(within(groups.get(&other))?).copied().collect();
                    if self.rect(&both, &between)?.is_some() {
                        for e in &between {
                            self.border.remove(e);
                        }
                        self.name(&both);
                        joined = true;
                        break 'groups;
                    }
                }
            }
            if !joined {
                break;
            }
        }
        Ok(())
    }
}

// ---- faces ----------------------------------------------------------------------------------------

/// One face of the surface: a rectangle of `size[0]` x `size[1]` quads as one bicubic B-spline,
/// u fastest. The parameter runs 0..`size[0]` in u and 0..`size[1]` in v, one per quad. Between
/// two quads there is one knot; a column (in u) or row (in v) of quads with an irregular corner
/// among them has double knots at its thirds, any other none. There are four fewer poles each way
/// than knots.
#[derive(Clone, Debug)]
pub struct CageFace {
    /// Quads each way.
    pub size: [usize; 2],
    /// The cage quad at each place (u fastest) and how it lies: its corner `rot` is the place's
    /// first corner, its edge from corner `rot` to `rot + 1` runs along u.
    pub quads: Vec<(usize, u8)>,
    /// Knots along u.
    pub knots_u: Vec<f64>,
    /// Knots along v.
    pub knots_v: Vec<f64>,
    /// The poles, u fastest, each a blend of cage points.
    pub poles: Vec<Blend>,
}

impl CageFace {
    /// The poles for these cage points.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a pole names a point `points` does not have.
    pub fn points(&self, points: &[Point3]) -> Result<Vec<Point3>> {
        let mut out = Vec::new();
        for b in &self.poles {
            let mut p = Point3::new(0.0, 0.0, 0.0);
            for &(i, w) in b {
                let q = points
                    .get(i as usize)
                    .ok_or_else(|| bad("a pole names a point the cage does not have"))?;
                p += *q * w;
            }
            out.push(p);
        }
        Ok(out)
    }
}

/// The surface of a quad cage: its faces.
#[derive(Clone, Debug)]
pub struct CageSurface {
    /// The faces, by their lowest quad.
    pub faces: Vec<CageFace>,
}

/// One way along a face: the knots in thirds of a quad, and each pole as weights on the eight
/// coefficients of one span (two, halved, where the pole sits on a joint).
struct Way {
    thirds: Vec<usize>,
    poles: Vec<Vec<(usize, [f64; 8])>>,
}

/// A span's polynomial read at three knots `at` (thirds from the span's start): the weights on
/// the span's eight coefficients. A `single` span is one cubic; any other is three, and `piece`
/// says which is read.
fn blossom(single: bool, piece: usize, at: [f64; 3]) -> Result<[f64; 8]> {
    let unit = |i: usize| {
        let mut w = [0.0; 8];
        if let Some(x) = w.get_mut(i) {
            *x = 1.0;
        }
        w
    };
    let mix = |terms: &[(f64, [f64; 8])]| {
        let mut w = [0.0; 8];
        for (s, t) in terms {
            for (x, y) in w.iter_mut().zip(t) {
                *x += s * y;
            }
        }
        w
    };
    let half = |i: usize, j: usize| mix(&[(0.5, unit(i)), (0.5, unit(j))]);
    // the piece's Bézier points and where it starts and ends
    let (bezier, from, to) = match (single, piece) {
        (true, _) => (
            [
                unit(0),
                mix(&[(3.0, unit(1)), (-2.0, unit(0))]),
                mix(&[(3.0, unit(6)), (-2.0, unit(7))]),
                unit(7),
            ],
            0.0,
            3.0,
        ),
        (false, 0) => ([unit(0), unit(1), unit(2), half(2, 3)], 0.0, 1.0),
        (false, 1) => ([half(2, 3), unit(3), unit(4), half(4, 5)], 1.0, 2.0),
        (false, 2) => ([half(4, 5), unit(5), unit(6), unit(7)], 2.0, 3.0),
        _ => return Err(fail("a span has three pieces")),
    };
    let mut level: Vec<[f64; 8]> = bezier.to_vec();
    for t in at {
        let s = (t - from) / (to - from);
        let mut next = Vec::new();
        for pair in level.windows(2) {
            if let [a, b] = pair {
                next.push(mix(&[(1.0 - s, *a), (s, *b)]));
            }
        }
        level = next;
    }
    within(level.first().copied())
}

impl Way {
    /// Along `single.len()` quads, each a single cubic that way or not.
    fn of(single: &[bool]) -> Result<Way> {
        let k = single.len();
        let mut thirds = vec![0; 4];
        for (t, &one) in single.iter().enumerate() {
            if !one {
                thirds.extend([3 * t + 1, 3 * t + 1, 3 * t + 2, 3 * t + 2]);
            }
            if t + 1 < k {
                thirds.push(3 * t + 3);
            }
        }
        thirds.extend([3 * k; 4]);
        let mut poles = Vec::new();
        for triple in thirds.windows(5) {
            let [_, a, b, c, _] = triple else {
                continue;
            };
            // the pole's place, in ninths of a quad
            let ninths = a + b + c;
            let read = |t: usize, halved: bool| -> Result<(usize, [f64; 8])> {
                let one = *within(single.get(t))?;
                // which piece of the span holds the pole's place
                let piece = if ninths <= 9 * t {
                    0
                } else if ninths >= 9 * t + 9 {
                    2
                } else {
                    (ninths - 9 * t) / 3
                };
                let local = |x: usize| x as f64 - (3 * t) as f64;
                let mut w = blossom(one, piece, [local(*a), local(*b), local(*c)])?;
                if halved {
                    for x in &mut w {
                        *x *= 0.5;
                    }
                }
                Ok((t, w))
            };
            let t = ninths / 9;
            if ninths % 9 == 0 && t > 0 && t < k {
                poles.push(vec![read(t - 1, true)?, read(t, true)?]);
            } else {
                poles.push(vec![read(t.min(k.saturating_sub(1)), false)?]);
            }
        }
        Ok(Way { thirds, poles })
    }

    fn knots(&self) -> Vec<f64> {
        self.thirds.iter().map(|&t| t as f64 / 3.0).collect()
    }
}

/// A rectangle of quads as one face: every pole its patch's own polynomial read at the pole's
/// knots.
fn face(built: &Built, size: [usize; 2], cells: Vec<(usize, u8)>) -> Result<CageFace> {
    let [nu, nv] = size;
    let cell = |iu: usize, iv: usize| within(cells.get(iu + nu * iv).copied());
    let regular_quad = |q: usize| -> Result<bool> {
        for c in 0..4 {
            if !*within(built.regular.get(built.cage.vert(q, c)?))? {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let mut single_u = vec![true; nu];
    let mut single_v = vec![true; nv];
    for iv in 0..nv {
        for iu in 0..nu {
            if !regular_quad(cell(iu, iv)?.0)? {
                *within(single_u.get_mut(iu))? = false;
                *within(single_v.get_mut(iv))? = false;
            }
        }
    }
    let (wu, wv) = (Way::of(&single_u)?, Way::of(&single_v)?);
    let mut poles = Vec::new();
    for pv in &wv.poles {
        for pu in &wu.poles {
            let mut terms: Vec<(f64, &Blend)> = Vec::new();
            for (iv, rv) in pv {
                for (iu, ru) in pu {
                    let (q, rot) = cell(*iu, *iv)?;
                    let grid = built.patches.grid(q)?;
                    for (i, &x) in ru.iter().enumerate() {
                        for (j, &y) in rv.iter().enumerate() {
                            if x * y != 0.0 {
                                let (a, b) = turned(usize::from(rot), i, j, 7)?;
                                terms.push((x * y, within(grid.get(a * 8 + b))?));
                            }
                        }
                    }
                }
            }
            poles.push(lin(&terms));
        }
    }
    Ok(CageFace {
        size,
        quads: cells,
        knots_u: wu.knots(),
        knots_v: wv.knots(),
        poles,
    })
}

/// The points the quads name, numbered as they come, and the quads in those numbers: the work is
/// the quads' size, whatever `points` says.
fn named(points: usize, quads: &[[u32; 4]]) -> Result<(Vec<Blend>, Vec<[usize; 4]>)> {
    if quads.is_empty() {
        return Err(bad("a cage with no quads"));
    }
    let mut number: BTreeMap<u32, usize> = BTreeMap::new();
    let mut named: Vec<Blend> = Vec::new();
    let mut own = Vec::new();
    for q in quads {
        let mut corners = [0usize; 4];
        for (slot, &v) in corners.iter_mut().zip(q) {
            if usize::try_from(v).map_or(true, |i| i >= points) {
                return Err(bad("a quad names a point the cage does not have"));
            }
            *slot = *number.entry(v).or_insert_with(|| {
                named.push(vec![(v, 1.0)]);
                named.len() - 1
            });
        }
        own.push(corners);
    }
    Ok((named, own))
}

impl CageSurface {
    /// The surface of the cage with `points` points and these quads, each counter-clockwise seen
    /// from outside, two to an edge at most.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when there are no quads, a quad names a point the cage does not have or one
    /// point twice, two quads run the same way along an edge, two rims or two sheets of the cage
    /// meet at one point, or a point inside the cage has fewer than three quads round it.
    pub fn of(points: usize, quads: &[[u32; 4]]) -> Result<CageSurface> {
        let (named, own) = named(points, quads)?;
        let built = build(named, own)?;
        let mut layout = Layout::new(&built.cage, &built.regular)?;
        layout.cut_rings()?;
        layout.merge()?;
        let mut faces = Vec::new();
        for (_, quads) in layout.groups() {
            match layout.rect(&quads, &[])? {
                Some((size, cells)) => faces.push(face(&built, size, cells)?),
                None => {
                    for &q in &quads {
                        faces.push(face(&built, [1, 1], vec![(q, 0)])?);
                    }
                }
            }
        }
        Ok(CageSurface { faces })
    }
}

#[cfg(test)]
#[path = "quad_cage_tests.rs"]
mod tests;
